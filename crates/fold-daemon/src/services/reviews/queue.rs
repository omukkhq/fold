use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::Duration;

use fold_core::policy::GateRefusal;
use fold_core::reviews::ProposalStanding;
use fold_core::reviews::checks::Stage;
use fold_core::{HumanName, ProposalId, ReviewBatchId, TaskId, TaskStatus, Timestamp};
use fold_store::StoreError;
use fold_store::reviews::{self, checks};

use super::conflicts::{ConflictedReason, conflicted_reason};
use super::integrate::{AutomaticSkip, AutomaticTurn};
use super::{ReviewError, ReviewService};
use crate::services::projects::load_config;
use crate::services::tasks::{Board, TaskService};

/// How many of the newest passed integration runs the start estimate averages.
pub const ESTIMATE_RUNS: u32 = 10;

/// What takes one turn of the automatic integration: a proposal on its own, or a submitted review batch whose ready members integrate together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueueItem {
    Proposal(ProposalId),
    Batch(ReviewBatchId),
}

impl fmt::Display for QueueItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueueItem::Proposal(proposal) => proposal.fmt(f),
            QueueItem::Batch(batch) => batch.fmt(f),
        }
    }
}

/// A place in the automatic integration queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuePlace {
    /// 1 is the next turn, or the turn that integrates now.
    pub position: u32,
    /// Its integration checks run now.
    pub integrating_now: bool,
    /// The item one place ahead; `None` at position 1.
    pub ahead: Option<QueueItem>,
    /// When its turn starts by the average of the recent integration runs: the start of the run for the item that integrates now, `None` without a passed integration run to average.
    pub estimated_start: Option<Timestamp>,
}

/// Why the automatic integration does not take an approved open proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitReason {
    /// `[integration].automatic` is false: a reviewer, the review lead, the main agent or a human integrates it.
    AutomaticOff,
    /// The project's `config.toml` is invalid, so the job skips every run.
    ConfigInvalid,
    /// main's `fold-checks.toml` is invalid, so every integration waits for a revision of main that fixes it.
    MainChecksInvalid,
    /// The proposal is flagged with a conflict; the reviewer that fixes it sends a new version.
    Conflict,
    /// Its integration checks failed; the proposal is with its author until a new version.
    ChecksFailed,
    /// A lane member behind the front of its lane.
    Lane { lane: String, position: u32 },
    /// Its lane moved since its version's base and names a `refresh`: the reviewer that redoes the step sends a new version.
    LaneRefresh { lane: String },
    /// A review batch holds it back to integrate with the other members, or the batch is not ready for its turn.
    Batch { batch: ReviewBatchId },
    /// fold could not run its integration checks on this version and main; a caller's integrate or a move of main tries again.
    FoldFailedChecks,
    /// The author's update or a reviewer's fix runs the checks of the next version.
    NextVersionRun,
    /// Prerequisite tasks that are not Done, in number order.
    Prerequisites {
        unfinished: Vec<(TaskId, TaskStatus)>,
    },
    /// The task's human review mark waits for a human approval, from the named human when the mark names one.
    HumanReview { reviewer: Option<HumanName> },
    /// The latest approval does not count for integration, such as one by an agent that does not decide reviews.
    NotApproved,
}

impl WaitReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            WaitReason::AutomaticOff => "automatic_off",
            WaitReason::ConfigInvalid => "config_invalid",
            WaitReason::MainChecksInvalid => "main_checks_invalid",
            WaitReason::Conflict => "conflict",
            WaitReason::ChecksFailed => "checks_failed",
            WaitReason::Lane { .. } => "lane",
            WaitReason::LaneRefresh { .. } => "lane_refresh",
            WaitReason::Batch { .. } => "batch",
            WaitReason::FoldFailedChecks => "fold_failed_checks",
            WaitReason::NextVersionRun => "next_version_run",
            WaitReason::Prerequisites { .. } => "prerequisites",
            WaitReason::HumanReview { .. } => "human_review",
            WaitReason::NotApproved => "not_approved",
        }
    }
}

/// Where an approved open proposal stands for the automatic integration: exactly one of a place or a reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueStanding {
    /// `batch` is the review batch it integrates with, whose place it shares.
    Queued {
        place: QueuePlace,
        batch: Option<ReviewBatchId>,
    },
    Waits(WaitReason),
}

/// The automatic integration queue of a project at one moment: every approved open proposal with its standing, in number order. A member of a ready review batch carries the batch's place.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntegrationQueue {
    pub proposals: Vec<(ProposalId, QueueStanding)>,
}

impl IntegrationQueue {
    /// The standing of `proposal`; `None` for a proposal that is not open or whose latest verdict is not an approval.
    pub fn standing(&self, proposal: ProposalId) -> Option<&QueueStanding> {
        self.proposals
            .iter()
            .find(|(id, _)| *id == proposal)
            .map(|(_, standing)| standing)
    }
}

/// The estimated start of each of `count` items in queue order. `running_since` is the start of an integration run that goes now; when `first_runs` the first item is that run, so its start is the run's own. Each item that waits starts one average duration after the one before it, and the first that waits starts once the running run is due to end, never before `now`. Without durations nothing waits for a known time, so only the running item has a start.
pub fn estimate_starts(
    now: Timestamp,
    durations: &[u64],
    running_since: Option<Timestamp>,
    first_runs: bool,
    count: usize,
) -> Vec<Option<Timestamp>> {
    let average = (!durations.is_empty()).then(|| {
        let total: u128 = durations.iter().map(|&ms| u128::from(ms)).sum();
        let len = u128::try_from(durations.len()).unwrap_or(u128::MAX);
        Duration::from_millis(u64::try_from(total / len).unwrap_or(u64::MAX))
    });
    let mut starts = Vec::with_capacity(count);
    let mut waiting = count;
    if first_runs && count > 0 {
        starts.push(running_since);
        waiting -= 1;
    }
    let Some(average) = average else {
        starts.extend(std::iter::repeat_n(None, waiting));
        return starts;
    };
    let mut next = match running_since.and_then(|since| since.checked_add(average)) {
        Some(due) if due > now => Some(due),
        _ => Some(now),
    };
    for _ in 0..waiting {
        starts.push(next);
        next = next.and_then(|at| at.checked_add(average));
    }
    starts
}

/// The places of `order`, the items in the order the job takes them, with their estimated starts.
pub fn places(
    order: &[QueueItem],
    integrating: Option<QueueItem>,
    now: Timestamp,
    durations: &[u64],
    running_since: Option<Timestamp>,
) -> Vec<(QueueItem, QueuePlace)> {
    let first_runs = integrating.is_some() && order.first() == integrating.as_ref();
    let starts = estimate_starts(now, durations, running_since, first_runs, order.len());
    order
        .iter()
        .zip(starts)
        .enumerate()
        .map(|(index, (item, estimated_start))| {
            let place = QueuePlace {
                position: u32::try_from(index).unwrap_or(u32::MAX).saturating_add(1),
                integrating_now: first_runs && index == 0,
                ahead: index.checked_sub(1).and_then(|at| order.get(at)).copied(),
                estimated_start,
            };
            (*item, place)
        })
        .collect()
}

fn skip_reason(skip: AutomaticSkip) -> WaitReason {
    match skip {
        AutomaticSkip::Conflicted => WaitReason::Conflict,
        AutomaticSkip::ChecksFailed => WaitReason::ChecksFailed,
        AutomaticSkip::FoldFailed => WaitReason::FoldFailedChecks,
        AutomaticSkip::NextVersionRun => WaitReason::NextVersionRun,
        AutomaticSkip::Gate(refusal) => gate_reason(refusal),
        AutomaticSkip::Held | AutomaticSkip::Missing => WaitReason::NotApproved,
    }
}

fn gate_reason(refusal: GateRefusal) -> WaitReason {
    match refusal {
        GateRefusal::PrerequisitesNotDone { unfinished } => {
            WaitReason::Prerequisites { unfinished }
        }
        GateRefusal::HumanApprovalMissing { reviewer } => WaitReason::HumanReview { reviewer },
        GateRefusal::ProposalNotOpen
        | GateRefusal::StaleVersion { .. }
        | GateRefusal::IntegratorRequired
        | GateRefusal::AuthorCannotIntegrate
        | GateRefusal::ChangesRequested
        | GateRefusal::NotApproved
        | GateRefusal::AgentApprovalRequired => WaitReason::NotApproved,
    }
}

impl ReviewService {
    /// The automatic integration queue as the job would take it now: the ready review batches first, in number order, then the ready proposals in number order, as [`ReviewService::integrate_ready`] takes them, with the item whose integration checks run now moved to the front. Every other approved open proposal gets the reason the job skips it. A read: it writes nothing.
    pub async fn integration_queue(&self) -> Result<IntegrationQueue, ReviewError> {
        let now = self.clock.now();
        let holds = self.batch_holds().await?;
        let (turns, standings) = self
            .automatic_turns(&holds, &HashSet::new(), &HashSet::new(), false)
            .await?;
        let mut read = self.store.read().begin().await.map_err(StoreError::from)?;
        let held_by_main = checks::held_by_main(&mut *read).await?.is_some();
        let running = checks::running_runs(&mut *read)
            .await?
            .into_iter()
            .find(|run| run.stage == Stage::Integration);
        let durations = checks::recent_integration_run_durations(&mut *read, ESTIMATE_RUNS).await?;
        read.commit().await.map_err(StoreError::from)?;
        let blocked = match load_config(self.workspaces.paths()).await {
            Ok(config) if !config.integration.automatic => Some(WaitReason::AutomaticOff),
            Ok(_) if held_by_main => Some(WaitReason::MainChecksInvalid),
            Ok(_) => None,
            Err(_) => Some(WaitReason::ConfigInvalid),
        };
        let mut order: Vec<QueueItem> = turns
            .into_iter()
            .map(|turn| match turn {
                AutomaticTurn::Batch(batch) => QueueItem::Batch(batch),
                AutomaticTurn::Proposal { proposal, .. } => QueueItem::Proposal(proposal),
            })
            .collect();
        let integrating = running
            .as_ref()
            .and_then(|run| match (run.batch, run.proposal) {
                (Some(batch), _) => Some(QueueItem::Batch(batch)),
                (None, Some(proposal)) => Some(QueueItem::Proposal(proposal)),
                (None, None) => None,
            });
        if let Some(at) = integrating.and_then(|item| order.iter().position(|x| *x == item)) {
            let item = order.remove(at);
            order.insert(0, item);
        }
        if blocked.is_some() {
            order.clear();
        }
        let placed: HashMap<QueueItem, QueuePlace> = places(
            &order,
            integrating,
            now,
            &durations,
            running.map(|run| run.created_at),
        )
        .into_iter()
        .collect();
        let lane_of = |proposal: ProposalId| {
            holds
                .lanes
                .iter()
                .find(|place| place.proposal == proposal && place.held())
        };
        let queued = |item: QueueItem, batch: Option<ReviewBatchId>, otherwise: WaitReason| match (
            placed.get(&item),
            &blocked,
        ) {
            (Some(place), _) => QueueStanding::Queued {
                place: place.clone(),
                batch,
            },
            (None, Some(reason)) => QueueStanding::Waits(reason.clone()),
            (None, None) => QueueStanding::Waits(otherwise),
        };
        let proposals = standings
            .into_iter()
            .map(|(proposal, standing)| {
                let standing = match standing {
                    Ok(_) => queued(QueueItem::Proposal(proposal), None, WaitReason::NotApproved),
                    Err(AutomaticSkip::Held) => match holds.by_batch.get(&proposal) {
                        Some(&batch) => queued(
                            QueueItem::Batch(batch),
                            Some(batch),
                            WaitReason::Batch { batch },
                        ),
                        None => QueueStanding::Waits(match lane_of(proposal) {
                            Some(place) => WaitReason::Lane {
                                lane: place.lane.clone(),
                                position: place.ahead.saturating_add(1),
                            },
                            None => WaitReason::NotApproved,
                        }),
                    },
                    Err(skip) => QueueStanding::Waits(skip_reason(skip)),
                };
                (proposal, standing)
            })
            .collect();
        let proposals = self.name_refreshes(proposals).await?;
        Ok(IntegrationQueue { proposals })
    }

    /// Each proposal of `proposals` that waits for a conflict which is its lane's refresh waits for [`WaitReason::LaneRefresh`] instead.
    async fn name_refreshes(
        &self,
        proposals: Vec<(ProposalId, QueueStanding)>,
    ) -> Result<Vec<(ProposalId, QueueStanding)>, ReviewError> {
        if !proposals
            .iter()
            .any(|(_, standing)| *standing == QueueStanding::Waits(WaitReason::Conflict))
        {
            return Ok(proposals);
        }
        let mut read = self.store.read().begin().await.map_err(StoreError::from)?;
        let mut named = Vec::with_capacity(proposals.len());
        for (proposal, standing) in proposals {
            let standing = match standing {
                QueueStanding::Waits(WaitReason::Conflict) => {
                    let version = reviews::get_proposal(&mut *read, proposal)
                        .await?
                        .map(|row| row.current_version);
                    let reason = match version {
                        Some(version) => conflicted_reason(&mut read, proposal, version).await?,
                        None => None,
                    };
                    QueueStanding::Waits(match reason {
                        Some(ConflictedReason::LaneRefresh { lane, .. }) => {
                            WaitReason::LaneRefresh { lane }
                        }
                        _ => WaitReason::Conflict,
                    })
                }
                other => other,
            };
            named.push((proposal, standing));
        }
        read.commit().await.map_err(StoreError::from)?;
        Ok(named)
    }
}

/// The board with the automatic integration queue, read once for every proposal on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedBoard {
    pub board: Board,
    pub queue: IntegrationQueue,
}

impl ReviewService {
    /// The board of `tasks` with one [`ReviewService::integration_queue`] read, so each card's proposal takes its standing from the same queue as every other card. Only an approved open proposal has a standing, and such a card stands `approved` or `conflict`, so a board without one skips the read.
    pub async fn board_with_queue(&self, tasks: &TaskService) -> Result<QueuedBoard, ReviewError> {
        let board = tasks.board().await?;
        let any_approved = board
            .columns
            .iter()
            .flat_map(|column| &column.cards)
            .filter_map(|card| card.proposal.as_ref())
            .any(|proposal| {
                matches!(
                    proposal.standing,
                    ProposalStanding::Approved | ProposalStanding::Conflict
                )
            });
        let queue = if any_approved {
            self.integration_queue().await?
        } else {
            IntegrationQueue::default()
        };
        Ok(QueuedBoard { board, queue })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis).unwrap()
    }

    fn p(number: u32) -> QueueItem {
        QueueItem::Proposal(ProposalId::from_number(number).unwrap())
    }

    #[test]
    fn without_history_no_waiting_item_has_a_start() {
        assert_eq!(estimate_starts(at(100), &[], None, false, 2), [None, None]);
        assert_eq!(
            estimate_starts(at(100), &[], Some(at(40)), true, 2),
            [Some(at(40)), None],
            "the running item started at its run"
        );
        assert!(estimate_starts(at(100), &[10], None, false, 0).is_empty());
    }

    #[test]
    fn waiting_items_start_one_average_apart_from_now() {
        assert_eq!(
            estimate_starts(at(1_000), &[100, 300], None, false, 3),
            [Some(at(1_000)), Some(at(1_200)), Some(at(1_400))]
        );
    }

    #[test]
    fn the_first_waiting_item_starts_when_the_running_run_is_due() {
        assert_eq!(
            estimate_starts(at(1_000), &[200], Some(at(900)), true, 3),
            [Some(at(900)), Some(at(1_100)), Some(at(1_300))]
        );
        assert_eq!(
            estimate_starts(at(1_000), &[200], Some(at(100)), true, 2),
            [Some(at(100)), Some(at(1_000))],
            "a run past its due time lets the next one start now"
        );
        assert_eq!(
            estimate_starts(at(1_000), &[200], Some(at(900)), false, 1),
            [Some(at(1_100))],
            "a run outside the queue still goes first"
        );
    }

    #[test]
    fn places_count_from_one_and_name_the_item_ahead() {
        let order = [p(3), p(1), p(2)];
        let placed = places(&order, Some(p(3)), at(1_000), &[100], Some(at(950)));
        let positions: Vec<_> = placed
            .iter()
            .map(|(item, place)| (*item, place.position, place.integrating_now, place.ahead))
            .collect();
        assert_eq!(
            positions,
            [
                (p(3), 1, true, None),
                (p(1), 2, false, Some(p(3))),
                (p(2), 3, false, Some(p(1))),
            ]
        );
        assert_eq!(placed[1].1.estimated_start, Some(at(1_050)));
        let idle = places(&order, Some(p(9)), at(1_000), &[], Some(at(950)));
        assert!(idle.iter().all(|(_, place)| !place.integrating_now));
    }

    #[test]
    fn every_gate_refusal_has_a_reason() {
        let unfinished = vec![(TaskId::from_number(2).unwrap(), TaskStatus::Review)];
        assert_eq!(
            gate_reason(GateRefusal::PrerequisitesNotDone {
                unfinished: unfinished.clone()
            }),
            WaitReason::Prerequisites { unfinished }
        );
        assert_eq!(
            gate_reason(GateRefusal::HumanApprovalMissing { reviewer: None }).as_str(),
            "human_review"
        );
        assert_eq!(
            gate_reason(GateRefusal::NotApproved).as_str(),
            "not_approved"
        );
        assert_eq!(
            QueueItem::Batch(ReviewBatchId::from_number(4).unwrap()).to_string(),
            "rb-4"
        );
    }
}
