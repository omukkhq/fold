use std::sync::atomic::Ordering;

use fold_core::events::EventFilter;
use fold_core::events::types;
use fold_core::{
    AgentName, CheckRunId, Event, EventActor, NewEvent, ProposalId, Role, WorkerState, codes,
};
use fold_store::reviews::{self, ProposalRow, ProposalState, checks};
use fold_store::{StoreError, agents, events};
use serde_json::{Value, json};

use super::message::SESSION_LOST;
use super::resume::SessionStart;
use super::{Input, Supervisor, lock};
use crate::services::agents::AgentError;
use crate::services::agents::workers::is_paused_refusal;
use crate::services::events::EventBuffer;
use crate::services::reviews::{Handoff, NoReviewer, handoff_notice_in_tx, reviewer_turn};

/// A conflict, or failed integration checks of a lane member after its lane moved, that an event gives to the reviewer that approved the version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Owed {
    pub proposal: ProposalId,
    pub version: u32,
    pub agent: AgentName,
    /// The lane main moved in since the version's base, for the note of the turn.
    pub lane: Option<String>,
    /// The failed integration run of a lane member; `None` for a conflict.
    pub run: Option<CheckRunId>,
}

/// Whether `event` hands a version to a reviewer to fix on main: `integration.conflicted`, or a failed integration `checks.finished` with a `lane`.
fn is_handoff(event: &Event) -> bool {
    event.event_type == types::INTEGRATION_CONFLICTED
        || (event.event_type == types::CHECKS_FINISHED
            && event.data.get("lane").and_then(Value::as_str).is_some())
}

/// What a handoff event gives the reviewer it names as `fixer`; `None` for any other event and for a handoff that went to the review lead.
pub(super) fn conflict_fixer(event: &Event) -> Option<Owed> {
    if !is_handoff(event) {
        return None;
    }
    let version = event
        .data
        .get("version")
        .and_then(Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())?;
    let agent = event
        .data
        .get("fixer")
        .and_then(Value::as_str)
        .and_then(|name| AgentName::parse(name).ok())?;
    let lane = event
        .data
        .get("lane")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let run = (event.event_type == types::CHECKS_FINISHED)
        .then(|| event.data.get("run").and_then(Value::as_str)?.parse().ok())
        .flatten();
    Some(Owed {
        proposal: event.proposal?,
        version,
        agent,
        lane,
        run,
    })
}

/// What the reviewer still owes for `version`, from the handoff and `worker.resumed` events of one proposal in `seq` order: the newest handoff of that version, unless a `worker.resumed` of reason `conflict` for that version follows it: a resume that ran, or with `resumed` false the notice that handed the conflict to the review lead.
pub(super) fn owed_fixer(events: &[Event], version: u32) -> Option<Owed> {
    let of_version = |event: &Event| {
        event.data.get("version").and_then(Value::as_u64) == Some(u64::from(version))
    };
    let at = events
        .iter()
        .rposition(|event| is_handoff(event) && of_version(event))?;
    let taken = events[at..].iter().any(|event| {
        event.event_type == types::WORKER_RESUMED
            && of_version(event)
            && event.data.get("reason").and_then(Value::as_str) == Some("conflict")
    });
    if taken {
        return None;
    }
    conflict_fixer(&events[at])
}

/// How the conflict reached the reviewer.
enum Given {
    /// The reviewer runs: the conflict is its next turn.
    Told,
    /// The resumed session runs its turn.
    Resumed,
    /// The reviewer cannot take it; `lost` is its worker row when its session is gone.
    Refused { lost: Option<i64> },
    /// fold stops the reviewer as a finished worker, so a turn sent now would be lost: the conflict goes to it once the stop has settled.
    AfterStop,
    /// The worker limits hold the resume back: the conflict stays owed, an owed resume ranked by its time, and a freed place gives it.
    Waits,
    /// Nothing to give now: the version moved on, was integrated or closed, its conflict is gone, or a pause holds the start back.
    Stale,
}

impl Supervisor {
    /// Gives the conflict of `version` to `agent`, the reviewer that approved it: a running reviewer gets the conflict report as its next turn, and a stopped or failed one is resumed in its own session with it, with `worker.resumed` of reason `conflict` once that session runs. A reviewer that fold stops as finished, as it does once the reviewer's approval is in, which is often just before the integration that approval started conflicts, gets the conflict once the stop has settled: resumed in its own session when it stopped, told when it kept running. A start that the worker limits refuse leaves the conflict owed, and the next freed place gives it (see [`Supervisor::give_owed_conflicts_to_ended`]). Only when the reviewer cannot take it, because it lost the review claim, its session is gone or its process ends before it runs, does the review lead get the notice to start a reviewer for the conflict. A version that moved on, or a proposal that left `open`, is left alone, and so is a start that a pause refuses: the resume of the pause gives the conflict.
    pub(super) async fn give_conflict_to_reviewer(self, owed: Owed) {
        self.give_conflict_once(owed, false).await;
    }

    /// Gives each conflict that [`Supervisor::give_owed_conflicts`] finds to its reviewer when that reviewer has stopped or failed, such as one whose start the worker limits refused; a running reviewer is left until it stops again. A retry that finds the reviewer cannot take the conflict, such as a resume whose process ends before its turn, a session another resume lost or a claim that moved, sends the review lead the notice, once for each version. The watcher runs it when a place may have freed: a worker stopped or failed, or the limits changed.
    pub(super) async fn give_owed_conflicts_to_ended(self) {
        self.give_owed(true).await;
    }

    async fn give_conflict_once(self, owed: Owed, ended_only: bool) {
        if !self.start_ticket().await {
            return;
        }
        let key = (owed.proposal, owed.version);
        if !lock(&self.inner.giving_conflicts).insert(key) {
            return;
        }
        let given = self.give_conflict(&owed, ended_only).await;
        lock(&self.inner.giving_conflicts).remove(&key);
        if let Err(error) = given {
            tracing::warn!(agent = %owed.agent, proposal = %owed.proposal, code = crate::services::agents::code_of(&error), "could not give a conflict to its reviewer");
        }
    }

    /// Gives each conflict that its reviewer has not taken, from the stored state: an open proposal flagged with a conflict and no requested changes, whose current version's newest `integration.conflicted` names a fixer that no `worker.resumed` of reason `conflict` answered (see [`owed_fixer`]). The watcher runs it after it fell behind the bus and a daemon start runs it, because the live event that gives a conflict can be lost; a reviewer that ran when fold told it of the conflict hears of it again then. A read that fails is logged and the rest is served.
    pub(super) async fn give_owed_conflicts(self) {
        self.give_owed(false).await;
    }

    async fn give_owed(self, ended_only: bool) {
        let read = self.inner.store.read();
        let open = match reviews::list_proposals(read, Some(ProposalState::Open), None).await {
            Ok(open) => open,
            Err(error) => {
                tracing::warn!(
                    code = error.log_code(),
                    "could not read the open proposals for owed conflicts"
                );
                return;
            }
        };
        for row in open
            .into_iter()
            .filter(|row| row.conflicted && !row.changes_requested)
        {
            let filter = EventFilter {
                types: vec![
                    types::INTEGRATION_CONFLICTED,
                    types::CHECKS_FINISHED,
                    types::WORKER_RESUMED,
                ],
                task: None,
                proposal: Some(row.id),
            };
            let owed = match events::read_after(read, 0, &filter, events::MAX_PAGE).await {
                Ok(found) => owed_fixer(&found, row.current_version),
                Err(error) => {
                    tracing::warn!(proposal = %row.id, code = error.log_code(), "could not read the events of a conflicted proposal");
                    continue;
                }
            };
            if let Some(owed) = owed {
                self.clone().give_conflict_once(owed, ended_only).await;
            }
        }
    }

    async fn give_conflict(&self, owed: &Owed, ended_only: bool) -> Result<(), AgentError> {
        let read = self.inner.store.read();
        let Some(row) = reviews::get_proposal(read, owed.proposal).await? else {
            return Ok(());
        };
        let report;
        let handoff = match owed.run {
            Some(run) => {
                let Some(run_row) = checks::get_run(read, run).await? else {
                    return Ok(());
                };
                Handoff::LaneChecks {
                    proposal: owed.proposal,
                    version: owed.version,
                    run,
                    main: run_row.base,
                }
            }
            None => {
                let Some(found) =
                    reviews::conflict_report(read, owed.proposal, owed.version).await?
                else {
                    return Ok(());
                };
                report = found;
                Handoff::Conflict(&report)
            }
        };
        let lane = owed.lane.as_deref();
        let mut given = self
            .offer(&row, owed.version, &handoff, &owed.agent, lane, ended_only)
            .await?;
        if matches!(given, Given::AfterStop) && self.until_stop_settled(&owed.agent).await {
            given = self
                .offer(&row, owed.version, &handoff, &owed.agent, lane, ended_only)
                .await?;
        }
        match given {
            // A stop that has not settled in time ends with `worker.stopped`, whose freed place gives the conflict.
            Given::Told | Given::Stale | Given::Waits | Given::AfterStop => Ok(()),
            Given::Resumed => {
                self.record_conflict_resumed(&row, owed.version, &owed.agent)
                    .await
            }
            Given::Refused { lost } => {
                let why = NoReviewer::NotResumed(owed.agent.clone());
                self.conflict_to_review_lead(&row, owed, &handoff, &why, lost, lane)
                    .await
            }
        }
    }

    async fn offer(
        &self,
        row: &ProposalRow,
        version: u32,
        handoff: &Handoff<'_>,
        agent: &AgentName,
        lane: Option<&str>,
        ended_only: bool,
    ) -> Result<Given, AgentError> {
        // The proposal, the claim and the row are read under the lock, so a second start of the reviewer finds it running.
        let resuming = self.inner.resuming.lock().await;
        let mut conn = self
            .inner
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let Some(current) = reviews::get_proposal(&mut *conn, row.id).await? else {
            return Ok(Given::Stale);
        };
        if current.state != ProposalState::Open
            || current.current_version != version
            || !current.conflicted
        {
            return Ok(Given::Stale);
        }
        let holds = reviews::unreleased_review_claim(&mut *conn, row.id)
            .await?
            .is_some_and(|claim| claim.worker.agent() == *agent);
        let Some(worker) = agents::find_worker_process(&mut conn, agent)
            .await?
            .filter(|worker| worker.role == Role::Reviewer && holds)
        else {
            return Ok(Given::Refused { lost: None });
        };
        drop(conn);
        // A session that another resume lost, such as one for a message, cannot take the conflict either.
        if worker.state == WorkerState::Failed
            && worker.failed_reason.as_deref() == Some(SESSION_LOST)
        {
            return Ok(Given::Refused { lost: None });
        }
        // A retry leaves a running reviewer alone: it was told, or it runs for other work and gets the conflict once it stops again.
        if ended_only && worker.state.is_running() {
            return Ok(Given::Stale);
        }
        if self.is_stopping(agent).await? {
            return Ok(Given::AfterStop);
        }
        let turn = reviewer_turn(handoff, row.task, lane);
        if worker.state.is_running() {
            return Ok(if self.tell_running(agent, turn) {
                Given::Told
            } else {
                Given::Refused { lost: None }
            });
        }
        // A parked reviewer waits for its go, so its review lead gives the conflict to another reviewer.
        if self.held_by_park(&worker).await? {
            return Ok(Given::Refused { lost: None });
        }
        let turn = format!("fold resumed you because {turn}");
        let watch = match self.resume(agent, turn.clone(), &[]).await {
            Ok(watch) => watch,
            Err(AgentError::Fold(error)) if error.code() == codes::WORKER_EXISTS => {
                return Ok(if self.tell_running(agent, turn) {
                    Given::Told
                } else {
                    Given::Refused { lost: None }
                });
            }
            // The conflict stays owed, and the resume of the pause gives it.
            Err(error) if is_paused_refusal(&error) => return Ok(Given::Stale),
            Err(AgentError::Fold(error)) if error.code() == codes::WORKER_LIMIT_REACHED => {
                return Ok(Given::Waits);
            }
            Err(error) => return Err(error),
        };
        drop(resuming);
        let outcome = match watch {
            Some(watch) => watch.outcome().await,
            None => SessionStart::Ended,
        };
        Ok(match outcome {
            SessionStart::Went => Given::Resumed,
            SessionStart::Lost => Given::Refused {
                lost: Some(worker.id),
            },
            // A pause that ended the process leaves the conflict owed for its resume.
            SessionStart::Ended if self.is_paused().await? => Given::Stale,
            SessionStart::Ended => Given::Refused { lost: None },
        })
    }

    /// Sends `turn` to the running process of `agent`; false when it has none that fold still reads.
    fn tell_running(&self, agent: &AgentName, turn: String) -> bool {
        let input = lock(&self.inner.running)
            .get(agent)
            .filter(|run| !run.detached.load(Ordering::SeqCst))
            .map(|run| run.input.clone());
        input.is_some_and(|input| input.send(Input::Turn(turn)).is_ok())
    }

    async fn record_conflict_resumed(
        &self,
        row: &ProposalRow,
        version: u32,
        agent: &AgentName,
    ) -> Result<(), AgentError> {
        let event = NewEvent::new(types::WORKER_RESUMED, EventActor::fold())?
            .task(row.task)
            .proposal(row.id)
            .data(json!({
                "agent": agent.as_str(),
                "proposal": row.id.to_string(),
                "version": version,
                "reason": "conflict",
                "resumed": true,
            }))?;
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        self.inner
            .store
            .write(async |tx| {
                buffer.append(tx, &event, now).await?;
                Ok::<_, AgentError>(())
            })
            .await?;
        self.inner.bus.publish(buffer);
        Ok(())
    }

    /// The notice for a conflict its reviewer could not take, to the review lead or, for a task of main, the main agent; `lost` is the worker row whose session is gone, recorded so a later start begins a new session. The same write appends `worker.resumed` of reason `conflict` with `resumed` false for the version, so [`owed_fixer`] counts the conflict as handed on and no retry or daemon start sends the notice again.
    async fn conflict_to_review_lead(
        &self,
        row: &ProposalRow,
        owed: &Owed,
        handoff: &Handoff<'_>,
        why: &NoReviewer,
        lost: Option<i64>,
        lane: Option<&str>,
    ) -> Result<(), AgentError> {
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        self.inner
            .store
            .write(async |tx| {
                if let Some(id) = lost {
                    agents::mark_session_lost(tx, id, SESSION_LOST).await?;
                }
                handoff_notice_in_tx(tx, &mut buffer, handoff, row.task, why, lane, now)
                    .await
                    .map_err(AgentError::from)?;
                let handed = NewEvent::new(types::WORKER_RESUMED, EventActor::fold())?
                    .task(row.task)
                    .proposal(row.id)
                    .data(json!({
                        "agent": owed.agent.as_str(),
                        "proposal": row.id.to_string(),
                        "version": owed.version,
                        "reason": "conflict",
                        "resumed": false,
                    }))?;
                buffer.append(tx, &handed, now).await?;
                Ok::<_, AgentError>(())
            })
            .await?;
        self.inner.bus.publish(buffer);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fold_core::Timestamp;

    fn event(kind: fold_core::events::EventType, data: Value) -> Event {
        Event {
            seq: 1,
            event_type: kind,
            actor: EventActor::fold(),
            task: None,
            proposal: ProposalId::from_number(4),
            workspace: None,
            revision: None,
            data,
            created_at: Timestamp::from_millis(0).unwrap(),
        }
    }

    #[test]
    fn a_conflict_is_owed_until_its_reviewer_is_resumed_for_that_version() {
        let conflicted = |version: u32, fixer: Value| {
            event(
                types::INTEGRATION_CONFLICTED,
                json!({"version": version, "fixer": fixer}),
            )
        };
        let resumed = |version: u32, reason: &str| {
            event(
                types::WORKER_RESUMED,
                json!({"agent": "reviewer-3", "version": version, "reason": reason}),
            )
        };
        let owed =
            |events: &[Event]| owed_fixer(events, 2).map(|owed| owed.agent.as_str().to_owned());
        assert_eq!(
            owed(&[conflicted(2, json!("reviewer-3"))]),
            Some("reviewer-3".to_owned())
        );
        assert_eq!(
            owed(&[
                conflicted(2, json!("reviewer-3")),
                resumed(2, "new_version")
            ]),
            Some("reviewer-3".to_owned())
        );
        assert_eq!(
            owed(&[conflicted(2, json!("reviewer-3")), resumed(1, "conflict")]),
            Some("reviewer-3".to_owned())
        );
        assert_eq!(
            owed(&[conflicted(2, json!("reviewer-3")), resumed(2, "conflict")]),
            None
        );
        assert_eq!(
            owed(&[resumed(2, "conflict"), conflicted(2, json!("reviewer-3"))]),
            Some("reviewer-3".to_owned())
        );
        assert_eq!(owed(&[conflicted(2, json!(null))]), None);
        assert_eq!(owed(&[conflicted(1, json!("reviewer-3"))]), None);
        assert_eq!(owed(&[]), None);
    }

    #[test]
    fn only_a_conflict_with_a_fixer_names_its_reviewer() {
        let given = event(
            types::INTEGRATION_CONFLICTED,
            json!({"version": 2, "fixer": "reviewer-3"}),
        );
        let owed = conflict_fixer(&given).unwrap();
        assert_eq!(owed.proposal.to_string(), "P-4");
        assert_eq!(owed.version, 2);
        assert_eq!(owed.agent.as_str(), "reviewer-3");
        assert_eq!((owed.lane, owed.run), (None, None));
        let lane_checks = event(
            types::CHECKS_FINISHED,
            json!({"version": 2, "fixer": "reviewer-3", "lane": "migrations", "run": "cr-9", "stage": "integration", "state": "failed"}),
        );
        let owed = conflict_fixer(&lane_checks).unwrap();
        assert_eq!(owed.lane.as_deref(), Some("migrations"));
        assert_eq!(owed.run.map(|run| run.to_string()).as_deref(), Some("cr-9"));
        let plain_checks = event(
            types::CHECKS_FINISHED,
            json!({"version": 2, "fixer": "reviewer-3", "run": "cr-9"}),
        );
        assert!(
            conflict_fixer(&plain_checks).is_none(),
            "only a lane failure is a handoff"
        );
        let to_lead = event(
            types::INTEGRATION_CONFLICTED,
            json!({"version": 2, "fixer": null}),
        );
        assert!(conflict_fixer(&to_lead).is_none());
        let other = event(
            types::PROPOSAL_OPENED,
            json!({"version": 2, "fixer": "reviewer-3"}),
        );
        assert!(conflict_fixer(&other).is_none());
    }

    #[test]
    fn a_lane_failure_is_owed_like_a_conflict() {
        let lane_checks = |version: u32| {
            event(
                types::CHECKS_FINISHED,
                json!({"version": version, "fixer": "reviewer-3", "lane": "m", "run": "cr-2"}),
            )
        };
        let resumed = event(
            types::WORKER_RESUMED,
            json!({"agent": "reviewer-3", "version": 2, "reason": "conflict"}),
        );
        let plain = event(types::CHECKS_FINISHED, json!({"version": 2, "run": "cr-3"}));
        let owed = owed_fixer(&[lane_checks(2), plain.clone()], 2).unwrap();
        assert_eq!(owed.run.map(|run| run.to_string()).as_deref(), Some("cr-2"));
        assert!(owed_fixer(&[lane_checks(2), resumed], 2).is_none());
        assert!(owed_fixer(&[plain], 2).is_none());
    }
}
