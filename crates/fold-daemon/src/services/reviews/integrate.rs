use std::collections::HashSet;

use fold_core::events::types;
use fold_core::merge::{Conflict, ReportTrees};
use fold_core::object::ObjectHash;
use fold_core::policy::{
    AgentStanding, CallerFacts, GateRefusal, IntegrateInput, ProposalFacts, can_integrate,
};
use fold_core::reviews::checks::{RunReason, Stage};
use fold_core::reviews::{Approver, ReviewFact, Verdict, latest_verdict};
use fold_core::revision::{Revision, StoredRevision};
use fold_core::{
    Actor, AddressedAs, AgentActor, ChangesetId, CheckRunId, EventActor, EventError, FoldError,
    HumanName, IdKind, IntegrationId, MessageBody, NewEvent, ProposalId, Recipient, RevisionId,
    Target, TaskId, Timestamp, WorkerName, WorkspaceId, codes, policy,
};
use fold_store::reviews::checks;
use fold_store::reviews::{
    self, IntegrationFacts, IntegrationSubject, NewIntegration, ProposalState, ReviewerRef,
};
use fold_store::tasks::claims::{self, ReleaseReason};
use fold_store::tasks::completions::{self, CompletionKind};
use fold_store::workspaces::{NewHumanChange, WorkspaceKind, WorkspaceRow, WorkspaceState};
use fold_store::{StoreError, WriteTx, agents, counters, tasks, workspaces};
use fold_vcs::integrate::MergeOutcome;
use serde_json::json;

use super::checks::{
    Integrating, IntegrationChecks, PassedRun, Verdict as RunVerdict, end_run_in_tx,
    fold_failed_refusal,
};
use super::conflicts::FoundConflict;
use super::notify;
use super::proposals::{Retirement, proposal_not_found, retire_proposal};
use super::views::integration_conflicted;
use super::{ReviewError, ReviewService};
use crate::services::events::EventBuffer;
#[cfg(feature = "failpoints")]
use crate::services::fired;
use crate::services::messages::send_from_fold_in_tx;
use crate::services::tasks::{TaskError, recompute};
use crate::services::workspaces::{
    ArchiveReason, WorkspaceError, archive_in_tx, main_agent, main_agent_in_tx,
};

/// How many times the procedure starts again from reading main when main moved under it.
const ATTEMPTS: usize = 3;

/// The longest remote commit id: a SHA-256 Git object id in hex.
const MAX_COMMIT_CHARS: usize = 64;

/// The body of the approval a human gives by integrating.
const INTEGRATING_APPROVAL: &str = "approved by integrating";

/// The longest Git commit message of a direct change, in characters.
const MAX_DIRECT_MESSAGE_CHARS: usize = 5000;

/// What `integration.succeeded` of a direct change names as its approval: the user's word in the main agent's conversation.
const DIRECT_APPROVAL: &str = "user_in_conversation";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrationOutcome {
    Succeeded {
        revision: RevisionId,
    },
    /// In path order. Nothing was published, no revision exists and the task is not completed.
    Conflicted(Vec<Conflict>),
    /// The integration checks of the merged tree failed: nothing was published, and the proposal went back to its author with a message from fold.
    ChecksFailed {
        run: CheckRunId,
    },
    /// The project has integration checks, so the integration waits its turn in the background, one at a time; fold sends the result as a message.
    Queued,
}

/// A clean `fld pull`: the newest commit of the remote branch with its tree, and the tree of the sync base, the newest commit fold main and the remote share, which is the merge base. Both trees are already in the object store. The base is a Git tree rather than a fold revision, because the revision that brought a commit in holds the merge with main, not the commit's own tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullIntegration {
    pub remote_commit: String,
    /// The name of the remote the pull fetched from.
    pub remote: String,
    pub branch: String,
    pub base_tree: ObjectHash,
    pub remote_tree: ObjectHash,
}

/// A clean change of the project folder by a human: what `fld record` read from the folder, or a Git commit the human made there. `base_tree` is the tree the change is based on and `tree` its own; both are already in the object store. The integration records the human change with `human_change.recorded` in its transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordIntegration {
    /// The full id of the human's Git commit; `None` for a change read from the folder.
    pub commit: Option<String>,
    pub base_tree: ObjectHash,
    pub tree: ObjectHash,
    pub human: HumanRecord,
}

/// A human change as its integration records it: the human whose change it is, and the data of its `human_change.recorded` event, which names the changed files, the open workspaces they touch and the leads of their tasks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanRecord {
    pub human: HumanName,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullOutcome {
    Succeeded {
        revision: RevisionId,
    },
    /// In path order. No row and no event was written.
    Conflicted(Vec<Conflict>),
}

/// A direct change of the main agent: a changeset of the main workspace, which must be the workspace's newest, and the Git commit message the main agent wrote for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectChange {
    pub changeset: ChangesetId,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectOutcome {
    Succeeded {
        revision: RevisionId,
    },
    /// In path order. No row and no event was written, and the main workspace stays open.
    Conflicted(Vec<Conflict>),
}

impl From<Integrated> for IntegrationOutcome {
    fn from(done: Integrated) -> Self {
        match done {
            Integrated::Succeeded(revision) => IntegrationOutcome::Succeeded { revision },
            Integrated::Conflicted(conflicts) => IntegrationOutcome::Conflicted(conflicts),
            Integrated::ChecksFailed(run) => IntegrationOutcome::ChecksFailed { run },
        }
    }
}

/// What one automatic integration did with a ready proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomaticIntegration {
    pub proposal: ProposalId,
    pub outcome: IntegrationOutcome,
}

#[derive(Clone, Copy)]
enum IntegrationRequest<'a> {
    Proposal {
        proposal: ProposalId,
        expected_version: Option<u32>,
        /// Set only by [`ReviewService::integrate_ready`], the one path on which `Actor::System` integrates.
        automatic: bool,
    },
    Pull(&'a PullIntegration),
    Direct(&'a DirectChange),
    Record(&'a RecordIntegration),
}

enum Integrated {
    Succeeded(RevisionId),
    Conflicted(Vec<Conflict>),
    ChecksFailed(CheckRunId),
}

/// Why an attempt stopped. `Retry` rolls the transaction back and starts again from reading main.
enum Step {
    Retry,
    Failed(ReviewError),
}

impl From<ReviewError> for Step {
    fn from(error: ReviewError) -> Self {
        Step::Failed(error)
    }
}

impl From<StoreError> for Step {
    fn from(error: StoreError) -> Self {
        Step::Failed(error.into())
    }
}

impl From<EventError> for Step {
    fn from(error: EventError) -> Self {
        Step::Failed(error.into())
    }
}

impl From<TaskError> for Step {
    fn from(error: TaskError) -> Self {
        Step::Failed(error.into())
    }
}

impl From<FoldError> for Step {
    fn from(error: FoldError) -> Self {
        Step::Failed(error.into())
    }
}

/// What one attempt merges and what its transaction records.
struct Plan {
    base_tree: ObjectHash,
    other_tree: ObjectHash,
    /// The task workspace or main workspace the transaction archives; its operation lock is held from the merge until the commit.
    workspace: Option<WorkspaceId>,
    subject: Subject,
}

enum Subject {
    Proposal {
        proposal: ProposalId,
        task: TaskId,
        version: u32,
        version_id: i64,
        expected_version: Option<u32>,
        automatic: bool,
    },
    Pull {
        remote_commit: String,
        remote: String,
        branch: String,
    },
    Record {
        commit: Option<String>,
        base_tree: ObjectHash,
        tree: ObjectHash,
        human: HumanRecord,
    },
    Direct {
        changeset: ChangesetId,
        workspace: WorkspaceId,
        message: String,
    },
}

/// The first refusal of the gates on `facts`, as `can_integrate` decides it.
pub(super) fn decide(
    facts: &IntegrationFacts,
    caller: &CallerFacts,
    expected_version: Option<u32>,
) -> Result<(), GateRefusal> {
    let proposal = ProposalFacts {
        open: facts.proposal.state == ProposalState::Open,
        author: facts.proposal.author.agent(),
        current_version: facts.target.version,
    };
    can_integrate(&IntegrateInput {
        caller,
        proposal: &proposal,
        expected_version,
        reviews: &facts.reviews,
        human_review: facts.human_review.as_ref(),
        prerequisites: &facts.prerequisites,
    })
}

impl ReviewService {
    /// Integrates the current version of an open proposal onto main, by an agent that decides reviews (the main agent, the review lead or a batch reviewer) and is not the author's agent, or by a named human, who approves in the same action. `expected_version` is the version the caller saw; another current version refuses with `stale_version`. A refusal by the open, version or caller gate, or of a human on a marked task whose version no reviewer agent approved (`agent_approval_required`), writes nothing; a refusal by the approval, human review or prerequisite gate records a `refused` integration and `integration.refused`, then returns the gate's error. A clean merge moves main by compare-and-swap, records the integration, completes the task, retires the proposal, archives its workspaces and releases its claims in one transaction. A conflicting merge publishes nothing: it records a `conflicted` integration with its report, flags the proposal, tells the author's worker and returns the task to In Progress, then returns the conflicts.
    pub async fn integrate_proposal(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        expected_version: Option<u32>,
    ) -> Result<IntegrationOutcome, ReviewError> {
        let request = IntegrationRequest::Proposal {
            proposal,
            expected_version,
            automatic: false,
        };
        if let Some(row) = reviews::get_proposal(self.store.read(), proposal).await?
            && row.state == ProposalState::Open
        {
            if row.checks_failed {
                return Err(checks_failed_refusal(proposal, row.current_version).into());
            }
            if row.integration_queued {
                return Err(already_queued_refusal(proposal, row.current_version).into());
            }
        }
        if self.integration_checks_may_run(proposal).await?
            && self.gates_pass(actor, proposal, expected_version).await?
        {
            self.queue_integration(actor, proposal, expected_version)
                .await?;
            return Ok(IntegrationOutcome::Queued);
        }
        Ok(self.integrate(actor, request).await?.into())
    }

    /// Whether the gates admit `actor` to integrate the current version now, read without recording anything. A refusal is reported by the procedure itself.
    async fn gates_pass(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        expected_version: Option<u32>,
    ) -> Result<bool, ReviewError> {
        let mut read = self.store.read().begin().await.map_err(StoreError::from)?;
        let standing = agents::standing_of(&mut *read, actor).await?;
        let Ok(caller) = caller_facts(actor, standing, false) else {
            return Ok(false);
        };
        let facts = reviews::integration_facts(&mut read, proposal).await?;
        read.commit().await.map_err(StoreError::from)?;
        Ok(facts.is_some_and(|facts| decide(&facts, &caller, expected_version).is_ok()))
    }

    /// Stores the request of the current version with `integration.queued`, then integrates in the background after the integrations ahead of it, in the order they were asked. A second request while one of the same version waits is refused at once. The stored request is what a daemon start serves again when a stop cut the queue short. A refusal or a conflict that the queue meets by then goes to the integrating agent as a message from fold, beside the conflict report the author gets; a success and failed checks send their own messages.
    async fn queue_integration(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        expected_version: Option<u32>,
    ) -> Result<(), ReviewError> {
        let now = self.clock.now();
        let label = actor.event_actor();
        let mut buffer = EventBuffer::new();
        let (id, version) = self
            .store
            .write(async |tx| {
                let row = reviews::get_proposal(tx.conn(), proposal)
                    .await?
                    .ok_or_else(|| proposal_not_found(proposal))?;
                let version = row.current_version;
                if checks::integration_queued(tx.conn(), proposal, version).await? {
                    return Err(already_queued_refusal(proposal, version).into());
                }
                let id = checks::insert_integration_request(
                    tx,
                    proposal,
                    version,
                    expected_version,
                    label.as_str(),
                    now,
                )
                .await?;
                let event = NewEvent::new(types::INTEGRATION_QUEUED, label.clone())?
                    .task(row.task)
                    .proposal(proposal)
                    .data(json!({"version": version}))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, ReviewError>((id, version))
            })
            .await?;
        self.bus.publish(buffer);
        self.serve_queued(id, actor.clone(), proposal, version, expected_version);
        Ok(())
    }

    /// Ends a stored request with `integration.unqueued`, so every screen that shows the queued integration lets it go.
    async fn end_request(
        &self,
        id: i64,
        proposal: ProposalId,
        version: u32,
    ) -> Result<(), ReviewError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                if !checks::end_integration_request(tx, id, now).await? {
                    return Ok(());
                }
                let Some(row) = reviews::get_proposal(tx.conn(), proposal).await? else {
                    return Ok(());
                };
                let event = NewEvent::new(types::INTEGRATION_UNQUEUED, EventActor::fold())?
                    .task(row.task)
                    .proposal(proposal)
                    .data(json!({"version": version}))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, ReviewError>(())
            })
            .await?;
        self.bus.publish(buffer);
        Ok(())
    }

    /// Serves one stored request in its turn and ends it.
    fn serve_queued(
        &self,
        id: i64,
        actor: Actor,
        proposal: ProposalId,
        version: u32,
        expected_version: Option<u32>,
    ) {
        let service = self.clone();
        let ticket = self.checks.ticket();
        self.checks.spawn(async move {
            ticket.turn().await;
            let request = IntegrationRequest::Proposal {
                proposal,
                expected_version,
                automatic: false,
            };
            let untold = match service.integrate(&actor, request).await {
                Ok(Integrated::Conflicted(conflicts)) => {
                    Some(ReviewError::from(integration_conflicted(proposal, &conflicts)))
                }
                Ok(_) => None,
                Err(error) => {
                    tracing::info!(proposal = %proposal, code = error.log_code(), "a queued integration was refused");
                    Some(error)
                }
            };
            if let Some(error) = untold
                && let Err(failed) = service.tell_queued_refusal(&actor, proposal, &error).await
            {
                tracing::warn!(proposal = %proposal, code = failed.log_code(), "the refusal of a queued integration could not be told");
            }
            if let Err(error) = service.end_request(id, proposal, version).await {
                tracing::warn!(proposal = %proposal, code = error.log_code(), "a queued integration could not end; the next daemon start serves it again");
            }
            drop(ticket);
        });
    }

    /// Queues again, in their order, the integrations a stopped daemon left waiting or cut short. Each is served as a new turn: its gates are checked when the turn comes, so one whose proposal moved on meanwhile is refused and its agent told. Returns how many were queued.
    pub async fn requeue_integrations(&self) -> Result<usize, ReviewError> {
        let waiting = checks::waiting_integration_requests(self.store.read()).await?;
        let mut queued = 0;
        for request in waiting {
            let Some(actor) = queued_actor(&request.actor) else {
                self.end_request(request.id, request.proposal, request.version)
                    .await?;
                continue;
            };
            self.serve_queued(
                request.id,
                actor,
                request.proposal,
                request.version,
                request.expected_version,
            );
            queued += 1;
        }
        if queued > 0 {
            tracing::info!(
                queued,
                "integrations a stop left in the queue are queued again"
            );
        }
        Ok(queued)
    }

    /// Tells the agent that queued an integration why it did not happen.
    async fn tell_queued_refusal(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        error: &ReviewError,
    ) -> Result<(), ReviewError> {
        let Actor::Agent(agent) = actor else {
            return Ok(());
        };
        let ReviewError::Fold(refusal) = error else {
            return Ok(());
        };
        let Some(row) = reviews::get_proposal(self.store.read(), proposal).await? else {
            return Ok(());
        };
        let body = MessageBody::parse(format!(
            "fold did not integrate {proposal}: {}. {}",
            refusal.message(),
            refusal.hint().unwrap_or_default()
        ))?;
        let to = Recipient {
            target: Target::Worker(agent.worker.clone()),
            addressed_as: AddressedAs::Worker,
        };
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                send_from_fold_in_tx(
                    tx,
                    &mut buffer,
                    vec![to],
                    (row.task, Some(proposal)),
                    &body,
                    now,
                )
                .await
                .map_err(ReviewError::from)
            })
            .await?;
        self.bus.publish(buffer);
        Ok(())
    }

    /// Integrates the newest commit of the Git remote onto main, by the named human who ran `fld pull`; an agent is refused with `human_required`. There is no proposal, task or review, so only the merge decides. A clean merge moves main and records a `pull` integration with the remote commit and `remote.pulled`; a conflicting one writes nothing and returns the conflicts to the caller.
    pub async fn integrate_pull(
        &self,
        actor: &Actor,
        pull: PullIntegration,
    ) -> Result<PullOutcome, ReviewError> {
        Ok(
            match self
                .integrate(actor, IntegrationRequest::Pull(&pull))
                .await?
            {
                Integrated::Succeeded(revision) => PullOutcome::Succeeded { revision },
                Integrated::Conflicted(conflicts) => PullOutcome::Conflicted(conflicts),
                Integrated::ChecksFailed(_) => return Err(not_a_proposal()),
            },
        )
    }

    /// Integrates a change of the project folder by the named human whose change it is: what `fld record` read from the folder, or a Git commit the human made there. There is no proposal, task or review, so only the merge decides, and no check runs: the integration checks run only when a proposal integrates. A clean merge moves main and records a `record` integration with the human change and `human_change.recorded`; a conflicting one writes nothing and returns the conflicts to the caller.
    pub async fn integrate_record(
        &self,
        actor: &Actor,
        record: RecordIntegration,
    ) -> Result<PullOutcome, ReviewError> {
        Ok(
            match self
                .integrate(actor, IntegrationRequest::Record(&record))
                .await?
            {
                Integrated::Succeeded(revision) => PullOutcome::Succeeded { revision },
                Integrated::Conflicted(conflicts) => PullOutcome::Conflicted(conflicts),
                Integrated::ChecksFailed(_) => return Err(not_a_proposal()),
            },
        )
    }

    /// Integrates a direct change of the main agent: the newest changeset of its main workspace, with the Git commit message it wrote. The user approved the change in the main agent's conversation, so no review gate and no check runs (the integration checks run only when a proposal integrates); `policy::can_change_main_directly` admits the main agent only, and the main workspace must be active and owned by its worker. A clean merge moves main by compare-and-swap, records a `direct` integration with the changeset and the message, and archives the main workspace, in one transaction. A conflicting merge writes nothing and returns the conflicts: the main agent syncs the workspace, records again and submits the new changeset.
    pub async fn integrate_direct(
        &self,
        actor: &Actor,
        change: DirectChange,
    ) -> Result<DirectOutcome, ReviewError> {
        Ok(
            match self
                .integrate(actor, IntegrationRequest::Direct(&change))
                .await?
            {
                Integrated::Succeeded(revision) => DirectOutcome::Succeeded { revision },
                Integrated::Conflicted(conflicts) => DirectOutcome::Conflicted(conflicts),
                Integrated::ChecksFailed(_) => return Err(not_a_proposal()),
            },
        )
    }

    /// Integrates, as fold, every open proposal whose current version is ready: an agent that decides reviews or a human approved it, the latest verdict asks for no changes, a marked task has a human approval the mark accepts, and every prerequisite is Done. Each integration is the same procedure a deciding agent runs. The open proposals are looked at in number order and looked at again after each integration, so a prerequisite's integration lets the proposals that need it follow in the same call. A proposal flagged with a conflict waits for its next version, which clears the flag. A failed attempt is logged by its code and left for the next call. Returns the integrations in the order they ran; a conflicted one recorded `integration.conflicted` and left its proposal open.
    pub async fn integrate_ready(&self) -> Result<Vec<AutomaticIntegration>, ReviewError> {
        let mut done = Vec::new();
        let mut passed = HashSet::new();
        while let Some((proposal, version)) = self.next_ready(&passed).await? {
            passed.insert(proposal);
            let request = IntegrationRequest::Proposal {
                proposal,
                expected_version: Some(version),
                automatic: true,
            };
            let outcome = match self.integrate(&Actor::System, request).await {
                Ok(done) => done.into(),
                Err(error) => {
                    tracing::warn!(proposal = %proposal, code = error.code().unwrap_or(codes::INTERNAL_ERROR), "the automatic integration of a proposal failed; the next trigger tries again");
                    continue;
                }
            };
            done.push(AutomaticIntegration { proposal, outcome });
        }
        Ok(done)
    }

    /// The lowest-numbered open proposal outside `passed` that is not flagged with a conflict or failed integration checks, whose integration checks fold did not fail to run on this version and main, and that passes every gate for the automatic caller, with its current version.
    async fn next_ready(
        &self,
        passed: &HashSet<ProposalId>,
    ) -> Result<Option<(ProposalId, u32)>, ReviewError> {
        let head = workspaces::head(self.store.read())
            .await?
            .ok_or(StoreError::Corrupt("refs.main"))?
            .number;
        let mut read = self.store.read().begin().await.map_err(StoreError::from)?;
        let open = reviews::list_proposals(&mut *read, Some(ProposalState::Open), None).await?;
        let mut ready = None;
        for row in open {
            // No version passes the automatic gates without an approval as its latest verdict, so only those proposals read their gate facts.
            if row.conflicted
                || row.checks_failed
                || passed.contains(&row.id)
                || row.latest_verdict != Some(Verdict::Approve)
            {
                continue;
            }
            // fold's own failure to run the integration checks is not retried on the same version and main: only a caller's integrate, or an automatic run after main moved, tries again.
            let newest = checks::runs_of_version(&mut *read, row.task, row.id, row.current_version)
                .await?
                .into_iter()
                .find(|run| run.stage == Stage::Integration);
            if newest
                .is_some_and(|run| run.reason == Some(RunReason::FoldFailed) && run.base == head)
            {
                continue;
            }
            let Some(facts) = reviews::integration_facts(&mut read, row.id).await? else {
                continue;
            };
            if decide(&facts, &CallerFacts::Automatic, None).is_ok() {
                ready = Some((row.id, facts.target.version));
                break;
            }
        }
        read.commit().await.map_err(StoreError::from)?;
        Ok(ready)
    }

    /// The one procedure behind both kinds. The project's integration mutex is held throughout, so two integrations never run at the same time; an attempt that finds main moved starts again, three times at most.
    async fn integrate(
        &self,
        actor: &Actor,
        request: IntegrationRequest<'_>,
    ) -> Result<Integrated, ReviewError> {
        let _serial = self.integration.lock().await;
        let mut carried = None;
        for _ in 0..ATTEMPTS {
            let attempt = self.attempt(actor, request, &mut carried).await;
            if !matches!(attempt, Err(Step::Retry)) {
                self.end_carried(&mut carried).await?;
            }
            match attempt {
                Ok(done) => return Ok(done),
                Err(Step::Retry) => continue,
                Err(Step::Failed(error)) => return Err(error),
            }
        }
        self.end_carried(&mut carried).await?;
        Err(FoldError::new(
            codes::HEAD_MOVED,
            format!("main moved during each of {ATTEMPTS} integration attempts"),
        )
        .with_hint("call integrate_proposal or submit_main_change again, or run fld pull again")
        .into())
    }

    async fn attempt(
        &self,
        actor: &Actor,
        request: IntegrationRequest<'_>,
        carried: &mut Option<PassedRun>,
    ) -> Result<Integrated, Step> {
        let head = workspaces::head(self.store.read())
            .await?
            .ok_or(StoreError::Corrupt("refs.main"))?;
        let plan = match request {
            IntegrationRequest::Proposal {
                proposal,
                expected_version,
                automatic,
            } => {
                self.proposal_plan(actor, proposal, expected_version, automatic)
                    .await?
            }
            IntegrationRequest::Pull(pull) => self.pull_plan(actor, pull).await?,
            IntegrationRequest::Direct(change) => self.direct_plan(actor, change).await?,
            IntegrationRequest::Record(record) => record_plan(actor, record)?,
        };
        let locks = self.workspaces.lock_for_integration(plan.workspace).await;
        let merged = self
            .workspaces
            .merge_for_integration(&locks, plan.base_tree, head.tree_hash, plan.other_tree)
            .await
            .map_err(ReviewError::from)?;
        let tree = match merged {
            MergeOutcome::Merged { tree_hash } => tree_hash,
            MergeOutcome::Conflicted(conflicts) => {
                if let Subject::Proposal {
                    proposal,
                    task,
                    version,
                    expected_version,
                    automatic,
                    ..
                } = plan.subject
                {
                    let found = FoundConflict {
                        proposal,
                        task,
                        version,
                        main: head.number,
                        trees: ReportTrees {
                            base: plan.base_tree,
                            main: head.tree_hash,
                            version: plan.other_tree,
                        },
                        conflicts: &conflicts,
                        expected_version,
                        automatic,
                    };
                    #[cfg(feature = "failpoints")]
                    fail::fail_point!("integration_before_conflict_record");
                    if !self.record_conflict(actor, found).await? {
                        return Err(Step::Retry);
                    }
                }
                return Ok(Integrated::Conflicted(conflicts));
            }
        };
        if let Subject::Proposal {
            proposal,
            task,
            version,
            ..
        } = plan.subject
        {
            let at = Integrating {
                proposal,
                task,
                version,
                head: head.number,
                head_tree: head.tree_hash,
                merged: tree,
            };
            match self.integration_checks(actor, at, carried).await? {
                IntegrationChecks::None => {}
                IntegrationChecks::Passed => {}
                IntegrationChecks::Failed(run) => return Ok(Integrated::ChecksFailed(run)),
                IntegrationChecks::FoldFailed(run) => {
                    return Err(Step::Failed(fold_failed_refusal(at, run).into()));
                }
            }
        }
        #[cfg(feature = "failpoints")]
        fail::fail_point!("integration_after_merge", |_| Err(Step::from(fired(
            "integration_after_merge"
        ))));
        // Allocated in its own write, so a failed attempt never gives its number to a later one.
        let number = self
            .store
            .write(async |tx| counters::allocate(tx, IdKind::Integration).await)
            .await?;
        let integration =
            IntegrationId::from_number(number).ok_or(StoreError::Corrupt("counters.next"))?;
        let next = RevisionId::from_number(head.number.number().saturating_add(1))
            .ok_or(StoreError::Corrupt("revisions.number"))?;
        let now = self.clock.now();
        let revision = Revision {
            tree,
            parent: Some(head.hash),
            number: next,
            integration: Some(integration),
            created_at: now,
        };
        let stored = self
            .workspaces
            .write_integration_revision(&locks, revision)
            .await
            .map_err(ReviewError::from)?;
        #[cfg(feature = "failpoints")]
        fail::fail_point!("integration_after_objects", |_| Err(Step::from(fired(
            "integration_after_objects"
        ))));
        let human = self.human_resolution(&plan, tree).await?;
        let mut buffer = EventBuffer::new();
        let commit = Commit {
            actor,
            plan: &plan,
            head: head.number,
            stored: &stored,
            integration,
            now,
            human: human.as_ref(),
            checks: carried.as_ref(),
        };
        let archived = self
            .store
            .write(async |tx| commit.run(tx, &mut buffer).await)
            .await?;
        #[cfg(feature = "failpoints")]
        fail::fail_point!("integration_after_commit", |_| Err(Step::from(fired(
            "integration_after_commit"
        ))));
        drop(locks);
        self.bus.publish(buffer);
        for workspace in archived {
            self.workspaces.detach_live(workspace).await;
        }
        self.follow_main_in_check_dir(tree);
        match &plan.subject {
            Subject::Proposal { proposal, .. } => {
                tracing::info!(proposal = %proposal, revision = %next, integration = %integration, "proposal integrated");
            }
            Subject::Pull { remote_commit, .. } => {
                tracing::info!(remote_commit = %remote_commit, revision = %next, integration = %integration, "remote commit integrated");
            }
            Subject::Record { commit, .. } => {
                tracing::info!(commit = commit.as_deref().unwrap_or("none"), revision = %next, integration = %integration, "human change integrated");
            }
            Subject::Direct { changeset, .. } => {
                tracing::info!(changeset = %changeset, revision = %next, integration = %integration, "direct change integrated");
            }
        }
        Ok(Integrated::Succeeded(next))
    }

    /// Step 2 of a proposal: the gates on stored facts, with the refusals of the review gates recorded. An automatic integration records no refusal: it runs only for a proposal it found ready, and one that stopped being ready in between waits for its next trigger.
    async fn proposal_plan(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        expected_version: Option<u32>,
        automatic: bool,
    ) -> Result<Plan, Step> {
        let mut read = self.store.read().begin().await.map_err(StoreError::from)?;
        let standing = agents::standing_of(&mut *read, actor).await?;
        let caller = caller_facts(actor, standing, automatic)?;
        let facts = reviews::integration_facts(&mut read, proposal)
            .await?
            .ok_or_else(|| proposal_not_found(proposal))?;
        let checks_failed = reviews::get_proposal(&mut *read, proposal)
            .await?
            .is_some_and(|row| row.checks_failed);
        read.commit().await.map_err(StoreError::from)?;
        if checks_failed && facts.proposal.state == ProposalState::Open {
            return Err(checks_failed_refusal(proposal, facts.target.version).into());
        }
        if let Err(refusal) = decide(&facts, &caller, expected_version) {
            if automatic {
                return Err(FoldError::from(refusal).into());
            }
            return Err(match refusal {
                GateRefusal::ChangesRequested
                | GateRefusal::NotApproved
                | GateRefusal::HumanApprovalMissing { .. }
                | GateRefusal::PrerequisitesNotDone { .. } => {
                    self.record_refusal(actor, &facts, refusal).await
                }
                GateRefusal::ProposalNotOpen
                | GateRefusal::StaleVersion { .. }
                | GateRefusal::IntegratorRequired
                | GateRefusal::AuthorCannotIntegrate
                | GateRefusal::AgentApprovalRequired => FoldError::from(refusal).into(),
            });
        }
        Ok(Plan {
            base_tree: facts.target.base_tree,
            other_tree: facts.target.tree,
            workspace: facts.live_workspace,
            subject: Subject::Proposal {
                proposal,
                task: facts.proposal.task,
                version: facts.target.version,
                version_id: facts.target.version_id,
                expected_version,
                automatic,
            },
        })
    }

    /// Records a refusal of a review gate as a `refused` integration with its report and `integration.refused`, and returns the gate's error to send once that is committed.
    async fn record_refusal(
        &self,
        actor: &Actor,
        facts: &IntegrationFacts,
        refusal: GateRefusal,
    ) -> Step {
        let error = FoldError::from(refusal);
        let (proposal, task, version) =
            (facts.proposal.id, facts.proposal.task, facts.target.version);
        let report = json!({
            "code": error.code(),
            "proposal": proposal.to_string(),
            "task": task.to_string(),
            "version": version,
            "details": error.details(),
        })
        .to_string();
        let label = actor.event_actor();
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let recorded = self
            .store
            .write(async |tx| {
                let number = counters::allocate(tx, IdKind::Integration).await?;
                let integration = IntegrationId::from_number(number)
                    .ok_or(StoreError::Corrupt("counters.next"))?;
                let new = NewIntegration {
                    number: integration,
                    subject: IntegrationSubject::Proposal {
                        proposal,
                        version,
                        remote_commit: None,
                    },
                    actor: label.as_str(),
                    created_at: now,
                };
                reviews::insert_refused(tx, &new, &report).await?;
                let event = NewEvent::new(types::INTEGRATION_REFUSED, label.clone())?
                    .task(task)
                    .proposal(proposal)
                    .data(json!({
                        "integration": integration.to_string(),
                        "code": error.code(),
                        "version": version,
                    }))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, ReviewError>(())
            })
            .await;
        match recorded {
            Ok(()) => {
                self.bus.publish(buffer);
                Step::Failed(error.into())
            }
            Err(failed) => Step::Failed(failed),
        }
    }

    /// Step 2 of a pull: only a named human brings remote commits in, and the merge base is the tree of the sync base.
    async fn pull_plan(&self, actor: &Actor, pull: &PullIntegration) -> Result<Plan, Step> {
        if !actor.is_human() {
            return Err(FoldError::new(
                codes::HUMAN_REQUIRED,
                "only the human who runs fld pull integrates remote commits",
            )
            .with_hint("ask a human to run fld pull")
            .into());
        }
        let commit = pull.remote_commit.as_str();
        if commit.is_empty()
            || commit.len() > MAX_COMMIT_CHARS
            || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                "the remote commit must be a Git object id in hex",
            )
            .with_details([("field", json!("remote_commit"))])
            .into());
        }
        Ok(Plan {
            base_tree: pull.base_tree,
            other_tree: pull.remote_tree,
            workspace: None,
            subject: Subject::Pull {
                remote_commit: commit.to_owned(),
                remote: pull.remote.clone(),
                branch: pull.branch.clone(),
            },
        })
    }
}

/// Step 2 of a record: only a named human records a change of the project folder, and the merge base is the tree the change is based on.
fn record_plan(actor: &Actor, record: &RecordIntegration) -> Result<Plan, Step> {
    if !actor.is_human() {
        return Err(FoldError::new(
            codes::HUMAN_REQUIRED,
            "only a human records a change of the project folder",
        )
        .with_hint("ask a human to run fld record")
        .into());
    }
    if let Some(commit) = &record.commit
        && !is_commit_id(commit)
    {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            "the human's commit must be a Git object id in hex",
        )
        .with_details([("field", json!("commit"))])
        .into());
    }
    Ok(Plan {
        base_tree: record.base_tree,
        other_tree: record.tree,
        workspace: None,
        subject: Subject::Record {
            commit: record.commit.clone(),
            base_tree: record.base_tree,
            tree: record.tree,
            human: record.human.clone(),
        },
    })
}

/// The caller of a stored queue request: a named human or an agent's worker. Its rights are read again from the database when its turn comes, so no tag is carried.
fn queued_actor(label: &str) -> Option<Actor> {
    let actor = EventActor::parse(label).ok()?;
    if let Some(name) = actor.human_name() {
        return HumanName::parse(name).ok().map(Actor::human);
    }
    let worker = WorkerName::parse(actor.as_str()).ok()?;
    Some(Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: Vec::new(),
    }))
}

/// The refusal of an approval or an integration of a version whose integration checks failed: only a new version from its author can be approved and integrated.
pub(super) fn checks_failed_refusal(proposal: ProposalId, version: u32) -> FoldError {
    FoldError::new(
        codes::INVALID_REQUEST,
        format!("the integration checks of {proposal} v{version} failed"),
    )
    .with_details([
        ("reason", json!("checks_failed")),
        ("proposal", json!(proposal.to_string())),
        ("version", json!(version)),
    ])
    .with_hint("the author sends a new version with update_proposal; call get_proposal to read the failed check run")
}

/// The refusal of a second request to integrate a version whose integration waits in the queue already.
fn already_queued_refusal(proposal: ProposalId, version: u32) -> FoldError {
    FoldError::new(
        codes::INVALID_REQUEST,
        format!("an integration of {proposal} v{version} is already queued"),
    )
    .with_details([
        ("reason", json!("integration_queued")),
        ("proposal", json!(proposal.to_string())),
        ("version", json!(version)),
    ])
    .with_hint("fold integrates it in its turn and sends the result as a message; get_proposal shows integration_queued until then")
}

/// Only a proposal's integration runs checks, so a pull, a record or a direct change never ends with failed checks.
fn not_a_proposal() -> ReviewError {
    StoreError::Invalid("only the integration of a proposal runs checks").into()
}

/// A full Git object id in lower-case hex, SHA-1 or SHA-256.
fn is_commit_id(commit: &str) -> bool {
    matches!(commit.len(), 40 | MAX_COMMIT_CHARS)
        && commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl ReviewService {
    /// Step 2 of a direct change: the main agent, its own active main workspace and that workspace's newest changeset, which must change something. The merge base is the tree of the changeset's base revision.
    async fn direct_plan(&self, actor: &Actor, change: &DirectChange) -> Result<Plan, Step> {
        main_agent(&self.store, actor)
            .await
            .map_err(ReviewError::from)?;
        let message = change.message.trim();
        if message.is_empty() || message.chars().count() > MAX_DIRECT_MESSAGE_CHARS {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("the commit message must be 1 to {MAX_DIRECT_MESSAGE_CHARS} characters"),
            )
            .with_details([("field", json!("message"))])
            .with_hint("call submit_main_change again with the Git commit message the user saw")
            .into());
        }
        let read = self.store.read();
        let changeset = workspaces::get_changeset(read, change.changeset)
            .await?
            .ok_or_else(|| {
                FoldError::new(
                    codes::NOT_FOUND,
                    format!("changeset {} does not exist", change.changeset),
                )
                .with_details([("changeset", json!(change.changeset))])
                .with_hint("call record_changes in the main workspace first")
            })?;
        let row = workspaces::get_workspace(read, changeset.workspace)
            .await?
            .ok_or(StoreError::Corrupt("changesets.workspace_id"))?;
        let latest = workspaces::latest_changeset(read, row.id)
            .await?
            .map(|newest| newest.id);
        check_direct(actor, &row, changeset.id, latest)?;
        let mut conn = read.acquire().await.map_err(StoreError::from)?;
        let held = workspaces::human_change_of_workspace(&mut conn, row.id).await?;
        drop(conn);
        if let Some(stale) = superseded(row.id, held.as_ref()) {
            return Err(stale.into());
        }
        let base_tree = workspaces::get_by_number(read, changeset.base_revision)
            .await?
            .tree_hash;
        if changeset.tree == base_tree {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("{} changes nothing against {}", changeset.id, changeset.base_revision),
            )
            .with_details([("reason", json!("no_changes"))])
            .with_hint("change files in the main workspace and call record_changes, or call abandon_workspace")
            .into());
        }
        Ok(Plan {
            base_tree,
            other_tree: changeset.tree,
            workspace: Some(row.id),
            subject: Subject::Direct {
                changeset: changeset.id,
                workspace: row.id,
                message: message.to_owned(),
            },
        })
    }
}

impl ReviewService {
    /// The conflicted human change an integration onto `tree` brings onto main, when one waits: the change whose merge the main workspace of a direct change holds, or one whose changes `tree` holds already. A record of the human closes the waiting change itself.
    async fn human_resolution(
        &self,
        plan: &Plan,
        tree: ObjectHash,
    ) -> Result<Option<HumanResolution>, Step> {
        if matches!(plan.subject, Subject::Record { .. }) {
            return Ok(None);
        }
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let waiting = workspaces::conflicted_human_change(&mut conn).await?;
        drop(conn);
        let Some(change) = waiting else {
            return Ok(None);
        };
        let held = matches!(
            (&plan.subject, change.workspace),
            (Subject::Direct { workspace, .. }, Some(held)) if held == *workspace
        );
        if !held
            && !self
                .workspaces
                .holds_human_change(change.base_tree, change.tree, tree)
                .await
                .map_err(ReviewError::from)?
        {
            return Ok(None);
        }
        let reach = self
            .workspaces
            .human_change_reach(change.base_tree, change.tree)
            .await
            .map_err(ReviewError::from)?;
        let mut data = reach.event_data(change.commit.as_deref());
        data["human"] = json!(format!("human:{}", change.human));
        Ok(Some(HumanResolution {
            id: change.id,
            data,
        }))
    }
}

/// A main workspace that holds the merge of a human change that no longer waits: the human replaced it with a newer one, so the main agent prepares the newer one, or another integration brought its changes onto main, so nothing is left to merge.
fn superseded(
    workspace: WorkspaceId,
    held: Option<&fold_store::workspaces::HumanChangeRow>,
) -> Option<FoldError> {
    use fold_store::workspaces::HumanChangeState;
    let held = held?;
    if held.state == HumanChangeState::Resolved {
        return Some(
            FoldError::new(
                codes::INVALID_REQUEST,
                format!("{workspace} holds the merge of a human change that main holds already"),
            )
            .with_details([
                ("reason", json!("human_change_resolved")),
                ("commit", json!(held.commit)),
            ])
            .with_hint("call abandon_workspace on it; main has the human's changes"),
        );
    }
    if held.state != HumanChangeState::Superseded {
        return None;
    }
    Some(
        FoldError::new(
            codes::INVALID_REQUEST,
            format!(
                "{workspace} holds the merge of an older human change; the human recorded a newer one"
            ),
        )
        .with_details([
            ("reason", json!("human_change_superseded")),
            ("commit", json!(held.commit)),
        ])
        .with_hint("call abandon_workspace on it, then open_main_workspace to prepare the newer change"),
    )
}

/// The checks of a direct change on one read: `row` is the workspace of `changeset`, which must be the main workspace, owned by the main agent's worker and active, and `latest` is its newest changeset, which must be `changeset`, so the change the user saw is the change that reaches main.
fn check_direct(
    actor: &Actor,
    row: &WorkspaceRow,
    changeset: ChangesetId,
    latest: Option<ChangesetId>,
) -> Result<(), FoldError> {
    if row.kind != WorkspaceKind::Main {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!(
                "{changeset} belongs to {}, which is not the main workspace",
                row.id
            ),
        )
        .with_details([
            ("reason", json!("not_main_workspace")),
            ("workspace", json!(row.id.to_string())),
        ])
        .with_hint("task work reaches main through a proposal: call create_proposal"));
    }
    if let Err(denied) = policy::can_change_workspace(actor, row.worker.as_ref()) {
        return Err(FoldError::from(denied).with_details([
            ("workspace", json!(row.id.to_string())),
            (
                "owner",
                json!(row.worker.as_ref().map(|worker| worker.as_str())),
            ),
        ]));
    }
    if row.state != WorkspaceState::Active {
        return Err(FoldError::new(
            codes::WORKSPACE_NOT_ACTIVE,
            format!("{} is {}", row.id, row.state.as_str()),
        )
        .with_details([
            ("workspace", json!(row.id.to_string())),
            ("state", json!(row.state.as_str())),
        ])
        .with_hint("call open_main_workspace to start a new change"));
    }
    if latest != Some(changeset) {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!("{changeset} is not the newest changeset of {}", row.id),
        )
        .with_details([
            ("reason", json!("stale_changeset")),
            ("latest", json!(latest.map(|id| id.to_string()))),
        ])
        .with_hint("show the user the diff of the newest changeset, then submit that one"));
    }
    Ok(())
}

/// The caller as the gates see it, with the acting agent's `standing`. The daemon integrates only through the automatic integration, so `Actor::System` is refused on every other path, and a proposal view reads the gates for an agent or a named human only.
pub(super) fn caller_facts(
    actor: &Actor,
    standing: AgentStanding,
    automatic: bool,
) -> Result<CallerFacts, FoldError> {
    match actor {
        Actor::Agent(agent) => Ok(CallerFacts::Agent {
            agent: agent.agent.clone(),
            standing,
        }),
        Actor::Human { name } => Ok(CallerFacts::Human(name.clone())),
        Actor::System if automatic => Ok(CallerFacts::Automatic),
        Actor::System => Err(FoldError::from(GateRefusal::IntegratorRequired)),
    }
}

/// The inputs of the one write transaction of an attempt.
struct Commit<'a> {
    actor: &'a Actor,
    plan: &'a Plan,
    /// The head the attempt read and merged onto.
    head: RevisionId,
    stored: &'a StoredRevision,
    integration: IntegrationId,
    now: Timestamp,
    /// The conflicted human change the integration brings onto main.
    human: Option<&'a HumanResolution>,
    /// The passed integration checks of the merged tree, ended in this transaction.
    checks: Option<&'a PassedRun>,
}

/// A conflicted human change that an integration brings onto main, with the data of its `human_change.resolved` event: the human, and the files, open workspaces and leads it touches.
struct HumanResolution {
    /// The row id of the waiting change.
    id: i64,
    data: serde_json::Value,
}

impl Commit<'_> {
    /// Step 6. Returns the workspaces it archived, whose live state stops after the commit.
    async fn run(
        &self,
        tx: &mut WriteTx,
        buffer: &mut EventBuffer,
    ) -> Result<Vec<WorkspaceId>, Step> {
        if workspaces::main_head(tx.conn()).await? != Some(self.head) {
            return Err(Step::Retry);
        }
        let label = self.actor.event_actor();
        let revision = self.stored.revision().number;
        let archived = match &self.plan.subject {
            Subject::Proposal {
                proposal,
                task,
                version,
                version_id,
                expected_version,
                automatic,
            } => {
                let facts = self
                    .recheck(tx, *proposal, *version_id, *expected_version, *automatic)
                    .await?;
                if let Some(passed) = self.checks
                    && !end_run_in_tx(
                        tx,
                        buffer,
                        &passed.run,
                        &passed.results,
                        RunVerdict::passed(),
                        self.now,
                    )
                    .await?
                {
                    return Err(Step::Retry);
                }
                // A sync task's integration marks the remote commit it resolves as merged, for the sync base walk and the Git writers.
                let remote_commit = tasks::remote_commit(tx.conn(), *task).await?;
                let subject = IntegrationSubject::Proposal {
                    proposal: *proposal,
                    version: *version,
                    remote_commit: remote_commit.as_deref(),
                };
                let succeeded = NewEvent::new(types::INTEGRATION_SUCCEEDED, label.clone())?
                    .task(*task)
                    .proposal(*proposal)
                    .data(json!({
                        "integration": self.integration.to_string(),
                        "kind": "proposal",
                        "version": version,
                    }))?;
                self.advance(tx, buffer, &label, subject, succeeded).await?;
                self.complete(tx, buffer, &label, facts).await?
            }
            Subject::Pull {
                remote_commit,
                remote,
                branch,
            } => {
                let subject = IntegrationSubject::Pull {
                    remote_commit: remote_commit.as_str(),
                };
                let succeeded =
                    NewEvent::new(types::INTEGRATION_SUCCEEDED, label.clone())?.data(json!({
                        "integration": self.integration.to_string(),
                        "kind": "pull",
                        "remote_commit": remote_commit,
                    }))?;
                self.advance(tx, buffer, &label, subject, succeeded).await?;
                let pulled = NewEvent::new(types::REMOTE_PULLED, label.clone())?
                    .revision(revision)
                    .data(json!({
                        "remote": remote,
                        "branch": branch,
                        "remote_commit": remote_commit,
                        "revision": revision.to_string(),
                    }))?;
                buffer.append(tx, &pulled, self.now).await?;
                buffer.extend(recompute(tx, &[], self.now).await?.events);
                Vec::new()
            }
            Subject::Record {
                commit,
                base_tree,
                tree,
                human,
            } => {
                let subject = IntegrationSubject::Record {
                    remote_commit: commit.as_deref(),
                };
                let succeeded =
                    NewEvent::new(types::INTEGRATION_SUCCEEDED, label.clone())?.data(json!({
                        "integration": self.integration.to_string(),
                        "kind": "record",
                        "commit": commit,
                    }))?;
                self.advance(tx, buffer, &label, subject, succeeded).await?;
                workspaces::supersede_conflicted(tx, self.now).await?;
                let change = NewHumanChange {
                    commit: commit.as_deref(),
                    human: human.human.as_str(),
                    base_tree: *base_tree,
                    tree: *tree,
                    now: self.now,
                };
                workspaces::insert_recorded_human_change(tx, &change, self.integration).await?;
                let mut data = human.data.clone();
                data["integration"] = json!(self.integration.to_string());
                let recorded = NewEvent::new(types::HUMAN_CHANGE_RECORDED, label.clone())?
                    .revision(revision)
                    .data(data)?;
                buffer.append(tx, &recorded, self.now).await?;
                buffer.extend(recompute(tx, &[], self.now).await?.events);
                Vec::new()
            }
            Subject::Direct {
                changeset,
                workspace,
                message,
            } => {
                self.recheck_direct(tx, *changeset, *workspace).await?;
                let subject = IntegrationSubject::Direct {
                    changeset: *changeset,
                    message: message.as_str(),
                };
                let succeeded = NewEvent::new(types::INTEGRATION_SUCCEEDED, label.clone())?
                    .workspace(*workspace)
                    .data(json!({
                        "integration": self.integration.to_string(),
                        "kind": "direct",
                        "changeset": changeset.to_string(),
                        "approval": DIRECT_APPROVAL,
                    }))?;
                self.advance(tx, buffer, &label, subject, succeeded).await?;
                let mut archived = Vec::new();
                if archive_in_tx::<Step>(
                    tx,
                    buffer,
                    &label,
                    *workspace,
                    ArchiveReason::Integrated,
                    self.now,
                )
                .await?
                {
                    archived.push(*workspace);
                }
                buffer.extend(recompute(tx, &[], self.now).await?.events);
                archived
            }
        };
        // The integration resolves the waiting human change it brings onto main, and the human's commit becomes a parent of the revision's commit, after a Git commit the integration merged, so the project folder follows main again.
        if let Some(human) = self.human
            && workspaces::resolve_human_change(tx, human.id, self.integration, self.now).await?
        {
            let mut data = human.data.clone();
            data["integration"] = json!(self.integration.to_string());
            let mut resolved =
                NewEvent::new(types::HUMAN_CHANGE_RESOLVED, label)?.revision(revision);
            if let Subject::Direct { workspace, .. } = &self.plan.subject {
                resolved = resolved.workspace(*workspace);
            }
            buffer.append(tx, &resolved.data(data)?, self.now).await?;
        }
        #[cfg(feature = "failpoints")]
        fail::fail_point!("integration_in_transaction", |_| Err(Step::from(fired(
            "integration_in_transaction"
        ))));
        Ok(archived)
    }

    /// Records the integration and its revision, moves `main` by compare-and-swap, and appends `succeeded` and `main.advanced` for the new revision.
    async fn advance(
        &self,
        tx: &mut WriteTx,
        buffer: &mut EventBuffer,
        label: &EventActor,
        subject: IntegrationSubject<'_>,
        succeeded: NewEvent,
    ) -> Result<(), Step> {
        let revision = self.stored.revision().number;
        let new = NewIntegration {
            number: self.integration,
            subject,
            actor: label.as_str(),
            created_at: self.now,
        };
        reviews::insert_succeeded(tx, &new, self.stored).await?;
        match workspaces::advance_main(tx, self.head, revision).await {
            Ok(()) => {}
            Err(StoreError::HeadMoved { .. }) => return Err(Step::Retry),
            Err(error) => return Err(error.into()),
        }
        buffer
            .append(tx, &succeeded.revision(revision), self.now)
            .await?;
        let advanced = NewEvent::new(types::MAIN_ADVANCED, label.clone())?
            .revision(revision)
            .data(json!({
                "revision": revision.to_string(),
                "previous": self.head.to_string(),
            }))?;
        buffer.append(tx, &advanced, self.now).await?;
        Ok(())
    }

    /// The gates again on the writer's connection, because reviews, versions and prerequisites change without the integration mutex. A gate that fails now starts the attempt again, whose gate step then reports it.
    async fn recheck(
        &self,
        tx: &mut WriteTx,
        proposal: ProposalId,
        version_id: i64,
        expected_version: Option<u32>,
        automatic: bool,
    ) -> Result<IntegrationFacts, Step> {
        let facts = reviews::integration_facts(tx.conn(), proposal)
            .await?
            .ok_or_else(|| proposal_not_found(proposal))?;
        let standing = agents::standing_of(tx.conn(), self.actor).await?;
        let caller = caller_facts(self.actor, standing, automatic)?;
        if facts.target.version_id != version_id
            || facts.live_workspace != self.plan.workspace
            || decide(&facts, &caller, expected_version).is_err()
        {
            return Err(Step::Retry);
        }
        Ok(facts)
    }

    /// The checks of a direct change again on the writer's connection, because the main tag, the workspace and its changesets change without the integration mutex. A check that fails now starts the attempt again, whose plan step then reports it.
    async fn recheck_direct(
        &self,
        tx: &mut WriteTx,
        changeset: ChangesetId,
        workspace: WorkspaceId,
    ) -> Result<(), Step> {
        match main_agent_in_tx(tx, self.actor).await {
            Ok(_) => {}
            Err(WorkspaceError::Fold(_)) => return Err(Step::Retry),
            Err(error) => return Err(Step::Failed(error.into())),
        }
        let row = workspaces::get_workspace(tx.conn(), workspace)
            .await?
            .ok_or(StoreError::Corrupt("workspaces.number"))?;
        let latest = workspaces::latest_changeset(tx.conn(), workspace)
            .await?
            .map(|newest| newest.id);
        if check_direct(self.actor, &row, changeset, latest).is_err() {
            return Err(Step::Retry);
        }
        let held = workspaces::human_change_of_workspace(tx.conn(), workspace).await?;
        if let Some(stale) = superseded(workspace, held.as_ref()) {
            return Err(Step::Failed(stale.into()));
        }
        Ok(())
    }

    /// The proposal half of step 6: the completion, the integrating human's approval, the retired proposal, the archived workspaces, the released claim and the recalculated statuses.
    async fn complete(
        &self,
        tx: &mut WriteTx,
        buffer: &mut EventBuffer,
        label: &EventActor,
        facts: IntegrationFacts,
    ) -> Result<Vec<WorkspaceId>, Step> {
        let (proposal, task, version) =
            (facts.proposal.id, facts.proposal.task, facts.target.version);
        let revision = self.stored.revision().number;
        completions::insert(
            tx,
            task,
            CompletionKind::Integration,
            Some(self.integration),
            self.now,
        )
        .await?;
        if let Actor::Human { name } = self.actor
            && !approved_last(&facts.reviews, name)
        {
            reviews::insert_review(
                tx,
                facts.target.version_id,
                ReviewerRef::Human(name),
                Verdict::Approve,
                INTEGRATING_APPROVAL,
                self.now,
            )
            .await?;
            let submitted = NewEvent::new(types::REVIEW_SUBMITTED, label.clone())?
                .task(task)
                .proposal(proposal)
                .data(json!({
                    "verdict": Verdict::Approve.as_str(),
                    "version": version,
                    "comments": 0,
                }))?;
            buffer.append(tx, &submitted, self.now).await?;
        }
        let integrated = NewEvent::new(types::PROPOSAL_INTEGRATED, label.clone())?
            .task(task)
            .proposal(proposal)
            .revision(revision)
            .data(json!({
                "version": version,
                "integration": self.integration.to_string(),
            }))?;
        buffer.append(tx, &integrated, self.now).await?;
        let mut archived = reviews::live_review_workspaces(tx.conn(), proposal).await?;
        retire_proposal(
            tx,
            buffer,
            label,
            proposal,
            Retirement::Integrated,
            self.now,
        )
        .await?;
        if let Some(workspace) = self.plan.workspace
            && archive_in_tx::<Step>(
                tx,
                buffer,
                label,
                workspace,
                ArchiveReason::Integrated,
                self.now,
            )
            .await?
        {
            archived.push(workspace);
        }
        if let Some(claim) = claims::unreleased_for_task(tx.conn(), task).await? {
            let reason = ReleaseReason::Integrated;
            claims::release(tx, task, claim.worker_id, reason, self.now).await?;
            let released = NewEvent::new(types::CLAIM_RELEASED, label.clone())?
                .task(task)
                .data(json!({
                    "kind": "implementation",
                    "worker": claim.worker.as_str(),
                    "reason": reason.as_str(),
                }))?;
            buffer.append(tx, &released, self.now).await?;
        }
        let recomputed = recompute(tx, &[task], self.now).await?;
        buffer.extend(recomputed.events);
        notify::integration_messages(
            tx,
            buffer,
            &facts,
            revision,
            &recomputed.became_todo,
            self.now,
        )
        .await?;
        Ok(archived)
    }
}

/// Whether the latest verdict on the version already is `human`'s approval, so integrating adds no second one.
fn approved_last(reviews: &[ReviewFact], human: &HumanName) -> bool {
    latest_verdict(reviews).is_some_and(|latest| {
        latest.verdict == Verdict::Approve
            && matches!(&latest.approver, Approver::Human(name) if name == human)
    })
}
