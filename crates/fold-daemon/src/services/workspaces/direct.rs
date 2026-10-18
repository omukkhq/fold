use std::fs;

use fold_core::events::types;
use fold_core::object::ObjectHash;
use fold_core::policy::{self, Starter};
use fold_core::{
    Actor, AgentActor, EventActor, FoldError, IdKind, NewEvent, RevisionId, Timestamp, WorkerName,
    WorkspaceId, codes, main_workspace_path,
};
use fold_store::workspaces::{self, ConflictRow, HumanChangeRow, NewMainWorkspace, WorkspaceRow};
use fold_store::{Store, StoreError, WriteTx, agents, counters};
use fold_vcs::integrate::tree_merge;
use fold_vcs::materialise::{MaterialiseOptions, materialise};
use fold_vcs::objects::ObjectStore;
use fold_vcs::platform::{create_dir_below, sync_dir};
use fold_vcs::sync::{ConflictedPath, MarkerLabels, write_merge};
use serde_json::json;
use std::sync::Arc;

use super::adoption::AdoptReason;
use super::lifecycle::{
    Placed, WorkspaceRef, WorkspaceView, io_error, rename_into_place, staging_dir, workspace_exists,
};
use super::sync_task::short_commit;
use super::{WorkspaceError, WorkspaceService, blocking, capabilities};
use crate::services::agents::starter_of;
use crate::services::events::EventBuffer;
use crate::services::tasks::worker_row;

/// What `open_main_workspace` returns: the live main workspace, and whether it was there already, so the main agent goes on with the files it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedMainWorkspace {
    pub workspace: WorkspaceView,
    pub reused: bool,
    /// The human change that conflicted with main and waits for the main agent, when one does.
    pub human_change: Option<WaitingHumanChange>,
}

/// A conflicted human change as the main agent sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitingHumanChange {
    /// The human's Git commit; `None` for a change `fld record` read from the project folder.
    pub commit: Option<String>,
    pub human: String,
    /// Whether the main workspace holds its merge. A main workspace that was open before the change came holds other work, which the main agent submits or abandons first.
    pub held: bool,
    /// The paths that hold conflict markers or a `.fold-main` side in the workspace when it holds the merge; else the paths that conflicted when the change was recorded. In path order.
    pub conflicted: Vec<String>,
}

impl WorkspaceService {
    /// Opens the main workspace for the main agent's worker: a copy of the current main revision under `.fold/ws/`, which belongs to no task. Only the main agent opens one (`policy::can_change_main_directly`). The project has at most one live main workspace: an open call while one is live returns it, and when another session held the main tag before, its worker gives the workspace to the actor's worker with its files as they are.
    ///
    /// As for a task workspace, the files are written and renamed into place before the transaction that inserts the row, the directory is removed again when the transaction fails, and the live state starts after the commit.
    pub async fn open_main_workspace(
        &self,
        actor: &Actor,
    ) -> Result<OpenedMainWorkspace, WorkspaceError> {
        let agent = main_agent(&self.store, actor).await?;
        let read = self.store.read();
        let waiting = {
            let mut conn = read.acquire().await.map_err(StoreError::from)?;
            workspaces::conflicted_human_change(&mut conn).await?
        };
        if let Some(live) = workspaces::live_main_workspace(read).await? {
            let mut opened = self.reuse(actor, agent, live).await?;
            opened.human_change = match waiting {
                Some(change) => Some(self.waiting(change, opened.workspace.id).await?),
                None => None,
            };
            return Ok(opened);
        }
        let base = workspaces::head(read).await?.ok_or_else(|| {
            FoldError::new(codes::REVISION_NOT_FOUND, "main has no revision")
                .with_hint("run fld init to create the first revision")
        })?;
        let taken = workspaces::main_workspace_paths(read).await?;
        let (placed, conflicts) = match &waiting {
            Some(change) => {
                self.place_human_merge(taken, change, base.number, base.tree_hash)
                    .await?
            }
            None => (
                self.place_named(main_workspace_path, taken, base.tree_hash, false)
                    .await?,
                Vec::new(),
            ),
        };
        let rows: Vec<ConflictRow> = conflicts
            .iter()
            .map(|conflict| ConflictRow {
                path: conflict.path.as_bytes().to_vec(),
                kind: conflict.kind,
            })
            .collect();
        let holds = waiting.as_ref().map(|change| change.commit.clone());
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let written = self
            .store
            .write(async |tx| {
                let id = insert_in_tx(
                    tx,
                    &mut buffer,
                    actor,
                    agent,
                    &placed.path,
                    base.number,
                    now,
                )
                .await?;
                // The merge was made of the change that waited at the read; one that a newer change replaced since is not held.
                if let Some(commit) = &holds {
                    let current = workspaces::conflicted_human_change(tx.conn()).await?;
                    if current.is_some_and(|change| &change.commit == commit) {
                        workspaces::set_conflicts(tx, id, &rows).await?;
                        workspaces::set_human_change_workspace(tx, id).await?;
                    } else {
                        return Err(FoldError::new(
                            codes::HEAD_MOVED,
                            "the human recorded a newer change while the merge was prepared",
                        )
                        .with_hint("call open_main_workspace again")
                        .into());
                    }
                }
                Ok::<_, WorkspaceError>(id)
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
        if let Some(live) = &self.live
            && let Err(error) = live.attach(id).await
        {
            tracing::warn!(workspace = %id, code = error.log_code(), "the live state did not start");
        }
        let human_change = match waiting {
            Some(change) => Some(WaitingHumanChange {
                commit: change.commit,
                human: change.human,
                held: true,
                conflicted: conflicts
                    .iter()
                    .map(|conflict| conflict.path.to_string())
                    .collect(),
            }),
            None => None,
        };
        Ok(OpenedMainWorkspace {
            workspace: self.get(WorkspaceRef::Workspace(id)).await?,
            reused: false,
            human_change,
        })
    }

    /// `change` as the main agent sees it beside the live main workspace `workspace`.
    async fn waiting(
        &self,
        change: HumanChangeRow,
        workspace: WorkspaceId,
    ) -> Result<WaitingHumanChange, WorkspaceError> {
        let held = change.workspace == Some(workspace);
        let paths: Vec<Vec<u8>> = if held {
            workspaces::list_conflicts(self.store.read(), workspace)
                .await?
                .into_iter()
                .map(|row| row.path)
                .collect()
        } else {
            change.conflicts.into_iter().map(|row| row.path).collect()
        };
        Ok(WaitingHumanChange {
            commit: change.commit,
            human: change.human,
            held,
            conflicted: paths
                .iter()
                .map(|path| String::from_utf8_lossy(path).into_owned())
                .collect(),
        })
    }

    /// Writes the main workspace for a conflicted human change: the human's tree, with main's changes since the change's base merged into it, conflict markers labelled `workspace`, the human's base commit and `main r<head>` in each text conflict, and main's side as `<path>.fold-main` for every other conflict. Its base is the head the merge ran against, so `submit_main_change` merges three-way again when main moves meanwhile. As for any main workspace, the directory is in place before the transaction that inserts the row.
    async fn place_human_merge(
        &self,
        taken: Vec<String>,
        change: &HumanChangeRow,
        head: RevisionId,
        head_tree: ObjectHash,
    ) -> Result<(Placed, Vec<ConflictedPath>), WorkspaceError> {
        let paths = self.paths.clone();
        let ws_dir = self.paths.ws_dir();
        let root = self.paths.root().to_path_buf();
        let probed = Arc::clone(&self.caps);
        let (base_tree, human_tree) = (change.base_tree, change.tree);
        let labels = MarkerLabels {
            ours: "workspace".to_owned(),
            base: Some(match &change.commit {
                Some(commit) => format!("base {}", short_commit(commit)),
                None => "base project folder".to_owned(),
            }),
            theirs: format!("main {head}"),
        };
        blocking(move || {
            create_dir_below(&paths.fold_dir(), &ws_dir).map_err(io_error)?;
            let objects = ObjectStore::open_existing(&paths.fold_dir()).map_err(FoldError::from)?;
            let merge = tree_merge(&objects, &base_tree, &human_tree, &head_tree)?;
            let caps = capabilities(&probed, &paths)?;
            let staging = staging_dir(&ws_dir)?;
            let opts = MaterialiseOptions {
                read_only: false,
                caps,
            };
            let written = materialise(&objects, &human_tree, &staging, &opts)
                .map_err(FoldError::from)
                .and_then(|done| {
                    let mut index = done.index;
                    let conflicted =
                        write_merge(&objects, &staging, &mut index, &merge, &human_tree, &labels)?;
                    Ok((index, conflicted))
                });
            let (index, conflicted) = match written {
                Ok(written) => written,
                Err(error) => {
                    let _ = fs::remove_dir_all(&staging);
                    return Err(error);
                }
            };
            match rename_into_place(&staging, &root, &main_workspace_path, &taken) {
                Ok((path, dir)) => match sync_dir(&ws_dir) {
                    Ok(()) => Ok((Placed { path, dir, index }, conflicted)),
                    Err(error) => {
                        let _ = fs::remove_dir_all(&dir);
                        Err(io_error(error))
                    }
                },
                Err(error) => {
                    let _ = fs::remove_dir_all(&staging);
                    Err(error)
                }
            }
        })
        .await
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
                human_change: None,
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
        if let Some(live) = &self.live {
            live.set_worker(workspace, &agent.worker);
        }
        Ok(OpenedMainWorkspace {
            workspace: self.get(WorkspaceRef::Workspace(workspace)).await?,
            reused: true,
            human_change: None,
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
