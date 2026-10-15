use fold_core::Timestamp;
use fold_core::stall::{WorkspaceFact, is_stalled};
use fold_store::tasks::claims;

use super::WorkspaceError;
use crate::projects::ProjectHandle;
use crate::services::projects::load_config;

/// Decides the stalled flag of every live workspace of `project` at `now`, with the project's `stall_minutes`, and sets it in the live state. A flag that changes publishes `workspace.changed`; nothing is stored and nothing else follows. Returns how many flags changed.
pub async fn check_stalls(
    project: &ProjectHandle,
    now: Timestamp,
) -> Result<usize, WorkspaceError> {
    let stall_minutes = load_config(&project.paths).await?.stall_minutes;
    let mut changed = 0;
    for state in project.live.all() {
        let view = &state.view;
        let claim = claims::unreleased_for_task(project.store.read(), view.task).await?;
        let has_impl_claim = claim.is_some_and(|claim| claim.worker.as_str() == view.worker);
        let workspace = if state.missing {
            WorkspaceFact::Missing
        } else {
            WorkspaceFact::Active
        };
        let stalled = is_stalled(
            now,
            has_impl_claim,
            workspace,
            view.last_activity_at,
            stall_minutes,
        );
        if stalled == view.stalled
            || !project
                .live
                .set_stalled(view.workspace, view.last_activity_at, stalled)
        {
            continue;
        }
        changed += 1;
        if stalled {
            tracing::info!(workspace = %view.workspace, worker = %view.worker, "the worker of a workspace is stalled");
        }
    }
    Ok(changed)
}
