use std::sync::Arc;

use fold_core::events::types;
use fold_core::merge::ConflictKind;
use fold_core::object::ObjectHash;
use fold_core::tree::{EntryKind, FlatTree};
use fold_core::{Actor, FoldError, NewEvent, RevisionId, Timestamp, codes, policy};
use fold_core::{TaskDescription, TaskKind, TaskTitle};
use fold_store::StoreError;
use fold_store::workspaces::{
    self, ConflictRow, NewStash, StashRow, StashState, apply_stash, conflict_stash, drop_stash,
    insert_stash, stash_stack,
};
use fold_vcs::checkout::{folder_changes, follow};
use fold_vcs::compare::load_flat;
use fold_vcs::git::Git;
use fold_vcs::git_remote::{BranchState, reset_index_paths};
use fold_vcs::index::StatIndex;
use fold_vcs::integrate::tree_merge;
use fold_vcs::objects::ObjectStore;
use fold_vcs::platform::Capabilities;
use fold_vcs::scan::Skipped;
use fold_vcs::stash::{changed_paths, occupied_paths, open_conflicts};
use fold_vcs::sync::{MarkerLabels, STASH_SIDECAR_SUFFIX, sidecar_with, write_merge_with};
use serde_json::json;

use super::record::{head_moved, snapshot_folder, sync, withdraw_waiting};
use super::remote::blocking;
use super::{FolderFollowed, ProjectError, load_config};
use crate::projects::ProjectHandle;
use crate::services::events::EventBuffer;
#[cfg(feature = "failpoints")]
use crate::services::fired;

/// The most paths a stash event lists; the rest are counted.
const LISTED_PATHS: usize = 50;

/// What `fld stash` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StashOutcome {
    /// The project folder holds no change that is not recorded.
    Nothing {
        revision: RevisionId,
        /// The entries on the stack.
        entries: usize,
        /// New files the read left out, such as one over the size limit, which stay in the folder.
        skipped: Vec<Skipped>,
    },
    /// The changes are the newest entry, `stash@{0}`, and their paths hold the checkout record's entries again; the folder follows main.
    Stashed {
        /// The main revision the folder holds now.
        revision: RevisionId,
        /// The paths of the entry, in path order.
        files: Vec<String>,
        /// The entries on the stack now.
        entries: usize,
        /// New files the read left out, such as one over the size limit, which stay in the folder and are not in the entry.
        skipped: Vec<Skipped>,
    },
}

/// One path where an unstash conflicted with the project folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnstashConflict {
    pub path: String,
    pub kind: ConflictKind,
    /// The file or directory beside the path that holds the stash's side of a conflict that takes no markers.
    pub sidecar: Option<String>,
}

/// What `fld unstash` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnstashOutcome {
    /// The newest entry merged cleanly into the folder and left the stack; its files are changes that no revision records again.
    Applied {
        /// The paths of the entry, in path order.
        files: Vec<String>,
        entries: usize,
    },
    /// The merge conflicted: the folder holds conflict markers, or the stash's side beside a file, and the entry stays on the stack until a record finds them fixed.
    Conflicted {
        /// The main revision the folder holds, which the markers name.
        revision: RevisionId,
        files: Vec<String>,
        /// In path order.
        conflicted: Vec<UnstashConflict>,
        entries: usize,
    },
}

/// One entry of the stack as `fld stash list` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashEntry {
    /// Its place on the stack: 0 is the newest, `stash@{0}`.
    pub position: usize,
    pub created_at: Timestamp,
    /// The main revision the folder held when the entry was made.
    pub base: RevisionId,
    /// The event actor label of the human who made it.
    pub actor: String,
    /// The paths of the entry, in path order.
    pub files: Vec<String>,
    /// The paths where its unstash conflicted, while the markers wait; empty otherwise.
    pub conflicted: Vec<String>,
}

/// Puts the human's unrecorded changes in the project folder aside, for the named human who runs `fld stash`, so the folder follows main without them and nothing is lost. fold's own stack keeps them, the same with Git sync on or off; it is never a Git stash, which fold's Git watch reads as a change fold did not make.
///
/// Under the project's remote lock and folder lock, the folder first follows main as `fld record` makes it, and a branch or `HEAD` that shows a change fold did not make is refused with the command that undoes it. The folder is then read as a record reads it. Its difference from the checkout record, files changed, added and removed, is the entry, with the record's tree as its base: its objects are stored, then one transaction pushes the entry and appends `stash.pushed`. Only then do those paths go back: while Git sync is on their index entries first go back to the branch's, so a staged change of them does not keep the branch where it is, and no Git stash is made; then the files go back to the record's entries, each only while it still holds what fold read, so a stop never leaves the changes in neither place. A conflicted human change from an earlier `fld record` that waits for the main agent is then closed with `human_change.withdrawn`, as a record that finds the folder clean closes it: the entry holds its changes. The folder then follows main. Nothing is stashed while an unstash's conflict markers wait in the folder, or while a file of the checkout record is too large or unreadable for the read, since its removal could not be put back.
pub async fn stash(
    handle: &Arc<ProjectHandle>,
    actor: &Actor,
    git: &Git,
) -> Result<StashOutcome, ProjectError> {
    policy::can_stash(actor).map_err(FoldError::from)?;
    let (handle, actor, git) = (Arc::clone(handle), actor.clone(), git.clone());
    spawned(async move { run_stash(&handle, &actor, &git).await }).await
}

/// Merges the newest entry of the stash into the project folder as it is now, for the named human who runs `fld unstash`: the entry's change from its base to its tree, merged three-way with the folder's files, including changes the human made since that no revision records.
///
/// Under the remote lock and folder lock the folder follows main first, with the same refusal as `fld stash`, and a folder that still holds the markers of an earlier unstash of the entry, which a stop between its files and its row left, is refused and keeps the entry as conflicted; so is a write where the folder holds a file the read leaves out, such as an ignored one. A clean merge writes the merged files, each only while it still holds what fold read, and one transaction then closes the entry with `stash.applied`; the files are changes that no revision records, which `fld record` records. A conflict writes markers labelled `main (r<n>)` and `stash@{0}` into each text file, and the stash's side of any other conflict as `<path>.fold-stash`, then keeps the entry as conflicted with its paths and `stash.conflicted`: the folder stays where it is until the human fixes them and `fld record` finds no marker left, which drops the entry. The files are written before the row, so a stop between leaves the entry on the stack and its change in the folder, and the next unstash finds it there.
pub async fn unstash(
    handle: &Arc<ProjectHandle>,
    actor: &Actor,
    git: &Git,
) -> Result<UnstashOutcome, ProjectError> {
    policy::can_stash(actor).map_err(FoldError::from)?;
    let (handle, actor, git) = (Arc::clone(handle), actor.clone(), git.clone());
    spawned(async move { run_unstash(&handle, &actor, &git).await }).await
}

/// The stack, newest entry first, with the paths of each.
pub async fn stash_list(handle: &ProjectHandle) -> Result<Vec<StashEntry>, ProjectError> {
    let stack = stack(handle).await?;
    let mut entries = Vec::with_capacity(stack.len());
    for (position, entry) in stack.into_iter().enumerate() {
        entries.push(StashEntry {
            position,
            created_at: entry.created_at,
            base: entry.revision,
            files: entry_files(handle, &entry).await?,
            conflicted: if entry.state == StashState::Conflicted {
                paths_of(&entry.conflicts)
            } else {
                Vec::new()
            },
            actor: entry.actor,
        });
    }
    Ok(entries)
}

/// Runs a command to its end on its own task, so a client that goes away does not stop it between the files and the row.
async fn spawned<T: Send + 'static>(
    work: impl Future<Output = Result<T, ProjectError>> + Send + 'static,
) -> Result<T, ProjectError> {
    tokio::spawn(work).await.unwrap_or_else(|error| {
        Err(FoldError::new(
            codes::INTERNAL_ERROR,
            format!("the command stopped: {error}"),
        )
        .into())
    })
}

async fn run_stash(
    handle: &Arc<ProjectHandle>,
    actor: &Actor,
    git: &Git,
) -> Result<StashOutcome, ProjectError> {
    let config = load_config(&handle.paths).await?;
    let _remote = handle.remote_lock.clone().lock_owned().await;
    let _folder = handle.folder_lock.clone().lock_owned().await;
    let (followed, _) = sync(handle, git, false).await?;
    refuse_branch(&config.git.branch, &followed)?;
    let stack = stack(handle).await?;
    if let Some(entry) = stack
        .first()
        .filter(|entry| entry.state == StashState::Conflicted)
    {
        return Err(markers_wait(&entry.conflicts).into());
    }
    let record = workspaces::checkout(handle.store.read())
        .await?
        .ok_or(StoreError::Corrupt("checkout"))?;
    let (tree, skipped) = snapshot_folder(handle, config.max_file_bytes).await?;
    let newest = stack.first().map(|entry| (entry.base_tree, entry.tree));
    let open = refuse_unstashable(handle, record.tree, tree, &skipped, newest).await?;
    if let Some(entry) = stack.first()
        && !open.is_empty()
    {
        return Err(
            hold_left_markers(handle, actor, entry, tree, open, stack.len())
                .await?
                .into(),
        );
    }
    if tree == record.tree {
        withdraw_waiting(handle, actor).await?;
        return Ok(StashOutcome::Nothing {
            revision: record.revision,
            entries: stack.len(),
            skipped,
        });
    }
    let files = changed(handle, record.tree, tree).await?;
    // A stop after the row and before the files left this very entry; it is not pushed twice.
    let pushed = stack
        .first()
        .is_some_and(|entry| entry.base_tree == record.tree && entry.tree == tree);
    if !pushed {
        let label = actor.event_actor().to_string();
        let event = stash_event(
            types::STASH_PUSHED,
            actor,
            record.revision,
            &files,
            json!({"stash": "stash@{0}", "entries": stack.len() + 1}),
        )?;
        let now = handle.clock.now();
        let new = NewStash {
            base_tree: record.tree,
            tree,
            revision: record.revision,
            actor: &label,
            now,
        };
        let buffer = handle
            .store
            .write(async |tx| -> Result<EventBuffer, ProjectError> {
                let mut buffer = EventBuffer::new();
                insert_stash(tx, &new).await?;
                buffer.append(tx, &event, now).await?;
                Ok(buffer)
            })
            .await?;
        handle.bus.publish(buffer);
    }
    #[cfg(feature = "failpoints")]
    fail::fail_point!("stash_after_row", |_| Err(fired("stash_after_row").into()));
    // Before the files, so a stop between leaves the changes in the folder, where the next stash finds them and resets the index again.
    if workspaces::git_sync(handle.store.read()).await?.is_some() {
        let (git, paths) = (git.clone(), handle.paths.clone());
        let reset: Vec<Vec<u8>> = files.iter().map(|file| file.as_bytes().to_vec()).collect();
        blocking(move || {
            let tip = git.ref_commit(&paths.git_dir(), "HEAD")?;
            reset_index_paths(&git, &paths, tip.as_ref(), &reset)
        })
        .await?;
    }
    put_back(handle, record.tree, tree, config.max_file_bytes).await?;
    // The folder holds none of the human's changes now and the stash holds them, so a conflicted human change that waits for the main agent is closed as a record that finds the folder clean closes it.
    withdraw_waiting(handle, actor).await?;
    let (followed, _) = sync(handle, git, false).await?;
    let revision = match followed.at {
        Some(at) => at,
        None => {
            workspaces::checkout(handle.store.read())
                .await?
                .ok_or(StoreError::Corrupt("checkout"))?
                .revision
        }
    };
    tracing::info!(files = files.len(), "the human's changes are stashed");
    Ok(StashOutcome::Stashed {
        revision,
        files,
        entries: stack.len() + usize::from(!pushed),
        skipped,
    })
}

/// Refuses a stash, before anything is written, that could not put its paths back: a file of the checkout record that the folder read left out, too large, special or unreadable, would be a removal in the entry that no follow makes while the file is there. Returns the paths where `tree` holds the markers or `.fold-stash` sides of an unstash of the newest entry, which a stop between the files and the row leaves in the folder and which must not go into a new entry.
async fn refuse_unstashable(
    handle: &ProjectHandle,
    record: ObjectHash,
    tree: ObjectHash,
    skipped: &[Skipped],
    newest: Option<(ObjectHash, ObjectHash)>,
) -> Result<Vec<Vec<u8>>, ProjectError> {
    let fold_dir = handle.paths.fold_dir();
    let skipped = skipped.to_vec();
    let (left_out, open) = blocking(move || {
        let objects = ObjectStore::open_existing(&fold_dir)?;
        let recorded = load_flat(&objects, &record)?;
        let left_out: Vec<(String, &'static str)> = skipped
            .iter()
            .filter(|skip| {
                let key = skip.path.as_bytes();
                let mut below = key.to_vec();
                below.push(b'/');
                recorded.contains_key(key)
                    || recorded
                        .range(below.clone()..)
                        .next()
                        .is_some_and(|(found, _)| found.starts_with(&below))
            })
            .map(|skip| (skip.path.to_string(), skip.reason.as_str()))
            .collect();
        let open = match newest {
            Some((base, stashed)) => {
                let changed = changed_paths(
                    &load_flat(&objects, &base)?,
                    &load_flat(&objects, &stashed)?,
                );
                open_conflicts(&objects, &load_flat(&objects, &tree)?, &changed)?
                    .iter()
                    .map(|path| path.as_bytes().to_vec())
                    .collect()
            }
            None => Vec::new(),
        };
        Ok((left_out, open))
    })
    .await?;
    if left_out.is_empty() {
        return Ok(open);
    }
    let named: Vec<String> = left_out
        .iter()
        .map(|(path, reason)| format!("{path} ({reason})"))
        .collect();
    let paths: Vec<&String> = left_out.iter().map(|(path, _)| path).collect();
    Err(FoldError::new(
        codes::INVALID_REQUEST,
        format!(
            "fold tracks files that it cannot read from the project folder, so it cannot stash them: {}",
            named.join(", ")
        ),
    )
    .with_hint("make each file readable and no larger than max_file_bytes in .fold/config.toml, or remove it, then run fld stash again")
    .with_details([("reason", json!("skipped_tracked")), ("paths", json!(paths))])
    .into())
}

/// Brings the paths that differ between `base`, the checkout record, and `tree`, what fold read from the folder, back to `base`'s entries, as a follow from `tree` to `base`: nothing is written when one of them no longer holds what fold read.
async fn put_back(
    handle: &ProjectHandle,
    base: ObjectHash,
    tree: ObjectHash,
    max_file_bytes: u64,
) -> Result<(), ProjectError> {
    let objects_guard = handle.objects.clone().read_owned().await;
    let paths = handle.paths.clone();
    let changed_meanwhile = blocking(move || {
        let _objects = objects_guard;
        let fold_dir = paths.fold_dir();
        let objects = ObjectStore::open_existing(&fold_dir)?;
        let caps = probe(&fold_dir)?;
        let (stashed, record) = (load_flat(&objects, &tree)?, load_flat(&objects, &base)?);
        let root = paths.root();
        let changes = folder_changes(root, &stashed, &record, max_file_bytes, &caps)?;
        let followed = follow(&objects, root, &stashed, &record, &changes, caps)?;
        Ok(followed
            .held
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>())
    })
    .await?;
    if changed_meanwhile.is_empty() {
        return Ok(());
    }
    Err(FoldError::new(
        codes::INVALID_REQUEST,
        format!(
            "files changed while fold stashed them, so the project folder keeps them: {}",
            changed_meanwhile.join(", ")
        ),
    )
    .with_hint("stash@{0} holds the changes as fold read them; fld unstash merges them back")
    .with_details([
        ("reason", json!("changed_during_stash")),
        ("paths", json!(changed_meanwhile)),
    ])
    .into())
}

async fn run_unstash(
    handle: &Arc<ProjectHandle>,
    actor: &Actor,
    git: &Git,
) -> Result<UnstashOutcome, ProjectError> {
    let config = load_config(&handle.paths).await?;
    let _remote = handle.remote_lock.clone().lock_owned().await;
    let _folder = handle.folder_lock.clone().lock_owned().await;
    let (followed, _) = sync(handle, git, false).await?;
    refuse_branch(&config.git.branch, &followed)?;
    let stack = stack(handle).await?;
    let Some(entry) = stack.first() else {
        return Err(FoldError::new(codes::INVALID_REQUEST, "the stash is empty")
            .with_hint("fld stash puts the changes of the project folder aside")
            .with_details([("reason", json!("stash_empty"))])
            .into());
    };
    if entry.state == StashState::Conflicted {
        return Err(markers_wait(&entry.conflicts).into());
    }
    let record = workspaces::checkout(handle.store.read())
        .await?
        .ok_or(StoreError::Corrupt("checkout"))?;
    let (folder, _) = snapshot_folder(handle, config.max_file_bytes).await?;
    let files = changed(handle, entry.base_tree, entry.tree).await?;
    let objects_guard = handle.objects.clone().read_owned().await;
    let paths = handle.paths.clone();
    let (base, tree) = (entry.base_tree, entry.tree);
    let labels = MarkerLabels::unstash(record.revision, 0);
    let max_file_bytes = config.max_file_bytes;
    let written = blocking(move || {
        let _objects = objects_guard;
        let fold_dir = paths.fold_dir();
        let objects = ObjectStore::open_existing(&fold_dir)?;
        let changed = changed_paths(&load_flat(&objects, &base)?, &load_flat(&objects, &tree)?);
        let folder_flat = load_flat(&objects, &folder)?;
        let open = open_conflicts(&objects, &folder_flat, &changed)?;
        if !open.is_empty() {
            return Ok(Err(Blocked::Markers(
                open.iter().map(|path| path.as_bytes().to_vec()).collect(),
            )));
        }
        let merge = tree_merge(&objects, &base, &folder, &tree)?;
        let caps = probe(&fold_dir)?;
        let occupied = occupied_paths(paths.root(), &merge, &folder_flat, max_file_bytes, &caps)?;
        if !occupied.is_empty() {
            return Ok(Err(Blocked::Occupied(
                occupied.iter().map(ToString::to_string).collect(),
            )));
        }
        let index_path = paths.folder_index();
        let mut index = StatIndex::load(&index_path, caps);
        let conflicted = write_merge_with(
            &objects,
            paths.root(),
            &mut index,
            &merge,
            &folder,
            &labels,
            STASH_SIDECAR_SUFFIX,
        )
        .map_err(changed_during_unstash)?;
        // The index only spares work; a failed save makes the next scan read the files again.
        let _ = index.save(&index_path);
        #[cfg(feature = "failpoints")]
        fail::fail_point!("unstash_after_write", |_| Err(fired("unstash_after_write")));
        Ok(Ok(conflicted))
    })
    .await?;
    let conflicted = match written {
        Ok(conflicted) => conflicted,
        Err(Blocked::Markers(open)) => {
            let named = paths_of_bytes(&open);
            hold_left_markers(handle, actor, entry, folder, open, stack.len()).await?;
            return Err(markers_left(&named).into());
        }
        Err(Blocked::Occupied(occupied)) => return Err(occupied_refusal(&occupied).into()),
    };
    let rows: Vec<ConflictRow> = conflicted
        .iter()
        .map(|conflict| ConflictRow {
            path: conflict.path.as_bytes().to_vec(),
            kind: conflict.kind,
        })
        .collect();
    let id = entry.id;
    let entries = stack.len();
    let now = handle.clock.now();
    let event = if rows.is_empty() {
        stash_event(
            types::STASH_APPLIED,
            actor,
            entry.revision,
            &files,
            json!({"stash": "stash@{0}", "entries": entries - 1}),
        )?
    } else {
        let paths = paths_of(&rows);
        stash_event(
            types::STASH_CONFLICTED,
            actor,
            entry.revision,
            &files,
            json!({
                "stash": "stash@{0}",
                "entries": entries,
                "folder": record.revision.to_string(),
                "conflicted": paths.iter().take(LISTED_PATHS).collect::<Vec<_>>(),
                "conflicted_count": paths.len(),
            }),
        )?
    };
    let buffer = handle
        .store
        .write(async |tx| -> Result<EventBuffer, ProjectError> {
            let mut buffer = EventBuffer::new();
            let moved = if rows.is_empty() {
                apply_stash(tx, id, now).await?
            } else {
                conflict_stash(tx, id, &rows, now).await?
            };
            if !moved {
                return Err(StoreError::Corrupt("folder_stashes.state").into());
            }
            buffer.append(tx, &event, now).await?;
            Ok(buffer)
        })
        .await?;
    handle.bus.publish(buffer);
    // The files are unrecorded changes now, and a conflict holds the folder, which the project's attention raises at once.
    sync(handle, git, true).await?;
    if rows.is_empty() {
        tracing::info!(files = files.len(), "the newest stash entry is applied");
        return Ok(UnstashOutcome::Applied {
            files,
            entries: entries - 1,
        });
    }
    tracing::info!(
        conflicted = rows.len(),
        "the newest stash entry conflicts with the project folder; it stays on the stack"
    );
    let conflicted = conflicted
        .into_iter()
        .map(|conflict| {
            let sidecar = (conflict.kind != ConflictKind::BothModifiedText)
                .then(|| sidecar_with(&conflict.path, STASH_SIDECAR_SUFFIX).ok())
                .flatten()
                .map(|side| side.to_string());
            UnstashConflict {
                path: conflict.path.to_string(),
                kind: conflict.kind,
                sidecar,
            }
        })
        .collect();
    Ok(UnstashOutcome::Conflicted {
        revision: record.revision,
        files,
        conflicted,
        entries,
    })
}

/// Why an unstash writes nothing.
enum Blocked {
    /// The folder holds the markers or `.fold-stash` sides of an earlier unstash of the entry at these paths.
    Markers(Vec<Vec<u8>>),
    /// The folder holds a file the read left out where the unstash writes.
    Occupied(Vec<String>),
}

fn paths_of_bytes(paths: &[Vec<u8>]) -> Vec<String> {
    paths
        .iter()
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect()
}

/// The refusal of an unstash that would write where the folder holds a file the read leaves out.
fn occupied_refusal(paths: &[String]) -> FoldError {
    FoldError::new(
        codes::INVALID_REQUEST,
        format!(
            "the project folder holds files that fold leaves out where fld unstash writes: {}",
            paths.join(", ")
        ),
    )
    .with_hint("an ignore rule or the size limit keeps these files out of fold; move or remove each one, then run fld unstash again")
    .with_details([("reason", json!("unstash_path_occupied")), ("paths", json!(paths))])
}

/// Keeps the stashed newest `entry` on the stack as conflicted, with `open`, the paths where `folder`, the tree read from the project folder, holds the markers or `.fold-stash` sides of its unstash: a stop between the unstash's files and its row left them there, so the entry waits for the human's fix and the folder holds, as after any unstash that conflicts, and the `fld record` after the fix drops it. The kind of each path is read from what the folder and the entry hold there. In one transaction with `stash.conflicted`; returns the refusal of a stash or an unstash while the conflict waits.
pub(super) async fn hold_left_markers(
    handle: &ProjectHandle,
    actor: &Actor,
    entry: &StashRow,
    folder: ObjectHash,
    open: Vec<Vec<u8>>,
    entries: usize,
) -> Result<FoldError, ProjectError> {
    let fold_dir = handle.paths.fold_dir();
    let (base, tree) = (entry.base_tree, entry.tree);
    let rows = blocking(move || {
        let objects = ObjectStore::open_existing(&fold_dir)?;
        let (base, tree, folder) = (
            load_flat(&objects, &base)?,
            load_flat(&objects, &tree)?,
            load_flat(&objects, &folder)?,
        );
        let mut rows = Vec::with_capacity(open.len());
        for path in open {
            let kind = left_kind(&objects, &base, &tree, &folder, &path)?;
            rows.push(ConflictRow { path, kind });
        }
        Ok(rows)
    })
    .await?;
    let record = workspaces::checkout(handle.store.read())
        .await?
        .ok_or(StoreError::Corrupt("checkout"))?;
    let files = changed(handle, entry.base_tree, entry.tree).await?;
    let paths = paths_of(&rows);
    let event = stash_event(
        types::STASH_CONFLICTED,
        actor,
        entry.revision,
        &files,
        json!({
            "stash": "stash@{0}",
            "entries": entries,
            "folder": record.revision.to_string(),
            "conflicted": paths.iter().take(LISTED_PATHS).collect::<Vec<_>>(),
            "conflicted_count": paths.len(),
        }),
    )?;
    let (id, now) = (entry.id, handle.clock.now());
    let buffer = handle
        .store
        .write(async |tx| -> Result<EventBuffer, ProjectError> {
            let mut buffer = EventBuffer::new();
            if conflict_stash(tx, id, &rows, now).await? {
                buffer.append(tx, &event, now).await?;
            }
            Ok(buffer)
        })
        .await?;
    handle.bus.publish(buffer);
    tracing::info!(
        conflicted = rows.len(),
        "an unstash left its conflict in the project folder; the entry waits for the fix"
    );
    Ok(markers_wait(&rows))
}

/// The kind of a conflict an unstash left at `path`: a text conflict where the file holds markers, else the clash its `.fold-stash` side stands for.
fn left_kind(
    objects: &ObjectStore,
    base: &FlatTree,
    tree: &FlatTree,
    folder: &FlatTree,
    path: &[u8],
) -> Result<ConflictKind, FoldError> {
    if let Some(found) = folder.get(path)
        && found.kind != EntryKind::Symlink
    {
        let blob = objects.read_expecting(&found.hash, fold_core::object::ObjectKind::Blob)?;
        if fold_core::merge::contains_stash_marker_line(&String::from_utf8_lossy(&blob.payload)) {
            return Ok(ConflictKind::BothModifiedText);
        }
    }
    let mut below = path.to_vec();
    below.push(b'/');
    let directory = |tree: &FlatTree| {
        tree.range(below.clone()..)
            .next()
            .is_some_and(|(found, _)| found.starts_with(&below))
    };
    let (theirs, ours) = (tree.get(path), folder.get(path));
    let symlink = |entry: Option<&fold_core::tree::FlatEntry>| {
        entry.is_some_and(|entry| entry.kind == EntryKind::Symlink)
    };
    Ok(
        if directory(tree)
            || directory(folder)
            || (theirs.is_some() && ours.is_some() && symlink(theirs) != symlink(ours))
        {
            ConflictKind::TypeChange
        } else if theirs.is_none() || ours.is_none() {
            ConflictKind::ModifyDelete
        } else if base.get(path).is_none() {
            ConflictKind::AddAdd
        } else {
            ConflictKind::BothModifiedBinary
        },
    )
}

/// The newest entry, conflicted, whose markers or `.fold-stash` sides main holds at its conflicted paths: a Git commit of the human with the markers left in, which the Git job records as any commit, put them there. The folder waits for nothing of the entry any more, so the entry is dropped, and a task to fix those files on main is created as fold, with `stash.dropped` naming it, in one transaction. Runs before every follow, so a stop between the commit's record and this step is finished by the next follow.
pub(super) async fn settle_markers_on_main(handle: &ProjectHandle) -> Result<(), ProjectError> {
    let stack = stack(handle).await?;
    let Some(entry) = stack
        .first()
        .filter(|entry| entry.state == StashState::Conflicted)
    else {
        return Ok(());
    };
    let Some(head) = workspaces::head(handle.store.read()).await? else {
        return Ok(());
    };
    let fold_dir = handle.paths.fold_dir();
    let (main, conflicted) = (
        head.tree_hash,
        entry
            .conflicts
            .iter()
            .map(|row| row.path.clone())
            .collect::<Vec<_>>(),
    );
    let on_main = blocking(move || {
        let objects = ObjectStore::open_existing(&fold_dir)?;
        Ok(
            open_conflicts(&objects, &load_flat(&objects, &main)?, &conflicted)?
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        )
    })
    .await?;
    if on_main.is_empty() {
        return Ok(());
    }
    let listed = on_main.join(", ");
    let title = TaskTitle::parse("Fix the conflict markers of fld unstash on main")
        .map_err(|error| FoldError::new(codes::INTERNAL_ERROR, error.to_string()))?;
    let description = TaskDescription::new(format!(
        "A Git commit of the human recorded the conflict of fld unstash on main in {}: {listed}. Remove the `<<<<<<< main (r…)`, `=======` and `>>>>>>> stash@{{…}}` lines and keep the content the human wants, take the stash's side of a binary or type conflict from its `.fold-stash` file, and delete each `.fold-stash` file. Ask the human which side to keep where it is not clear.",
        head.number
    ));
    let input = crate::services::tasks::CreateTask {
        title,
        description,
        labels: Vec::new(),
    };
    let (id, now, revision) = (entry.id, handle.clock.now(), entry.revision);
    let entries = stack.len() - 1;
    let buffer = handle
        .store
        .write(async |tx| -> Result<EventBuffer, ProjectError> {
            let mut buffer = EventBuffer::new();
            if !drop_stash(tx, id, now).await? {
                return Ok(buffer);
            }
            let task = crate::services::tasks::create_in_tx(
                tx,
                &mut buffer,
                &Actor::System,
                &input,
                TaskKind::Proposal,
                now,
            )
            .await
            .map_err(FoldError::from)?;
            let recomputed = crate::services::tasks::recompute(tx, &[task], now)
                .await
                .map_err(FoldError::from)?;
            buffer.extend(recomputed.events);
            let event = NewEvent::new(types::STASH_DROPPED, Actor::System.event_actor())
                .and_then(|event| {
                    event.revision(revision).task(task).data(json!({
                        "base": revision.to_string(),
                        "main": head.number.to_string(),
                        "conflicted": on_main.iter().take(LISTED_PATHS).collect::<Vec<_>>(),
                        "conflicted_count": on_main.len(),
                        "entries": entries,
                        "task": task.to_string(),
                    }))
                })
                .map_err(|error| FoldError::new(codes::INTERNAL_ERROR, error.to_string()))?;
            buffer.append(tx, &event, now).await?;
            Ok(buffer)
        })
        .await?;
    handle.bus.publish(buffer);
    tracing::info!(
        revision = %head.number,
        "a Git commit put the conflict of an unstash on main; a task fixes it"
    );
    Ok(())
}

/// Closes the stashed newest entry as `applied` with `stash.applied` when `human_tree`, the folder a record found, holds the entry's own entry at every path the entry changes: the folder holds its changes, such as after a stop between a clean unstash's files and its row, so a later unstash does not apply them again.
pub(super) async fn apply_held_stash(
    handle: &ProjectHandle,
    actor: &Actor,
    human_tree: ObjectHash,
) -> Result<(), ProjectError> {
    let stack = stack(handle).await?;
    let Some(entry) = stack
        .first()
        .filter(|entry| entry.state == StashState::Stashed)
    else {
        return Ok(());
    };
    let fold_dir = handle.paths.fold_dir();
    let (base, tree) = (entry.base_tree, entry.tree);
    let held = blocking(move || {
        let objects = ObjectStore::open_existing(&fold_dir)?;
        let (base, tree, folder) = (
            load_flat(&objects, &base)?,
            load_flat(&objects, &tree)?,
            load_flat(&objects, &human_tree)?,
        );
        Ok(changed_paths(&base, &tree)
            .iter()
            .all(|path| folder.get(path) == tree.get(path)))
    })
    .await?;
    if !held {
        return Ok(());
    }
    let files = changed(handle, entry.base_tree, entry.tree).await?;
    let event = stash_event(
        types::STASH_APPLIED,
        actor,
        entry.revision,
        &files,
        json!({"stash": "stash@{0}", "entries": stack.len() - 1}),
    )?;
    let (id, now) = (entry.id, handle.clock.now());
    let buffer = handle
        .store
        .write(async |tx| -> Result<EventBuffer, ProjectError> {
            let mut buffer = EventBuffer::new();
            if apply_stash(tx, id, now).await? {
                buffer.append(tx, &event, now).await?;
            }
            Ok(buffer)
        })
        .await?;
    handle.bus.publish(buffer);
    tracing::info!("the folder holds the newest stash entry's changes; it is applied");
    Ok(())
}

/// A branch that is not fold's keeps the folder's files those of another state, so neither command reads or writes them.
fn refuse_branch(branch: &str, followed: &FolderFollowed) -> Result<(), FoldError> {
    match &followed.branch {
        Some(BranchState::Moved(moved)) => Err(head_moved(branch, moved)),
        Some(BranchState::Ahead { .. }) => Err(FoldError::new(
            codes::INVALID_REQUEST,
            "your Git commits on the project's branch wait for the main agent, so the project folder holds them",
        )
        .with_hint("ask the main agent to resolve them, then run the command again")
        .with_details([("reason", json!("git_commits_wait"))])),
        _ => Ok(()),
    }
}

async fn stack(handle: &ProjectHandle) -> Result<Vec<StashRow>, ProjectError> {
    let mut conn = handle
        .store
        .read()
        .acquire()
        .await
        .map_err(StoreError::from)?;
    Ok(stash_stack(&mut conn).await?)
}

async fn entry_files(
    handle: &ProjectHandle,
    entry: &StashRow,
) -> Result<Vec<String>, ProjectError> {
    changed(handle, entry.base_tree, entry.tree).await
}

/// The paths that differ between the stored trees `base` and `tree`, in path order.
async fn changed(
    handle: &ProjectHandle,
    base: ObjectHash,
    tree: ObjectHash,
) -> Result<Vec<String>, ProjectError> {
    let fold_dir = handle.paths.fold_dir();
    blocking(move || {
        let objects = ObjectStore::open_existing(&fold_dir)?;
        Ok(
            changed_paths(&load_flat(&objects, &base)?, &load_flat(&objects, &tree)?)
                .iter()
                .map(|path| String::from_utf8_lossy(path).into_owned())
                .collect(),
        )
    })
    .await
}

fn paths_of(rows: &[ConflictRow]) -> Vec<String> {
    rows.iter()
        .map(|row| String::from_utf8_lossy(&row.path).into_owned())
        .collect()
}

fn stash_event(
    kind: fold_core::EventType,
    actor: &Actor,
    base: RevisionId,
    files: &[String],
    mut data: serde_json::Value,
) -> Result<NewEvent, FoldError> {
    data["base"] = json!(base.to_string());
    data["files"] = json!(files.iter().take(LISTED_PATHS).collect::<Vec<_>>());
    data["file_count"] = json!(files.len());
    NewEvent::new(kind, actor.event_actor())
        .and_then(|event| event.revision(base).data(data))
        .map_err(|error| FoldError::new(codes::INTERNAL_ERROR, error.to_string()))
}

/// The refusal while the markers of an unstash wait in the folder.
fn markers_wait(conflicts: &[ConflictRow]) -> FoldError {
    let paths = paths_of(conflicts);
    FoldError::new(
        codes::INVALID_REQUEST,
        format!(
            "the conflict of fld unstash waits in the project folder: {}",
            paths.join(", ")
        ),
    )
    .with_hint("fix the conflict markers in these files, then run fld record")
    .with_details([
        ("reason", json!("stash_conflicted")),
        ("paths", json!(paths)),
    ])
}

/// The refusal of files that still hold the conflict of an unstash, which `fld record` and `fld unstash` share.
pub(super) fn markers_left(paths: &[String]) -> FoldError {
    FoldError::new(
        codes::INVALID_REQUEST,
        format!(
            "files still hold the conflict of fld unstash: {}",
            paths.join(", ")
        ),
    )
    .with_hint(
        "remove the conflict markers, keep the side you want and delete each .fold-stash file, then run the command again",
    )
    .with_details([("reason", json!("stash_markers")), ("paths", json!(paths))])
}

/// A file that changed while the unstash read the folder is refused with nothing written.
fn changed_during_unstash(error: FoldError) -> FoldError {
    if error.code() != codes::WORKSPACE_CHANGED_DURING_SYNC {
        return error;
    }
    FoldError::new(
        codes::INVALID_REQUEST,
        "files of the project folder changed while fold read them; nothing was written",
    )
    .with_hint("run fld unstash again")
    .with_details([("reason", json!("changed_during_unstash"))])
}

fn probe(fold_dir: &std::path::Path) -> Result<Capabilities, FoldError> {
    Capabilities::probe(fold_dir).map_err(|error| {
        FoldError::new(
            codes::WORKSPACE_IO,
            format!("cannot probe the file system: {}", error.kind()),
        )
    })
}
