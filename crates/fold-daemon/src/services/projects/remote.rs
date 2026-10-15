use std::sync::Arc;

use fold_core::{Actor, FoldError, NewEvent, RevisionId, codes, events::types, policy};
use fold_store::workspaces::RevisionRow;
use fold_vcs::git::{CommitId, Git};
use fold_vcs::git_export::{ExportRevision, ExportTarget, Signature, write_commits};
use fold_vcs::git_remote::{
    MIRROR_DIR, fetch_branch, fold_ref, is_ancestor, prepare_mirror, push_branch, remote_ahead,
    set_ref,
};
use fold_vcs::git_sync_map::SyncMap;
use fold_vcs::objects::ObjectStore;
use serde_json::json;

use super::{ProjectError, load_config};
use crate::projects::ProjectHandle;
use crate::services::events::EventBuffer;
use crate::services::workspaces::{commit_message, unsynced};

/// What a push did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushOutcome {
    /// Every main revision is synced already; `last` is the newest mapped one.
    UpToDate {
        branch: String,
        last: Option<RevisionId>,
    },
    Synced {
        branch: String,
        first: RevisionId,
        last: RevisionId,
        commits: usize,
        /// The commit the remote branch points to.
        head: CommitId,
        /// False when an earlier push had reached the remote and only its record was lost, so nothing was sent.
        sent: bool,
    },
}

/// Writes the main revisions after the newest one in `.fold/git-sync.map` as commits in the bare mirror `.fold/git/` and pushes them to the `[git]` branch of the remote, never with force. `[git]` is read on every call, so `fold remote set` takes effect without a restart. A remote branch with commits that main lacks fails with `remote_ahead` before anything is sent. Only after the push succeeds does the map gain the new lines, and then one transaction appends `remote.pushed`. A push never changes main, tasks, workspaces or claims; a failed one writes no map line and no event, and the commits it wrote stay in the mirror for the next push. The project's remote lock and its Git sync lock are held from before the map is read until after it is written, so pushes and exports run one after the other. The push runs as its own task, so a caller that stops waiting never leaves the map written without its event.
pub async fn push(
    handle: &Arc<ProjectHandle>,
    actor: &Actor,
    git: &Git,
) -> Result<PushOutcome, ProjectError> {
    policy::can_push(actor).map_err(FoldError::from)?;
    let (handle, actor, git) = (Arc::clone(handle), actor.clone(), git.clone());
    tokio::spawn(async move { run(&handle, &actor, &git).await })
        .await
        .unwrap_or_else(|error| {
            Err(FoldError::new(codes::INTERNAL_ERROR, format!("the push stopped: {error}")).into())
        })
}

async fn run(
    handle: &ProjectHandle,
    actor: &Actor,
    git: &Git,
) -> Result<PushOutcome, ProjectError> {
    let config = load_config(&handle.paths).await?.git;
    let Some(remote) = config.remote else {
        return Err(
            FoldError::new(codes::REMOTE_NOT_SET, "the project has no Git remote")
                .with_hint("run fold remote set <url> in the project first")
                .into(),
        );
    };
    let branch = config.branch;
    let remote_guard = handle.remote_lock.clone().lock_owned().await;
    let fold_dir = handle.paths.fold_dir();
    let mirror = fold_dir.join(MIRROR_DIR);
    // Both locks move into each blocking step and come back out, so a task stopped by a shutdown never releases them while Git still runs.
    let (held, map, fetched) = {
        let (git, fold_dir, mirror, branch) = (
            git.clone(),
            fold_dir.clone(),
            mirror.clone(),
            branch.clone(),
        );
        blocking(move || {
            let (sync_lock, map) = SyncMap::lock(&fold_dir)?;
            prepare_mirror(&git, &mirror, &remote)?;
            let fetched = fetch_branch(&git, &mirror, &branch)?;
            Ok(((remote_guard, sync_lock), map, fetched))
        })
        .await?
    };
    let last = map.last().cloned();
    let (rows, base) = unsynced(&handle.store, last.as_ref()).await?;
    if rows.is_empty() {
        return Ok(PushOutcome::UpToDate {
            branch,
            last: last.map(|(number, _)| number),
        });
    }
    let revisions: Vec<ExportRevision> = rows.iter().map(export_revision).collect();
    let (added, sent) = {
        let (git, branch) = (git.clone(), branch.clone());
        blocking(move || {
            let _held = held;
            if let Some((number, commit)) = &last
                && !git.has_commit(&mirror, commit)?
            {
                return Err(diverged(*number, commit));
            }
            let refname = fold_ref(&branch);
            set_ref(
                &git,
                &mirror,
                &refname,
                base.as_ref().map(|base| &base.commit),
            )?;
            let objects = ObjectStore::open(&fold_dir)?;
            let target = ExportTarget {
                repo: &mirror,
                refname: &refname,
            };
            let added = write_commits(
                &objects,
                &git,
                &target,
                base.as_ref(),
                &revisions,
                &fold_dir,
            )?;
            let Some((_, tip)) = added.last() else {
                return Ok((added, false));
            };
            let send = match &fetched {
                None => true,
                Some(remote) if remote == tip => false,
                Some(remote) if is_ancestor(&git, &mirror, remote, tip)? => true,
                Some(remote) => return Err(remote_ahead(&branch, Some(remote))),
            };
            if send {
                push_branch(&git, &mirror, &branch)?;
            }
            let mut map = map;
            map.append(&added)?;
            Ok((added, send))
        })
        .await?
    };
    let (Some((first, _)), Some((newest, head))) = (added.first(), added.last()) else {
        return Err(FoldError::new(codes::INTERNAL_ERROR, "a push wrote no commits").into());
    };
    let event = NewEvent::new(types::REMOTE_PUSHED, actor.event_actor())
        .and_then(|event| {
            event.data(json!({
                "branch": branch,
                "first": first.to_string(),
                "last": newest.to_string(),
                "commits": added.len(),
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
        branch,
        first: *first,
        last: *newest,
        commits: added.len(),
        head: head.clone(),
        sent,
    })
}

fn export_revision(row: &RevisionRow) -> ExportRevision {
    let fold = Signature {
        name: "fold".to_owned(),
        email: "fold@localhost".to_owned(),
    };
    let proposal = row
        .integration
        .as_ref()
        .and_then(|integration| integration.proposal.as_ref());
    let author = match proposal {
        Some(proposal) => author(&proposal.agent, &proposal.worker),
        None => fold.clone(),
    };
    ExportRevision {
        number: row.number,
        tree: row.tree_hash,
        created_at: row.created_at,
        message: commit_message(row),
        author,
        committer: fold,
    }
}

/// The worker that wrote a proposal as a Git author: `frontend/ui` is `frontend/ui <frontend.ui@fold.local>` and the main worker `frontend` is `frontend <frontend.frontend@fold.local>`. A deeper worker such as `frontend/ui/forms` gives `frontend.ui.forms@fold.local`.
fn author(agent: &str, worker: &str) -> Signature {
    let local = match worker.split_once('/') {
        Some((_, rest)) => rest.replace('/', "."),
        None => worker.to_owned(),
    };
    Signature {
        name: worker.to_owned(),
        email: format!("{agent}.{local}@fold.local"),
    }
}

fn diverged(revision: RevisionId, commit: &CommitId) -> FoldError {
    FoldError::new(
        codes::EXPORT_DIVERGED,
        format!(
            "the remote does not hold {commit}, the commit {revision} was synced as, so new commits cannot continue it"
        ),
    )
    .with_hint("set the remote to the repository this project was imported from or exported to")
    .with_details([
        ("revision", json!(revision.to_string())),
        ("commit", json!(commit.as_str())),
    ])
}

async fn blocking<T: Send + 'static>(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_author_email_joins_the_agent_and_the_worker_below_it() {
        let cases = [
            (
                "frontend",
                "frontend/ui",
                "frontend/ui",
                "frontend.ui@fold.local",
            ),
            (
                "frontend",
                "frontend",
                "frontend",
                "frontend.frontend@fold.local",
            ),
            (
                "frontend",
                "frontend/ui/forms",
                "frontend/ui/forms",
                "frontend.ui.forms@fold.local",
            ),
        ];
        for (agent, worker, name, email) in cases {
            let signature = author(agent, worker);
            assert_eq!(
                (signature.name.as_str(), signature.email.as_str()),
                (name, email)
            );
        }
    }
}
