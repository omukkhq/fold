use fold_core::object::ObjectHash;
use fold_core::reviews::checks::ChecksFile;
use fold_core::{
    Actor, AddressedAs, MessageBody, ProposalId, Recipient, Target, TaskId, WorkerName, WorkspaceId,
};
use fold_store::workspaces::{self, WorkspaceRow};

use super::super::{ReviewError, ReviewService};
use super::runner::{CommandSpec, run_commands_for};
use super::{TAIL_CHARS, TAIL_LINES, log_tail};
use crate::services::events::EventBuffer;
use crate::services::messages::send_from_fold_in_tx;
use crate::services::workspaces::{CreatedWorkspace, WorkspaceError};

/// Whether fold runs the `[setup]` of the checks file in a folder it just made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupState {
    /// The setup runs in the background; a message from fold reports its end.
    Running,
    /// The folder's checks file has no `[setup]`, or no valid checks file, so nothing runs.
    None,
}

impl SetupState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::None => "none",
        }
    }
}

impl ReviewService {
    /// [`crate::services::workspaces::WorkspaceService::create_workspace`], then the `[setup]` of the base tree's checks file in the new workspace, in the background.
    pub async fn create_workspace(
        &self,
        actor: &Actor,
        task: TaskId,
    ) -> Result<(CreatedWorkspace, SetupState), WorkspaceError> {
        let created = self.workspaces.create_workspace(actor, task).await?;
        let setup = self.start_setup(created.workspace.id, None).await;
        Ok((created, setup))
    }

    /// Starts the `[setup]` command of the checks file of `tree`, or of the workspace's base tree when `tree` is `None`, in `workspace`'s folder, so its dependencies are there before the worker builds. It runs under a machine-wide permit; a check run of the folder waits for it. At its end fold sends the folder's worker a message with the outcome and, after a failure, the end of the log. A folder whose tree has no checks file, an invalid one or one without `[setup]` starts nothing, and so does a read that fails: the first check run runs the setup anyway.
    pub(crate) async fn start_setup(
        &self,
        workspace: WorkspaceId,
        tree: Option<ObjectHash>,
    ) -> SetupState {
        match self.setup_of(workspace, tree).await {
            Ok(Some((row, run))) => {
                self.spawn_setup(row, run);
                SetupState::Running
            }
            Ok(None) => SetupState::None,
            Err(error) => {
                tracing::warn!(workspace = %workspace, code = error.log_code(), "the setup of a new folder was not started");
                SetupState::None
            }
        }
    }

    /// The folder's row and its setup command, if it has one.
    async fn setup_of(
        &self,
        workspace: WorkspaceId,
        tree: Option<ObjectHash>,
    ) -> Result<Option<(WorkspaceRow, String)>, ReviewError> {
        let Some(row) = workspaces::get_workspace(self.store.read(), workspace).await? else {
            return Ok(None);
        };
        let tree = match tree {
            Some(tree) => tree,
            None => {
                workspaces::get_by_number(self.store.read(), row.base)
                    .await?
                    .tree_hash
            }
        };
        let input = self.workspaces.checks_input(tree, tree).await?;
        let setup = input
            .file
            .and_then(|text| ChecksFile::parse(&text).ok())
            .and_then(|file| file.setup);
        Ok(setup.map(|run| (row, run)))
    }

    fn spawn_setup(&self, row: WorkspaceRow, run: String) {
        let service = self.clone();
        let done = self.checks.setup_started(row.id);
        self.checks.spawn(async move {
            let workspace = row.id;
            let permit = service.checks.permits().acquire().await;
            let owner = service
                .checks
                .owner(row.worker.as_ref().map(WorkerName::agent));
            let dir = service.workspaces.workspace_dir(&row);
            let spec = CommandSpec {
                check: None,
                run,
                required: true,
                runs: true,
                timeout: None,
                max_parallel: None,
            };
            let results =
                run_commands_for(&owner, &dir, vec![spec], service.checks.limits(), &*service.clock)
                    .await;
            drop(permit);
            drop(done);
            let body = match results.first() {
                Some(result) if result.outcome.is_failure() => {
                    let code = result
                        .exit_code
                        .map_or_else(String::new, |code| format!(" (exit {code})"));
                    format!(
                        "The setup of fold-checks.toml failed in {workspace}{code}. Fix it, or a check run of the folder fails the same way.\n\nThe end of its log:\n{}",
                        log_tail(&result.log, TAIL_LINES, TAIL_CHARS)
                    )
                }
                _ => format!("The setup of fold-checks.toml passed in {workspace}."),
            };
            if let Err(error) = service.tell_setup(&row, &body).await {
                tracing::warn!(workspace = %workspace, code = error.log_code(), "the end of a setup was not told");
            }
            tracing::info!(workspace = %workspace, "the setup of a new folder ended");
        });
    }

    /// Sends `body` from fold to the folder's worker, linked to its task and its proposal.
    async fn tell_setup(&self, row: &WorkspaceRow, body: &str) -> Result<(), ReviewError> {
        let Some(worker) = row.worker.clone() else {
            return Ok(());
        };
        let task = row.task;
        let proposal: Option<ProposalId> = row.proposal;
        let body = MessageBody::parse(body)?;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let to = Recipient {
                    target: Target::Worker(worker.clone()),
                    addressed_as: AddressedAs::Worker,
                };
                let task = match (task, proposal) {
                    (Some(task), _) => task,
                    (None, Some(proposal)) => {
                        match fold_store::reviews::get_proposal(tx.conn(), proposal).await? {
                            Some(row) => row.task,
                            None => return Ok::<_, ReviewError>(()),
                        }
                    }
                    (None, None) => return Ok(()),
                };
                send_from_fold_in_tx(tx, &mut buffer, vec![to], (task, proposal), &body, now)
                    .await?;
                Ok(())
            })
            .await?;
        self.bus.publish(buffer);
        Ok(())
    }
}
