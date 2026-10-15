use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use fold_core::events::types;
use fold_core::merge::{ConflictKind, TreeMerge};
use fold_core::object::ObjectHash;
use fold_core::{
    Actor, FoldError, IdKind, NewEvent, RevisionId, TaskDescription, TaskId, TaskTitle,
    WorkspaceId, codes, task_workspace_path,
};
use fold_store::workspaces::{self, ConflictRow, NewTaskWorkspace};
use fold_store::{StoreError, counters};
use fold_vcs::materialise::{MaterialiseOptions, materialise};
use fold_vcs::objects::ObjectStore;
use fold_vcs::platform::sync_dir;
use fold_vcs::sync::{ConflictedPath, MarkerLabels, write_merge};
use serde_json::json;

use super::lifecycle::{io_error, rename_into_place, staging_dir};
use super::{WorkspaceError, WorkspaceService, blocking, capabilities};
use crate::services::events::EventBuffer;
use crate::services::tasks::{CreateTask, create_in_tx, recompute};

/// The most conflicted paths a sync task's description lists; the rest are counted.
const LISTED_PATHS: usize = 200;

/// A `fold pull` whose merge conflicted, as the sync task records it.
#[derive(Debug, Clone)]
pub struct SyncWorkspace {
    /// The name of the remote the pull fetched from.
    pub remote: String,
    pub branch: String,
    /// The fetched tip of the remote branch, in full.
    pub remote_commit: String,
    pub remote_tree: ObjectHash,
    /// The marker label of the sync base: `base <short commit>`, or `base r1` without one.
    pub base_label: String,
    /// The head the merge ran against, which becomes the workspace's base.
    pub head: RevisionId,
    /// The merge of the sync base, the remote tree (ours) and main (theirs), with conflicts.
    pub merge: TreeMerge,
}

/// The sync task and its workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncTaskCreated {
    pub task: TaskId,
    pub workspace: WorkspaceId,
    /// Relative to the project root.
    pub path: String,
    /// In path order.
    pub conflicted: Vec<ConflictedPath>,
}

impl WorkspaceService {
    /// Creates the sync task of a conflicting pull with a task workspace that holds the remote's version and the merge written into it: markers labelled `workspace` (the remote), the sync base and `main r<head>` in each text conflict, and main's side as `<path>.fold-main` for every other conflict. The workspace has no worker, so the first agent that claims the task adopts it; its base is the head the merge ran against, and its conflicted paths keep `record_changes` refusing until they are resolved.
    ///
    /// The files are written in a staging directory first. One transaction then creates the task with the human as its creator and the remote commit in its `remote_commit` column, renames the directory to the task's directory before it commits, inserts the workspace, records the conflicts, runs `recompute` (the task shows In Progress) and appends `task.created`, `workspace.created` and `remote.pull_conflicted`. A failed transaction removes the directory. The caller holds the project's integration mutex, so main cannot move while this runs.
    pub async fn create_sync_workspace(
        &self,
        actor: &Actor,
        sync: SyncWorkspace,
    ) -> Result<SyncTaskCreated, WorkspaceError> {
        let paths = self.paths.clone();
        let probed = Arc::clone(&self.caps);
        let labels = MarkerLabels {
            ours: "workspace".to_owned(),
            base: sync.base_label.clone(),
            theirs: format!("main {}", sync.head),
        };
        let (remote_tree, merge) = (sync.remote_tree, sync.merge.clone());
        let (staging, mut index, conflicted) = blocking(move || {
            let ws_dir = paths.ws_dir();
            fs::create_dir_all(&ws_dir).map_err(io_error)?;
            let objects = ObjectStore::open(&paths.fold_dir()).map_err(FoldError::from)?;
            let caps = capabilities(&probed, &paths)?;
            let staging = staging_dir(&ws_dir)?;
            let opts = MaterialiseOptions {
                read_only: false,
                caps,
            };
            let written = materialise(&objects, &remote_tree, &staging, &opts)
                .map_err(FoldError::from)
                .and_then(|done| {
                    let mut index = done.index;
                    let conflicted = write_merge(
                        &objects,
                        &staging,
                        &mut index,
                        &merge,
                        &remote_tree,
                        &labels,
                    )?;
                    Ok((index, conflicted))
                });
            match written {
                Ok((index, conflicted)) => Ok((staging, index, conflicted)),
                Err(error) => {
                    let _ = fs::remove_dir_all(&staging);
                    Err(error)
                }
            }
        })
        .await?;
        let input = CreateTask {
            title: sync_title(&sync.remote, &sync.branch, &sync.remote_commit)?,
            description: sync_description(&sync, &conflicted),
            labels: Vec::new(),
        };
        let rows: Vec<ConflictRow> = conflicted
            .iter()
            .map(|conflict| ConflictRow {
                path: conflict.path.as_bytes().to_vec(),
                kind: conflict.kind,
            })
            .collect();
        // The directory the transaction renamed the staging directory to, removed again when the commit fails.
        let placed: Mutex<Option<PathBuf>> = Mutex::new(None);
        let root = self.paths.root().to_path_buf();
        let ws_dir = self.paths.ws_dir();
        let now = self.clock.now();
        let label = actor.event_actor();
        let mut buffer = EventBuffer::new();
        let written = self
            .store
            .write(async |tx| {
                let task = create_in_tx(tx, &mut buffer, actor, &input, now).await?;
                fold_store::tasks::set_remote_commit(tx, task, &sync.remote_commit).await?;
                let (from, ws) = (staging.clone(), ws_dir.clone());
                let root = root.clone();
                let (path, dir) = blocking(move || {
                    let name = move |earlier| task_workspace_path(task, earlier);
                    let (path, dir) = rename_into_place(&from, &root, &name, &[])?;
                    sync_dir(&ws).map_err(io_error)?;
                    Ok((path, dir))
                })
                .await?;
                *placed.lock().unwrap_or_else(PoisonError::into_inner) = Some(dir);
                let number = counters::allocate(tx, IdKind::Workspace).await?;
                let id =
                    WorkspaceId::from_number(number).ok_or(StoreError::Corrupt("counters.next"))?;
                let new = NewTaskWorkspace {
                    id,
                    task,
                    worker_id: None,
                    path: &path,
                    base: sync.head,
                    now,
                };
                workspaces::insert_task_workspace(tx, &new).await?;
                workspaces::set_conflicts(tx, id, &rows).await?;
                let created = NewEvent::new(types::WORKSPACE_CREATED, label.clone())?
                    .task(task)
                    .workspace(id)
                    .revision(sync.head)
                    .data(json!({"path": path, "worker": null}))?;
                buffer.append(tx, &created, now).await?;
                let conflicted = NewEvent::new(types::REMOTE_PULL_CONFLICTED, label.clone())?
                    .task(task)
                    .workspace(id)
                    .data(json!({
                        "remote": sync.remote,
                        "branch": sync.branch,
                        "remote_commit": sync.remote_commit,
                        "conflicted": rows.len(),
                    }))?;
                buffer.append(tx, &conflicted, now).await?;
                buffer.extend(recompute(tx, &[task], now).await?.events);
                Ok::<_, WorkspaceError>((task, id, path))
            })
            .await;
        let (task, id, path) = match written {
            Ok(created) => created,
            Err(error) => {
                let dir = placed
                    .into_inner()
                    .unwrap_or_else(PoisonError::into_inner)
                    .unwrap_or(staging);
                let removed = tokio::task::spawn_blocking(move || fs::remove_dir_all(dir)).await;
                if !matches!(removed, Ok(Ok(()))) {
                    tracing::warn!("a sync workspace directory without a row was not removed");
                }
                return Err(error);
            }
        };
        self.bus.publish(buffer);
        let index_path = self.paths.workspace_index(id);
        let saved = tokio::task::spawn_blocking(move || index.save(&index_path)).await;
        if !matches!(saved, Ok(Ok(()))) {
            tracing::warn!(workspace = %id, "the stat index was not saved; the next scan rebuilds it");
        }
        if let Some(live) = &self.live
            && let Err(error) = live.attach(id).await
        {
            tracing::warn!(workspace = %id, code = error.code().unwrap_or("store_error"), "the live state did not start");
        }
        tracing::info!(task = %task, workspace = %id, remote_commit = %sync.remote_commit, conflicted = conflicted.len(), "pull conflicted; sync task created");
        Ok(SyncTaskCreated {
            task,
            workspace: id,
            path,
            conflicted,
        })
    }
}

/// The first seven hex digits, as Git abbreviates a commit.
pub fn short_commit(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

fn sync_title(remote: &str, branch: &str, commit: &str) -> Result<TaskTitle, FoldError> {
    TaskTitle::parse(&format!("Sync {remote}/{branch} @{}", short_commit(commit))).map_err(|_| {
        FoldError::new(
            codes::INVALID_CONFIG,
            "the branch name does not make a task title",
        )
        .with_hint("set a branch name without control characters with fold remote set")
    })
}

fn sync_description(sync: &SyncWorkspace, conflicted: &[ConflictedPath]) -> TaskDescription {
    let mut text = format!(
        "`fold pull` brought in the commit `{commit}` of `{remote}/{branch}`, and it changes the same paths as fold main. This task's workspace holds the remote's version with main `{head}` merged into it; resolve it so main gets both sides.\n\n**Conflicted paths** ({count}):\n\n",
        commit = sync.remote_commit,
        remote = sync.remote,
        branch = sync.branch,
        head = sync.head,
        count = conflicted.len(),
    );
    for conflict in conflicted.iter().take(LISTED_PATHS) {
        text.push_str(&format!(
            "- `{}` ({})\n",
            String::from_utf8_lossy(conflict.path.as_bytes()),
            kind_text(conflict.kind)
        ));
    }
    if conflicted.len() > LISTED_PATHS {
        text.push_str(&format!(
            "- and {} more; `get_workspace` lists them all\n",
            conflicted.len() - LISTED_PATHS
        ));
    }
    text.push_str(&format!(
        "\n**Reading the markers.** In a text conflict, the lines after `<<<<<<< workspace` are the remote's, the lines after `||||||| {base}` are the sync base's, and the lines after `=======` up to `>>>>>>> main {head}` are main's. Every other conflict keeps the remote's version at the path and puts main's beside it as `<path>.fold-main`. Edit each path to the result both sides need, delete the markers and the `.fold-main` copies, then record the changes and open a proposal. Its integration marks the remote commit as merged, and the next `fold push` sends it as a merge.\n",
        base = sync.base_label,
        head = sync.head,
    ));
    TaskDescription::new(text)
}

fn kind_text(kind: ConflictKind) -> &'static str {
    match kind {
        ConflictKind::BothModifiedText => "both sides changed the text",
        ConflictKind::BothModifiedBinary => "both sides changed the binary file",
        ConflictKind::ModifyDelete => "one side changed it and the other deleted it",
        ConflictKind::AddAdd => "both sides added it",
        ConflictKind::TypeChange => "the sides hold different kinds of entry",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_commit_is_seven_digits_or_the_whole_text() {
        assert_eq!(short_commit("0123456789abcdef"), "0123456");
        assert_eq!(short_commit("abc"), "abc");
    }

    #[test]
    fn a_branch_that_breaks_the_title_is_refused() {
        assert_eq!(
            sync_title("origin", "main", "0123456789").unwrap().as_str(),
            "Sync origin/main @0123456"
        );
        assert_eq!(
            sync_title("origin", "a\nb", "0123456789")
                .unwrap_err()
                .code(),
            codes::INVALID_CONFIG
        );
    }

    #[test]
    fn the_description_names_the_commit_and_counts_the_paths_it_leaves_out() {
        let sync = SyncWorkspace {
            remote: "origin".to_owned(),
            branch: "main".to_owned(),
            remote_commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            remote_tree: fold_core::object::hash_object(fold_core::object::ObjectKind::Tree, b""),
            base_label: "base 0123456".to_owned(),
            head: RevisionId::from_number(4).unwrap(),
            merge: TreeMerge {
                merged: fold_core::tree::FlatTree::new(),
                new_blobs: Vec::new(),
                conflicts: Vec::new(),
            },
        };
        let conflicted: Vec<ConflictedPath> = (0..LISTED_PATHS + 3)
            .map(|index| ConflictedPath {
                path: fold_vcs::paths::RepoPath::parse(format!("f{index:03}").as_bytes()).unwrap(),
                kind: ConflictKind::AddAdd,
            })
            .collect();
        let text = sync_description(&sync, &conflicted);
        let text = text.as_str();
        assert!(text.contains("`0123456789abcdef0123456789abcdef01234567` of `origin/main`"));
        assert!(text.contains("- `f000` (both sides added it)"));
        assert!(!text.contains("`f200`"));
        assert!(text.contains("- and 3 more"));
        assert!(text.contains("`||||||| base 0123456`"));
        assert!(text.contains("`>>>>>>> main r4`"));
    }

    #[test]
    fn every_kind_has_its_own_text() {
        let texts: std::collections::BTreeSet<_> = [
            ConflictKind::BothModifiedText,
            ConflictKind::BothModifiedBinary,
            ConflictKind::ModifyDelete,
            ConflictKind::AddAdd,
            ConflictKind::TypeChange,
        ]
        .into_iter()
        .map(kind_text)
        .collect();
        assert_eq!(texts.len(), 5);
    }
}
