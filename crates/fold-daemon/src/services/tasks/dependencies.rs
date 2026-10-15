use fold_core::dag::{Dag, EdgeRefusal};
use fold_core::events::types;
use fold_core::{
    Actor, DependencyOrigin, FoldError, NewEvent, TaskId, TaskStatus, Timestamp, codes,
};
use fold_store::WriteTx;
use fold_store::tasks::{self, EdgeRow};
use serde_json::json;

use super::{TaskError, TaskService, recompute, task_not_found};
use crate::services::events::EventBuffer;

/// One edge of the project graph: `task` needs `needs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub task: TaskId,
    pub needs: TaskId,
    pub origin: DependencyOrigin,
    pub created_by: String,
    pub created_at: Timestamp,
}

impl From<EdgeRow> for Edge {
    fn from(row: EdgeRow) -> Self {
        Edge {
            task: row.task,
            needs: row.needs,
            origin: row.origin,
            created_by: row.created_by,
            created_at: row.created_at,
        }
    }
}

impl TaskService {
    /// Adds the direct edge `task` needs `needs` and recomputes `task` and its dependents in the same transaction.
    pub async fn add_dependency(
        &self,
        actor: &Actor,
        task: TaskId,
        needs: TaskId,
    ) -> Result<Edge, TaskError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                add_edge_in_tx(
                    tx,
                    &mut buffer,
                    task,
                    needs,
                    DependencyOrigin::Direct,
                    actor,
                    now,
                )
                .await?;
                buffer.extend(recompute(tx, &[task], now).await?.events);
                Ok::<_, TaskError>(())
            })
            .await?;
        self.bus.publish(buffer);
        Ok(Edge {
            task,
            needs,
            origin: DependencyOrigin::Direct,
            created_by: actor.event_actor().as_str().to_owned(),
            created_at: now,
        })
    }

    /// Removes a direct edge and recomputes `task` and its dependents in the same transaction. A checklist edge is removed only with its linked checklist item.
    pub async fn remove_dependency(
        &self,
        actor: &Actor,
        task: TaskId,
        needs: TaskId,
    ) -> Result<(), TaskError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let edge = tasks::fetch_edge(tx.conn(), task, needs)
                    .await?
                    .ok_or_else(|| {
                        refusal(
                            codes::DEPENDENCY_NOT_FOUND,
                            format!("{task} does not need {needs}"),
                            [
                                ("task", json!(task.to_string())),
                                ("needs", json!(needs.to_string())),
                            ],
                            "call get_task to see its dependencies",
                        )
                    })?;
                if edge.origin == DependencyOrigin::Checklist {
                    return Err(refusal(
                        codes::INVALID_REQUEST,
                        format!("the edge from {task} to {needs} belongs to a checklist item"),
                        [
                            ("reason", json!("checklist_origin")),
                            ("task", json!(task.to_string())),
                            ("needs", json!(needs.to_string())),
                        ],
                        "remove the linked checklist item with remove_checklist_item instead",
                    ));
                }
                tasks::delete_edge(tx, task, needs).await?;
                let event = NewEvent::new(types::DEPENDENCY_REMOVED, actor.event_actor())?
                    .task(task)
                    .data(json!({"needs": needs.to_string(), "origin": edge.origin.as_str()}))?;
                buffer.append(tx, &event, now).await?;
                buffer.extend(recompute(tx, &[task], now).await?.events);
                Ok::<_, TaskError>(())
            })
            .await?;
        self.bus.publish(buffer);
        Ok(())
    }

    /// Every edge of the project, read from the read pool. There is one graph per project; no edge is scoped to an area or an actor.
    pub async fn edges(&self) -> Result<Vec<Edge>, TaskError> {
        Ok(tasks::edges(self.store.read())
            .await?
            .into_iter()
            .map(Edge::from)
            .collect())
    }
}

/// Checks and inserts one edge inside the caller's transaction and appends `dependency.added`. Linked checklist items call this with `DependencyOrigin::Checklist`. The caller runs `recompute` on `task` before it commits. The graph is read on the writer's connection, and the project has one writer, so the cycle check and the insert are atomic: two adds that would close a cycle together cannot both pass.
pub async fn add_edge_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    task: TaskId,
    needs: TaskId,
    origin: DependencyOrigin,
    actor: &Actor,
    now: Timestamp,
) -> Result<(), TaskError> {
    // A task absent from the graph has no edges, so an unknown task never forms a cycle and still reaches task_not_found below.
    let graph = Dag::from_edges(
        tasks::edges_conn(tx.conn())
            .await?
            .into_iter()
            .map(|edge| (edge.task, edge.needs)),
    );
    match graph.check_edge(task, needs) {
        Ok(()) => {}
        Err(EdgeRefusal::SelfDependency) => {
            return Err(refusal(
                codes::INVALID_REQUEST,
                format!("{task} cannot need itself"),
                [
                    ("reason", json!("self_dependency")),
                    ("task", json!(task.to_string())),
                ],
                "name two different tasks",
            ));
        }
        Err(EdgeRefusal::Cycle(path)) => {
            let path: Vec<String> = path.iter().map(TaskId::to_string).collect();
            return Err(refusal(
                codes::DEPENDENCY_CYCLE,
                format!("{task} needing {needs} would close a cycle"),
                [
                    ("path", json!(path)),
                    ("task", json!(task.to_string())),
                    ("needs", json!(needs.to_string())),
                ],
                "remove an edge on the path with remove_dependency first",
            ));
        }
    }
    let row = tasks::fetch(tx.conn(), task)
        .await?
        .ok_or_else(|| task_not_found(task))?;
    let needs_row = tasks::fetch(tx.conn(), needs)
        .await?
        .ok_or_else(|| task_not_found(needs))?;
    for closed in [&row, &needs_row] {
        if closed.archived_at.is_some() {
            return Err(refusal(
                codes::TASK_CLOSED,
                format!("{} is archived", closed.id),
                [
                    ("reason", json!("archived")),
                    ("task", json!(task.to_string())),
                    ("needs", json!(needs.to_string())),
                    ("archived", json!(closed.id.to_string())),
                ],
                "an archived task takes no dependencies; call get_task_graph to pick an open task",
            ));
        }
    }
    if row.status == TaskStatus::Done {
        return Err(refusal(
            codes::TASK_CLOSED,
            format!("{task} is done"),
            [
                ("reason", json!("done")),
                ("task", json!(task.to_string())),
                ("needs", json!(needs.to_string())),
            ],
            "a Done task takes no new dependencies; call create_task for the follow-up work",
        ));
    }
    if tasks::fetch_edge(tx.conn(), task, needs).await?.is_some() {
        return Err(refusal(
            codes::DEPENDENCY_EXISTS,
            format!("{task} already needs {needs}"),
            [
                ("task", json!(task.to_string())),
                ("needs", json!(needs.to_string())),
            ],
            "the edge exists; call get_task to see its dependencies",
        ));
    }
    tasks::insert_edge(tx, task, needs, origin, actor.event_actor().as_str(), now).await?;
    let event = NewEvent::new(types::DEPENDENCY_ADDED, actor.event_actor())?
        .task(task)
        .data(json!({"needs": needs.to_string(), "origin": origin.as_str()}))?;
    buffer.append(tx, &event, now).await?;
    Ok(())
}

pub(super) fn refusal(
    code: &'static str,
    message: String,
    details: impl IntoIterator<Item = (&'static str, serde_json::Value)>,
    hint: &str,
) -> TaskError {
    TaskError::Fold(
        FoldError::new(code, message)
            .with_details(details)
            .with_hint(hint),
    )
}
