use std::collections::BTreeMap;

use fold_core::container::{ContainerFacts, ItemFact};
use fold_core::status::{ProposalFact, TaskFacts, TaskStatus};
use fold_core::{TaskId, TaskKind};
use sqlx::SqliteConnection;

use crate::error::StoreError;
use crate::store::WriteTx;

/// One task's stored cache next to the facts that decide it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedFacts {
    pub task: TaskId,
    pub status: TaskStatus,
    pub blocked: bool,
    pub archived: bool,
    pub facts: TaskFacts,
    pub container: ContainerFacts,
}

/// Reads the facts of `ids` from the tables that hold them: the task's kind, completions, dependencies with the prerequisites' stored statuses, task workspaces, the unreleased implementation claim, the open report, the open proposal, its highest version and the latest verdict on that version, and for the container rule the checklist items with their linked children's stored statuses and whether a task workspace ever existed. Reviews with the verdict `comment` are not verdicts and are skipped; among the rest the newest by `created_at`, then by row id, decides, whoever wrote it. Run on the writer's connection (`WriteTx::conn`) it sees the prerequisite statuses this transaction last wrote; on a read connection it sees the committed ones. Unknown numbers are left out; the result is in number order.
pub async fn load_facts(
    conn: &mut SqliteConnection,
    ids: &[TaskId],
) -> Result<Vec<LoadedFacts>, StoreError> {
    let numbers =
        serde_json::Value::from(ids.iter().map(|id| id.to_db()).collect::<Vec<i64>>()).to_string();
    let mut prerequisites: BTreeMap<i64, Vec<(TaskId, TaskStatus)>> = BTreeMap::new();
    let edges = sqlx::query!(
        r#"SELECT t.number AS "task!: i64", n.number AS "needs!: i64", n.status AS "status!: String"
           FROM dependencies d
           JOIN tasks t ON t.id = d.task_id
           JOIN tasks n ON n.id = d.needs_task_id
           WHERE t.number IN (SELECT value FROM json_each(?1))
           ORDER BY t.number, n.number"#,
        numbers
    )
    .fetch_all(&mut *conn)
    .await?;
    for edge in edges {
        let needs = TaskId::try_from_db(edge.needs).ok_or(StoreError::Corrupt("tasks.number"))?;
        prerequisites
            .entry(edge.task)
            .or_default()
            .push((needs, parse_status(&edge.status)?));
    }
    let mut items: BTreeMap<i64, Vec<ItemFact>> = BTreeMap::new();
    let item_rows = sqlx::query!(
        r#"SELECT t.number AS "task!: i64",
                  i.done_at IS NOT NULL AS "ticked!: bool",
                  l.status AS "child_status?: String"
           FROM checklist_items i
           JOIN checklists c ON c.id = i.checklist_id
           JOIN tasks t ON t.id = c.task_id
           LEFT JOIN tasks l ON l.id = i.linked_task_id
           WHERE t.number IN (SELECT value FROM json_each(?1))
           ORDER BY t.number, c.position, i.position"#,
        numbers
    )
    .fetch_all(&mut *conn)
    .await?;
    for row in item_rows {
        let fact = match row.child_status {
            None => ItemFact::Step { ticked: row.ticked },
            Some(status) => ItemFact::Linked {
                child_done: parse_status(&status)? == TaskStatus::Done,
            },
        };
        items.entry(row.task).or_default().push(fact);
    }
    let rows = sqlx::query!(
        r#"SELECT t.number AS "number!: i64",
                  t.status,
                  t.blocked AS "blocked: bool",
                  t.archived_at IS NOT NULL AS "archived!: bool",
                  t.kind,
                  EXISTS (SELECT 1 FROM completions c WHERE c.task_id = t.id) AS "completed!: bool",
                  EXISTS (
                      SELECT 1 FROM claims c
                      WHERE c.task_id = t.id AND c.kind = 'implementation' AND c.released_at IS NULL
                  ) AS "claimed!: bool",
                  EXISTS (
                      SELECT 1 FROM task_reports r WHERE r.task_id = t.id AND r.resolution IS NULL
                  ) AS "reported!: bool",
                  EXISTS (
                      SELECT 1 FROM workspaces w
                      WHERE w.task_id = t.id AND w.kind = 'task' AND w.state IN ('active', 'missing')
                  ) AS "active_workspace!: bool",
                  EXISTS (
                      SELECT 1 FROM workspaces w WHERE w.task_id = t.id AND w.kind = 'task'
                  ) AS "ever_had_workspace!: bool",
                  p.conflicted AS "conflicted?: bool",
                  (SELECT pv.verdict FROM proposal_verdicts pv WHERE pv.proposal_id = p.id) AS "verdict?: String"
           FROM tasks t
           LEFT JOIN proposals p ON p.task_id = t.id AND p.state = 'open'
           WHERE t.number IN (SELECT value FROM json_each(?1))
           ORDER BY t.number"#,
        numbers
    )
    .fetch_all(&mut *conn)
    .await?;
    rows.into_iter()
        .map(|row| {
            let proposal = match row.conflicted {
                None => ProposalFact::None,
                Some(conflicted) => ProposalFact::open(
                    row.verdict.as_deref() == Some("request_changes"),
                    conflicted,
                ),
            };
            let kind = row
                .kind
                .parse::<TaskKind>()
                .map_err(|_| StoreError::Corrupt("tasks.kind"))?;
            Ok(LoadedFacts {
                task: TaskId::try_from_db(row.number).ok_or(StoreError::Corrupt("tasks.number"))?,
                status: parse_status(&row.status)?,
                blocked: row.blocked,
                archived: row.archived,
                facts: TaskFacts {
                    kind,
                    completed: row.completed,
                    proposal,
                    has_active_workspace: row.active_workspace,
                    claimed: row.claimed,
                    reported: row.reported,
                    prerequisites: prerequisites.remove(&row.number).unwrap_or_default(),
                },
                container: ContainerFacts {
                    items: items.remove(&row.number).unwrap_or_default(),
                    ever_had_workspace: row.ever_had_workspace,
                    completed: row.completed,
                    archived: row.archived,
                    human_task: kind == TaskKind::Human,
                },
            })
        })
        .collect()
}

/// Stores a calculated status and blocked flag. Recompute is the only caller: the two columns are a cache of `fold_core::status::calculate`, and any other writer would let the board disagree with the facts.
pub async fn write_cache(
    tx: &mut WriteTx,
    task: TaskId,
    status: TaskStatus,
    blocked: bool,
) -> Result<(), StoreError> {
    let number = task.to_db();
    let status = status.as_str();
    let result = sqlx::query!(
        "UPDATE tasks SET status = ?2, blocked = ?3 WHERE number = ?1",
        number,
        status,
        blocked
    )
    .execute(tx.conn())
    .await?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid("the task does not exist"))
    }
}

fn parse_status(text: &str) -> Result<TaskStatus, StoreError> {
    text.parse()
        .map_err(|_| StoreError::Corrupt("tasks.status"))
}
