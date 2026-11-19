use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use fold_core::{AgentName, FoldError, ProjectPaths, Timestamp, codes};
use fold_store::agents::{self, EndedWorker};
use fold_store::{Store, messages};
use fold_vcs::gc::{locate_worker_dir, remove_tree, tree_bytes};

use super::AgentError;
use crate::support::blocking;

/// The name a worker's folder takes while fold deletes it. No agent name holds a `.`, so it never meets the folder of an agent.
const REMOVING_PREFIX: &str = ".removing-";

/// Why fold keeps the folder of a task worker or reviewer that stopped before the `closed_workspace_days` window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderKeep {
    /// A worker row of the agent holds an unreleased claim.
    Claim,
    /// The agent has an open session, or an open session works in the folder.
    Session,
    /// An agent stopped the worker to wait for its go.
    Parked,
    /// Its last turn ended for a cause outside it, and fold owes it a re-run.
    Waiting,
    /// fold owes the worker a resume: requested changes, a conflict, failed checks, a send-back or a new version to review.
    OwedResume,
    /// A message waits for the worker, which fold starts again to answer it.
    OwedMessage,
    /// The folder is no directory, or a symlink.
    InvalidPath,
    /// The removal failed; the next run tries again.
    Failed,
}

/// What fold does, or would do, with the folder of one worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderDecision {
    Remove,
    Keep(FolderKeep),
}

/// The folder `.fold/workers/<agent>` of a task worker or reviewer that stopped before the window, with its size and what fold does about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerFolder {
    pub agent: AgentName,
    /// The bytes of the files below the folder.
    pub bytes: u64,
    pub decision: FolderDecision,
}

/// What ties ended workers to work beyond their own rows: the resumes and the messages fold owes them, and the open sessions' folders.
struct Owed {
    resumes: HashSet<AgentName>,
    messages: HashSet<AgentName>,
    session_folders: Vec<PathBuf>,
}

impl Owed {
    async fn read(store: &Store) -> Result<Self, AgentError> {
        let read = store.read();
        let resumes = agents::owed_starts(read)
            .await?
            .into_iter()
            .map(|owed| owed.resume.agent)
            .collect();
        let messages = messages::owed_to_stopped(read, None)
            .await?
            .into_iter()
            .map(|owed| owed.agent)
            .collect();
        let session_folders = agents::open_session_folders(read)
            .await?
            .into_iter()
            .map(PathBuf::from)
            .collect();
        Ok(Self {
            resumes,
            messages,
            session_folders,
        })
    }

    /// Whether the worker can never run again unless someone sends it new work, and nothing uses its folder, named by each of `dirs`.
    fn decide(&self, worker: &EndedWorker, dirs: &[PathBuf]) -> FolderDecision {
        let keep = if worker.claim {
            Some(FolderKeep::Claim)
        } else if worker.session || used(&self.session_folders, dirs) {
            Some(FolderKeep::Session)
        } else if worker.parked {
            Some(FolderKeep::Parked)
        } else if worker.waiting {
            Some(FolderKeep::Waiting)
        } else if self.resumes.contains(&worker.agent) {
            Some(FolderKeep::OwedResume)
        } else if self.messages.contains(&worker.agent) {
            Some(FolderKeep::OwedMessage)
        } else {
            None
        };
        keep.map_or(FolderDecision::Remove, FolderDecision::Keep)
    }
}

/// Removes, or with `dry_run` only lists, the folders `.fold/workers/<agent>` of the task workers and reviewers that stopped or failed before `cutoff` and can never run again on their own: no claim, no open session, not parked, no turn that waits, and no resume or message that fold owes them. Leads and the main agent keep theirs. Every file in the folder is one fold writes again at the worker's next start, which also makes its start folder again, so a message that starts the worker later still works; what the worker's shell left in its start folder is gone at that start too. Each removal first moves the folder aside inside a write that reads the worker's row again, so a start, which commits its row as `starting` before it writes the folder, either comes first and keeps the folder or comes after and writes a new one; the moved folder is deleted after the commit, and a run finishes the deletion that a stopped run left. Returns every such folder that exists, by agent name.
pub async fn collect_worker_folders(
    store: &Store,
    paths: &ProjectPaths,
    cutoff: Option<Timestamp>,
    dry_run: bool,
) -> Result<Vec<WorkerFolder>, AgentError> {
    if !dry_run {
        let workers = paths.workers_dir();
        if let Err(error) = blocking(move || remove_moved_folders(&workers)).await {
            tracing::warn!(
                code = error.code(),
                "a worker folder that fold moved aside was not deleted; the next run tries again"
            );
        }
    }
    let Some(cutoff) = cutoff else {
        return Ok(Vec::new());
    };
    let ended = agents::ended_short_lived_workers(store.read(), cutoff, None).await?;
    if ended.is_empty() {
        return Ok(Vec::new());
    }
    let workers = paths.workers_dir();
    let present = blocking(move || folder_names(&workers)).await?;
    let ended: Vec<EndedWorker> = ended
        .into_iter()
        .filter(|worker| present.contains(worker.agent.as_str()))
        .collect();
    if ended.is_empty() {
        return Ok(Vec::new());
    }
    let owed = Owed::read(store).await?;
    let mut folders = Vec::new();
    for worker in ended {
        let located = {
            let paths = paths.clone();
            let name = worker.agent.as_str().to_owned();
            blocking(move || {
                Ok(match locate_worker_dir(&paths, &name) {
                    Ok(Some(dir)) => tree_bytes(&dir).map(|bytes| Some((dir, bytes))),
                    Ok(None) => Ok(None),
                    Err(error) => Err(error),
                })
            })
            .await?
        };
        let (dir, bytes) = match located {
            Ok(Some(found)) => found,
            Ok(None) => continue,
            Err(error) => {
                let keep = if error.kind() == io::ErrorKind::InvalidInput {
                    FolderKeep::InvalidPath
                } else {
                    FolderKeep::Failed
                };
                tracing::warn!(agent = %worker.agent, kind = %error.kind(), "a worker folder was not read");
                folders.push(WorkerFolder {
                    agent: worker.agent,
                    bytes: 0,
                    decision: FolderDecision::Keep(keep),
                });
                continue;
            }
        };
        let mut decision = owed.decide(&worker, &[dir, paths.worker_dir(&worker.agent)]);
        if decision == FolderDecision::Remove && !dry_run {
            decision = match remove_folder(store, paths, &worker, cutoff).await {
                Ok(Some(decision)) => decision,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(agent = %worker.agent, code = error.log_code(), "a worker folder was not removed; the next run tries again");
                    FolderDecision::Keep(FolderKeep::Failed)
                }
            };
        }
        folders.push(WorkerFolder {
            agent: worker.agent,
            bytes,
            decision,
        });
    }
    Ok(folders)
}

/// Moves the folder of `worker` aside and deletes it, once a write has read its row again and found it ended at the same time with nothing new that ties it to work. `None` when the row changed or the folder is gone, so there is nothing to report.
async fn remove_folder(
    store: &Store,
    paths: &ProjectPaths,
    worker: &EndedWorker,
    cutoff: Timestamp,
) -> Result<Option<FolderDecision>, AgentError> {
    let moved = paths
        .workers_dir()
        .join(format!("{REMOVING_PREFIX}{}", worker.agent));
    let outcome = store
        .write(async |tx| -> Result<Option<FolderDecision>, AgentError> {
            let again =
                agents::ended_short_lived_workers(tx.conn(), cutoff, Some(&worker.agent)).await?;
            let Some(again) = again
                .into_iter()
                .find(|again| again.stopped_at == worker.stopped_at)
            else {
                return Ok(None);
            };
            let folders: Vec<PathBuf> = agents::open_session_folders(tx.conn())
                .await?
                .into_iter()
                .map(PathBuf::from)
                .collect();
            let dir = paths.worker_dir(&worker.agent);
            if again.claim
                || again.session
                || again.parked
                || again.waiting
                || used(&folders, &[dir])
            {
                return Ok(None);
            }
            let (paths, name, moved) = (
                paths.clone(),
                worker.agent.as_str().to_owned(),
                moved.clone(),
            );
            let renamed = blocking(move || {
                let located = locate_worker_dir(&paths, &name).map_err(folder_error)?;
                let Some(dir) = located else {
                    return Ok(false);
                };
                remove_tree(&moved).map_err(folder_error)?;
                fs::rename(&dir, &moved).map_err(folder_error)?;
                Ok(true)
            })
            .await?;
            Ok(renamed.then_some(FolderDecision::Remove))
        })
        .await?;
    if outcome.is_none() {
        return Ok(None);
    }
    #[cfg(feature = "failpoints")]
    fail::fail_point!("worker_folder_after_move", |_| {
        Err(AgentError::from(FoldError::new(
            codes::FAILPOINT,
            "worker_folder_after_move fired",
        )))
    });
    blocking(move || remove_tree(&moved).map_err(folder_error)).await?;
    tracing::debug!(agent = %worker.agent, "the folder of a finished worker was removed");
    Ok(outcome)
}

/// Whether an open session works in one of `dirs` or below it. A session's folder is the path its process reported, which may name the folder by its canonical path or by the project's.
fn used(session_folders: &[PathBuf], dirs: &[PathBuf]) -> bool {
    session_folders
        .iter()
        .any(|folder| dirs.iter().any(|dir| folder.starts_with(dir)))
}

/// The names of the entries below `workers`, so a run reads only the folders that are there. Blocking.
fn folder_names(workers: &Path) -> Result<HashSet<String>, FoldError> {
    let entries = match fs::read_dir(workers) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(error) => return Err(folder_error(error)),
    };
    let mut names = HashSet::new();
    for entry in entries {
        if let Ok(name) = entry.map_err(folder_error)?.file_name().into_string() {
            names.insert(name);
        }
    }
    Ok(names)
}

/// Deletes every folder below `workers` that fold moved aside to delete and a stopped run left. Blocking.
fn remove_moved_folders(workers: &Path) -> Result<(), FoldError> {
    let entries = match fs::read_dir(workers) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(folder_error(error)),
    };
    for entry in entries {
        let entry = entry.map_err(folder_error)?;
        let name = entry.file_name();
        if name
            .to_str()
            .is_some_and(|name| name.starts_with(REMOVING_PREFIX))
        {
            match fs::symlink_metadata(entry.path()) {
                Ok(meta) if meta.is_dir() => {
                    remove_tree(&entry.path()).map_err(folder_error)?;
                }
                Ok(_) => fs::remove_file(entry.path()).map_err(folder_error)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(folder_error(error)),
            }
        }
    }
    Ok(())
}

fn folder_error(error: io::Error) -> FoldError {
    let code = if error.kind() == io::ErrorKind::InvalidInput {
        codes::INVALID_PATH
    } else {
        codes::INTERNAL_ERROR
    };
    FoldError::new(
        code,
        format!("a worker folder could not be removed: {}", error.kind()),
    )
}
