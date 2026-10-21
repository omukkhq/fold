use std::collections::{BTreeSet, HashSet};

use fold_core::events::types;
use fold_core::object::ObjectHash;
use fold_core::policy::{CallerFacts, GateRefusal};
use fold_core::reviews::batches::{
    BatchMemberFacts, BatchOutcome, BatchReadiness, MemberStanding, StandingFacts, batch_readiness,
    member_standing,
};
use fold_core::reviews::checks::{RunReason, Stage};
use fold_core::revision::{Revision, StoredRevision};
use fold_core::{
    Actor, AddressedAs, CheckRunId, EventActor, FoldError, IdKind, IntegrationId, MessageBody,
    NewEvent, ProposalId, Recipient, ReviewBatchId, RevisionId, Target, TaskId, WorkspaceId, codes,
};
use fold_store::reviews::batches::{self, BatchRevisionRow, BatchState, MemberRelease};
use fold_store::reviews::{self, IntegrationFacts, ProposalState, checks};
use fold_store::{SqliteConnection, StoreError, agents, counters, workspaces};
use fold_vcs::integrate::MergeOutcome;
use serde_json::{Value, json};

use super::batches::batch_not_found;
use super::checks::{Integrating, IntegrationChecks, PassedRun, fold_failed_refusal};
use super::integrate::{
    ATTEMPTS, Commit, HumanResolution, Integrated, IntegrationOutcome, IntegrationRequest, Plan,
    Step, Subject, caller_facts, decide, queued_actor,
};
use super::{ReviewError, ReviewService};
use crate::services::events::EventBuffer;
#[cfg(feature = "failpoints")]
use crate::services::fired;
use crate::services::messages::send_from_fold_in_tx;
use crate::services::tasks::TaskError;

/// What one member's integration did in a batch's turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberIntegration {
    pub proposal: ProposalId,
    pub outcome: IntegrationOutcome,
}

/// What a batch's integration turn did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchIntegration {
    /// The ready members merged and passed one check run together and were published in one transaction, one revision each, in merge order. A member that conflicted was taken out of the merge and integrated on its own after it, which sent it back with its conflict report; those follow in `members` with their own outcomes.
    Together { members: Vec<MemberIntegration> },
    /// The one check run of the members together failed, so fold integrated them one at a time in merge order, each with its own run; a member whose run failed went back to its author, and a member that needs one that did not integrate was left for its own rules.
    OneAtATime {
        run: CheckRunId,
        members: Vec<MemberIntegration>,
    },
    /// No member was ready to integrate with the batch; each integrates on its own rules.
    Nothing,
    /// The project has integration checks, so the batch waits its turn in the integration queue; fold sends each member's result as a message.
    Queued,
}

impl BatchIntegration {
    /// Each member's integration in the order it ran.
    pub fn into_members(self) -> Vec<MemberIntegration> {
        match self {
            BatchIntegration::Together { members }
            | BatchIntegration::OneAtATime { members, .. } => members,
            BatchIntegration::Nothing | BatchIntegration::Queued => Vec::new(),
        }
    }
}

/// Where one batch stands for integration, as one read sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchStanding {
    pub batch: ReviewBatchId,
    pub state: BatchState,
    pub members: Vec<BatchMemberFacts>,
    /// `None` for an open or closed batch, and for a submitted batch whose turn ended.
    pub readiness: Option<BatchReadiness>,
    pub ended: Option<batches::BatchIntegrationRow>,
    /// A request to integrate the batch waits in the queue.
    pub queued: bool,
}

/// The batch's integration as a view shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchIntegrationView {
    /// The batch is open or closed: its members are not integrated together.
    NotSubmitted,
    /// Members without a final state hold the approved ones back.
    Waiting { pending: Vec<ProposalId> },
    /// Every member has its final state; `members` integrate together in this order, none when it is empty.
    Ready {
        members: Vec<ProposalId>,
        queued: bool,
    },
    /// The batch's turn ended; `revisions` are the members it published together.
    Ended {
        outcome: BatchOutcome,
        run: Option<CheckRunId>,
        revisions: Vec<BatchRevisionRow>,
    },
}

impl BatchStanding {
    pub fn view(&self, revisions: Vec<BatchRevisionRow>) -> BatchIntegrationView {
        if let Some(ended) = &self.ended {
            return BatchIntegrationView::Ended {
                outcome: ended.outcome,
                run: ended.run,
                revisions,
            };
        }
        match &self.readiness {
            None => BatchIntegrationView::NotSubmitted,
            Some(BatchReadiness::Waiting { pending }) => BatchIntegrationView::Waiting {
                pending: pending.clone(),
            },
            Some(BatchReadiness::Final { members }) => BatchIntegrationView::Ready {
                members: members.clone(),
                queued: self.queued,
            },
        }
    }
}

/// The proposals the batches hold back from integrating on their own, and the submitted batches whose members can take their turn.
#[derive(Debug, Default)]
pub(super) struct BatchHolds {
    pub held: HashSet<ProposalId>,
    /// In number order.
    pub ready: Vec<ReviewBatchId>,
}

/// One member as the batch's procedure merges and publishes it.
struct MemberPlan {
    proposal: ProposalId,
    task: TaskId,
    version: u32,
    version_id: i64,
    base_tree: ObjectHash,
    tree: ObjectHash,
    workspace: Option<WorkspaceId>,
    /// The prerequisites that are not Done; each is the task of an earlier member.
    waits_for: Vec<TaskId>,
}

impl MemberPlan {
    fn plan(&self, automatic: bool) -> Plan {
        Plan {
            base_tree: self.base_tree,
            other_tree: self.tree,
            workspace: self.workspace,
            subject: Subject::Proposal {
                proposal: self.proposal,
                task: self.task,
                version: self.version,
                version_id: self.version_id,
                expected_version: None,
                automatic,
            },
        }
    }
}

/// What one attempt of a batch's procedure ended with, before the members it took out run on their own.
enum Attempted {
    Nothing,
    /// main's own checks file is not valid; nothing ran or was published.
    Held,
    Published {
        revisions: Vec<MemberIntegration>,
        set_aside: Vec<MemberPlan>,
    },
    ChecksFailed {
        run: CheckRunId,
        members: Vec<MemberPlan>,
    },
}

/// A member's standing with the facts its integration needs.
struct ReadMember {
    standing: BatchMemberFacts,
    facts: Option<IntegrationFacts>,
}

/// Every member of the batch with its standing, read on one connection.
async fn read_members(
    conn: &mut SqliteConnection,
    batch: ReviewBatchId,
) -> Result<Vec<ReadMember>, ReviewError> {
    let mut read = Vec::new();
    for member in batches::members(&mut *conn, batch).await? {
        let facts = reviews::integration_facts(&mut *conn, member.proposal).await?;
        let standing = match &facts {
            None => MemberStanding::Out,
            Some(facts) => {
                let row = &facts.proposal;
                member_standing(&StandingFacts {
                    removed: member.release == Some(MemberRelease::Removed),
                    ended: row.state != ProposalState::Open,
                    flagged: row.conflicted || row.checks_failed,
                    sent_back: batches::changes_requested_since(
                        &mut *conn,
                        member.proposal,
                        member.version,
                    )
                    .await?,
                    gates: decide(facts, &CallerFacts::Automatic, None),
                })
            }
        };
        read.push(ReadMember {
            standing: BatchMemberFacts {
                proposal: member.proposal,
                task: member.task,
                standing,
            },
            facts,
        });
    }
    Ok(read)
}

async fn standing_on(
    conn: &mut SqliteConnection,
    batch: ReviewBatchId,
) -> Result<(BatchStanding, Vec<ReadMember>), ReviewError> {
    let row = batches::get_batch(&mut *conn, batch)
        .await?
        .ok_or_else(|| batch_not_found(batch))?;
    let members = read_members(conn, batch).await?;
    let ended = batches::batch_integration(&mut *conn, batch).await?;
    let queued = batches::batch_request_waiting(&mut *conn, batch).await?;
    let facts: Vec<BatchMemberFacts> = members.iter().map(|m| m.standing.clone()).collect();
    let readiness =
        (row.state == BatchState::Submitted && ended.is_none()).then(|| batch_readiness(&facts));
    Ok((
        BatchStanding {
            batch,
            state: row.state,
            members: facts,
            readiness,
            ended,
            queued,
        },
        members,
    ))
}

impl ReviewService {
    /// Where the batch stands for integration. It reads only.
    pub async fn batch_standing(&self, batch: ReviewBatchId) -> Result<BatchStanding, ReviewError> {
        let mut read = self.store.read().begin().await.map_err(StoreError::from)?;
        let (standing, _) = standing_on(&mut read, batch).await?;
        read.commit().await.map_err(StoreError::from)?;
        Ok(standing)
    }

    /// The batch's integration with the revisions it published, for a view. It reads only.
    pub async fn batch_integration_view(
        &self,
        batch: ReviewBatchId,
    ) -> Result<BatchIntegrationView, ReviewError> {
        let standing = self.batch_standing(batch).await?;
        let revisions = batches::batch_revisions(self.store.read(), batch).await?;
        Ok(standing.view(revisions))
    }

    /// The proposals that batches hold back from integrating on their own: every member of an open batch, whose review is not submitted yet, and every member of a submitted batch whose turn has not ended that waits for the others or integrates with them. A submitted batch whose members all have their final state is ready for its turn, unless fold failed to run its checks on the same version and main, which only a caller's integrate or a move of main tries again.
    pub(super) async fn batch_holds(&self) -> Result<BatchHolds, ReviewError> {
        let head = workspaces::head(self.store.read())
            .await?
            .ok_or(StoreError::Corrupt("refs.main"))?
            .number;
        let mut holds = BatchHolds::default();
        let mut read = self.store.read().begin().await.map_err(StoreError::from)?;
        for open in batches::list_batches(&mut *read, true).await? {
            for member in batches::members(&mut *read, open.id).await? {
                if member.is_open() {
                    holds.held.insert(member.proposal);
                }
            }
        }
        for batch in batches::undecided_batches(&mut *read).await? {
            let (standing, members) = standing_on(&mut read, batch).await?;
            let Some(readiness) = &standing.readiness else {
                continue;
            };
            for member in &standing.members {
                if readiness.holds(&standing.members, member.proposal) {
                    holds.held.insert(member.proposal);
                }
            }
            let BatchReadiness::Final { members: ready } = readiness else {
                continue;
            };
            let last = ready.last().and_then(|proposal| {
                members
                    .iter()
                    .find(|member| member.standing.proposal == *proposal)
                    .and_then(|member| member.facts.as_ref())
            });
            if let Some(last) = last {
                let newest = checks::runs_of_version(
                    &mut *read,
                    last.proposal.task,
                    last.proposal.id,
                    last.target.version,
                )
                .await?
                .into_iter()
                .find(|run| run.stage == Stage::Integration);
                if newest.is_some_and(|run| {
                    run.reason == Some(RunReason::FoldFailed) && run.base == head
                }) {
                    continue;
                }
            }
            holds.ready.push(batch);
        }
        read.commit().await.map_err(StoreError::from)?;
        Ok(holds)
    }

    /// Integrates the approved members of a submitted batch together, by an agent that decides reviews or a named human, once every member has its final state for the batch. A member that went back to its author, was taken out, ended, or waits for a human or for a member that is not in the integration is left out and integrates later on its own rules. The integration is one turn of the project's integration mutex: the members merge onto main in dependency order, the integration checks run once on the whole merge, and on a pass one transaction publishes one revision per member and completes each task. A member that conflicts integrates on its own after the others, which sends it back alone; a failed check run integrates the members one at a time, each with its own run. When the project has integration checks, the batch waits in the integration queue and the answer is `Queued`. Integrating one member alone with `integrate_proposal` stays possible.
    ///
    /// Refusals: `review_batch_not_found`; `invalid_request` with the reason `batch_not_submitted` for an open or closed batch, `batch_waiting` while a member has no final state (details list them), `batch_integrated` once the batch's turn ended, and `integration_queued` while a request of the batch waits; and the caller gates of `integrate_proposal` for each member, such as `author_cannot_integrate` for the agent that wrote a member's current version.
    pub async fn integrate_review_batch(
        &self,
        actor: &Actor,
        batch: ReviewBatchId,
    ) -> Result<BatchIntegration, ReviewError> {
        let members = self.batch_plan(actor, batch, false).await?;
        if self.batch_checks_may_run(&members).await? {
            self.queue_batch(actor, batch).await?;
            return Ok(BatchIntegration::Queued);
        }
        let _serial = self.integration.lock().await;
        self.batch_turn_locked(actor, batch, false)
            .await?
            .ok_or_else(|| held_batch_refusal(batch).into())
    }

    /// The automatic turn of a ready batch, as fold. `None` when main's invalid checks file holds it; main's next revision is an event that tries again.
    pub(super) async fn integrate_batch_automatically(
        &self,
        batch: ReviewBatchId,
    ) -> Result<Option<BatchIntegration>, ReviewError> {
        let _serial = self.integration.lock().await;
        self.batch_turn_locked(&Actor::System, batch, true).await
    }

    /// Whether the integration checks can run for the members: the checks file of a member's version or of main names an integration check.
    async fn batch_checks_may_run(&self, members: &[MemberPlan]) -> Result<bool, ReviewError> {
        for member in members {
            if self.integration_checks_may_run(member.proposal).await? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The members that integrate together, in merge order, read and checked on one read: the batch must be submitted, with its turn not ended and every member's final state known, and every member must pass the caller gates for `actor`. Empty when no member is ready.
    async fn batch_plan(
        &self,
        actor: &Actor,
        batch: ReviewBatchId,
        automatic: bool,
    ) -> Result<Vec<MemberPlan>, ReviewError> {
        let mut read = self.store.read().begin().await.map_err(StoreError::from)?;
        let standing = agents::standing_of(&mut *read, actor).await?;
        let caller = caller_facts(actor, standing, automatic)?;
        let (standing, mut members) = standing_on(&mut read, batch).await?;
        read.commit().await.map_err(StoreError::from)?;
        if let Some(ended) = &standing.ended {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("the integration of {batch} ended already"),
            )
            .with_details([
                ("reason", json!("batch_integrated")),
                ("batch", json!(batch.to_string())),
                ("outcome", json!(ended.outcome.as_str())),
            ])
            .with_hint("integrate a member that is left with integrate_proposal")
            .into());
        }
        let order = match standing.readiness {
            None => {
                return Err(FoldError::new(
                    codes::INVALID_REQUEST,
                    format!("{batch} is {}, not submitted", standing.state.as_str()),
                )
                .with_details([
                    ("reason", json!("batch_not_submitted")),
                    ("batch", json!(batch.to_string())),
                ])
                .with_hint("a batch integrates once its review is submitted with submit_batch_review; integrate a member alone with integrate_proposal")
                .into());
            }
            Some(BatchReadiness::Waiting { pending }) => {
                let pending: Vec<String> = pending.iter().map(ToString::to_string).collect();
                return Err(FoldError::new(
                    codes::INVALID_REQUEST,
                    format!("members of {batch} have no final state yet"),
                )
                .with_details([
                    ("reason", json!("batch_waiting")),
                    ("batch", json!(batch.to_string())),
                    ("pending", json!(pending)),
                ])
                .with_hint("each pending member needs its verdict; integrate a member alone with integrate_proposal")
                .into());
            }
            Some(BatchReadiness::Final { members }) => members,
        };
        let mut plans = Vec::with_capacity(order.len());
        for proposal in order {
            let Some(index) = members
                .iter()
                .position(|member| member.standing.proposal == proposal)
            else {
                continue;
            };
            let member = members.swap_remove(index);
            let (Some(facts), MemberStanding::Approved { waits_for }) =
                (member.facts, member.standing.standing)
            else {
                continue;
            };
            match decide(&facts, &caller, None) {
                Ok(()) | Err(GateRefusal::PrerequisitesNotDone { .. }) => {}
                Err(refusal) => {
                    return Err(FoldError::from(refusal)
                        .with_details([
                            ("batch", json!(batch.to_string())),
                            ("proposal", json!(proposal.to_string())),
                        ])
                        .into());
                }
            }
            plans.push(MemberPlan {
                proposal,
                task: facts.proposal.task,
                version: facts.target.version,
                version_id: facts.target.version_id,
                base_tree: facts.target.base_tree,
                tree: facts.target.tree,
                workspace: facts.live_workspace,
                waits_for,
            });
        }
        Ok(plans)
    }

    /// One turn of the batch under the integration mutex, which the caller holds: the attempts of the members together, then the members it took out on their own. `None` when main's invalid checks file holds the batch.
    async fn batch_turn_locked(
        &self,
        actor: &Actor,
        batch: ReviewBatchId,
        automatic: bool,
    ) -> Result<Option<BatchIntegration>, ReviewError> {
        let mut carried = None;
        for _ in 0..ATTEMPTS {
            let attempt = self
                .batch_attempt(actor, batch, automatic, &mut carried)
                .await;
            if !matches!(attempt, Err(Step::Retry)) {
                self.end_carried(&mut carried).await?;
            }
            match attempt {
                Ok(done) => return self.after_attempt(actor, batch, automatic, done).await,
                Err(Step::Retry) => continue,
                Err(Step::Failed(error)) => return Err(error),
            }
        }
        self.end_carried(&mut carried).await?;
        Err(FoldError::new(
            codes::HEAD_MOVED,
            format!("main moved during each of {ATTEMPTS} integration attempts of {batch}"),
        )
        .with_hint("call integrate_review_batch again")
        .into())
    }

    async fn batch_attempt(
        &self,
        actor: &Actor,
        batch: ReviewBatchId,
        automatic: bool,
        carried: &mut Option<PassedRun>,
    ) -> Result<Attempted, Step> {
        let head = workspaces::head(self.store.read())
            .await?
            .ok_or(StoreError::Corrupt("refs.main"))?;
        let members = self.batch_plan(actor, batch, automatic).await?;
        if members.is_empty() {
            return Ok(Attempted::Nothing);
        }
        let locks = self
            .workspaces
            .lock_for_batch_integration(members.iter().filter_map(|m| m.workspace).collect())
            .await;
        let mut tree = head.tree_hash;
        let mut merged: Vec<(MemberPlan, ObjectHash)> = Vec::new();
        let mut set_aside: Vec<MemberPlan> = Vec::new();
        let mut aside: BTreeSet<TaskId> = BTreeSet::new();
        for member in members {
            if member.waits_for.iter().any(|task| aside.contains(task)) {
                aside.insert(member.task);
                set_aside.push(member);
                continue;
            }
            match self
                .workspaces
                .merge_for_integration(&locks, member.base_tree, tree, member.tree)
                .await
                .map_err(ReviewError::from)?
            {
                MergeOutcome::Merged { tree_hash } => {
                    tree = tree_hash;
                    merged.push((member, tree_hash));
                }
                MergeOutcome::Conflicted(_) => {
                    aside.insert(member.task);
                    set_aside.push(member);
                }
            }
        }
        let Some((last, _)) = merged.last() else {
            return Ok(Attempted::Published {
                revisions: Vec::new(),
                set_aside,
            });
        };
        let at = Integrating {
            proposal: last.proposal,
            task: last.task,
            version: last.version,
            head: head.number,
            head_tree: head.tree_hash,
            merged: tree,
            batch: Some(batch),
        };
        match self.integration_checks(actor, at, carried).await? {
            IntegrationChecks::None | IntegrationChecks::Passed => {}
            IntegrationChecks::Failed(run) => {
                let mut members: Vec<MemberPlan> = merged.into_iter().map(|(m, _)| m).collect();
                members.extend(set_aside);
                return Ok(Attempted::ChecksFailed { run, members });
            }
            IntegrationChecks::FoldFailed(run) => {
                return Err(Step::Failed(fold_failed_refusal(at, run).into()));
            }
            IntegrationChecks::Held => return Ok(Attempted::Held),
        }
        // Allocated in their own write, so a failed attempt never gives its numbers to a later one.
        let count = merged.len();
        let numbers = self
            .store
            .write(async |tx| {
                let mut numbers = Vec::with_capacity(count);
                for _ in 0..count {
                    numbers.push(counters::allocate(tx, IdKind::Integration).await?);
                }
                Ok::<_, StoreError>(numbers)
            })
            .await?;
        let now = self.clock.now();
        let mut parent = head.hash;
        let mut previous = head.number;
        let mut written: Vec<(IntegrationId, RevisionId, StoredRevision)> = Vec::new();
        for ((_, tree), number) in merged.iter().zip(numbers) {
            let integration =
                IntegrationId::from_number(number).ok_or(StoreError::Corrupt("counters.next"))?;
            let next = RevisionId::from_number(previous.number().saturating_add(1))
                .ok_or(StoreError::Corrupt("revisions.number"))?;
            let revision = Revision {
                tree: *tree,
                parent: Some(parent),
                number: next,
                integration: Some(integration),
                created_at: now,
            };
            let stored = self
                .workspaces
                .write_integration_revision(&locks, revision)
                .await
                .map_err(ReviewError::from)?;
            parent = stored.hash();
            written.push((integration, previous, stored));
            previous = next;
        }
        #[cfg(feature = "failpoints")]
        fail::fail_point!("batch_integration_after_objects", |_| Err(Step::from(
            fired("batch_integration_after_objects")
        )));
        let plans: Vec<Plan> = merged.iter().map(|(m, _)| m.plan(automatic)).collect();
        let mut human: Option<(usize, HumanResolution)> = None;
        for (index, (plan, (_, tree))) in plans.iter().zip(&merged).enumerate() {
            if let Some(found) = self.human_resolution(plan, *tree).await? {
                human = Some((index, found));
                break;
            }
        }
        let run = carried.as_ref().map(|passed| passed.run.id);
        let commits: Vec<Commit<'_>> = plans
            .iter()
            .zip(&written)
            .enumerate()
            .map(|(index, (plan, (integration, before, stored)))| Commit {
                actor,
                plan,
                head: *before,
                stored,
                integration: *integration,
                now,
                human: human
                    .as_ref()
                    .filter(|(at, _)| *at == index)
                    .map(|(_, found)| found),
                checks: if index == 0 { carried.as_ref() } else { None },
                batch: Some(batch),
            })
            .collect();
        let published: Vec<Value> = merged
            .iter()
            .zip(&written)
            .map(|((member, _), (integration, _, stored))| {
                json!({
                    "proposal": member.proposal.to_string(),
                    "task": member.task.to_string(),
                    "version": member.version,
                    "integration": integration.to_string(),
                    "revision": stored.revision().number.to_string(),
                })
            })
            .collect();
        let label = actor.event_actor();
        let mut buffer = EventBuffer::new();
        let archived = self
            .store
            .write(async |tx| {
                let mut archived = Vec::new();
                for (index, commit) in commits.iter().enumerate() {
                    if index == 1 {
                        #[cfg(feature = "failpoints")]
                        fail::fail_point!("batch_integration_in_transaction", |_| Err(Step::from(
                            fired("batch_integration_in_transaction")
                        )));
                    }
                    archived.extend(commit.run(tx, &mut buffer).await?);
                }
                if !batches::end_batch_integration(tx, batch, BatchOutcome::Together, run, now)
                    .await?
                {
                    return Err(Step::Retry);
                }
                let event = turn_event(&label, batch, BatchOutcome::Together, run, &published)?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, Step>(archived)
            })
            .await?;
        drop(commits);
        drop(locks);
        self.bus.publish(buffer);
        for workspace in archived {
            self.workspaces.detach_live(workspace).await;
        }
        self.follow_main_in_check_dir(tree);
        for held in self.checks.take_held() {
            self.serve_queued(
                held.id,
                held.actor,
                held.proposal,
                held.version,
                held.expected_version,
            );
        }
        let revisions = merged
            .iter()
            .zip(&written)
            .map(|((member, _), (_, _, stored))| MemberIntegration {
                proposal: member.proposal,
                outcome: IntegrationOutcome::Succeeded {
                    revision: stored.revision().number,
                },
            })
            .collect::<Vec<_>>();
        tracing::info!(batch = %batch, members = revisions.len(), revision = %previous, "review batch integrated");
        Ok(Attempted::Published {
            revisions,
            set_aside,
        })
    }

    /// What follows an attempt in the same turn: the members it took out integrate on their own, a failed run integrates every member one at a time, and the turn's end is recorded when its transaction did not record it.
    async fn after_attempt(
        &self,
        actor: &Actor,
        batch: ReviewBatchId,
        automatic: bool,
        attempted: Attempted,
    ) -> Result<Option<BatchIntegration>, ReviewError> {
        match attempted {
            Attempted::Held => Ok(None),
            Attempted::Nothing => {
                self.end_turn(actor, batch, BatchOutcome::Nothing, None, &[])
                    .await?;
                Ok(Some(BatchIntegration::Nothing))
            }
            Attempted::Published {
                mut revisions,
                set_aside,
            } => {
                let together = !revisions.is_empty();
                let alone = self.one_at_a_time(actor, automatic, set_aside).await?;
                if !together {
                    self.end_turn(actor, batch, BatchOutcome::Nothing, None, &alone)
                        .await?;
                }
                revisions.extend(alone);
                Ok(Some(BatchIntegration::Together { members: revisions }))
            }
            Attempted::ChecksFailed { run, members } => {
                tracing::info!(batch = %batch, run = %run, "the integration checks of a batch failed; its members integrate one at a time");
                let alone = self.one_at_a_time(actor, automatic, members).await?;
                self.end_turn(actor, batch, BatchOutcome::OneAtATime, Some(run), &alone)
                    .await?;
                Ok(Some(BatchIntegration::OneAtATime {
                    run,
                    members: alone,
                }))
            }
        }
    }

    /// Integrates each member on its own, in order, under the mutex the caller holds; a member that needs one that did not integrate is left for its own rules.
    async fn one_at_a_time(
        &self,
        actor: &Actor,
        automatic: bool,
        members: Vec<MemberPlan>,
    ) -> Result<Vec<MemberIntegration>, ReviewError> {
        let mut left: BTreeSet<TaskId> = BTreeSet::new();
        let mut done = Vec::new();
        for member in members {
            if member.waits_for.iter().any(|task| left.contains(task)) {
                left.insert(member.task);
                continue;
            }
            let request = IntegrationRequest::Proposal {
                proposal: member.proposal,
                expected_version: Some(member.version),
                automatic,
            };
            let outcome = match self.integrate_locked(actor, request).await {
                Ok(Integrated::Succeeded(revision)) => IntegrationOutcome::Succeeded { revision },
                Ok(Integrated::Conflicted(conflicts)) => IntegrationOutcome::Conflicted(conflicts),
                Ok(Integrated::ChecksFailed(run)) => IntegrationOutcome::ChecksFailed { run },
                Ok(Integrated::Held) => {
                    left.insert(member.task);
                    continue;
                }
                Err(error) => {
                    tracing::warn!(proposal = %member.proposal, code = error.log_code(), "a member of a batch did not integrate on its own");
                    left.insert(member.task);
                    continue;
                }
            };
            if !matches!(outcome, IntegrationOutcome::Succeeded { .. }) {
                left.insert(member.task);
            }
            done.push(MemberIntegration {
                proposal: member.proposal,
                outcome,
            });
        }
        Ok(done)
    }

    /// Records the end of a batch's turn that published nothing together, with `review.batch_integrated` naming the members that integrated on their own.
    async fn end_turn(
        &self,
        actor: &Actor,
        batch: ReviewBatchId,
        outcome: BatchOutcome,
        run: Option<CheckRunId>,
        members: &[MemberIntegration],
    ) -> Result<(), ReviewError> {
        let published: Vec<Value> = members
            .iter()
            .filter_map(|member| match member.outcome {
                IntegrationOutcome::Succeeded { revision } => Some(json!({
                    "proposal": member.proposal.to_string(),
                    "revision": revision.to_string(),
                })),
                _ => None,
            })
            .collect();
        let label = actor.event_actor();
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                if batches::end_batch_integration(tx, batch, outcome, run, now).await? {
                    let event = turn_event(&label, batch, outcome, run, &published)?;
                    buffer.append(tx, &event, now).await?;
                }
                Ok::<_, ReviewError>(())
            })
            .await?;
        self.bus.publish(buffer);
        Ok(())
    }

    /// Stores the request of the batch with `integration.queued`, then runs the batch's turn in the background after the integrations ahead of it. A second request while one waits is refused at once.
    async fn queue_batch(&self, actor: &Actor, batch: ReviewBatchId) -> Result<(), ReviewError> {
        let now = self.clock.now();
        let label = actor.event_actor();
        let mut buffer = EventBuffer::new();
        let id = self
            .store
            .write(async |tx| {
                if batches::batch_request_waiting(tx.conn(), batch).await? {
                    return Err(already_queued_batch(batch).into());
                }
                let id = batches::insert_batch_request(tx, batch, label.as_str(), now).await?;
                let event = NewEvent::new(types::INTEGRATION_QUEUED, label.clone())?
                    .data(json!({"batch": batch.to_string()}))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, ReviewError>(id)
            })
            .await?;
        self.bus.publish(buffer);
        self.serve_batch_request(id, actor.clone(), batch);
        Ok(())
    }

    /// Serves one stored batch request in its turn and ends it with `integration.unqueued`. A refusal the turn meets goes to the caller as a message from fold; each member's integration sends its own messages.
    fn serve_batch_request(&self, id: i64, actor: Actor, batch: ReviewBatchId) {
        let service = self.clone();
        let ticket = self.checks.ticket();
        self.checks.spawn(async move {
            ticket.turn().await;
            let turn = {
                let _serial = service.integration.lock().await;
                service.batch_turn_locked(&actor, batch, false).await
            };
            let untold = match turn {
                Ok(Some(_)) => None,
                Ok(None) => Some(ReviewError::from(held_batch_refusal(batch))),
                Err(error) => {
                    tracing::info!(batch = %batch, code = error.log_code(), "a queued batch integration was refused");
                    Some(error)
                }
            };
            if let Some(error) = untold
                && let Err(failed) = service.tell_batch_refusal(&actor, batch, &error).await
            {
                tracing::warn!(batch = %batch, code = failed.log_code(), "the refusal of a queued batch integration could not be told");
            }
            if let Err(error) = service.end_batch_request(id, batch).await {
                tracing::warn!(batch = %batch, code = error.log_code(), "a queued batch integration could not end; the next daemon start serves it again");
            }
            drop(ticket);
        });
    }

    async fn end_batch_request(&self, id: i64, batch: ReviewBatchId) -> Result<(), ReviewError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                if !batches::end_batch_request(tx, id, now).await? {
                    return Ok(());
                }
                let event = NewEvent::new(types::INTEGRATION_UNQUEUED, EventActor::fold())?
                    .data(json!({"batch": batch.to_string()}))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, ReviewError>(())
            })
            .await?;
        self.bus.publish(buffer);
        Ok(())
    }

    /// Queues again, in their order, the batch integrations a stopped daemon left waiting. Returns how many were queued.
    pub(super) async fn requeue_batch_requests(&self) -> Result<usize, ReviewError> {
        let mut queued = 0;
        for request in batches::waiting_batch_requests(self.store.read()).await? {
            let Some(actor) = queued_actor(&request.actor) else {
                self.end_batch_request(request.id, request.batch).await?;
                continue;
            };
            self.serve_batch_request(request.id, actor, request.batch);
            queued += 1;
        }
        Ok(queued)
    }

    /// Tells the agent or the human that queued a batch's integration why it did not happen.
    async fn tell_batch_refusal(
        &self,
        actor: &Actor,
        batch: ReviewBatchId,
        error: &ReviewError,
    ) -> Result<(), ReviewError> {
        let to = match actor {
            Actor::Agent(agent) => Recipient {
                target: Target::Worker(agent.worker.clone()),
                addressed_as: AddressedAs::Worker,
            },
            Actor::Human { name } => Recipient {
                target: Target::Human(name.clone()),
                addressed_as: AddressedAs::Human,
            },
            Actor::System => return Ok(()),
        };
        let (ReviewError::Fold(refusal) | ReviewError::Task(TaskError::Fold(refusal))) = error
        else {
            return Ok(());
        };
        let Some(first) = batches::members(self.store.read(), batch)
            .await?
            .into_iter()
            .next()
        else {
            return Ok(());
        };
        let body = MessageBody::parse(format!(
            "fold did not integrate {batch}: {}. {}",
            refusal.message(),
            refusal.hint().unwrap_or_default()
        ))?;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                send_from_fold_in_tx(tx, &mut buffer, vec![to], (first.task, None), &body, now)
                    .await
                    .map_err(ReviewError::from)
            })
            .await?;
        self.bus.publish(buffer);
        Ok(())
    }
}

/// `review.batch_integrated`: how the batch's turn ended and the members it published, each with its revision.
fn turn_event(
    label: &EventActor,
    batch: ReviewBatchId,
    outcome: BatchOutcome,
    run: Option<CheckRunId>,
    members: &[Value],
) -> Result<NewEvent, fold_core::EventError> {
    NewEvent::new(types::REVIEW_BATCH_INTEGRATED, label.clone())?.data(json!({
        "batch": batch.to_string(),
        "outcome": outcome.as_str(),
        "run": run.map(|run| run.to_string()),
        "members": members,
    }))
}

/// The answer to a batch's integration that main's invalid checks file holds.
fn held_batch_refusal(batch: ReviewBatchId) -> FoldError {
    FoldError::new(
        codes::CHECKS_FILE_INVALID,
        format!("fold-checks.toml on main is not valid, so fold did not integrate {batch}"),
    )
    .with_details([
        ("reason", json!("main")),
        ("batch", json!(batch.to_string())),
    ])
    .with_hint("integrate the batch again once fold-checks.toml is fixed on main; Needs you shows the file and the line")
}

/// The refusal of a second request to integrate a batch whose integration waits in the queue already.
fn already_queued_batch(batch: ReviewBatchId) -> FoldError {
    FoldError::new(
        codes::INVALID_REQUEST,
        format!("an integration of {batch} is already queued"),
    )
    .with_details([
        ("reason", json!("integration_queued")),
        ("batch", json!(batch.to_string())),
    ])
    .with_hint("fold integrates it in its turn and sends each member's result as a message")
}
