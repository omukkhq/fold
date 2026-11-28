use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::Arc;

use fold_core::agents::push::notice_for;
use fold_core::events::{Event, types};
use fold_core::requests::RequestState;
use fold_core::{AgentName, LabelName, Role, TaskId, TaskStatus, WorkerState};
use fold_store::agents::{self, WorkerProcessRow};
use fold_store::tasks::{self, claims, requests};
use fold_store::{StoreError, messages};

use super::{Supervisor, lock};
use crate::services::agents::AgentError;
use crate::services::agents::workers::code_of;

/// The context size above which an area lead whose area has no open task is asked for a hand-off note at its turn's end, so its next wake-up starts small.
pub(super) const IDLE_LEAD_HANDOFF_TOKENS: u64 = 150_000;

impl Supervisor {
    /// The turn that tells a lead of the work that waits for it, `None` when nothing does: its ready tasks or proposals (see [`Supervisor::catch_up`]), its unread messages, the requests that wait for it to draft them and, for an area lead, the stale claims of its area's tasks. `kept` names the workers a daemon start runs again.
    pub(super) async fn lead_owed_turn(
        &self,
        row: &WorkerProcessRow,
        kept: &BTreeSet<AgentName>,
    ) -> Option<String> {
        let mut parts: Vec<String> = self.catch_up(row, kept).await.into_iter().collect();
        match self.lead_other_work(row).await {
            Ok(other) => parts.extend(other),
            Err(error) => {
                tracing::warn!(agent = %row.agent, code = code_of(&error), "could not read the work that waits for a lead");
            }
        }
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    }

    async fn lead_other_work(&self, row: &WorkerProcessRow) -> Result<Vec<String>, AgentError> {
        let read = self.inner.store.read();
        let mut parts = Vec::new();
        if messages::undelivered_count(read, &row.agent).await? > 0 {
            parts.push("Messages wait for you: read them with get_inbox.".to_owned());
        }
        let filter = requests::RequestFilter {
            state: Some(RequestState::Open),
            ..requests::RequestFilter::default()
        };
        let mut conn = read.acquire().await.map_err(StoreError::from)?;
        let waiting: Vec<String> = requests::list(&mut conn, &filter)
            .await?
            .into_iter()
            .filter(|request| request.drafter.is_none())
            .filter(|request| {
                request.escalated_at.is_some()
                    || request.area.is_none()
                    || (row.role == Role::AreaLead && request.area == row.area)
            })
            .map(|request| request.id.to_string())
            .collect();
        drop(conn);
        if !waiting.is_empty() {
            parts.push(format!(
                "These requests wait for a drafter: {}. Read each with get_request and claim yours with claim_request.",
                waiting.join(", ")
            ));
        }
        if row.role == Role::AreaLead
            && let Some(label) = self.area_label(row).await?
        {
            let mut stale = Vec::new();
            for task in open_tasks_of(read, &label).await? {
                let lapsed = claims::unreleased_for_task(read, task)
                    .await?
                    .is_some_and(|claim| claim.stale_at.is_some());
                if lapsed {
                    stale.push(task.to_string());
                }
            }
            if !stale.is_empty() {
                let mut text = String::from("The claims of these tasks of your area went stale: ");
                let _ = write!(
                    text,
                    "{}. Read each with get_task and list_workers.",
                    stale.join(", ")
                );
                parts.push(text);
            }
        }
        Ok(parts)
    }

    /// The label of the row's area, for an area lead.
    pub(super) async fn area_label(
        &self,
        row: &WorkerProcessRow,
    ) -> Result<Option<LabelName>, StoreError> {
        let Some(area) = &row.area else {
            return Ok(None);
        };
        Ok(agents::find_area(self.inner.store.read(), area)
            .await?
            .map(|area| area.label))
    }

    /// Keeps a lead that a daemon start found idle with nothing waiting for it without a process: its row stays `idle`, so the next daemon start looks at it again, and [`Supervisor::wake_dormant`] runs it once work comes.
    pub(super) fn keep_dormant(&self, agent: &AgentName) {
        lock(&self.inner.dormant).insert(agent.clone());
    }

    /// Whether `agent` is a lead that fold keeps without a process until work comes.
    pub fn is_dormant(&self, agent: &AgentName) -> bool {
        lock(&self.inner.dormant).contains(agent)
    }

    /// Runs each dormant lead that work now waits for again in its own session, with the turn that names the work. A lead whose row left `idle` or that runs is no longer dormant.
    pub(super) async fn wake_dormant(self) {
        let dormant: Vec<AgentName> = lock(&self.inner.dormant).iter().cloned().collect();
        for agent in dormant {
            if let Err(error) = self.wake_one(&agent, &[]).await {
                tracing::warn!(%agent, code = code_of(&error), "could not wake a lead for its work");
            }
        }
    }

    /// Runs each dormant lead that `event` sends a notice again, with the notice and the work that waits for it: a lead without a process has no session to post the notice to, so the notice becomes its turn. A pause or its resume wakes nobody; the resume of a pause runs every idle row.
    pub(super) async fn wake_dormant_for(self, event: Arc<Event>) {
        if event.event_type == types::PROJECT_PAUSED || event.event_type == types::PROJECT_RESUMED {
            return;
        }
        let (event, ctx) = match crate::push::push_input(&self.inner.store, &event).await {
            Ok(loaded) => loaded,
            Err(error) => {
                tracing::warn!(
                    seq = event.seq,
                    code = error.log_code(),
                    "could not read whom an event tells, to wake a lead"
                );
                return;
            }
        };
        let mut owed: BTreeMap<AgentName, Vec<String>> = BTreeMap::new();
        for notice in notice_for(&event, &ctx) {
            if self.is_dormant(&notice.agent) {
                owed.entry(notice.agent).or_default().push(notice.text);
            }
        }
        for (agent, notices) in owed {
            if let Err(error) = self.wake_one(&agent, &notices).await {
                tracing::warn!(%agent, code = code_of(&error), "could not wake a lead for its work");
            }
        }
    }

    async fn wake_one(&self, agent: &AgentName, notices: &[String]) -> Result<(), AgentError> {
        let _resuming = self.inner.resuming.lock().await;
        if !self.is_dormant(agent) {
            return Ok(());
        }
        let mut conn = self
            .inner
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let row = agents::find_worker_process(&mut conn, agent).await?;
        drop(conn);
        let Some(row) = row.filter(|row| row.state == WorkerState::Idle && !self.is_running(agent))
        else {
            lock(&self.inner.dormant).remove(agent);
            return Ok(());
        };
        if self.is_paused().await? {
            return Ok(());
        }
        let mut parts = Vec::new();
        if !notices.is_empty() {
            let mut text = String::from("fold started you for work that waits for you:");
            for notice in notices {
                text.push_str("\n- ");
                text.push_str(notice);
            }
            parts.push(text);
        }
        parts.extend(self.lead_owed_turn(&row, &BTreeSet::new()).await);
        if parts.is_empty() {
            return Ok(());
        }
        lock(&self.inner.dormant).remove(agent);
        let launched = self.relaunch(&row, Some(parts.join("\n\n")), None).await;
        if launched.is_err() && !self.is_running(agent) {
            lock(&self.inner.dormant).insert(agent.clone());
        }
        launched
    }

    /// Whether the area of an area lead has no open task, one that is neither archived nor Done.
    pub(super) async fn area_is_empty(&self, row: &WorkerProcessRow) -> Result<bool, StoreError> {
        if row.role != Role::AreaLead {
            return Ok(false);
        }
        let Some(label) = self.area_label(row).await? else {
            return Ok(false);
        };
        Ok(open_tasks_of(self.inner.store.read(), &label)
            .await?
            .is_empty())
    }
}

/// The tasks with `label` that are not archived and not Done.
async fn open_tasks_of(
    read: &fold_store::SqlitePool,
    label: &LabelName,
) -> Result<Vec<TaskId>, StoreError> {
    let snapshot = tasks::ready_snapshot(read).await?;
    let mut conn = read.acquire().await.map_err(StoreError::from)?;
    let labels = tasks::labels_by_task(&mut conn, false).await?;
    Ok(snapshot
        .statuses
        .iter()
        .filter(|(_, status)| **status != TaskStatus::Done)
        .filter(|(task, _)| {
            labels
                .get(task)
                .is_some_and(|labels| labels.iter().any(|row| row.name == *label))
        })
        .map(|(task, _)| *task)
        .collect())
}
