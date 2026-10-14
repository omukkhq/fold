use std::collections::BTreeMap;

use fold_core::TaskId;
use fold_core::status::{ProposalFact, TaskFacts, TaskStatus};

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
}

/// Reads the facts of `ids` from the tables that hold them: completions, dependencies with the prerequisites' stored statuses, task workspaces, the open proposal, its highest version and the latest verdict on that version. Reviews with the verdict `comment` are not verdicts and are skipped; among the rest the newest by `created_at`, then by row id, decides, whoever wrote it. It reads on the writer's connection, so a prerequisite's status is the one this transaction last wrote. Unknown numbers are left out; the result is in number order.
pub async fn load_facts(tx: &mut WriteTx, ids: &[TaskId]) -> Result<Vec<LoadedFacts>, StoreError> {
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
    .fetch_all(tx.conn())
    .await?;
    for edge in edges {
        let needs = TaskId::try_from_db(edge.needs).ok_or(StoreError::Corrupt("tasks.number"))?;
        prerequisites
            .entry(edge.task)
            .or_default()
            .push((needs, parse_status(&edge.status)?));
    }
    let rows = sqlx::query!(
        r#"SELECT t.number AS "number!: i64",
                  t.status,
                  t.blocked AS "blocked: bool",
                  t.archived_at IS NOT NULL AS "archived!: bool",
                  EXISTS (SELECT 1 FROM completions c WHERE c.task_id = t.id) AS "completed!: bool",
                  EXISTS (
                      SELECT 1 FROM workspaces w
                      WHERE w.task_id = t.id AND w.kind = 'task' AND w.state IN ('active', 'missing')
                  ) AS "active_workspace!: bool",
                  p.conflicted AS "conflicted?: bool",
                  (
                      SELECT r.verdict FROM reviews r
                      WHERE r.version_id = (
                                SELECT v.id FROM proposal_versions v
                                WHERE v.proposal_id = p.id
                                ORDER BY v.version DESC
                                LIMIT 1
                            )
                        AND r.verdict <> 'comment'
                      ORDER BY r.created_at DESC, r.id DESC
                      LIMIT 1
                  ) AS "verdict?: String"
           FROM tasks t
           LEFT JOIN proposals p ON p.task_id = t.id AND p.state = 'open'
           WHERE t.number IN (SELECT value FROM json_each(?1))
           ORDER BY t.number"#,
        numbers
    )
    .fetch_all(tx.conn())
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
            Ok(LoadedFacts {
                task: TaskId::try_from_db(row.number).ok_or(StoreError::Corrupt("tasks.number"))?,
                status: parse_status(&row.status)?,
                blocked: row.blocked,
                archived: row.archived,
                facts: TaskFacts {
                    completed: row.completed,
                    proposal,
                    has_active_workspace: row.active_workspace,
                    prerequisites: prerequisites.remove(&row.number).unwrap_or_default(),
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
