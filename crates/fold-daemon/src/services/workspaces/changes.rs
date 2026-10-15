use std::sync::Arc;

use fold_core::{CheckpointId, FoldError, TaskId, WorkspaceId, codes};
use fold_store::workspaces::{self, WorkspaceKind, WorkspaceState};
use serde_json::json;

use super::{WorkspaceError, WorkspaceService, WorkspaceView};
use crate::live::WorkspaceLive;
use crate::services::tasks::{AttentionItem, attention};

/// A workspace as the workspace routes list it: its row, its live state when it is attached, and the attention items of its task.
#[derive(Debug, Clone)]
pub struct WorkspaceEntry {
    pub workspace: WorkspaceView,
    /// `None` for a missing or closed workspace, a review checkout, and an active one whose live state did not start.
    pub live: Option<Arc<WorkspaceLive>>,
    pub attention: Vec<AttentionItem>,
}

/// The working changes of an active task workspace after a fresh scan. These are working files, never history.
#[derive(Debug, Clone)]
pub struct WorkspaceChanges {
    pub live: Arc<WorkspaceLive>,
    /// The checkpoint of a sync or restore that stopped after it began to write files; the agent runs `sync_workspace` or `restore_checkpoint` again to finish it.
    pub unfinished: Option<CheckpointId>,
}

impl WorkspaceService {
    /// Every task workspace in state `active` or `missing`, by id.
    pub async fn list_workspaces(&self) -> Result<Vec<WorkspaceEntry>, WorkspaceError> {
        let rows = workspaces::list_live_task_workspaces(self.store.read()).await?;
        let views: Vec<WorkspaceView> = rows.into_iter().map(WorkspaceView::from).collect();
        self.entries(views).await
    }

    /// One workspace in any state, in the shape of a list entry. An unknown id is `workspace_not_found`.
    pub async fn workspace_entry(
        &self,
        workspace: WorkspaceId,
    ) -> Result<WorkspaceEntry, WorkspaceError> {
        let row = workspaces::get_workspace(self.store.read(), workspace)
            .await?
            .ok_or_else(|| not_found(workspace))?;
        let mut entries = self.entries(vec![WorkspaceView::from(row)]).await?;
        entries.pop().ok_or_else(|| not_found(workspace).into())
    }

    async fn entries(
        &self,
        views: Vec<WorkspaceView>,
    ) -> Result<Vec<WorkspaceEntry>, WorkspaceError> {
        let tasks: Vec<TaskId> = views.iter().map(|view| view.task).collect();
        let items = attention(&self.store, &tasks, self.clock.now()).await?;
        Ok(views
            .into_iter()
            .map(|workspace| WorkspaceEntry {
                live: self.live.as_ref().and_then(|live| live.get(workspace.id)),
                attention: items.get(&workspace.task).cloned().unwrap_or_default(),
                workspace,
            })
            .collect())
    }

    /// The live state of an active task workspace after a full scan, so a change the watcher has not delivered yet is in it. An unknown id is `workspace_not_found`; a missing or closed workspace, a review checkout, or a service without live state is `workspace_not_active`.
    pub async fn changes(
        &self,
        workspace: WorkspaceId,
    ) -> Result<WorkspaceChanges, WorkspaceError> {
        let read = self.store.read();
        let row = workspaces::get_workspace(read, workspace)
            .await?
            .ok_or_else(|| not_found(workspace))?;
        if row.kind != WorkspaceKind::Task || row.state != WorkspaceState::Active {
            return Err(not_active(workspace, row.kind, row.state).into());
        }
        let Some(live) = &self.live else {
            return Err(not_active(workspace, row.kind, row.state).into());
        };
        // Attaching an attached workspace only rescans it, and one whose live state did not start at open starts here.
        let live = live.attach(workspace).await?;
        let unfinished = workspaces::sync_checkpoint(read, workspace).await?;
        Ok(WorkspaceChanges { live, unfinished })
    }
}

fn not_found(workspace: WorkspaceId) -> FoldError {
    FoldError::new(
        codes::WORKSPACE_NOT_FOUND,
        format!("{workspace} does not exist"),
    )
    .with_details([("workspace", json!(workspace))])
    .with_hint("call get_workspace with the task to find its workspace")
}

fn not_active(workspace: WorkspaceId, kind: WorkspaceKind, state: WorkspaceState) -> FoldError {
    let (what, hint) = match (kind, state) {
        (WorkspaceKind::Review, _) => (
            "a review checkout".to_owned(),
            "compare the proposal's changeset C-<n> with compare_revisions instead",
        ),
        (_, WorkspaceState::Missing) => (
            state.as_str().to_owned(),
            "the directory is gone; compare one of its checkpoints ck-<n> instead",
        ),
        _ => (
            state.as_str().to_owned(),
            "compare the workspace's last changeset C-<n> instead",
        ),
    };
    FoldError::new(
        codes::WORKSPACE_NOT_ACTIVE,
        format!("{workspace} is {what}"),
    )
    .with_details([
        ("workspace", json!(workspace)),
        ("state", json!(state.as_str())),
    ])
    .with_hint(hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_review_checkout_and_a_missing_workspace_name_their_way_out() {
        let ws = WorkspaceId::from_number(4).unwrap();
        let review = not_active(ws, WorkspaceKind::Review, WorkspaceState::Active);
        assert_eq!(review.code(), codes::WORKSPACE_NOT_ACTIVE);
        assert!(review.message().contains("review checkout"));
        let missing = not_active(ws, WorkspaceKind::Task, WorkspaceState::Missing);
        assert!(missing.hint().unwrap_or_default().contains("checkpoints"));
        let closed = not_active(ws, WorkspaceKind::Task, WorkspaceState::Archived);
        assert!(closed.message().ends_with("archived"));
    }
}
