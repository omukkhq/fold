use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use fold_core::claims::is_stale;
use fold_core::events::types;
use fold_core::reviews::ProposalStanding;
use fold_core::status::status_reason;
use fold_core::tasks::areas::{FeedbackReader, areas_conflict, feedback_readers};
use fold_core::{
    AgentName, AreaName, ChangesetId, ChecklistName, DependencyOrigin, Event, HoldMark, LabelName,
    ProposalId, RevisionId, TaskId, TaskKind, TaskStatus, TaskTitle, Timestamp, WorkerName,
    WorkspaceId,
};
use fold_store::reviews::{self, ProposalSummary, ReviewEntry};
use fold_store::tasks::checklists::{self, ItemContent};
use fold_store::tasks::claims;
use fold_store::tasks::reports;
use fold_store::tasks::{self, BoardSnapshot};
use fold_store::workspaces::{self, Checkpoint};
use fold_store::{SqliteConnection, StoreError, agents, events};

pub use fold_store::tasks::{ChecklistCounts, TaskRef};

use super::attention::{AttentionItem, attention, attention_in, stalled_tasks};
use super::{
    ChecklistItemView, ClaimView, HumanReviewMark, LabelView, REPORT_LIMIT, ReportView, TaskError,
    TaskService, TaskView, area_labels, label_view, task_not_found, view,
};
use crate::live::LiveCheckRun;
use crate::services::reviews::{ProposalPlan, proposal_plans};

/// How many Done cards the board returns: the most recently completed ones. The Done column's `total` counts them all.
pub const BOARD_DONE_LIMIT: usize = 20;

/// How many history rows a task read returns.
pub const HISTORY_LIMIT: usize = 50;

/// The live task workspace as the task shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceInfo {
    /// Relative to the project root.
    pub directory: String,
    pub base: RevisionId,
    pub main: RevisionId,
    /// How many revisions main moved since `base`.
    pub behind: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangesetSummary {
    pub id: ChangesetId,
    pub message: String,
    pub created_at: Timestamp,
}

/// The newest proposal of the task that is open or integrated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskProposal {
    pub id: ProposalId,
    pub title: String,
    pub standing: ProposalStanding,
    /// The worker that holds the unreleased review claim.
    pub review_claim: Option<String>,
    /// The files the current version changes.
    pub files: u32,
    pub version: u32,
    /// The integration checks of the current version failed.
    pub checks_failed: bool,
    /// Newest first.
    pub reviews: Vec<ReviewEntry>,
}

/// The integration that published the task's proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskIntegration {
    pub revision: RevisionId,
    pub proposal: ProposalId,
    /// A worker name or `human:<name>`.
    pub by: String,
    pub at: Timestamp,
    /// The Git commit the revision became; `None` until it is written.
    pub commit: Option<String>,
}

/// The last tracked edit in the task's live workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    /// The file, when exactly one is changed.
    pub path: Option<String>,
    pub at: Timestamp,
}

/// The proposal pill of a board card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardProposal {
    pub id: ProposalId,
    pub standing: ProposalStanding,
    /// The highest version number.
    pub version: u32,
    /// The integration checks of the current version failed.
    pub checks_failed: bool,
    /// The revision an integrated proposal became.
    pub revision: Option<RevisionId>,
    /// The review plan the open proposal waits in.
    pub review_plan: Option<ProposalPlan>,
}

/// A checklist item with the title and status of the task it links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailItem {
    pub item: ChecklistItemView,
    pub linked: Option<TaskRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailChecklist {
    pub name: ChecklistName,
    pub items: Vec<DetailItem>,
}

/// One end of an edge as the task at the other end sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailEdge {
    pub task: TaskRef,
    pub origin: DependencyOrigin,
}

/// Everything one task shows, with public ids only.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskDetail {
    pub task: TaskView,
    pub checklists: Vec<DetailChecklist>,
    pub progress: ChecklistCounts,
    /// The tasks this task needs.
    pub needs: Vec<DetailEdge>,
    /// The tasks that need this task.
    pub needed_by: Vec<DetailEdge>,
    pub claim: Option<ClaimView>,
    pub workspace: Option<WorkspaceId>,
    pub attention: Vec<AttentionItem>,
    /// When the human review mark was last set: the time of the newest `task.human_review_set` event. `None` without a mark.
    pub human_review_set_at: Option<Timestamp>,
    /// The rule of the status calculation that decided the status, named with the ids it used.
    pub status_reason: String,
    pub workspace_info: Option<WorkspaceInfo>,
    pub activity: Option<Activity>,
    /// See [`TaskService::edits_after_version`].
    pub edits_after_version: Option<bool>,
    /// See [`TaskService::check_run`].
    pub check_run: Option<LiveCheckRun>,
    /// Of the live workspace, or of the latest one when none is live; newest first.
    pub changesets: Vec<ChangesetSummary>,
    /// Of the live workspace, or of the latest one when none is live; pruned ones left out, newest first.
    pub checkpoints: Vec<Checkpoint>,
    pub proposal: Option<TaskProposal>,
    pub integration: Option<TaskIntegration>,
    /// The task worker's reports on a task without a proposal, newest first, at most [`REPORT_LIMIT`]; the first is the open one while the task waits for a lead check.
    pub reports: Vec<ReportView>,
    /// The task's newest [`HISTORY_LIMIT`] events, newest first.
    pub history: Vec<Event>,
    /// The task can get a blocker or a new reason now: it is not archived and no agent has started it (`policy::can_hold_task`). Who may set it is decided by the command.
    pub blocker_allowed: bool,
}

/// The task graph as `graph` reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskGraph {
    pub tasks: Vec<GraphTask>,
    /// Every Done task that is not archived, also when `tasks` leaves them out.
    pub done_count: usize,
}

/// One node of the task graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphTask {
    pub id: TaskId,
    pub title: TaskTitle,
    pub status: TaskStatus,
    pub blocked: bool,
    pub kind: TaskKind,
    pub labels: Vec<LabelView>,
    /// Every direct prerequisite, Done or not.
    pub needs: Vec<TaskId>,
    pub holder: Option<WorkerName>,
    /// `None` while the task is not on hold.
    pub hold: Option<HoldMark>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardCard {
    pub id: TaskId,
    pub title: TaskTitle,
    pub status: TaskStatus,
    pub blocked: bool,
    pub kind: TaskKind,
    /// The note of the task worker's open report on a task without a proposal: the card waits for a lead check.
    pub report: Option<String>,
    pub labels: Vec<LabelView>,
    /// The labels hold two or more areas' labels, as [`TaskView::areas_conflict`] says.
    pub areas_conflict: bool,
    pub checklist: ChecklistCounts,
    /// Every direct prerequisite.
    pub needs: Vec<TaskId>,
    /// The direct prerequisites that are not Done.
    pub blocked_by: Vec<TaskId>,
    /// How many direct dependents that are not Done wait for this task; 0 once it is Done.
    pub blocks: u32,
    pub holder: Option<WorkerName>,
    pub workspace: Option<WorkspaceId>,
    pub human_review: Option<HumanReviewMark>,
    /// `None` while the task is not on hold.
    pub hold: Option<HoldMark>,
    /// How many messages name the task.
    pub messages: u32,
    pub completed_at: Option<Timestamp>,
    pub attention: Vec<AttentionItem>,
    pub proposal: Option<CardProposal>,
    pub activity: Option<Activity>,
    /// See [`TaskService::edits_after_version`].
    pub edits_after_version: Option<bool>,
    /// See [`TaskService::check_run`].
    pub check_run: Option<LiveCheckRun>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardColumn {
    pub status: TaskStatus,
    pub cards: Vec<BoardCard>,
    /// Every task with this status; more than `cards.len()` only in a Done column cut at [`BOARD_DONE_LIMIT`].
    pub total: u32,
}

/// The five columns in board order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    pub columns: Vec<BoardColumn>,
}

fn standing(summary: &ProposalSummary) -> ProposalStanding {
    ProposalStanding::of(summary.integrated, summary.conflicted, summary.verdict)
}

fn counts_of(items: &[checklists::ItemRow]) -> ChecklistCounts {
    let mut counts = ChecklistCounts::default();
    for item in items {
        let done = u32::from(item.done_at.is_some());
        match item.content {
            ItemContent::Step(_) => {
                counts.steps_total += 1;
                counts.steps_done += done;
            }
            ItemContent::Linked(_) => {
                counts.linked_total += 1;
                counts.linked_done += done;
            }
        }
    }
    counts
}

fn total(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// Read models shared by the MCP tools and the HTTP routes.
impl TaskService {
    /// One task with its checklists, both directions of its edges, its claim and its live workspace, read in one transaction. An archived task is found too. The lease is read only when the task has a claim, so a task without one stays readable under an invalid configuration.
    pub async fn get_view(&self, id: TaskId) -> Result<TaskDetail, TaskError> {
        let now = self.clock.now();
        let mut tx = self.store.read().begin().await.map_err(StoreError::from)?;
        let row = tasks::fetch(&mut tx, id)
            .await?
            .ok_or_else(|| task_not_found(id))?;
        let labels = tasks::labels_for_conn(&mut tx, id).await?;
        let areas = area_labels(&mut tx).await?;
        let names = checklists::checklists(&mut tx, id).await?;
        let items = checklists::items(&mut tx, id).await?;
        let edges = tasks::edges_of(&mut tx, id).await?;
        let claim = claims::unreleased_for_task(&mut *tx, id).await?;
        let facts = tasks::load_facts(&mut tx, &[id])
            .await?
            .pop()
            .ok_or_else(|| task_not_found(id))?
            .facts;
        let live = workspaces::live_task_workspace(&mut *tx, id).await?;
        let workspace = live.as_ref().map(|row| row.id);
        let main = workspaces::main_head(&mut *tx).await?;
        let info = match (&live, main) {
            (Some(live), Some(main)) => Some(WorkspaceInfo {
                directory: live.path.clone(),
                base: live.base,
                main,
                behind: main.number().saturating_sub(live.base.number()),
            }),
            _ => None,
        };
        let latest = match &live {
            Some(_) => None,
            None => workspaces::latest_task_workspace(&mut *tx, id).await?,
        };
        let changesets = match live.as_ref().or(latest.as_ref()) {
            Some(row) => workspaces::list_changesets(&mut *tx, row.id)
                .await?
                .into_iter()
                .rev()
                .map(|changeset| ChangesetSummary {
                    id: changeset.id,
                    message: changeset.message,
                    created_at: changeset.created_at,
                })
                .collect(),
            None => Vec::new(),
        };
        let checkpoints = match live.as_ref().or(latest.as_ref()) {
            Some(row) => {
                let mut checkpoints = workspaces::list_checkpoints(&mut *tx, row.id, false).await?;
                checkpoints.reverse();
                checkpoints
            }
            None => Vec::new(),
        };
        let blocker_allowed = row.archived_at.is_none()
            && fold_core::policy::can_hold_task(&fold_core::policy::TaskStart {
                status: row.status,
                claimed: claim.is_some(),
                workspace: workspace.is_some(),
                open_proposal: reviews::open_for_task(&mut *tx, id).await?.is_some(),
            })
            .is_ok();
        let summary = reviews::proposal_summaries(&mut tx, Some(id)).await?.pop();
        let integration = summary.as_ref().and_then(|summary| {
            summary.integration.as_ref().map(|record| TaskIntegration {
                revision: record.revision,
                proposal: summary.id,
                by: record.actor.clone(),
                at: record.at,
                commit: record.commit.clone(),
            })
        });
        let proposal = match summary {
            Some(summary) => Some(TaskProposal {
                standing: standing(&summary),
                title: summary.title.clone(),
                review_claim: summary.review_claim.clone(),
                files: summary.files,
                version: summary.version,
                checks_failed: summary.checks_failed,
                reviews: reviews::review_entries(&mut tx, summary.id).await?,
                id: summary.id,
            }),
            None => None,
        };
        let reports = reports::for_task(&mut *tx, id, REPORT_LIMIT)
            .await?
            .into_iter()
            .map(ReportView::from)
            .collect();
        let history = events::newest_for_task(&mut *tx, id, HISTORY_LIMIT).await?;
        let human_review_set_at = match row.human_review {
            Some(_) => {
                events::newest_at_for_task(&mut *tx, id, types::TASK_HUMAN_REVIEW_SET).await?
            }
            None => None,
        };

        let needs: Vec<(TaskId, DependencyOrigin)> = edges
            .iter()
            .filter(|edge| edge.task == id)
            .map(|edge| (edge.needs, edge.origin))
            .collect();
        let needed_by: Vec<(TaskId, DependencyOrigin)> = edges
            .iter()
            .filter(|edge| edge.needs == id)
            .map(|edge| (edge.task, edge.origin))
            .collect();
        let mut related: BTreeSet<TaskId> = needs.iter().chain(&needed_by).map(|e| e.0).collect();
        related.extend(items.iter().filter_map(|item| match item.content {
            ItemContent::Linked(child) => Some(child),
            ItemContent::Step(_) => None,
        }));
        let related: Vec<TaskId> = related.into_iter().collect();
        let refs: BTreeMap<TaskId, TaskRef> = tasks::task_refs(&mut tx, &related)
            .await?
            .into_iter()
            .map(|task| (task.id, task))
            .collect();
        let stalled = stalled_tasks(self.live.as_ref());
        let attention = attention_in(&mut tx, &[id], &stalled)
            .await?
            .remove(&id)
            .unwrap_or_default();
        tx.commit().await.map_err(StoreError::from)?;

        let edge = |(task, origin): &(TaskId, DependencyOrigin)| {
            refs.get(task).map(|task| DetailEdge {
                task: task.clone(),
                origin: *origin,
            })
        };
        let progress = counts_of(&items);
        let mut checklists: Vec<DetailChecklist> = names
            .into_iter()
            .map(|name| DetailChecklist {
                name,
                items: Vec::new(),
            })
            .collect();
        for item in items {
            let linked = match item.content {
                ItemContent::Linked(child) => refs.get(&child).cloned(),
                ItemContent::Step(_) => None,
            };
            if let Some(list) = checklists
                .iter_mut()
                .find(|list| list.name == item.checklist)
            {
                list.items.push(DetailItem {
                    item: item.into(),
                    linked,
                });
            }
        }
        let claim = match claim {
            Some(claim) => Some(ClaimView {
                task: id,
                stale: is_stale(claim.renewed_at, now, self.lease_minutes().await?),
                holder: claim.worker,
                acquired_at: claim.acquired_at,
                renewed_at: claim.renewed_at,
            }),
            None => None,
        };
        let activity = self.activity_of(workspace, live.as_ref().and_then(|w| w.last_activity_at));
        let open = proposal
            .as_ref()
            .filter(|p| p.standing != ProposalStanding::Integrated)
            .map(|p| p.id);
        let edits_after_version = self.edits_after_version(workspace, open);
        let check_run = self.check_run(workspace);
        let status_reason = status_reason(&facts, workspace, open);
        Ok(TaskDetail {
            task: view(row, labels, &areas),
            checklists,
            progress,
            needs: needs.iter().filter_map(edge).collect(),
            needed_by: needed_by.iter().filter_map(edge).collect(),
            claim,
            workspace,
            attention,
            human_review_set_at,
            status_reason,
            workspace_info: info,
            activity,
            edits_after_version,
            check_run,
            changesets,
            checkpoints,
            proposal,
            integration,
            reports,
            history,
            blocker_allowed,
        })
    }

    /// The last edit of a live workspace: the time from the live state when it holds the workspace, else the stored column, and the file when the live state names exactly one changed.
    fn activity_of(
        &self,
        workspace: Option<WorkspaceId>,
        stored: Option<Timestamp>,
    ) -> Option<Activity> {
        let live = workspace.and_then(|id| self.live.as_ref()?.get(id));
        let at = live.as_ref().map(|w| w.view.last_activity_at).or(stored)?;
        let path = live.and_then(|w| match w.view.files.as_slice() {
            [only] => Some(only.path.clone()),
            _ => None,
        });
        Some(Activity { path, at })
    }

    /// Whether the files of the live workspace differ from the tree of the open proposal's newest version, as the live state compared them on its last scan: content, never times, so an automatic checkpoint of unchanged files is no edit. `None` without an open proposal, or while the live state does not hold the workspace or has not compared it with that proposal.
    fn edits_after_version(
        &self,
        workspace: Option<WorkspaceId>,
        open: Option<ProposalId>,
    ) -> Option<bool> {
        let open = open?;
        let live = workspace.and_then(|id| self.live.as_ref()?.get(id))?;
        if live.view.proposal != Some(open) {
            return None;
        }
        live.view.edits_after_version
    }

    /// The check run that goes in the live workspace, as the live state holds it. `None` while no run goes, and while the live state does not hold the workspace.
    fn check_run(&self, workspace: Option<WorkspaceId>) -> Option<LiveCheckRun> {
        let live = workspace.and_then(|id| self.live.as_ref()?.get(id))?;
        live.view.check_run
    }

    /// The readers a human's feedback on the task goes to, the default first, as [`feedback_readers`] chooses them, read in one read transaction.
    pub async fn readers(&self, id: TaskId) -> Result<Vec<FeedbackReader>, TaskError> {
        let mut tx = self.store.read().begin().await.map_err(StoreError::from)?;
        tasks::fetch(&mut tx, id)
            .await?
            .ok_or_else(|| task_not_found(id))?;
        let readers = readers_in(&mut tx, id).await?;
        tx.commit().await.map_err(StoreError::from)?;
        Ok(readers)
    }

    /// The tasks that are not archived, in number order, with their direct prerequisites and claim holders. Done tasks only when `include_done`; `done_count` counts them either way.
    pub async fn graph(&self, include_done: bool) -> Result<TaskGraph, TaskError> {
        let mut snapshot = tasks::board_snapshot(self.store.read()).await?;
        let needs = needs_by_task(&snapshot);
        let done_count = snapshot
            .tasks
            .iter()
            .filter(|row| row.status == TaskStatus::Done)
            .count();
        let tasks = std::mem::take(&mut snapshot.tasks)
            .into_iter()
            .filter(|row| include_done || row.status != TaskStatus::Done)
            .map(|row| GraphTask {
                needs: needs.get(&row.id).cloned().unwrap_or_default(),
                labels: labels_of(&mut snapshot, row.id),
                holder: snapshot.holders.remove(&row.id),
                id: row.id,
                title: row.title,
                status: row.status,
                blocked: row.blocked,
                kind: row.kind,
                hold: row.hold,
            })
            .collect();
        Ok(TaskGraph { tasks, done_count })
    }

    /// The five columns in board order. Backlog to Review hold every task in number order; Done holds the [`BOARD_DONE_LIMIT`] most recently completed tasks, newest first, and counts the rest in `total`.
    pub async fn board(&self) -> Result<Board, TaskError> {
        let mut snapshot = tasks::board_snapshot(self.store.read()).await?;
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let areas = area_labels(&mut conn).await?;
        let mut plans = proposal_plans(&mut conn).await?;
        drop(conn);
        let ids: Vec<TaskId> = snapshot.tasks.iter().map(|row| row.id).collect();
        let stalled = stalled_tasks(self.live.as_ref());
        let mut attention = attention(&self.store, &ids, &stalled).await?;
        let statuses: BTreeMap<TaskId, TaskStatus> = snapshot
            .tasks
            .iter()
            .map(|row| (row.id, row.status))
            .collect();
        let is_done = |task: &TaskId| statuses.get(task) == Some(&TaskStatus::Done);
        let needs = needs_by_task(&snapshot);
        let mut blocks: BTreeMap<TaskId, u32> = BTreeMap::new();
        for edge in &snapshot.edges {
            if !is_done(&edge.task) && !is_done(&edge.needs) {
                *blocks.entry(edge.needs).or_default() += 1;
            }
        }
        let mut columns: Vec<BoardColumn> = TaskStatus::ALL
            .iter()
            .map(|status| BoardColumn {
                status: *status,
                cards: Vec::new(),
                total: 0,
            })
            .collect();
        for row in std::mem::take(&mut snapshot.tasks) {
            let needs = needs.get(&row.id).cloned().unwrap_or_default();
            let labels = labels_of(&mut snapshot, row.id);
            let proposal = snapshot.proposals.get(&row.id).map(|summary| CardProposal {
                id: summary.id,
                standing: standing(summary),
                version: summary.version,
                checks_failed: summary.checks_failed,
                revision: summary.integration.as_ref().map(|record| record.revision),
                review_plan: plans.remove(&summary.id),
            });
            let edits_after_version = self.edits_after_version(
                snapshot.workspaces.get(&row.id).copied(),
                proposal
                    .as_ref()
                    .filter(|p| p.standing != ProposalStanding::Integrated)
                    .map(|p| p.id),
            );
            let card = BoardCard {
                areas_conflict: card_areas_conflict(&labels, &areas),
                blocked_by: needs
                    .iter()
                    .filter(|need| !is_done(need))
                    .copied()
                    .collect(),
                needs,
                blocks: blocks.get(&row.id).copied().unwrap_or(0),
                labels,
                checklist: snapshot
                    .checklists
                    .get(&row.id)
                    .copied()
                    .unwrap_or_default(),
                holder: snapshot.holders.remove(&row.id),
                workspace: snapshot.workspaces.get(&row.id).copied(),
                human_review: row.human_review,
                hold: row.hold,
                messages: snapshot.messages.get(&row.id).copied().unwrap_or(0),
                completed_at: snapshot.completions.get(&row.id).copied(),
                proposal,
                edits_after_version,
                check_run: self.check_run(snapshot.workspaces.get(&row.id).copied()),
                activity: self.activity_of(
                    snapshot.workspaces.get(&row.id).copied(),
                    snapshot.activity.get(&row.id).copied(),
                ),
                attention: attention.remove(&row.id).unwrap_or_default(),
                report: snapshot.reports.remove(&row.id),
                id: row.id,
                title: row.title,
                status: row.status,
                blocked: row.blocked,
                kind: row.kind,
            };
            if let Some(column) = columns.iter_mut().find(|c| c.status == card.status) {
                column.cards.push(card);
            }
        }
        for column in &mut columns {
            column.total = total(column.cards.len());
            if column.status == TaskStatus::Done {
                column
                    .cards
                    .sort_by_key(|card| Reverse((card.completed_at, card.id)));
                column.cards.truncate(BOARD_DONE_LIMIT);
            }
        }
        Ok(Board { columns })
    }
}

fn needs_by_task(snapshot: &BoardSnapshot) -> BTreeMap<TaskId, Vec<TaskId>> {
    let mut needs: BTreeMap<TaskId, Vec<TaskId>> = BTreeMap::new();
    for edge in &snapshot.edges {
        needs.entry(edge.task).or_default().push(edge.needs);
    }
    for list in needs.values_mut() {
        list.sort();
    }
    needs
}

fn card_areas_conflict(labels: &[LabelView], areas: &[(AreaName, LabelName)]) -> bool {
    let names: Vec<LabelName> = labels.iter().map(|label| label.name.clone()).collect();
    areas_conflict(&names, areas)
}

fn labels_of(snapshot: &mut BoardSnapshot, task: TaskId) -> Vec<LabelView> {
    snapshot
        .labels
        .remove(&task)
        .unwrap_or_default()
        .into_iter()
        .map(label_view)
        .collect()
}

/// The readers of a human's feedback on `task`, read on `conn`: the task listing and the feedback send choose them from the same rows.
pub(crate) async fn readers_in(
    conn: &mut SqliteConnection,
    task: TaskId,
) -> Result<Vec<FeedbackReader>, StoreError> {
    let labels: Vec<LabelName> = tasks::labels_for_conn(conn, task)
        .await?
        .into_iter()
        .map(|label| label.name)
        .collect();
    let areas: Vec<_> = agents::list_areas(&mut *conn)
        .await?
        .into_iter()
        .map(|area| (area.name, area.label, area.lead))
        .collect();
    let holder = claims::unreleased_for_task(&mut *conn, task)
        .await?
        .map(|claim| claim.worker);
    let main: BTreeSet<i64> = agents::all_tags(&mut *conn)
        .await?
        .into_iter()
        .filter(|(_, tag)| tag.is_main())
        .map(|(agent, _)| agent)
        .collect();
    let main_agents: Vec<AgentName> = agents::list_agents(&mut *conn)
        .await?
        .into_iter()
        .filter(|agent| main.contains(&agent.id))
        .map(|agent| agent.name)
        .collect();
    Ok(feedback_readers(
        holder.as_ref(),
        &labels,
        &areas,
        &main_agents,
    ))
}
