use std::sync::Arc;

use fold_core::{
    Actor, Clock, FoldError, NewEvent, ProjectConfig, ProjectPaths, RemoteConfig, RevisionId,
    codes, events::types, policy,
};
use fold_store::Store;
use fold_vcs::git::{CommitId, Git};
use fold_vcs::git_remote::{
    create_local_remote, fetch_branch, is_ancestor, prepare_mirror, project_repository,
    push_branch, remote_ahead,
};
use fold_vcs::git_sync_map::SyncMap;
use serde_json::json;

use super::commits::commit_locked;
use super::{GitAttention, ProjectAttention, ProjectError, load_config};
use crate::projects::ProjectHandle;
use crate::services::events::{EventBuffer, EventBus};

/// What a push did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushOutcome {
    /// The remote branch is on main's newest commit already; `last` is its revision.
    UpToDate {
        remote: String,
        branch: String,
        last: Option<RevisionId>,
    },
    Synced {
        remote: String,
        branch: String,
        first: RevisionId,
        last: RevisionId,
        commits: usize,
        /// The commit the remote branch points to.
        head: CommitId,
    },
}

/// Writes the commits that the project's repository lacks for main, as every integration does, then pushes main's newest commit to the branch of the remote `remote`, never with force, and appends `remote.pushed` in one transaction. Without a name the push goes to the only remote. The configuration is read on every call, so `fld remote set` takes effect without a restart. A remote that is an absolute local path that does not exist is created as a bare repository first. A remote branch with commits that main lacks fails with `remote_ahead` before anything is sent; one already on main's newest commit is up to date. A push never changes main, tasks, workspaces or claims, and a failed one writes no event. The project's remote lock and its Git sync lock are held throughout, so pushes, pulls and the commit of each integration run one after the other. The push runs as its own task, so a caller that stops waiting never leaves the remote moved without its event.
pub async fn push(
    handle: &Arc<ProjectHandle>,
    actor: &Actor,
    git: &Git,
    remote: Option<&str>,
) -> Result<PushOutcome, ProjectError> {
    policy::can_push(actor).map_err(FoldError::from)?;
    push_with(
        RemoteContext::of(handle),
        actor,
        git,
        remote.map(str::to_owned),
    )
    .await
}

/// What a push needs of an open project. The automatic push keeps one instead of the project's handle, so it never keeps a closed project open.
#[derive(Clone)]
pub struct RemoteContext {
    pub(crate) slug: String,
    pub(crate) paths: ProjectPaths,
    pub(crate) store: Store,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) bus: EventBus,
    pub(crate) remote_lock: Arc<tokio::sync::Mutex<()>>,
    /// What the commit writer last found that needs a person.
    pub(crate) git_attention: Arc<GitAttention>,
}

impl RemoteContext {
    /// The context of a project whose services run without a [`ProjectHandle`], with its own remote lock.
    pub fn new(
        slug: String,
        paths: ProjectPaths,
        store: Store,
        clock: Arc<dyn Clock>,
        bus: EventBus,
    ) -> Self {
        Self {
            slug,
            paths,
            store,
            clock,
            bus,
            remote_lock: Arc::new(tokio::sync::Mutex::new(())),
            git_attention: Arc::new(GitAttention::default()),
        }
    }

    /// What the commit writer last found that needs a person, as the project overview shows it.
    pub fn attention(&self) -> Vec<ProjectAttention> {
        self.git_attention.kinds()
    }

    pub(crate) fn of(handle: &ProjectHandle) -> Self {
        Self {
            slug: handle.slug.clone(),
            paths: handle.paths.clone(),
            store: handle.store.clone(),
            clock: handle.clock.clone(),
            bus: handle.bus.clone(),
            remote_lock: handle.remote_lock.clone(),
            git_attention: handle.git_attention.clone(),
        }
    }
}

/// [`push`] without the policy check, for a caller that checked it: the push runs as its own task.
pub(crate) async fn push_with(
    context: RemoteContext,
    actor: &Actor,
    git: &Git,
    remote: Option<String>,
) -> Result<PushOutcome, ProjectError> {
    let (actor, git) = (actor.clone(), git.clone());
    tokio::spawn(async move { run(&context, &actor, &git, remote.as_deref()).await })
        .await
        .unwrap_or_else(|error| {
            Err(FoldError::new(codes::INTERNAL_ERROR, format!("the push stopped: {error}")).into())
        })
}

async fn run(
    handle: &RemoteContext,
    actor: &Actor,
    git: &Git,
    remote: Option<&str>,
) -> Result<PushOutcome, ProjectError> {
    let config = load_config(&handle.paths).await?;
    let (name, remote) = select_remote(&config, remote)?;
    let (url, branch, local) = (remote.url, remote.branch, config.git.branch);
    let remote_guard = handle.remote_lock.clone().lock_owned().await;
    let mirror = handle.paths.git_dir();
    // Both locks move into each blocking step and come back out, so a task stopped by a shutdown never releases them while Git still runs.
    let (held, map, fetched) = {
        let (git, paths, name, branch) = (
            git.clone(),
            handle.paths.clone(),
            name.clone(),
            branch.clone(),
        );
        blocking(move || {
            let (sync_lock, map) = SyncMap::lock(&paths.fold_dir())?;
            let mirror = project_repository(&paths)?;
            create_local_remote(&git, &url, &branch)?;
            prepare_mirror(&git, &mirror, &name, &url)?;
            let fetched = fetch_branch(&git, &mirror, &name, &branch)?;
            Ok(((remote_guard, sync_lock), map, fetched))
        })
        .await?
    };
    let (held, map, _) = commit_locked(
        &handle.store,
        &handle.paths,
        git,
        &local,
        held,
        map,
        handle.clock.now(),
    )
    .await?;
    let entries = map.entries().to_vec();
    let Some((last, tip)) = entries.last().cloned() else {
        return Err(FoldError::new(codes::INTERNAL_ERROR, "main has no commit to push").into());
    };
    let sent_from = {
        let (git, name, branch, tip) = (git.clone(), name.clone(), branch.clone(), tip.clone());
        blocking(move || {
            let _held = held;
            let first = match &fetched {
                None => 0,
                Some(remote) if *remote == tip => return Ok(None),
                Some(remote) if is_ancestor(&git, &mirror, remote, &tip)? => {
                    first_unreached(&git, &mirror, &entries, remote)?
                }
                Some(remote) => return Err(remote_ahead(&branch, Some(remote))),
            };
            push_branch(&git, &mirror, &name, &tip, &branch)?;
            Ok(Some(first))
        })
        .await?
    };
    let Some(first_index) = sent_from else {
        return Ok(PushOutcome::UpToDate {
            remote: name,
            branch,
            last: Some(last),
        });
    };
    let first = map
        .entries()
        .get(first_index)
        .map_or(last, |(number, _)| *number);
    let commits = map.entries().len() - first_index;
    let event = NewEvent::new(types::REMOTE_PUSHED, actor.event_actor())
        .and_then(|event| {
            event.data(json!({
                "remote": name,
                "branch": branch,
                "first": first.to_string(),
                "last": last.to_string(),
                "commits": commits,
            }))
        })
        .map_err(|error| FoldError::new(codes::INTERNAL_ERROR, error.to_string()))?;
    let now = handle.clock.now();
    let buffer = handle
        .store
        .write(async |tx| -> Result<EventBuffer, ProjectError> {
            let mut buffer = EventBuffer::new();
            buffer.append(tx, &event, now).await?;
            Ok(buffer)
        })
        .await?;
    handle.bus.publish(buffer);
    Ok(PushOutcome::Synced {
        remote: name,
        branch,
        first,
        last,
        commits,
        head: tip,
    })
}

/// The index of the first map entry whose commit `reached` does not reach: the entry after the one that is `reached` or is its newest mapped ancestor, or 0 when it reaches none. Blocking.
pub(crate) fn first_unreached(
    git: &Git,
    repo: &std::path::Path,
    entries: &[(RevisionId, CommitId)],
    reached: &CommitId,
) -> Result<usize, FoldError> {
    if let Some(index) = entries.iter().position(|(_, commit)| commit == reached) {
        return Ok(index + 1);
    }
    for (index, (_, commit)) in entries.iter().enumerate().rev() {
        if is_ancestor(git, repo, commit, reached)? {
            return Ok(index + 1);
        }
    }
    Ok(0)
}

pub(super) fn remote_not_set() -> FoldError {
    FoldError::new(codes::REMOTE_NOT_SET, "the project has no Git remote")
        .with_hint("run fld remote set <name> <url> in the project first")
}

/// The remote `name` names, or the only remote when `name` is `None`. No remote, an unknown name and several remotes without a name are refused with a hint that names `fld remote list`.
pub(crate) fn select_remote(
    config: &ProjectConfig,
    name: Option<&str>,
) -> Result<(String, RemoteConfig), FoldError> {
    match name {
        Some(name) => config
            .remotes
            .get(name)
            .map(|remote| (name.to_owned(), remote.clone()))
            .ok_or_else(|| {
                FoldError::new(
                    codes::REMOTE_NOT_SET,
                    format!("the project has no Git remote named {name}"),
                )
                .with_hint("run fld remote list to see the remotes")
            }),
        None => {
            let mut remotes = config.remotes.iter();
            match (remotes.next(), remotes.next()) {
                (Some((name, remote)), None) => Ok((name.clone(), remote.clone())),
                (None, _) => Err(remote_not_set()),
                (Some(_), Some(_)) => Err(FoldError::new(
                    codes::INVALID_REQUEST,
                    "the project has several Git remotes, so the command must name one",
                )
                .with_hint("run fld remote list, then name the remote")
                .with_details([("reason", json!("several_remotes"))])),
            }
        }
    }
}

pub(super) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, FoldError> + Send + 'static,
) -> Result<T, ProjectError> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|error| {
            Err(FoldError::new(
                codes::INTERNAL_ERROR,
                format!("blocking work stopped: {error}"),
            ))
        })
        .map_err(ProjectError::from)
}
