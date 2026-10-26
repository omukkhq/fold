use std::sync::Arc;

use fold_core::object::ObjectKind;
use fold_core::tree::{EntryKind, FlatEntry};
use fold_core::{Actor, FoldError, RevisionId, codes, policy};
use fold_store::StoreError;
use fold_store::workspaces::{self, git_sync};
use fold_vcs::atomic::write_atomic;
use fold_vcs::compare::load_flat;
use fold_vcs::git::{CommitId, Git};
use fold_vcs::git_remote::{FOLD_STASH_REF, STASH_REF, exclude_fold_dir, set_ref};
use fold_vcs::git_sync_map::SyncMap;
use fold_vcs::ignore::GITIGNORE;
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::write_initial_tree;
use serde_json::json;

use super::commits::commit_main;
use super::folder::follow_folder;
use super::record::{HumanFolderChange, OnConflict, RecordOutcome, integrate_change};
use super::remote::blocking;
use super::{ProjectError, enable_sync, load_config, plan_gitignore};
use crate::projects::ProjectHandle;
use crate::services::events::EventBuffer;
#[cfg(feature = "failpoints")]
use crate::services::fired;

/// The Git identity a human gave for the project's own Git config; a value left out is not written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitIdentity {
    pub name: Option<String>,
    pub email: Option<String>,
}

/// What turning Git on did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitEnabled {
    /// Main's head once Git sync is on.
    pub head: RevisionId,
    /// The revision that records the `.gitignore` fold wrote or changed, when it changed one.
    pub gitignore: Option<RevisionId>,
    /// The commit the `[git]` branch points to now, when the project has a revision after the empty genesis.
    pub commit: Option<CommitId>,
    /// The configured remotes, by name.
    pub remotes: Vec<String>,
}

/// Turns Git on for a project that has none, for the named human who runs `fld init --with-git` in it. The project's remote lock and folder lock are held to the end. A project whose Git sync is on already is refused, and so is a `.git` with commits of its own, which fold cannot join to main's history; a `.git` without a commit, as `git init` or a stopped run leaves it, is taken as it is. The steps go in the order a stopped run can repeat: `git init` on the `[git]` branch, the identity into the repository's own config, `info/exclude` as the link to `.fold/exclude`, the folder brought to main, then the `.gitignore` copy rule (a missing `.gitignore` becomes a copy of `.foldignore` with a `.fold/` line, an existing one gets the line when it lacks it), recorded at once as the human's own revision with nothing else of the folder, then Git sync on with `git.sync_enabled`, and last the commits of every revision after the empty genesis with the branch and the index on the newest.
pub async fn enable_git(
    handle: &Arc<ProjectHandle>,
    actor: &Actor,
    git: &Git,
    identity: GitIdentity,
) -> Result<GitEnabled, ProjectError> {
    policy::can_enable_git(actor).map_err(FoldError::from)?;
    let Actor::Human { name: human } = actor else {
        return Err(
            FoldError::new(codes::HUMAN_REQUIRED, "only a human turns Git on")
                .with_hint("ask a human to run `fld init --with-git` in the project folder")
                .into(),
        );
    };
    for (field, value) in [("git_name", &identity.name), ("git_email", &identity.email)] {
        if value
            .as_deref()
            .is_some_and(|value| !fits_git_config(value))
        {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("{field} must be text a Git author line can carry"),
            )
            .with_details([("field", json!(field))])
            .into());
        }
    }
    let (handle, actor, git, human) = (
        Arc::clone(handle),
        actor.clone(),
        git.clone(),
        human.clone(),
    );
    tokio::spawn(async move {
        let _remote = handle.remote_lock.clone().lock_owned().await;
        let _folder = handle.folder_lock.clone().lock_owned().await;
        if git_sync(handle.store.read()).await?.is_some() {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                "Git sync is on already in this project",
            )
            .with_hint(
                "run `fld remote add <name> <url> [<branch>]` to push the project to a remote",
            )
            .with_details([("reason", json!("git_sync_on"))])
            .into());
        }
        let config = load_config(&handle.paths).await?;
        let branch = config.git.branch.clone();
        {
            let (git, paths, branch) = (git.clone(), handle.paths.clone(), branch.clone());
            blocking(move || prepare_repository(&git, &paths, &branch, &identity)).await?;
        }
        #[cfg(feature = "failpoints")]
        fail::fail_point!("enable_git_after_repository", |_| Err(fired(
            "enable_git_after_repository"
        )
        .into()));
        follow_folder(
            &handle.store,
            &handle.paths,
            &handle.objects,
            &git,
            handle.clock.now(),
        )
        .await?;
        let gitignore = record_gitignore(&handle, &actor, &human).await?;
        let label = actor.event_actor();
        let now = handle.clock.now();
        let buffer = handle
            .store
            .write(async |tx| -> Result<EventBuffer, ProjectError> {
                let mut buffer = EventBuffer::new();
                enable_sync(tx, &mut buffer, &label, &branch, now).await?;
                Ok(buffer)
            })
            .await?;
        handle.bus.publish(buffer);
        let followed = follow_folder(
            &handle.store,
            &handle.paths,
            &handle.objects,
            &git,
            handle.clock.now(),
        )
        .await?;
        let mut committed = commit_main(
            &handle.store,
            &handle.paths,
            &git,
            followed.at,
            handle.clock.as_ref(),
        )
        .await?;
        handle.bus.publish(std::mem::take(&mut committed.events));
        let head = workspaces::head(handle.store.read())
            .await?
            .ok_or(StoreError::Corrupt("refs.main"))?;
        let commit = SyncMap::load(&handle.paths.fold_dir())?
            .commit_of(head.number)
            .cloned();
        tracing::info!(head = %head.number, "Git sync is on");
        Ok(GitEnabled {
            head: head.number,
            gitignore,
            commit,
            remotes: config.remotes.keys().cloned().collect(),
        })
    })
    .await
    .unwrap_or_else(|error| {
        Err(FoldError::new(
            codes::INTERNAL_ERROR,
            format!("turning Git on stopped: {error}"),
        )
        .into())
    })
}

/// Makes the project folder's own repository ready for fold's commits: created on `branch` when `.git` is missing, the identity in its config and its `info/exclude` linked to `.fold/exclude`. A `.git` that holds a commit, or that is not a directory, is refused. Blocking.
fn prepare_repository(
    git: &Git,
    paths: &fold_core::ProjectPaths,
    branch: &str,
    identity: &GitIdentity,
) -> Result<(), FoldError> {
    let repo = paths.git_dir();
    match std::fs::symlink_metadata(&repo) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            git.init_checkout(paths.root(), branch)?;
        }
        Ok(meta) if meta.is_dir() => {
            if git.has_commits(&repo)? {
                return Err(FoldError::new(
                    codes::INVALID_REQUEST,
                    "the project folder holds a Git repository with commits of its own, which fold cannot join to main's history",
                )
                .with_hint("move .git out of the project folder, then run `fld init --with-git` again")
                .with_details([("reason", json!("git_history"))]));
            }
        }
        _ => {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                "the project folder holds a .git that is not a directory",
            )
            .with_hint("move .git out of the project folder, then run `fld init --with-git` again")
            .with_details([("reason", json!("git_dir_elsewhere"))]));
        }
    }
    for (key, value) in [
        ("user.name", &identity.name),
        ("user.email", &identity.email),
    ] {
        if let Some(value) = value {
            git.config_set_local(&repo, key, value)?;
        }
    }
    exclude_fold_dir(&repo)?;
    // A stash in the new repository is the human's own; only a change of it counts as one fold did not make.
    let stash = git.ref_commit(&repo, STASH_REF)?;
    if let Some(stash) = &stash {
        set_ref(git, &repo, FOLD_STASH_REF, Some(stash))?;
    }
    Ok(())
}

/// Writes the `.gitignore` of the copy rule and records it at once as the human's revision, with nothing else of the folder; `None` when the `.gitignore` keeps `.fold/` out already or is not a regular file.
async fn record_gitignore(
    handle: &ProjectHandle,
    actor: &Actor,
    human: &fold_core::HumanName,
) -> Result<Option<RevisionId>, ProjectError> {
    let root = handle.paths.root().to_owned();
    let Some(plan) = super::blocking(move || plan_gitignore(&root)).await? else {
        return Ok(None);
    };
    let head = workspaces::head(handle.store.read())
        .await?
        .ok_or(StoreError::Corrupt("refs.main"))?;
    let objects_guard = handle.objects.clone().read_owned().await;
    let fold_dir = handle.paths.fold_dir();
    let base = head.tree_hash;
    let tree = blocking(move || {
        let _objects = objects_guard;
        write_atomic(&plan.path, &plan.new).map_err(|error| {
            FoldError::new(
                codes::WORKSPACE_IO,
                format!("cannot write the .gitignore: {}", error.kind()),
            )
        })?;
        let objects = ObjectStore::open_existing(&fold_dir)?;
        let blob = objects.write(ObjectKind::Blob, &plan.new)?;
        let mut entries = load_flat(&objects, &base)?;
        entries.insert(
            GITIGNORE.as_bytes().to_vec(),
            FlatEntry {
                kind: EntryKind::File,
                hash: blob,
            },
        );
        write_initial_tree(&objects, entries)
    })
    .await?;
    if tree == base {
        return Ok(None);
    }
    let change = HumanFolderChange {
        human,
        base_tree: base,
        tree,
        commit: None,
    };
    match integrate_change(handle, actor, &change, OnConflict::Wait).await? {
        RecordOutcome::Recorded { revision, .. } => Ok(Some(revision)),
        other => Err(FoldError::new(
            codes::INTERNAL_ERROR,
            format!("the .gitignore did not reach main: {other:?}"),
        )
        .into()),
    }
}

/// Not blank, and no control character, `<` or `>`, which would break the author line of every commit.
fn fits_git_config(value: &str) -> bool {
    !value.trim().is_empty()
        && !value
            .chars()
            .any(|c| c.is_control() || c == '<' || c == '>')
}
