use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

use fold_core::dag::Dag;
use fold_core::readiness::{Candidate, ready_order, unblock_count};
use fold_core::requests::RequestState;
use fold_core::roles::{
    CatchUpFacts, FLD_MISSING, MainCatchUp, Role, StartFacts, catch_up_turn, first_turn,
    main_catch_up_turn, needs_restart,
};
use fold_core::{AgentName, AreaName, ProposalId, RequestId, WorkerName, WorkerState};
use fold_store::agents::{self, WorkerProcessRow};
use fold_store::reviews::{self, ProposalRow, ProposalState};
use fold_store::tasks::LabelRow;
use fold_store::tasks::requests;
use fold_store::{StoreError, messages, tasks};

use super::{Supervisor, lock, resume, reviewer_batch};
use crate::services::agents::AgentError;
use crate::services::agents::workers::code_of;
use crate::services::projects::load_config;

/// The turn a worker that was working gets after a daemon restart resumed its session.
pub const RESTART_TURN: &str = "fold restarted while you were working. Check your claims and your workspace, then continue your work.";

/// The turn of a worker that could not reach fold, which a daemon start resumed. Its claims stayed with it, but their leases ran on.
pub const MCP_RETRY_TURN: &str = "fold resumed you after your fold MCP server failed to start or connect, so your last turn may have run without the fold tools. You kept your claims, but a lease can have expired while you waited: check your claims before you continue. Check your inbox and your workspace, then continue your work.";

/// The first turn of each worker that the resume of a pause runs again in its own session. Its claims kept their leases through the pause.
pub const PAUSE_TURN: &str = "fold resumed you after a pause of every agent of the project. Your claims kept their leases. Check your inbox, your claims and your workspace, then continue your work.";

impl Supervisor {
    /// Runs again, after a daemon start, every worker that was working or idle when the daemon stopped, and every worker that could not reach fold: `--resume` of its session, with a turn that tells a working one to go on and a turn that tells the other one that its fold connection failed. A lead hears in the same turn of the work that waits for it, and an idle lead with nothing waiting gets no turn. A worker whose fld binary was gone resumes only once the fld path names a file again. A start reads the rows once, so a worker that fails the same way again stays failed until the next start. A worker whose session is gone starts again in a new one, a lead with its latest hand-off note. A worker with an open question waits without a process until a human answers it, and then resumes with the answer as its first turn. A worker left `starting` ends `failed` and one left `stopping` ends `stopped`. A row that cannot be served is logged and the next row is served. Requested changes, conflicts, send-backs and reviews of new versions that the worker limits held back are then served. A start in a paused project does none of this: the resume of the pause runs the workers again, those that could not reach fold among them.
    pub async fn resume_after_restart(&self) -> Result<(), AgentError> {
        if self.is_paused().await? {
            self.inner.unreachable_owed.store(true, Ordering::SeqCst);
            return Ok(());
        }
        let mut conn = self
            .inner
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let rows = agents::list_worker_processes(&mut conn).await?;
        drop(conn);
        let fld = self.fld_present().await;
        let kept: BTreeSet<AgentName> = rows
            .iter()
            .filter(|row| {
                matches!(row.state, WorkerState::Working | WorkerState::Idle)
                    || (row.state == WorkerState::Failed && reaches_fold_again(row, fld))
            })
            .map(|row| row.agent.clone())
            .collect();
        for row in rows {
            if let Err(error) = self.restart_row(&row, fld, &kept).await {
                tracing::warn!(agent = %row.agent, code = code_of(&error), "could not serve a worker after the daemon start");
            }
        }
        tokio::spawn(self.clone().resume_pending());
        tokio::spawn(self.clone().resume_owed_send_backs());
        tokio::spawn(self.clone().resume_owed_gate_runs());
        tokio::spawn(self.clone().resume_owed_reviewers(None));
        tokio::spawn(self.clone().resume_owed_messages());
        tokio::spawn(self.clone().catch_up_main());
        Ok(())
    }

    /// What a daemon start does with one row; see [`Supervisor::resume_after_restart`]. `fld` tells whether the fld binary of the workers' MCP configuration is there, and `kept` names the workers the start runs again.
    async fn restart_row(
        &self,
        row: &WorkerProcessRow,
        fld: bool,
        kept: &BTreeSet<AgentName>,
    ) -> Result<(), AgentError> {
        match row.state {
            WorkerState::Working | WorkerState::Idle => {
                let turn = (row.state == WorkerState::Working).then(|| RESTART_TURN.to_owned());
                let turn = self.with_catch_up(row, turn, kept).await;
                self.relaunch(row, turn, None).await?;
            }
            WorkerState::Starting => self.fail(row.id, &row.agent, "daemon_restart").await?,
            WorkerState::Stopping => self.stopped_without_process(row).await?,
            WorkerState::Failed if reaches_fold_again(row, fld) => {
                let turn = self
                    .with_catch_up(row, Some(MCP_RETRY_TURN.to_owned()), kept)
                    .await;
                self.relaunch(row, turn, None).await?;
            }
            WorkerState::Stopped | WorkerState::Failed => {}
        }
        Ok(())
    }

    /// Runs a worker whose row kept its state without a process again in its own session with `--resume`, after a daemon start or a pause; a worker whose session is gone starts again in a new one, which also gets `lost` after its own note. A worker with an open question waits without a process until a human answers it.
    async fn relaunch(
        &self,
        row: &WorkerProcessRow,
        turn: Option<String>,
        lost: Option<String>,
    ) -> Result<(), AgentError> {
        if self.waits_for_answer(row).await? {
            return Ok(());
        }
        if row.role == Role::Main {
            let config = load_config(&self.inner.paths).await?;
            if self.main_refusal(&config).is_some() {
                return self.end_unallowed_main(row).await;
            }
        }
        let now = self.inner.clock.now();
        self.inner
            .store
            .write(async |tx| agents::restart_worker_process(tx, row.id, now).await)
            .await?;
        if let Some(watch) = self.launch(row, true, turn).await? {
            tokio::spawn(self.clone().restart_lost(row.agent.clone(), watch, lost));
        }
        Ok(())
    }

    pub(in crate::services::agents::workers) async fn is_paused(&self) -> Result<bool, StoreError> {
        Ok(agents::pause_state(self.inner.store.read())
            .await?
            .is_paused())
    }

    /// Ends a pause that began at `paused_at`: each worker that was working, idle or starting runs again in its own session with [`PAUSE_TURN`], followed by the first turn a starting worker never read, the answer to a question a human answered during the pause, the requested changes, conflicts and send-backs its task worker missed, and the work that waits for a lead; a worker left `stopping` ends `stopped`. When the daemon started during the pause, each worker that could not reach fold runs again with [`MCP_RETRY_TURN`], as that start would have resumed it. The resumes, send-backs, reviews of new versions and fresh starts of lost sessions that the pause held back are then served. A row that cannot be served is logged and the next row is served.
    pub(in crate::services::agents::workers) async fn resume_after_pause(
        &self,
        paused_at: fold_core::Timestamp,
    ) -> Result<(), AgentError> {
        let mut conn = self
            .inner
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let rows = agents::list_worker_processes(&mut conn).await?;
        drop(conn);
        let mut owed = self.owed_to_running().await;
        let mut sent_back = self.send_backs_owed_to_paused().await;
        let unreachable = self.inner.unreachable_owed.swap(false, Ordering::SeqCst);
        let fld = unreachable && self.fld_present().await;
        let kept: BTreeSet<AgentName> = rows
            .iter()
            .filter(|row| {
                matches!(
                    row.state,
                    WorkerState::Working | WorkerState::Idle | WorkerState::Starting
                ) || (row.state == WorkerState::Failed
                    && unreachable
                    && reaches_fold_again(row, fld))
            })
            .map(|row| row.agent.clone())
            .collect();
        for row in rows {
            if self.is_running(&row.agent) {
                continue;
            }
            let served = match row.state {
                WorkerState::Working | WorkerState::Idle | WorkerState::Starting => {
                    let owed_here = owed.remove(&row.agent);
                    let sent_back_here = sent_back.remove(&row.agent);
                    match self
                        .unpaused_turn(&row, paused_at, owed_here, sent_back_here, &kept)
                        .await
                    {
                        Ok((parts, first)) => {
                            self.relaunch(&row, Some(parts.join("\n\n")), first).await
                        }
                        Err(error) => Err(error),
                    }
                }
                WorkerState::Stopping => self.stopped_without_process(&row).await,
                WorkerState::Failed if unreachable && reaches_fold_again(&row, fld) => {
                    let turn = self
                        .with_catch_up(&row, Some(MCP_RETRY_TURN.to_owned()), &kept)
                        .await;
                    self.relaunch(&row, turn, None).await
                }
                WorkerState::Stopped | WorkerState::Failed => Ok(()),
            };
            if let Err(error) = served {
                tracing::warn!(agent = %row.agent, code = code_of(&error), "could not resume a worker after a pause");
            }
        }
        let lost: Vec<(AgentName, Option<String>)> = lock(&self.inner.held_lost).drain().collect();
        for (agent, instructions) in lost {
            tokio::spawn(self.clone().start_lost(agent, instructions));
        }
        tokio::spawn(self.clone().resume_pending());
        tokio::spawn(self.clone().resume_owed_send_backs());
        tokio::spawn(self.clone().resume_owed_gate_runs());
        tokio::spawn(self.clone().resume_owed_reviewers(None));
        tokio::spawn(self.clone().resume_owed_messages());
        tokio::spawn(self.clone().catch_up_main());
        Ok(())
    }

    /// The parts of the turn that ends a pause for `row`: [`PAUSE_TURN`], then the first turn of a `starting` worker, whose process never read it, the answer to its question that a human gave during the pause, which the worker's repeat of the call then takes, the work owed to it, and the work that waits for a lead, where `kept` names the workers the resume runs again. A running worker owes `owed` and `sent_back`, recorded since its process started; a `starting` worker owes what its bound proposals and reports still ask for, because the turn that told of it went unread. The first turn also comes back on its own, for a worker whose session turns out to be gone.
    async fn unpaused_turn(
        &self,
        row: &WorkerProcessRow,
        paused_at: fold_core::Timestamp,
        owed: Option<Vec<resume::Found>>,
        sent_back: Option<String>,
        kept: &BTreeSet<AgentName>,
    ) -> Result<(Vec<String>, Option<String>), AgentError> {
        let mut parts = vec![PAUSE_TURN.to_owned()];
        let first = if row.state == WorkerState::Starting {
            Some(self.rebuilt_first_turn(row).await?)
        } else {
            None
        };
        if let Some(first) = &first {
            parts.push(first.clone());
        }
        if let Some(question) = self.answered_during_pause(row, paused_at).await?
            && let Some(answer) = self.answer_turn(&question).await?
        {
            self.replay(&question);
            parts.push(answer);
        }
        let (owed, sent_back) = if row.state == WorkerState::Starting {
            let owed = self.owed_to_unstarted(row).await?;
            (
                (!owed.is_empty()).then_some(owed),
                self.send_backs_of_unstarted(row).await?,
            )
        } else {
            (owed, sent_back)
        };
        if let Some(found) = owed {
            parts.push(resume::notice_turn(&found));
        }
        parts.extend(sent_back);
        parts.extend(self.catch_up(row, kept).await);
        Ok((parts, first))
    }

    /// `turn` with the work that waits for a lead joined to it. A lead whose process starts again in its own session may have missed the notices of that work, because fold stores none and one sent while the lead had no process is lost. Joining it to the turn the start sends anyway gives the lead one turn, never two, and a lead with nothing waiting and no turn stays idle.
    async fn with_catch_up(
        &self,
        row: &WorkerProcessRow,
        turn: Option<String>,
        kept: &BTreeSet<AgentName>,
    ) -> Option<String> {
        let parts: Vec<String> = turn
            .into_iter()
            .chain(self.catch_up(row, kept).await)
            .collect();
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    }

    /// The work that waits for a lead, see [`catch_up_turn`]. A claim hides its task or proposal while its worker runs or is among `kept`, the workers the same start runs again; a review claim also hides its proposal while fold owes its reviewer that has ended a resume for the newest version (see [`agents::reviews_owed_to_ended_reviewers`]). A read that fails is logged and names nothing, so the lead still runs.
    pub(super) async fn catch_up(
        &self,
        row: &WorkerProcessRow,
        kept: &BTreeSet<AgentName>,
    ) -> Option<String> {
        match self.waiting_for(row, kept).await {
            Ok(text) => text,
            Err(error) => {
                tracing::warn!(agent = %row.agent, code = code_of(&error), "could not read the work that waits for a lead");
                None
            }
        }
    }

    async fn waiting_for(
        &self,
        row: &WorkerProcessRow,
        kept: &BTreeSet<AgentName>,
    ) -> Result<Option<String>, AgentError> {
        let holds = |worker: &WorkerName| {
            let agent = worker.agent();
            kept.contains(&agent) || self.is_running(&agent)
        };
        let read = self.inner.store.read();
        let (tasks, proposals) = match row.role {
            Role::AreaLead => {
                let Some(area) = &row.area else {
                    return Ok(None);
                };
                let Some(area) = agents::find_area(read, area).await? else {
                    return Ok(None);
                };
                (ready_in_area(read, &area.label, &holds).await?, Vec::new())
            }
            Role::Main => {
                let Some(area) = agents::find_area(read, &AreaName::main()).await? else {
                    return Ok(None);
                };
                let tasks = ready_in_area(read, &area.label, &holds).await?;
                let filter = requests::RequestFilter {
                    state: Some(RequestState::Open),
                    ..requests::RequestFilter::default()
                };
                let mut conn = read.acquire().await.map_err(StoreError::from)?;
                let mut waiting: Vec<RequestId> = requests::list(&mut conn, &filter)
                    .await?
                    .into_iter()
                    .filter(|request| request.drafter.is_none())
                    .map(|request| request.id)
                    .collect();
                waiting.reverse();
                let unread = messages::unread_count(&mut *conn, row.agent_id).await?;
                return Ok(main_catch_up_turn(&MainCatchUp {
                    tasks: &tasks,
                    requests: &waiting,
                    unread,
                }));
            }
            Role::ReviewLead => {
                let open = reviews::list_proposals(read, Some(ProposalState::Open), None).await?;
                let owed: BTreeSet<ProposalId> =
                    agents::reviews_owed_to_ended_reviewers(read, None)
                        .await?
                        .into_iter()
                        .map(|review| review.proposal)
                        .collect();
                let mut proposals = Vec::new();
                for proposal in open {
                    let waits = waits_for_review(&proposal)
                        && proposal
                            .review_claim
                            .as_ref()
                            .is_none_or(|holder| !holds(holder) && !owed.contains(&proposal.id));
                    // A human or the main agent decides a proposal of a task of Main, never the review lead.
                    if waits
                        && !tasks::labels_for(read, proposal.task)
                            .await?
                            .iter()
                            .any(|label| label.name.is_main())
                    {
                        proposals.push(proposal.id);
                    }
                }
                (Vec::new(), proposals)
            }
            Role::TaskWorker | Role::Reviewer => return Ok(None),
        };
        Ok(catch_up_turn(&CatchUpFacts {
            role: row.role,
            area: row.area.as_ref(),
            tasks: &tasks,
            proposals: &proposals,
        }))
    }

    /// The first turn of a start, from the row's role, area and bound tasks and proposals. fold never stores a starter's instructions, so this turn goes without them.
    async fn rebuilt_first_turn(&self, row: &WorkerProcessRow) -> Result<String, AgentError> {
        let label = match &row.area {
            Some(area) => agents::find_area(self.inner.store.read(), area)
                .await?
                .map(|area| area.label),
            None => None,
        };
        Ok(first_turn(&StartFacts {
            role: row.role,
            name: &row.agent,
            area: row.area.as_ref().zip(label.as_ref()),
            tasks: &row.tasks,
            proposals: &row.proposals,
            batch: reviewer_batch(&self.inner.store, row).await?,
            instructions: None,
        }))
    }
}

/// The ready tasks of the area of `label` in the ready order, with the Todo tasks whose claim a worker holds that `holds` says no longer runs.
async fn ready_in_area(
    read: &fold_store::SqlitePool,
    label: &fold_core::LabelName,
    holds: &impl Fn(&WorkerName) -> bool,
) -> Result<Vec<fold_core::TaskId>, StoreError> {
    let in_area = |labels: &[LabelRow]| labels.iter().any(|row| row.name == *label);
    let snapshot = tasks::ready_snapshot(read).await?;
    let held = tasks::held_todo(read).await?;
    let dag = Dag::from_edges(snapshot.edges);
    let candidates: Vec<Candidate> = snapshot
        .candidates
        .iter()
        .filter(|task| in_area(&task.labels))
        .map(|task| (task.id, task.created_at))
        .chain(
            held.iter()
                .filter(|task| in_area(&task.labels) && !holds(&task.holder))
                .map(|task| (task.id, task.created_at)),
        )
        .map(|(task, created_at)| Candidate {
            task,
            unblock_count: unblock_count(task, &dag, &snapshot.statuses),
            created_at,
        })
        .collect();
    Ok(ready_order(&candidates))
}

/// An open proposal whose current version has no verdict and nothing its author must change first.
fn waits_for_review(proposal: &ProposalRow) -> bool {
    proposal.latest_verdict.is_none() && !proposal.conflicted && !proposal.checks_failed
}

/// A failed worker that a start resumes: one whose fold MCP server did not connect, or whose fld binary was gone and is there again (`fld`).
fn reaches_fold_again(row: &WorkerProcessRow, fld: bool) -> bool {
    row.failed_reason
        .as_deref()
        .is_some_and(|reason| needs_restart(reason) && (reason != FLD_MISSING || fld))
}
