use std::collections::{BTreeMap, BTreeSet, HashMap};

use fold_core::{AgentName, TaskId};
use fold_store::SqliteConnection;
use fold_store::agents::{self, ClaimKind};
use fold_store::tasks::claims::{self, ClaimForPerson, ClaimTarget};
use fold_store::{Store, StoreError, messages};

use crate::live::LiveState;
use crate::services::reviews::awaiting_human_review_in;

/// Why a task needs a person. The daemon decides every kind; no client derives one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AttentionKind {
    /// An unanswered fold message from an agent to a human, linked to the task.
    Question,
    /// A proposal of the task waits for a human approval.
    HumanReview,
    /// The task's active workspace is stalled in the live state.
    Stalled,
    /// The task's implementation claim, or the review claim of its open proposal, went stale, and the agent fold told of it, the holder's lead or else the main agent, let one more stall window pass, or no agent can act on it.
    StaleClaim,
}

/// What a person does about a `Stalled` or `StaleClaim` item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonAction {
    /// Restart the worker in a new session: fold runs it, and its process runs or it stalled.
    RestartWorker,
    /// Message the worker: its process ended, so the message resumes it in its own session, or fold does not run its session, which hears the message.
    MessageWorker,
}

impl PersonAction {
    pub fn as_str(self) -> &'static str {
        match self {
            PersonAction::RestartWorker => "restart_worker",
            PersonAction::MessageWorker => "message_worker",
        }
    }
}

impl AttentionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AttentionKind::Question => "question",
            AttentionKind::HumanReview => "human_review",
            AttentionKind::Stalled => "stalled",
            AttentionKind::StaleClaim => "stale_claim",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttentionItem {
    pub kind: AttentionKind,
    /// For a `Question`: one of the task's open asks is closable, so a human can mark it answered, because it is no worker's `AskUserQuestion`, which only a reply in its thread ends. False for every other kind.
    pub closable: bool,
    /// For a `Stalled` or `StaleClaim` item: what the person does about the worker; `None` for every other kind.
    pub action: Option<PersonAction>,
}

impl AttentionItem {
    /// An item of `kind` that marks nothing closable and names no action.
    pub fn of(kind: AttentionKind) -> Self {
        Self {
            kind,
            closable: false,
            action: None,
        }
    }
}

/// The attention items of each task in `task_ids`, read in one read transaction of `read`; `stalled` holds the tasks whose workspace is stalled, from [`stalled_tasks`]. No item depends on the time of the read: the lease sweeper marks a stale claim that needs a person and the stall job sets the stalled flag. Every id gets an entry; an empty list means nothing needs a person.
pub async fn attention(
    read: &Store,
    task_ids: &[TaskId],
    stalled: &BTreeSet<TaskId>,
) -> Result<BTreeMap<TaskId, Vec<AttentionItem>>, StoreError> {
    let mut tx = read.read().begin().await?;
    let items = attention_in(&mut tx, task_ids, stalled).await?;
    tx.commit().await?;
    Ok(items)
}

/// [`attention`] on `conn`, so a caller reads it in the same snapshot as the rest of its view.
pub async fn attention_in(
    conn: &mut SqliteConnection,
    task_ids: &[TaskId],
    stalled: &BTreeSet<TaskId>,
) -> Result<BTreeMap<TaskId, Vec<AttentionItem>>, StoreError> {
    let questions = messages::tasks_awaiting_human(&mut *conn).await?;
    let human_review: BTreeSet<TaskId> = awaiting_human_review_in(&mut *conn)
        .await?
        .into_iter()
        .map(|waiting| waiting.task)
        .collect();
    let stale_claims = claims::tasks_with_stale_claims(&mut *conn).await?;
    let actions = if stalled.is_empty() && stale_claims.is_empty() {
        Actions::default()
    } else {
        Actions::read(conn).await?
    };
    Ok(task_ids
        .iter()
        .map(|id| {
            let mut items = Vec::new();
            if let Some(&closable) = questions.get(id) {
                items.push(AttentionItem {
                    kind: AttentionKind::Question,
                    closable,
                    action: None,
                });
            }
            if human_review.contains(id) {
                items.push(AttentionItem::of(AttentionKind::HumanReview));
            }
            if stalled.contains(id) {
                items.push(AttentionItem {
                    action: Some(actions.stalled(*id)),
                    ..AttentionItem::of(AttentionKind::Stalled)
                });
            }
            if stale_claims.contains(id) {
                items.push(AttentionItem {
                    action: Some(actions.stale(*id)),
                    ..AttentionItem::of(AttentionKind::StaleClaim)
                });
            }
            (*id, items)
        })
        .collect())
}

/// The holder of each task's stale claim that needs a person: the holder of its implementation claim before the reviewer of a review claim on its proposal, because the task's card is the implementation's.
fn stale_holders(claims: Vec<ClaimForPerson>) -> HashMap<TaskId, AgentName> {
    let mut holders = HashMap::new();
    for claim in claims {
        match claim.target {
            ClaimTarget::Task(task) => {
                holders.insert(task, claim.worker.agent());
            }
            ClaimTarget::Review { task, .. } => {
                holders.entry(task).or_insert_with(|| claim.worker.agent());
            }
        }
    }
    holders
}

/// What the attention items read to name a person's action: the holder of each task's implementation claim and of each stale claim, and the state of each agent's supervised process.
#[derive(Debug, Default)]
struct Actions {
    holders: HashMap<TaskId, AgentName>,
    stale_holders: HashMap<TaskId, AgentName>,
    /// Whether the process of each agent fold runs runs now.
    running: HashMap<AgentName, bool>,
}

impl Actions {
    async fn read(conn: &mut SqliteConnection) -> Result<Self, StoreError> {
        let names: HashMap<i64, AgentName> = agents::all_workers(&mut *conn)
            .await?
            .into_iter()
            .map(|worker| (worker.id, worker.name.agent()))
            .collect();
        let holders = agents::held_claims(&mut *conn)
            .await?
            .into_iter()
            .filter(|claim| claim.kind == ClaimKind::Implementation)
            .filter_map(|claim| Some((claim.task, names.get(&claim.worker_id)?.clone())))
            .collect();
        let stale_holders = stale_holders(claims::claims_for_person(&mut *conn).await?);
        let running = agents::list_worker_processes(conn)
            .await?
            .into_iter()
            .map(|row| (row.agent, row.state.is_running()))
            .collect();
        Ok(Self {
            holders,
            stale_holders,
            running,
        })
    }

    /// A stalled worker that fold runs restarts in a new session; one that fold does not run hears a message.
    fn stalled(&self, task: TaskId) -> PersonAction {
        let supervised = self
            .holders
            .get(&task)
            .is_some_and(|agent| self.running.contains_key(agent));
        if supervised {
            PersonAction::RestartWorker
        } else {
            PersonAction::MessageWorker
        }
    }

    /// The holder of a stale claim whose process fold runs and that runs restarts in a new session: it runs and makes no fold call. A message resumes a holder whose process ended in its own session, and reaches a holder that fold does not run.
    fn stale(&self, task: TaskId) -> PersonAction {
        let running = self
            .stale_holders
            .get(&task)
            .and_then(|agent| self.running.get(agent))
            .copied()
            .unwrap_or(false);
        if running {
            PersonAction::RestartWorker
        } else {
            PersonAction::MessageWorker
        }
    }
}

/// The tasks whose workspace has the stalled flag in `live`.
pub fn stalled_tasks(live: Option<&LiveState>) -> BTreeSet<TaskId> {
    live.map(LiveState::all)
        .unwrap_or_default()
        .iter()
        .filter(|state| state.view.stalled)
        .filter_map(|state| state.view.task)
        .collect()
}

#[cfg(test)]
mod tests {
    use fold_core::{ProposalId, Timestamp, WorkerName};

    use super::*;

    fn claim(target: ClaimTarget, worker: &str) -> ClaimForPerson {
        ClaimForPerson {
            target,
            worker: WorkerName::parse(worker).unwrap(),
            since: Timestamp::from_millis(0).unwrap(),
        }
    }

    #[test]
    fn the_implementation_holder_names_the_action_of_a_task_before_a_reviewer() {
        let task = TaskId::from_number(4).unwrap();
        let review = ClaimTarget::Review {
            proposal: ProposalId::from_number(4).unwrap(),
            task,
        };
        for claims in [
            vec![
                claim(review, "review-dates"),
                claim(ClaimTarget::Task(task), "web-dates"),
            ],
            vec![
                claim(ClaimTarget::Task(task), "web-dates"),
                claim(review, "review-dates"),
            ],
        ] {
            assert_eq!(stale_holders(claims)[&task].as_str(), "web-dates");
        }
        assert_eq!(
            stale_holders(vec![claim(review, "review-dates")])[&task].as_str(),
            "review-dates"
        );
    }
}
