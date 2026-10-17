use std::fs;

use fold_core::events::types;
use fold_core::policy::{self, Starter};
use fold_core::{
    Actor, AgentActor, EventActor, FoldError, IdKind, NewEvent, RevisionId, Timestamp, WorkerName,
    WorkspaceId, codes, main_workspace_path,
};
use fold_store::workspaces::{self, NewMainWorkspace, WorkspaceRow};
use fold_store::{Store, StoreError, WriteTx, agents, counters};
use serde_json::json;

use super::adoption::AdoptReason;
use super::lifecycle::{Placed, WorkspaceRef, WorkspaceView, workspace_exists};
use super::{WorkspaceError, WorkspaceService};
use crate::services::agents::starter_of;
use crate::services::events::EventBuffer;
use crate::services::tasks::worker_row;

/// What `open_main_workspace` returns: the live main workspace, and whether it was there already, so the main agent goes on with the files it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedMainWorkspace {
    pub workspace: WorkspaceView,
    pub reused: bool,
}

impl WorkspaceService {
    /// Opens the main workspace for the main agent's worker: a copy of the current main revision under `.fold/ws/`, which belongs to no task. Only the main agent opens one (`policy::can_change_main_directly`). The project has at most one live main workspace: an open call while one is live returns it, and when another session held the main tag before, its worker gives the workspace to the actor's worker with its files as they are.
    ///
    /// As for a task workspace, the files are written and renamed into place before the transaction that inserts the row, and the directory is removed again when the transaction fails. The main workspace has no live state, so nothing streams its working files.
    pub async fn open_main_workspace(
        &self,
        actor: &Actor,
    ) -> Result<OpenedMainWorkspace, WorkspaceError> {
        let agent = main_agent(&self.store, actor).await?;
        let read = self.store.read();
        if let Some(live) = workspaces::live_main_workspace(read).await? {
            return self.reuse(actor, agent, live).await;
        }
        let base = workspaces::head(read).await?.ok_or_else(|| {
            FoldError::new(codes::REVISION_NOT_FOUND, "main has no revision")
                .with_hint("run fld init to create the first revision")
        })?;
        let taken = workspaces::main_workspace_paths(read).await?;
        let placed = self
            .place_named(main_workspace_path, taken, base.tree_hash, false)
            .await?;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let written = self
            .store
            .write(async |tx| {
                insert_in_tx(
                    tx,
                    &mut buffer,
                    actor,
                    agent,
                    &placed.path,
                    base.number,
                    now,
                )
                .await
            })
            .await;
        let Placed {
            path,
            dir,
            mut index,
        } = placed;
        let id = match written {
            Ok(id) => id,
            Err(error) => {
                let removed = tokio::task::spawn_blocking(move || fs::remove_dir_all(dir)).await;
                if !matches!(removed, Ok(Ok(()))) {
                    tracing::warn!(path = %path, "a main workspace directory without a row was not removed");
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
        Ok(OpenedMainWorkspace {
            workspace: self.get(WorkspaceRef::Workspace(id)).await?,
            reused: false,
        })
    }

    /// Returns the live main workspace, given to the actor's worker first when another worker owns it. The operation lock keeps a command of the previous owner from writing its files meanwhile.
    async fn reuse(
        &self,
        actor: &Actor,
        agent: &AgentActor,
        live: WorkspaceRow,
    ) -> Result<OpenedMainWorkspace, WorkspaceError> {
        if live.worker.as_ref() == Some(&agent.worker) {
            return Ok(OpenedMainWorkspace {
                workspace: live.into(),
                reused: true,
            });
        }
        let _op = self.ops.lock(live.id).await;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let workspace = live.id;
        self.store
            .write(async |tx| {
                main_agent_in_tx(tx, actor).await?;
                let worker = worker_row(tx, agent).await?;
                let current = workspaces::live_main_workspace(tx.conn()).await?;
                let Some(current) = current.filter(|row| row.id == workspace) else {
                    return Err(FoldError::new(
                        codes::WORKSPACE_NOT_ACTIVE,
                        format!("{workspace} closed while it changed hands"),
                    )
                    .with_hint("call open_main_workspace again")
                    .into());
                };
                if current.worker_id == Some(worker.id) {
                    return Ok::<_, WorkspaceError>(());
                }
                if !workspaces::set_worker(tx, workspace, worker.id).await? {
                    return Err(StoreError::Corrupt("workspaces.worker_id").into());
                }
                let event =
                    NewEvent::new(types::WORKSPACE_ADOPTED, EventActor::worker(&worker.name))?
                        .workspace(workspace)
                        .data(json!({
                            "reason": AdoptReason::MainAgent.as_str(),
                            "from": current.worker.as_ref().map(WorkerName::as_str),
                            "to": worker.name.as_str(),
                        }))?;
                buffer.append(tx, &event, now).await?;
                Ok(())
            })
            .await?;
        self.bus.publish(buffer);
        Ok(OpenedMainWorkspace {
            workspace: self.get(WorkspaceRef::Workspace(workspace)).await?,
            reused: true,
        })
    }
}

/// The acting main agent, when `policy::can_change_main_directly` admits the actor on the read connection. The command checks again inside its transaction with [`main_agent_in_tx`].
pub(crate) async fn main_agent<'a>(
    store: &Store,
    actor: &'a Actor,
) -> Result<&'a AgentActor, WorkspaceError> {
    let supervised = match actor {
        Actor::Agent(agent) => {
            let mut conn = store.read().acquire().await.map_err(StoreError::from)?;
            agents::find_worker_process(&mut conn, &agent.agent)
                .await?
                .map(|row| (row.role, row.area))
        }
        Actor::Human { .. } | Actor::System => None,
    };
    admitted(actor, &Starter::of(actor, supervised))
}

/// [`main_agent`] on the writer's connection.
pub(crate) async fn main_agent_in_tx<'a>(
    tx: &mut WriteTx,
    actor: &'a Actor,
) -> Result<&'a AgentActor, WorkspaceError> {
    let starter = starter_of(tx, actor).await?;
    admitted(actor, &starter)
}

fn admitted<'a>(actor: &'a Actor, starter: &Starter) -> Result<&'a AgentActor, WorkspaceError> {
    policy::can_change_main_directly(starter).map_err(FoldError::from)?;
    match actor {
        Actor::Agent(agent) => Ok(agent),
        Actor::Human { .. } | Actor::System => {
            Err(StoreError::Invalid("only an agent is the main agent").into())
        }
    }
}

/// The insert of the main workspace and its `workspace.created`. A race with a second open ends in `workspace_exists` from the unique index.
async fn insert_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &Actor,
    agent: &AgentActor,
    path: &str,
    base: RevisionId,
    now: Timestamp,
) -> Result<WorkspaceId, WorkspaceError> {
    main_agent_in_tx(tx, actor).await?;
    let worker = worker_row(tx, agent).await?;
    let number = counters::allocate(tx, IdKind::Workspace).await?;
    let id = WorkspaceId::from_number(number).ok_or(StoreError::Corrupt("counters.next"))?;
    let new = NewMainWorkspace {
        id,
        worker_id: worker.id,
        path,
        base,
        now,
    };
    match workspaces::insert_main_workspace(tx, &new).await {
        Ok(()) => {}
        Err(error) if error.unique_violation() => {
            return match workspaces::live_main_workspace(tx.conn()).await? {
                Some(live) => Err(workspace_exists(&live).into()),
                None => Err(error.into()),
            };
        }
        Err(error) => return Err(error.into()),
    }
    let event = NewEvent::new(types::WORKSPACE_CREATED, EventActor::worker(&worker.name))?
        .workspace(id)
        .revision(base)
        .data(json!({"path": path, "worker": worker.name.as_str(), "kind": "main"}))?;
    buffer.append(tx, &event, now).await?;
    Ok(id)
}
