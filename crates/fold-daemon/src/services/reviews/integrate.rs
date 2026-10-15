use fold_core::events::types;
use fold_core::merge::{Conflict, ReportTrees};
use fold_core::object::ObjectHash;
use fold_core::policy::{CallerFacts, GateRefusal, IntegrateInput, ProposalFacts, can_integrate};
use fold_core::reviews::{Approver, ReviewFact, Verdict, latest_verdict};
use fold_core::revision::{Revision, StoredRevision};
use fold_core::{
    Actor, EventActor, EventError, FoldError, HumanName, IdKind, IntegrationId, NewEvent,
    ProposalId, RevisionId, TaskId, Timestamp, WorkspaceId, codes,
};
use fold_store::reviews::{
    self, IntegrationFacts, IntegrationSubject, NewIntegration, ProposalState, ReviewerRef,
};
use fold_store::tasks::claims::{self, ReleaseReason};
use fold_store::tasks::completions::{self, CompletionKind};
use fold_store::{StoreError, WriteTx, counters, workspaces};
use fold_vcs::integrate::MergeOutcome;
use serde_json::json;

use super::conflicts::FoundConflict;
use super::notify;
use super::proposals::{Retirement, proposal_not_found, retire_proposal};
use super::{ReviewError, ReviewService};
use crate::services::events::EventBuffer;
use crate::services::tasks::{TaskError, recompute};
use crate::services::workspaces::{ArchiveReason, archive_in_tx};

/// How many times the procedure starts again from reading main when main moved under it.
const ATTEMPTS: usize = 3;

/// The longest remote commit id: a SHA-256 Git object id in hex.
const MAX_COMMIT_CHARS: usize = 64;

/// The body of the approval a human gives by integrating.
const INTEGRATING_APPROVAL: &str = "approved by integrating";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrationOutcome {
    Succeeded {
        revision: RevisionId,
    },
    /// In path order. Nothing was published, no revision exists and the task is not completed.
    Conflicted(Vec<Conflict>),
}

/// A clean `fold pull`: the newest commit of the Git remote, its tree already in the object store, and the fold revision of the last synced commit, which is the merge base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullIntegration {
    pub remote_commit: String,
    pub sync_base: RevisionId,
    pub remote_tree: ObjectHash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullOutcome {
    Succeeded {
        revision: RevisionId,
    },
    /// In path order. No row and no event was written.
    Conflicted(Vec<Conflict>),
}

#[derive(Clone, Copy)]
enum IntegrationRequest<'a> {
    Proposal {
        proposal: ProposalId,
        expected_version: Option<u32>,
    },
    Pull(&'a PullIntegration),
}

enum Integrated {
    Succeeded(RevisionId),
    Conflicted(Vec<Conflict>),
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
    /// The task workspace the transaction archives; its operation lock is held from the merge until the commit.
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
    },
    Pull {
        remote_commit: String,
    },
}

/// The first refusal of the gates on `facts`, as `can_integrate` decides it.
fn decide(
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
    /// Integrates the current version of an open proposal onto main, by an agent with the `integrator` tag that is not the author's agent, or by a named human, who approves in the same action. `expected_version` is the version the caller saw; another current version refuses with `stale_version`. A refusal by the open, version or caller gate writes nothing; a refusal by the approval, human review or prerequisite gate records a `refused` integration and `integration.refused`, then returns the gate's error. A clean merge moves main by compare-and-swap, records the integration, completes the task, retires the proposal, archives its workspaces and releases its claims in one transaction. A conflicting merge publishes nothing: it records a `conflicted` integration with its report, flags the proposal, tells the author's worker and returns the task to In Progress, then returns the conflicts.
    pub async fn integrate_proposal(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        expected_version: Option<u32>,
    ) -> Result<IntegrationOutcome, ReviewError> {
        let request = IntegrationRequest::Proposal {
            proposal,
            expected_version,
        };
        Ok(match self.integrate(actor, request).await? {
            Integrated::Succeeded(revision) => IntegrationOutcome::Succeeded { revision },
            Integrated::Conflicted(conflicts) => IntegrationOutcome::Conflicted(conflicts),
        })
    }

    /// Integrates the newest commit of the Git remote onto main, by the named human who ran `fold pull`; an agent is refused with `human_required`. There is no proposal, task or review, so only the merge decides. A clean merge moves main and records a `pull` integration with the remote commit and `remote.pulled`; a conflicting one writes nothing and returns the conflicts to the caller.
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
            },
        )
    }

    /// The one procedure behind both kinds. The project's integration mutex is held throughout, so two integrations never run at the same time; an attempt that finds main moved starts again, three times at most.
    async fn integrate(
        &self,
        actor: &Actor,
        request: IntegrationRequest<'_>,
    ) -> Result<Integrated, ReviewError> {
        let _serial = self.integration.lock().await;
        for _ in 0..ATTEMPTS {
            match self.attempt(actor, request).await {
                Ok(done) => return Ok(done),
                Err(Step::Retry) => continue,
                Err(Step::Failed(error)) => return Err(error),
            }
        }
        Err(FoldError::new(
            codes::HEAD_MOVED,
            format!("main moved during each of {ATTEMPTS} integration attempts"),
        )
        .with_hint("call integrate_proposal again, or run fold pull again")
        .into())
    }

    async fn attempt(
        &self,
        actor: &Actor,
        request: IntegrationRequest<'_>,
    ) -> Result<Integrated, Step> {
        let head = workspaces::head(self.store.read())
            .await?
            .ok_or(StoreError::Corrupt("refs.main"))?;
        let plan = match request {
            IntegrationRequest::Proposal {
                proposal,
                expected_version,
            } => {
                self.proposal_plan(actor, proposal, expected_version)
                    .await?
            }
            IntegrationRequest::Pull(pull) => self.pull_plan(actor, pull).await?,
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
        let mut buffer = EventBuffer::new();
        let commit = Commit {
            actor,
            plan: &plan,
            head: head.number,
            stored: &stored,
            integration,
            now,
        };
        let archived = self
            .store
            .write(async |tx| commit.run(tx, &mut buffer).await)
            .await?;
        drop(locks);
        self.bus.publish(buffer);
        for workspace in archived {
            self.workspaces.detach_live(workspace).await;
        }
        match &plan.subject {
            Subject::Proposal { proposal, .. } => {
                tracing::info!(proposal = %proposal, revision = %next, integration = %integration, "proposal integrated");
            }
            Subject::Pull { remote_commit } => {
                tracing::info!(remote_commit = %remote_commit, revision = %next, integration = %integration, "remote commit integrated");
            }
        }
        Ok(Integrated::Succeeded(next))
    }

    /// Step 2 of a proposal: the gates on stored facts, with the refusals of the review gates recorded.
    async fn proposal_plan(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        expected_version: Option<u32>,
    ) -> Result<Plan, Step> {
        let caller = caller_facts(actor)?;
        let mut read = self.store.read().begin().await.map_err(StoreError::from)?;
        let facts = reviews::integration_facts(&mut read, proposal)
            .await?
            .ok_or_else(|| proposal_not_found(proposal))?;
        read.commit().await.map_err(StoreError::from)?;
        if let Err(refusal) = decide(&facts, &caller, expected_version) {
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
                | GateRefusal::AuthorCannotIntegrate => FoldError::from(refusal).into(),
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
                    subject: IntegrationSubject::Proposal { proposal, version },
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

    /// Step 2 of a pull: only a named human brings remote commits in, and the merge base is the last synced revision.
    async fn pull_plan(&self, actor: &Actor, pull: &PullIntegration) -> Result<Plan, Step> {
        if !actor.is_human() {
            return Err(FoldError::new(
                codes::HUMAN_REQUIRED,
                "only the human who runs fold pull integrates remote commits",
            )
            .with_hint("ask a human to run fold pull")
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
        let base = workspaces::get_by_number(self.store.read(), pull.sync_base).await?;
        Ok(Plan {
            base_tree: base.tree_hash,
            other_tree: pull.remote_tree,
            workspace: None,
            subject: Subject::Pull {
                remote_commit: commit.to_owned(),
            },
        })
    }
}

/// The caller as the gates see it. The daemon itself never integrates.
fn caller_facts(actor: &Actor) -> Result<CallerFacts, Step> {
    match actor {
        Actor::Agent(agent) => Ok(CallerFacts::Agent {
            agent: agent.agent.clone(),
            tags: agent.tags.clone(),
        }),
        Actor::Human { name } => Ok(CallerFacts::Human(name.clone())),
        Actor::System => Err(FoldError::from(GateRefusal::IntegratorRequired).into()),
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
        match &self.plan.subject {
            Subject::Proposal {
                proposal,
                task,
                version,
                version_id,
                expected_version,
            } => {
                let facts = self
                    .recheck(tx, *proposal, *version_id, *expected_version)
                    .await?;
                let subject = IntegrationSubject::Proposal {
                    proposal: *proposal,
                    version: *version,
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
                self.complete(tx, buffer, &label, facts).await
            }
            Subject::Pull { remote_commit } => {
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
                let pulled = NewEvent::new(types::REMOTE_PULLED, label)?
                    .revision(revision)
                    .data(json!({
                        "revision": revision.to_string(),
                        "remote_commit": remote_commit,
                    }))?;
                buffer.append(tx, &pulled, self.now).await?;
                buffer.extend(recompute(tx, &[], self.now).await?.events);
                Ok(Vec::new())
            }
        }
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

    /// The gates again on the writer's connection, because reviews, versions and prerequisites change without the integration mutex. Anything that no longer holds starts the attempt again, whose gate step then reports it.
    async fn recheck(
        &self,
        tx: &mut WriteTx,
        proposal: ProposalId,
        version_id: i64,
        expected_version: Option<u32>,
    ) -> Result<IntegrationFacts, Step> {
        let facts = reviews::integration_facts(tx.conn(), proposal)
            .await?
            .ok_or_else(|| proposal_not_found(proposal))?;
        let caller = caller_facts(self.actor)?;
        if facts.target.version_id != version_id
            || facts.live_workspace != self.plan.workspace
            || decide(&facts, &caller, expected_version).is_err()
        {
            return Err(Step::Retry);
        }
        Ok(facts)
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
        buffer.extend(recompute(tx, &[task], self.now).await?.events);
        notify::integration_messages(tx, buffer, &facts, revision, self.now).await?;
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
