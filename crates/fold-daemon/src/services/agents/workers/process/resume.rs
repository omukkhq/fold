use std::collections::BTreeMap;
use std::fmt::Write as _;

use fold_core::events::types;
use fold_core::reviews::Verdict;
use fold_core::roles::{StartFacts, check_limits, first_turn};
use fold_core::{
    AgentName, AreaName, Event, EventActor, FoldError, NewEvent, ProposalId, Role, TaskId,
    WorkerState, WorkspaceId, codes,
};
use fold_store::agents;
use fold_store::reviews::{self, CommentRow, ProposalState, ReviewRow};
use fold_store::{StoreError, workspaces};
use serde_json::{Value, json};
use tokio::sync::watch;

use super::{Input, Supervisor, lock};
use crate::services::agents::AgentError;
use crate::services::agents::claude::{ClaudeMessage, SystemMessage};
use crate::services::agents::workers::{new_session, not_supervised};
use crate::services::events::EventBuffer;
use crate::services::projects::load_config;

/// Why fold resumes a coder: a review asked for changes, or the proposal conflicts with main at integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reason {
    Changes,
    Conflict,
}

impl Reason {
    fn as_str(self) -> &'static str {
        match self {
            Reason::Changes => "changes",
            Reason::Conflict => "conflict",
        }
    }
}

/// One version of a proposal that needs its author again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ResumeItem {
    pub proposal: ProposalId,
    pub version: u32,
    pub reason: Reason,
}

/// What the first output of a launched process told about its session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::services::agents::workers) enum SessionStart {
    /// The session runs its turn.
    Went,
    /// `--resume` found no conversation; the process has exited.
    Lost,
    /// The process ended before it told, such as a stop or a crash.
    Ended,
}

/// The receivers of one launched process, taken when it starts so a process that ends at once is still read.
#[derive(Debug, Clone)]
pub(in crate::services::agents::workers) struct StartWatch {
    pub(super) session: watch::Receiver<Option<bool>>,
    pub(super) exited: watch::Receiver<bool>,
}

impl StartWatch {
    /// Waits for the process's first word about its session. A lost session is reported only once the process has exited and its exit is recorded, so the row can start again.
    pub(in crate::services::agents::workers) async fn outcome(mut self) -> SessionStart {
        loop {
            let seen = *self.session.borrow_and_update();
            match seen {
                Some(true) => return SessionStart::Went,
                Some(false) => {
                    let _ = self.exited.wait_for(|done| *done).await;
                    return SessionStart::Lost;
                }
                None => {}
            }
            if *self.exited.borrow_and_update() {
                return match *self.session.borrow() {
                    Some(true) => SessionStart::Went,
                    Some(false) => SessionStart::Lost,
                    None => SessionStart::Ended,
                };
            }
            tokio::select! {
                _ = self.session.changed() => {}
                _ = self.exited.changed() => {}
            }
        }
    }
}

/// What a line says about the session of a process: Claude Code answers `--resume` of an unknown session with an error result of no turns and exits, while a session that goes on sends `system/init`, an assistant message or a result of its turn.
pub(super) fn session_signal(message: &ClaudeMessage, resumed: bool) -> Option<bool> {
    match message {
        ClaudeMessage::Result(result) => {
            Some(!(resumed && result.is_error && result.num_turns == 0))
        }
        ClaudeMessage::Assistant(_) | ClaudeMessage::System(SystemMessage::Init(_)) => Some(true),
        _ => None,
    }
}

/// The proposal versions an event sends back to their authors. A batch review appends one `review.submitted` per member before its `review.batch_submitted`; the batch event alone counts, so a coder with several members is resumed once.
pub(super) fn resume_items(event: &Event) -> Vec<ResumeItem> {
    let kind = event.event_type;
    let version = |data: &Value| {
        data.get("version")
            .and_then(Value::as_u64)
            .and_then(|version| u32::try_from(version).ok())
    };
    let changes = Verdict::RequestChanges.as_str();
    if kind == types::REVIEW_SUBMITTED {
        let batched = event
            .data
            .get("batch")
            .is_some_and(|batch| !batch.is_null());
        let verdict = event.data.get("verdict").and_then(Value::as_str);
        if batched || verdict != Some(changes) {
            return Vec::new();
        }
        return event
            .proposal
            .zip(version(&event.data))
            .map(|(proposal, version)| ResumeItem {
                proposal,
                version,
                reason: Reason::Changes,
            })
            .into_iter()
            .collect();
    }
    if kind == types::REVIEW_BATCH_SUBMITTED {
        let Some(reviewed) = event.data.get("reviewed").and_then(Value::as_array) else {
            return Vec::new();
        };
        return reviewed
            .iter()
            .filter(|review| review.get("verdict").and_then(Value::as_str) == Some(changes))
            .filter_map(|review| {
                let proposal = review.get("proposal")?.as_str()?.parse().ok()?;
                Some(ResumeItem {
                    proposal,
                    version: version(review)?,
                    reason: Reason::Changes,
                })
            })
            .collect();
    }
    if kind == types::INTEGRATION_CONFLICTED {
        return event
            .proposal
            .zip(version(&event.data))
            .map(|(proposal, version)| ResumeItem {
                proposal,
                version,
                reason: Reason::Conflict,
            })
            .into_iter()
            .collect();
    }
    Vec::new()
}

/// One proposal version to hand back, with what the turn tells about it.
#[derive(Debug, Clone)]
struct Found {
    item: ResumeItem,
    task: TaskId,
    workspace: Option<WorkspaceId>,
    section: Section,
}

#[derive(Debug, Clone)]
enum Section {
    Changes {
        reviews: Vec<ReviewRow>,
        comments: Vec<CommentRow>,
    },
    /// The author message of the stored conflict report.
    Conflict(Option<String>),
}

impl Supervisor {
    /// Starts a stopped or failed worker again in the same agent and session with `--resume`, binds it to `bind` as well, and sends it `turn`. The worker counts against the limits again. A running worker is refused with `worker_exists`.
    pub(in crate::services::agents::workers) async fn resume(
        &self,
        agent: &AgentName,
        turn: String,
        bind: &[TaskId],
    ) -> Result<Option<StartWatch>, AgentError> {
        let config = load_config(&self.inner.paths).await?;
        let limits = config.workers.limits();
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        let row = self
            .inner
            .store
            .write(async |tx| {
                let row = agents::find_worker_process(tx.conn(), agent)
                    .await?
                    .ok_or_else(|| not_supervised(agent))?;
                if row.state.is_running() {
                    return Err(FoldError::new(
                        codes::WORKER_EXISTS,
                        format!("worker {agent} is already running"),
                    )
                    .with_details([("agent", json!(agent.as_str()))])
                    .with_hint("call list_workers")
                    .into());
                }
                let (running, in_area) =
                    agents::running_counts(tx.conn(), row.role, row.area_id).await?;
                check_limits(
                    limits,
                    row.role,
                    running,
                    row.area.as_ref().map(|area| (area, in_area)),
                )?;
                agents::restart_worker_process(tx.conn(), row.id, now).await?;
                for task in bind {
                    agents::bind_task(tx.conn(), row.id, *task).await?;
                }
                let event =
                    NewEvent::new(types::WORKER_STARTED, EventActor::fold())?.data(json!({
                        "agent": agent.as_str(),
                        "role": row.role.as_str(),
                        "area": row.area.as_ref().map(AreaName::as_str),
                        "resumed": true,
                    }))?;
                buffer.append(tx, &event, now).await?;
                agents::find_worker_process(tx.conn(), agent)
                    .await?
                    .ok_or(StoreError::Corrupt("worker_processes"))
                    .map_err(AgentError::from)
            })
            .await?;
        self.inner.bus.publish(buffer);
        self.launch(&row, true, Some(turn)).await
    }

    /// Hands each proposal version back to the task coder that wrote it. A stale item (the proposal moved on or is no longer open) and an author that is not a supervised task coder are left alone: an interactive author and a running coder get the push notice on their own.
    pub(super) async fn resume_authors(self, items: Vec<ResumeItem>) {
        let mut by_author: BTreeMap<AgentName, Vec<ResumeItem>> = BTreeMap::new();
        for item in items {
            match reviews::get_proposal(self.inner.store.read(), item.proposal).await {
                Ok(Some(row))
                    if row.state == ProposalState::Open && row.current_version == item.version =>
                {
                    by_author.entry(row.author.agent()).or_default().push(item);
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(proposal = %item.proposal, code = error.code().unwrap_or("store_error"), "could not read a proposal to resume its author");
                }
            }
        }
        for (agent, items) in by_author {
            if let Err(error) = self.resume_author(&agent, &items).await {
                tracing::warn!(%agent, code = crate::services::agents::code_of(&error), "could not resume the author of a proposal");
            }
        }
    }

    /// Resumes the authors of every requested change and conflict that no start of theirs has served yet, such as one the worker limits refused.
    pub(super) async fn resume_pending(self) {
        let pending = match agents::pending_resumes(self.inner.store.read()).await {
            Ok(pending) => pending,
            Err(error) => {
                tracing::warn!(
                    code = error.code().unwrap_or("store_error"),
                    "could not read the pending resumes"
                );
                return;
            }
        };
        let mut items = Vec::new();
        for owed in pending {
            for (reason, due) in [
                (Reason::Changes, owed.changes),
                (Reason::Conflict, owed.conflict),
            ] {
                if due {
                    items.push(ResumeItem {
                        proposal: owed.proposal,
                        version: owed.version,
                        reason,
                    });
                }
            }
        }
        if !items.is_empty() {
            self.resume_authors(items).await;
        }
    }

    async fn resume_author(
        &self,
        agent: &AgentName,
        items: &[ResumeItem],
    ) -> Result<(), AgentError> {
        // The row is read under the lock, so a second event finds the coder running and leaves it to its stdin notices.
        let resuming = self.inner.resuming.lock().await;
        let mut conn = self
            .inner
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let worker = agents::find_worker_process(&mut conn, agent).await?;
        drop(conn);
        let Some(worker) = worker else {
            return Ok(());
        };
        if worker.role != Role::TaskCoder || worker.state.is_running() {
            return Ok(());
        }
        let mut found = Vec::with_capacity(items.len());
        for item in items {
            found.push(self.load(*item).await?);
        }
        let tasks: Vec<TaskId> = found.iter().map(|found| found.task).collect();
        let turn = resume_turn(&found);
        let watch = match self.resume(agent, turn.clone(), &tasks).await {
            Ok(watch) => watch,
            // A start outside this path, such as `resume_worker`, came first; the coder takes this turn as its next one.
            Err(AgentError::Fold(error)) if error.code() == codes::WORKER_EXISTS => {
                if let Some(run) = lock(&self.inner.running).get(agent) {
                    let _ = run.input.send(Input::Turn(turn));
                }
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        drop(resuming);
        let outcome = match watch {
            Some(watch) => watch.outcome().await,
            None => SessionStart::Ended,
        };
        match outcome {
            SessionStart::Went => self.record_resumed(agent, &found).await,
            SessionStart::Lost => {
                let instructions = fresh_instructions(&found_turn_parts(&found));
                self.start_fresh(agent, &found, &instructions).await
            }
            SessionStart::Ended => Ok(()),
        }
    }

    /// A worker that a daemon start or a later start resumed with `--resume`: when its session is gone, it starts again in a new one with its role's first turn, which for a lead holds its latest hand-off note. `instructions` are the starter's, for a start that resumed a stopped lead.
    pub(in crate::services::agents::workers) async fn restart_lost(
        self,
        agent: AgentName,
        watch: StartWatch,
        instructions: Option<String>,
    ) {
        if watch.outcome().await != SessionStart::Lost {
            return;
        }
        let fresh = match self
            .lost_instructions(&agent, instructions.as_deref())
            .await
        {
            Ok(text) => self.start_fresh(&agent, &[], &text).await,
            Err(error) => Err(error),
        };
        if let Err(error) = fresh {
            tracing::warn!(%agent, code = crate::services::agents::code_of(&error), "could not start a worker whose session is gone");
        }
    }

    /// What a worker whose session is gone is told in its new session: a task coder to check its claims and proposals, a batch reviewer to check its verdicts, a lead to read its latest hand-off note, then the starter's instructions.
    async fn lost_instructions(
        &self,
        agent: &AgentName,
        instructions: Option<&str>,
    ) -> Result<String, AgentError> {
        let mut conn = self
            .inner
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let row = agents::find_worker_process(&mut conn, agent)
            .await?
            .ok_or_else(|| not_supervised(agent))?;
        let mut text = match row.role {
            Role::TaskCoder => RESTART_LOST.to_owned(),
            Role::BatchReviewer => REVIEWER_LOST.to_owned(),
            Role::AreaLead | Role::ReviewLead => {
                match agents::latest_handoff(&mut *conn, row.agent_id).await? {
                    Some(note) => format!(
                        "{LEAD_LOST} Your latest hand-off note:\n\n{}",
                        note.body.trim()
                    ),
                    None => LEAD_LOST.to_owned(),
                }
            }
        };
        if let Some(extra) = instructions.map(str::trim).filter(|text| !text.is_empty()) {
            let _ = write!(text, "\n\n{extra}");
        }
        Ok(text)
    }

    async fn load(&self, item: ResumeItem) -> Result<Found, AgentError> {
        let read = self.inner.store.read();
        let row = reviews::get_proposal(read, item.proposal)
            .await?
            .ok_or(StoreError::Corrupt("proposals.number"))?;
        let workspace = workspaces::live_task_workspace(read, row.task)
            .await?
            .map(|workspace| workspace.id);
        let section = match item.reason {
            Reason::Changes => Section::Changes {
                reviews: reviews::list_reviews(read, item.proposal)
                    .await?
                    .into_iter()
                    .filter(|review| {
                        review.version == item.version && review.verdict == Verdict::RequestChanges
                    })
                    .collect(),
                comments: reviews::list_comments(read, item.proposal, Some(item.version)).await?,
            },
            Reason::Conflict => Section::Conflict(
                reviews::conflict_report(read, item.proposal, item.version)
                    .await?
                    .map(|report| report.author_message()),
            ),
        };
        Ok(Found {
            item,
            task: row.task,
            workspace,
            section,
        })
    }

    /// `worker.resumed` for each proposal version, once the resumed session runs.
    async fn record_resumed(&self, agent: &AgentName, found: &[Found]) -> Result<(), AgentError> {
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        self.inner
            .store
            .write(async |tx| {
                for found in found {
                    buffer
                        .append(tx, &resumed_event(agent, found, true)?, now)
                        .await?;
                }
                Ok::<_, AgentError>(())
            })
            .await?;
        self.inner.bus.publish(buffer);
        Ok(())
    }

    /// The worker's session is gone: the same row, so the same author or lead, starts again with a new session, its binding and `instructions`, which name what `found` asks of it. With nothing found it records one `worker.resumed` of reason `session_lost`. A row that did not end `failed` was stopped on purpose and stays stopped.
    async fn start_fresh(
        &self,
        agent: &AgentName,
        found: &[Found],
        instructions: &str,
    ) -> Result<(), AgentError> {
        let config = load_config(&self.inner.paths).await?;
        let limits = config.workers.limits();
        let session = new_session()?;
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        let row = self
            .inner
            .store
            .write(async |tx| {
                let row = agents::find_worker_process(tx.conn(), agent)
                    .await?
                    .ok_or_else(|| not_supervised(agent))?;
                if row.state != WorkerState::Failed {
                    return Ok(None);
                }
                let (running, in_area) =
                    agents::running_counts(tx.conn(), row.role, row.area_id).await?;
                check_limits(
                    limits,
                    row.role,
                    running,
                    row.area.as_ref().map(|area| (area, in_area)),
                )?;
                agents::refresh_session(tx.conn(), row.id, &session, now).await?;
                let event =
                    NewEvent::new(types::WORKER_STARTED, EventActor::fold())?.data(json!({
                        "agent": agent.as_str(),
                        "role": row.role.as_str(),
                        "area": row.area.as_ref().map(AreaName::as_str),
                        "resumed": false,
                    }))?;
                buffer.append(tx, &event, now).await?;
                for found in found {
                    buffer
                        .append(tx, &resumed_event(agent, found, false)?, now)
                        .await?;
                }
                if found.is_empty() {
                    let event = NewEvent::new(types::WORKER_RESUMED, EventActor::fold())?.data(
                        json!({"agent": agent.as_str(), "reason": "session_lost", "resumed": false}),
                    )?;
                    buffer.append(tx, &event, now).await?;
                }
                Ok::<_, AgentError>(agents::find_worker_process(tx.conn(), agent).await?)
            })
            .await?;
        self.inner.bus.publish(buffer);
        let Some(row) = row else {
            return Ok(());
        };
        let label = match &row.area {
            Some(area) => agents::find_area(self.inner.store.read(), area)
                .await?
                .map(|area| area.label),
            None => None,
        };
        let turn = first_turn(&StartFacts {
            role: row.role,
            name: agent,
            area: row.area.as_ref().zip(label.as_ref()),
            tasks: &row.tasks,
            proposals: &row.proposals,
            instructions: Some(instructions),
        });
        self.launch(&row, false, Some(turn)).await?;
        Ok(())
    }
}

fn resumed_event(
    agent: &AgentName,
    found: &Found,
    resumed: bool,
) -> Result<NewEvent, fold_core::EventError> {
    let ResumeItem {
        proposal,
        version,
        reason,
    } = found.item;
    NewEvent::new(types::WORKER_RESUMED, EventActor::fold())?
        .task(found.task)
        .proposal(proposal)
        .data(json!({
            "agent": agent.as_str(),
            "proposal": proposal.to_string(),
            "version": version,
            "reason": reason.as_str(),
            "resumed": resumed,
        }))
}

fn found_turn_parts(found: &[Found]) -> String {
    found
        .iter()
        .map(section_text)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The turn a resumed coder gets: each proposal version it must change, then how it ends.
fn resume_turn(found: &[Found]) -> String {
    format!(
        "fold resumed you because work you proposed needs you again.\n\n{}\n\n{STOP_LINE}",
        found_turn_parts(found)
    )
}

/// A new session has no memory of the work, so it is told which proposals it wrote and where.
fn fresh_instructions(parts: &str) -> String {
    format!(
        "Your earlier session is gone, so fold started you in a new one. You are the author of the proposal versions below. Keep working in the task workspace each one names and send each new version with update_proposal; do not open a new proposal.\n\n{parts}\n\n{STOP_LINE}"
    )
}

const STOP_LINE: &str = "fold stops you once each of these proposals has its new version.";

/// What a task coder whose session a daemon start could not resume is told.
const RESTART_LOST: &str = "fold restarted and your earlier session is gone, so fold started you in a new one. Check your claims, your task workspaces and the proposals you already sent before you go on. Send a new version of an open proposal with update_proposal; do not open a second one.";

/// What a lead whose session could not be resumed is told before its note.
const LEAD_LOST: &str = "Your earlier session is gone, so fold started you in a new one. Read your area with get_task_graph and your workers with list_workers before you go on.";

/// What a batch reviewer whose session could not be resumed is told.
const REVIEWER_LOST: &str = "Your earlier session is gone, so fold started you in a new one. Read each of your proposals with get_proposal and give a verdict only to those that have none from you.";

fn section_text(found: &Found) -> String {
    let ResumeItem {
        proposal, version, ..
    } = found.item;
    let task = found.task;
    let place = match found.workspace {
        Some(workspace) => format!("Your task workspace is {workspace}; change files only there."),
        None => format!(
            "{task} has no active task workspace; call get_workspace before you change files."
        ),
    };
    match &found.section {
        Section::Changes { reviews, comments } => {
            let mut text = format!("{proposal} v{version} for {task}: changes requested. {place}");
            for review in reviews {
                let body = review.body.trim();
                if !body.is_empty() {
                    let _ = write!(text, "\nReview by {}: {body}", review.reviewer.as_str());
                }
            }
            for comment in comments {
                let location = match (&comment.path, comment.start_line, comment.end_line) {
                    (Some(path), Some(start), Some(end)) if start == end => {
                        format!("{path}:{start}")
                    }
                    (Some(path), Some(start), Some(end)) => format!("{path}:{start}-{end}"),
                    _ => "whole version".to_owned(),
                };
                let _ = write!(
                    text,
                    "\n- {location} ({}): {}",
                    comment.author.as_str(),
                    comment.body.trim()
                );
            }
            let next = version.saturating_add(1);
            let _ = write!(
                text,
                "\nMake the changes, record them, then call update_proposal for {proposal} to send v{next}. get_review_comments shows the comments again."
            );
            text
        }
        Section::Conflict(message) => {
            let detail = message.clone().unwrap_or_else(|| {
                format!("{proposal} v{version} conflicts with main. Call get_proposal to read the conflict report, then sync_workspace to merge main into your workspace, resolve each conflicted path and call update_proposal.")
            });
            format!("{proposal} v{version} for {task}: conflicts with main. {place}\n{detail}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fold_core::Timestamp;
    use fold_core::reviews::Side;

    fn event(kind: fold_core::events::EventType, proposal: Option<u32>, data: Value) -> Event {
        Event {
            seq: 1,
            event_type: kind,
            actor: EventActor::fold(),
            task: None,
            proposal: proposal.and_then(ProposalId::from_number),
            workspace: None,
            revision: None,
            data,
            created_at: Timestamp::from_millis(0).unwrap(),
        }
    }

    fn proposal(number: u32) -> ProposalId {
        ProposalId::from_number(number).unwrap()
    }

    #[test]
    fn requested_changes_and_conflicts_resume_and_a_batch_counts_once() {
        let single = event(
            types::REVIEW_SUBMITTED,
            Some(3),
            json!({"verdict": "request_changes", "version": 2, "comments": 1}),
        );
        assert_eq!(
            resume_items(&single),
            [ResumeItem {
                proposal: proposal(3),
                version: 2,
                reason: Reason::Changes
            }]
        );
        let member = event(
            types::REVIEW_SUBMITTED,
            Some(3),
            json!({"verdict": "request_changes", "version": 2, "batch": "rb-1"}),
        );
        assert!(resume_items(&member).is_empty());
        let approve = event(
            types::REVIEW_SUBMITTED,
            Some(3),
            json!({"verdict": "approve", "version": 2}),
        );
        assert!(resume_items(&approve).is_empty());
        let batch = event(
            types::REVIEW_BATCH_SUBMITTED,
            None,
            json!({"batch": "rb-1", "reviewed": [
                {"proposal": "P-3", "version": 1, "verdict": "request_changes", "comments": 0},
                {"proposal": "P-4", "version": 2, "verdict": "approve", "comments": 0},
                {"proposal": "P-5", "version": 1, "verdict": "request_changes", "comments": 2},
                {"proposal": "bad", "version": 1, "verdict": "request_changes"}
            ], "left_out": []}),
        );
        assert_eq!(
            resume_items(&batch)
                .iter()
                .map(|item| (item.proposal, item.version))
                .collect::<Vec<_>>(),
            [(proposal(3), 1), (proposal(5), 1)]
        );
        assert!(resume_items(&event(types::REVIEW_BATCH_SUBMITTED, None, json!({}))).is_empty());
        let conflict = event(
            types::INTEGRATION_CONFLICTED,
            Some(7),
            json!({"version": 1, "files": 2}),
        );
        assert_eq!(resume_items(&conflict)[0].reason, Reason::Conflict);
        assert!(
            resume_items(&event(
                types::INTEGRATION_CONFLICTED,
                None,
                json!({"version": 1})
            ))
            .is_empty()
        );
        assert!(resume_items(&event(types::PROPOSAL_OPENED, Some(7), json!({}))).is_empty());
    }

    #[test]
    fn only_a_resume_answered_by_an_empty_error_result_lost_its_session() {
        let lost = ClaudeMessage::parse(
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"num_turns":0,"session_id":"s"}"#,
        )
        .unwrap();
        assert_eq!(session_signal(&lost, true), Some(false));
        assert_eq!(session_signal(&lost, false), Some(true));
        let interrupted = ClaudeMessage::parse(
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"num_turns":1,"session_id":"s"}"#,
        )
        .unwrap();
        assert_eq!(session_signal(&interrupted, true), Some(true));
        let assistant = ClaudeMessage::parse(
            r#"{"type":"assistant","message":{"role":"assistant","content":[]},"session_id":"s"}"#,
        )
        .unwrap();
        assert_eq!(session_signal(&assistant, true), Some(true));
        let other =
            ClaudeMessage::parse(r#"{"type":"system","subtype":"hook_started","session_id":"s"}"#)
                .unwrap();
        assert_eq!(session_signal(&other, true), None);
    }

    #[test]
    fn a_turn_names_the_workspace_the_comments_and_the_next_step() {
        let at = Timestamp::from_millis(0).unwrap();
        let task = TaskId::from_number(1).unwrap();
        let reviewer = EventActor::parse("review").unwrap();
        let changes = Found {
            item: ResumeItem {
                proposal: proposal(3),
                version: 1,
                reason: Reason::Changes,
            },
            task,
            workspace: WorkspaceId::from_number(2),
            section: Section::Changes {
                reviews: vec![ReviewRow {
                    version: 1,
                    verdict: Verdict::RequestChanges,
                    reviewer: reviewer.clone(),
                    body: "Split the page.".to_owned(),
                    comments: 2,
                    created_at: at,
                }],
                comments: vec![
                    CommentRow {
                        id: fold_core::CommentId::from_number(1).unwrap(),
                        version: 1,
                        verdict: Verdict::RequestChanges,
                        author: reviewer.clone(),
                        path: Some("page.html".to_owned()),
                        side: Some(Side::New),
                        start_line: Some(1),
                        end_line: Some(2),
                        reply_to: None,
                        body: "Use a heading.".to_owned(),
                        created_at: at,
                        batch_comment: None,
                    },
                    CommentRow {
                        id: fold_core::CommentId::from_number(2).unwrap(),
                        version: 1,
                        verdict: Verdict::RequestChanges,
                        author: reviewer,
                        path: None,
                        side: None,
                        start_line: None,
                        end_line: None,
                        reply_to: None,
                        body: "Add a test.".to_owned(),
                        created_at: at,
                        batch_comment: None,
                    },
                ],
            },
        };
        let turn = resume_turn(std::slice::from_ref(&changes));
        assert!(turn.contains("P-3 v1 for T-1: changes requested"), "{turn}");
        assert!(turn.contains("Your task workspace is ws-2"), "{turn}");
        assert!(turn.contains("Review by review: Split the page."), "{turn}");
        assert!(
            turn.contains("- page.html:1-2 (review): Use a heading."),
            "{turn}"
        );
        assert!(
            turn.contains("- whole version (review): Add a test."),
            "{turn}"
        );
        assert!(
            turn.contains("update_proposal for P-3 to send v2"),
            "{turn}"
        );
        assert!(turn.ends_with(STOP_LINE), "{turn}");

        let conflict = Found {
            item: ResumeItem {
                proposal: proposal(4),
                version: 2,
                reason: Reason::Conflict,
            },
            task,
            workspace: None,
            section: Section::Conflict(None),
        };
        let text = section_text(&conflict);
        assert!(
            text.contains("P-4 v2 for T-1: conflicts with main"),
            "{text}"
        );
        assert!(text.contains("T-1 has no active task workspace"), "{text}");
        assert!(text.contains("sync_workspace"), "{text}");
        let reported = Found {
            section: Section::Conflict(Some("the stored report".to_owned())),
            ..conflict
        };
        assert!(section_text(&reported).ends_with("the stored report"));
        let fresh = fresh_instructions(&found_turn_parts(&[changes]));
        assert!(fresh.starts_with("Your earlier session is gone"), "{fresh}");
        assert!(fresh.contains("P-3 v1 for T-1"), "{fresh}");
    }
}
