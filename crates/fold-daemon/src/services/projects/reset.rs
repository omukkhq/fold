use std::sync::Arc;

use fold_core::events::types;
use fold_core::object::ObjectHash;
use fold_core::{Actor, FoldError, NewEvent, RevisionId, WorkspaceId, codes, policy};
use fold_store::StoreError;
use fold_store::workspaces::{self, ClosedState, NewStash, StashState, drop_stash, insert_stash};
use fold_vcs::git::Git;
use fold_vcs::git_remote::reset_index_paths;
use fold_vcs::objects::ObjectStore;
use fold_vcs::paths::RepoPath;
use fold_vcs::scan::Skipped;
use fold_vcs::stash::restore_tree;
use serde_json::json;

use super::record::{snapshot_folder, sync};
use super::remote::blocking;
use super::stash::{
    Finish, LISTED_PATHS, UnstashOutcome, changed, finish_open_unstash, markers_wait, paths_of,
    put_back, refuse_branch, refuse_unstashable, spawned, stack, stash_event,
};
use super::{ProjectError, load_config};
use crate::projects::ProjectHandle;
use crate::services::events::EventBuffer;
#[cfg(feature = "failpoints")]
use crate::services::fired;

/// What `fld reset` or `fld restore` puts back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResetScope {
    /// The whole project folder.
    Folder,
    /// These paths, files or directories, relative to the project folder.
    Paths(Vec<String>),
}

/// The conflicted human change that waited for the main agent and that the reset dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedChange {
    /// The human's Git commit; `None` for a change `fld record` read from the folder.
    pub commit: Option<String>,
    /// The paths that conflicted with main, in path order.
    pub conflicted: Vec<String>,
    /// The main workspace that holds its merge, when the main agent had opened one.
    pub workspace: Option<WorkspaceId>,
    /// Whether the command abandoned that workspace: `fld reset` does, while after `fld restore` the main agent's notice asks it to abandon the workspace itself.
    pub workspace_abandoned: bool,
}

/// What `fld reset` or `fld restore` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetOutcome {
    /// The main revision the folder holds now, apart from the changes that no revision records.
    pub revision: RevisionId,
    /// False when something still keeps the folder on an older revision than main's head, such as unrecorded changes that `fld restore` left on a path main changed.
    pub at_head: bool,
    /// The paths that went back to fold's entries, in path order; empty when nothing was unrecorded there.
    pub files: Vec<String>,
    /// The entries on the stack after the reset; the safety entry, when the reset made one, is `stash@{0}`.
    pub entries: usize,
    /// Whether the reset put the discarded changes aside as `stash@{0}`.
    pub stashed: bool,
    /// The named paths of `fld restore` where the folder held no change.
    pub unchanged: Vec<String>,
    /// The conflicted paths of the stash entry whose unstash conflict waited in the folder, which `fld reset` dropped: the safety entry holds the folder with its markers.
    pub dropped_unstash: Vec<String>,
    /// The conflicted human change that waited for the main agent, which the reset dropped.
    pub dropped_change: Option<DroppedChange>,
    /// New files the read left out, such as one over the size limit, which stay in the folder.
    pub skipped: Vec<Skipped>,
    /// The unstash that a stop had left part-way, which the reset finished first.
    pub unstash: Option<UnstashOutcome>,
}

/// Makes the project folder match the checkout record again for the named human who runs `fld reset` or `fld restore <path>...`, discarding the changes that no revision records, so the folder follows main. Nothing is lost: the discarded changes are first stashed as `stash@{0}`, which `fld unstash` brings back, and `fld stash drop` drops for good.
///
/// Under the remote lock and folder lock the folder first follows main, with the refusals of `fld stash`: a Git branch or `HEAD` that shows a change fold did not make, and the human's Git commits that wait for the main agent, are refused with the command that undoes them, since a reset never moves Git. An unstash that a stop left part-way is finished first. A tracked file the read cannot take, too large or unreadable, is refused as `fld stash` refuses it. `fld restore` refuses a stash entry whose unstash conflict waits in the folder; `fld reset` drops it with `stash.discarded` in the transaction that pushes the safety entry with `stash.pushed`, because the safety entry takes the folder with its markers, so a refusal or a stop leaves the entry waiting. Then the paths go back to the record's entries as `fld stash` puts them back, each only while it still holds what fold read. Then the conflicted human change that waits for the main agent is withdrawn with `human_change.withdrawn` when the folder holds none of it any more: always for `fld reset`, and for `fld restore` when the named paths held every unrecorded change. Only `fld reset` abandons the main workspace its merge made, with `workspace.abandoned` in the same transaction; after `fld restore` the main agent's notice asks it to abandon the workspace, as after `fld stash`. The folder then follows main. Each step can stop on its own: the next reset finds the entry pushed already, the files back or the change still waiting, and finishes the rest.
pub async fn reset(
    handle: &Arc<ProjectHandle>,
    actor: &Actor,
    git: &Git,
    scope: ResetScope,
) -> Result<ResetOutcome, ProjectError> {
    policy::can_reset_folder(actor).map_err(FoldError::from)?;
    let paths = match &scope {
        ResetScope::Folder => None,
        ResetScope::Paths(paths) => Some(restore_paths(paths)?),
    };
    let (handle, actor, git) = (Arc::clone(handle), actor.clone(), git.clone());
    spawned(async move { run_reset(&handle, &actor, &git, paths).await }).await
}

/// The named paths, each checked to be relative, normalised and inside the folder, without duplicates.
fn restore_paths(paths: &[String]) -> Result<Vec<RepoPath>, FoldError> {
    if paths.is_empty() {
        return Err(
            FoldError::new(codes::INVALID_REQUEST, "name at least one path")
                .with_hint("`fld reset` puts the whole project folder back")
                .with_details([("reason", json!("no_paths"))]),
        );
    }
    let mut parsed = Vec::with_capacity(paths.len());
    for path in paths {
        let repo = RepoPath::parse(path.as_bytes()).map_err(|_| {
            FoldError::new(
                codes::INVALID_REQUEST,
                format!("{path} is not a path inside the project folder"),
            )
            .with_hint("name a file or directory of the project folder, such as src/app.rs")
            .with_details([("reason", json!("invalid_path")), ("path", json!(path))])
        })?;
        if !parsed.contains(&repo) {
            parsed.push(repo);
        }
    }
    Ok(parsed)
}

async fn run_reset(
    handle: &Arc<ProjectHandle>,
    actor: &Actor,
    git: &Git,
    paths: Option<Vec<RepoPath>>,
) -> Result<ResetOutcome, ProjectError> {
    let config = load_config(&handle.paths).await?;
    let _remote = handle.remote_lock.clone().lock_owned().await;
    let _folder = handle.folder_lock.clone().lock_owned().await;
    let mut unstash = None;
    loop {
        let (followed, _) = sync(handle, git, false).await?;
        refuse_branch(&config.git.branch, &followed)?;
        match finish_open_unstash(handle, config.max_file_bytes).await? {
            Finish::Nothing => break,
            Finish::Abandoned => {}
            Finish::Finished(outcome) => unstash = Some(outcome),
        }
    }
    let stack = stack(handle).await?;
    // The entry is dropped only in the transaction that pushes the safety entry, after every refusal, so a refused or stopped reset leaves it and its markers waiting as they were.
    let conflicted = stack
        .first()
        .filter(|entry| entry.state == StashState::Conflicted);
    if let Some(entry) = conflicted
        && paths.is_some()
    {
        return Err(markers_wait(&entry.conflicts)
            .with_hint("fix the conflict markers in these files and run `fld record`, or run `fld reset` to put the whole project folder back")
            .into());
    }
    let dropped_unstash = conflicted
        .map(|entry| paths_of(&entry.conflicts))
        .unwrap_or_default();
    let below = &stack[usize::from(conflicted.is_some())..];
    let record = workspaces::checkout(handle.store.read())
        .await?
        .ok_or(StoreError::Corrupt("checkout"))?;
    let (folder_tree, skipped) =
        snapshot_folder(handle, record.tree, config.max_file_bytes).await?;
    let (tree, unchanged) = match &paths {
        None => (folder_tree, Vec::new()),
        Some(paths) => {
            let (fold_dir, paths, record_tree) =
                (handle.paths.fold_dir(), paths.clone(), record.tree);
            blocking(move || {
                restore_tree(
                    &ObjectStore::open_existing(&fold_dir)?,
                    &record_tree,
                    &folder_tree,
                    &paths,
                )
            })
            .await?
        }
    };
    let considered: Vec<Skipped> = match &paths {
        None => skipped.clone(),
        Some(paths) => skipped
            .iter()
            .filter(|skip| paths.iter().any(|path| skip.path.is_at_or_below(path)))
            .cloned()
            .collect(),
    };
    refuse_unstashable(handle, record.tree, tree, &considered, None).await?;
    let files = changed(handle, record.tree, tree).await?;
    let mut stashed = false;
    let drop = match conflicted {
        Some(entry) => Some(dropped_entry(handle, actor, entry, stack.len()).await?),
        None => None,
    };
    if tree == record.tree
        && let Some(drop) = &drop
    {
        // The folder holds none of the entry's changes and no markers any more, so only the entry goes.
        write_entries(handle, Some(drop), None).await?;
    }
    if tree != record.tree {
        // A stop after the row and before the files left this very entry; it is not pushed twice.
        let pushed = drop.is_none()
            && below
                .first()
                .is_some_and(|entry| entry.base_tree == record.tree && entry.tree == tree);
        if !pushed {
            let push = pushed_entry(
                actor,
                record.revision,
                record.tree,
                tree,
                &files,
                below.len(),
            )?;
            write_entries(handle, drop.as_ref(), Some(&push)).await?;
        }
        stashed = true;
        #[cfg(feature = "failpoints")]
        fail::fail_point!("reset_after_row", |_| Err(fired("reset_after_row").into()));
        if workspaces::git_sync(handle.store.read()).await?.is_some() {
            let (git, folder_paths) = (git.clone(), handle.paths.clone());
            let reset: Vec<Vec<u8>> = files.iter().map(|file| file.as_bytes().to_vec()).collect();
            blocking(move || {
                let tip = git.ref_commit(&folder_paths.git_dir(), "HEAD")?;
                reset_index_paths(&git, &folder_paths, tip.as_ref(), &reset)
            })
            .await?;
        }
        put_back(handle, record.tree, tree, config.max_file_bytes).await?;
    }
    #[cfg(feature = "failpoints")]
    fail::fail_point!("reset_after_files", |_| Err(
        fired("reset_after_files").into()
    ));
    // The folder holds none of the human's changes when the reset took them all, so the change that waits holds nothing to merge.
    let dropped_change = if tree == folder_tree {
        drop_waiting_change(handle, actor, paths.is_none()).await?
    } else {
        None
    };
    let (followed, _) = sync(handle, git, true).await?;
    let revision = match followed.at {
        Some(at) => at,
        None => {
            workspaces::checkout(handle.store.read())
                .await?
                .ok_or(StoreError::Corrupt("checkout"))?
                .revision
        }
    };
    let head = workspaces::head(handle.store.read())
        .await?
        .ok_or(StoreError::Corrupt("refs.main"))?;
    let entries = super::stash::stack(handle).await?.len();
    tracing::info!(files = files.len(), "the project folder is reset");
    Ok(ResetOutcome {
        revision,
        at_head: revision == head.number,
        files,
        entries,
        stashed,
        unchanged: unchanged.iter().map(ToString::to_string).collect(),
        dropped_unstash,
        dropped_change,
        skipped,
        unstash,
    })
}

/// The safety entry of a reset, from the record's tree `base` to `tree`, with its `stash.pushed`.
struct Push {
    base: ObjectHash,
    tree: ObjectHash,
    revision: RevisionId,
    actor: String,
    event: NewEvent,
}

fn pushed_entry(
    actor: &Actor,
    revision: RevisionId,
    base: ObjectHash,
    tree: ObjectHash,
    files: &[String],
    entries: usize,
) -> Result<Push, ProjectError> {
    let event = stash_event(
        types::STASH_PUSHED,
        actor,
        revision,
        files,
        json!({"stash": "stash@{0}", "entries": entries + 1, "reset": true}),
    )?;
    Ok(Push {
        base,
        tree,
        revision,
        actor: actor.event_actor().to_string(),
        event,
    })
}

/// The newest entry, whose unstash conflict waits in the folder, as the reset drops it, with its `stash.discarded`: the safety entry of the reset takes the folder as it is, markers and all.
struct ConflictDrop {
    id: i64,
    event: NewEvent,
}

async fn dropped_entry(
    handle: &ProjectHandle,
    actor: &Actor,
    entry: &fold_store::workspaces::StashRow,
    entries: usize,
) -> Result<ConflictDrop, ProjectError> {
    let files = changed(handle, entry.base_tree, entry.tree).await?;
    let conflicted = paths_of(&entry.conflicts);
    let event = stash_event(
        types::STASH_DISCARDED,
        actor,
        entry.revision,
        &files,
        json!({
            "stash": "stash@{0}",
            "dropped": 1,
            "entries": entries - 1,
            "conflicted": conflicted.iter().take(LISTED_PATHS).collect::<Vec<_>>(),
            "conflicted_count": conflicted.len(),
            "reset": true,
        }),
    )?;
    Ok(ConflictDrop {
        id: entry.id,
        event,
    })
}

/// Drops the conflicted entry and pushes the safety entry in one transaction, with their events, so a stop leaves both or neither.
async fn write_entries(
    handle: &ProjectHandle,
    drop: Option<&ConflictDrop>,
    push: Option<&Push>,
) -> Result<(), ProjectError> {
    let now = handle.clock.now();
    let buffer = handle
        .store
        .write(async |tx| -> Result<EventBuffer, ProjectError> {
            let mut buffer = EventBuffer::new();
            if let Some(drop) = drop {
                if !drop_stash(tx, drop.id, now).await? {
                    return Err(StoreError::Corrupt("folder_stashes.state").into());
                }
                buffer.append(tx, &drop.event, now).await?;
            }
            if let Some(push) = push {
                let new = NewStash {
                    base_tree: push.base,
                    tree: push.tree,
                    revision: push.revision,
                    actor: &push.actor,
                    now,
                };
                insert_stash(tx, &new).await?;
                buffer.append(tx, &push.event, now).await?;
            }
            Ok(buffer)
        })
        .await?;
    handle.bus.publish(buffer);
    Ok(())
}

/// Closes the conflicted human change that waits for the main agent, when one does, as `superseded` with `human_change.withdrawn`, under the integration mutex, so no integration resolves it meanwhile. With `abandon`, as for `fld reset`, the same transaction abandons the main workspace that its merge made, with `workspace.abandoned`, under the workspace's operation lock too, so no command of the main agent's worker runs meanwhile, and the main agent's notice says fold abandoned it. Without, as for `fld restore`, the workspace stays as `fld stash` leaves it, and the notice asks the main agent to abandon it.
async fn drop_waiting_change(
    handle: &ProjectHandle,
    actor: &Actor,
    abandon: bool,
) -> Result<Option<DroppedChange>, ProjectError> {
    let _serial = handle.integration.clone().lock_owned().await;
    let waiting = {
        let mut conn = handle
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        workspaces::conflicted_human_change(&mut conn).await?
    };
    let Some(waiting) = waiting else {
        return Ok(None);
    };
    let workspace = waiting.workspace;
    let abandoned_workspace = workspace.filter(|_| abandon);
    let _op = match abandoned_workspace {
        Some(workspace) => Some(handle.workspace_locks.lock(workspace).await),
        None => None,
    };
    let conflicted = paths_of(&waiting.conflicts);
    let internal = |error: fold_core::events::EventError| {
        FoldError::new(codes::INTERNAL_ERROR, error.to_string())
    };
    let command = if abandon { "reset" } else { "restore" };
    let withdrawn = |abandoned: bool| {
        NewEvent::new(types::HUMAN_CHANGE_WITHDRAWN, actor.event_actor())
            .and_then(|event| {
                event.data(json!({
                    "human": format!("human:{}", waiting.human),
                    "commit": waiting.commit,
                    "conflicted": conflicted.iter().take(LISTED_PATHS).collect::<Vec<_>>(),
                    "conflicted_count": conflicted.len(),
                    "workspace": workspace.map(|workspace| workspace.to_string()),
                    "workspace_abandoned": abandoned,
                    command: true,
                }))
            })
            .map_err(internal)
    };
    let abandoned = match abandoned_workspace {
        Some(workspace) => Some(
            NewEvent::new(types::WORKSPACE_ABANDONED, actor.event_actor())
                .and_then(|event| {
                    event.workspace(workspace).data(json!({
                        "reason": "the human reset the project folder and dropped the change whose merge it held",
                    }))
                })
                .map_err(internal)?,
        ),
        None => None,
    };
    let now = handle.clock.now();
    let written = handle
        .store
        .write(
            async |tx| -> Result<Option<(EventBuffer, bool)>, ProjectError> {
                let mut buffer = EventBuffer::new();
                if !workspaces::supersede_conflicted(tx, now).await? {
                    return Ok(None);
                }
                let closed = match abandoned_workspace {
                    Some(workspace) => {
                        workspaces::close(tx, workspace, ClosedState::Abandoned, now).await?
                    }
                    None => false,
                };
                buffer.append(tx, &withdrawn(closed)?, now).await?;
                if closed && let Some(event) = &abandoned {
                    buffer.append(tx, event, now).await?;
                }
                Ok(Some((buffer, closed)))
            },
        )
        .await?;
    let Some((buffer, closed)) = written else {
        return Ok(None);
    };
    handle.bus.publish(buffer);
    if closed && let Some(workspace) = abandoned_workspace {
        handle.live.detach(workspace).await;
    }
    tracing::info!(
        conflicted = conflicted.len(),
        "the reset or restore dropped the human change that waited for the main agent"
    );
    Ok(Some(DroppedChange {
        commit: waiting.commit,
        conflicted,
        workspace,
        workspace_abandoned: closed,
    }))
}
