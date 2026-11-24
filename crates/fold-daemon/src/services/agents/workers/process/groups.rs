use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::sync::{oneshot, watch};

use fold_core::{AgentName, Timestamp, WorkerState};
use fold_store::StoreError;
use fold_store::agents::{self, WorkerProcessRow};

use super::super::resume_groups::{
    DEFAULT_RESUME_GAP, DEFAULT_RESUME_GROUP, GroupQueue, ResumeRank,
};
use super::{Inner, Supervisor, lock};
use crate::services::agents::AgentError;
use crate::services::agents::workers::code_of;

/// The workers that daemon starts run again and that wait for their group, across every project that shares it. The daemon gives one to every project it opens, so the groups count the workers of all projects together; the live job runs the group that is due on each tick.
#[derive(Clone)]
pub struct ResumeGroups {
    shared: Arc<Shared>,
}

struct Shared {
    queue: Mutex<GroupQueue<Queued>>,
    group: usize,
    gap: Duration,
}

/// One start that waits for its group: a worker row that the group runs again, or a resume fold owes, which goes on once its group comes. The project's supervisor is held weakly, so a project that closes meanwhile drops its waiting starts.
struct Queued {
    supervisor: Weak<Inner>,
    start: Start,
}

enum Start {
    Row {
        agent: AgentName,
        /// The workers the same start runs again, which the work that waits for a lead leaves out.
        kept: Arc<BTreeSet<AgentName>>,
        /// Whether the fld binary of the workers' MCP configuration was there at the start.
        fld: bool,
    },
    /// Dropping it without a send tells the owed resume that its project stopped or paused.
    Ticket(oneshot::Sender<()>),
}

tokio::task_local! {
    /// The owed-resume sweeps of one daemon start, each run with its own [`Sweep`] as a task-local value, so its resumes take a ticket in the groups.
    static SWEEP: Sweep;
}

#[derive(Clone)]
struct Sweep {
    groups: ResumeGroups,
    /// Set once the sweep waits for its first ticket or ends.
    settled: Arc<AtomicBool>,
    /// How many sweeps of the start are not settled yet.
    left: Arc<watch::Sender<usize>>,
}

impl Sweep {
    fn settle(&self) {
        if !self.settled.swap(true, Ordering::SeqCst) {
            self.left.send_modify(|left| *left = left.saturating_sub(1));
        }
    }
}

/// One owed-resume sweep of a daemon start.
pub(super) type SweepTask = Pin<Box<dyn Future<Output = ()> + Send>>;

impl Default for ResumeGroups {
    fn default() -> Self {
        Self::new(DEFAULT_RESUME_GROUP, DEFAULT_RESUME_GAP)
    }
}

impl std::fmt::Debug for ResumeGroups {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResumeGroups")
            .field("group", &self.shared.group)
            .field("gap", &self.shared.gap)
            .finish_non_exhaustive()
    }
}

impl ResumeGroups {
    /// Groups of `group` workers, at least one, with `gap` on the clock between two groups.
    pub fn new(group: usize, gap: Duration) -> Self {
        Self {
            shared: Arc::new(Shared {
                queue: Mutex::new(GroupQueue::default()),
                group,
                gap,
            }),
        }
    }

    /// How many workers wait for their group.
    pub fn waiting(&self) -> usize {
        lock(&self.shared.queue).len()
    }

    /// Runs again the workers of the group that is due at `now`, one after the other, and gives how many the group held. A worker that runs already, or whose project closed, is left out.
    pub async fn run_due(&self, now: Timestamp) -> usize {
        let group = lock(&self.shared.queue).due(now, self.shared.group, self.shared.gap);
        let count = group.len();
        for queued in group {
            match queued.start {
                Start::Ticket(go) => {
                    let _ = go.send(());
                }
                Start::Row { agent, kept, fld } => {
                    let Some(inner) = queued.supervisor.upgrade() else {
                        continue;
                    };
                    Supervisor { inner }.resume_queued(&agent, &kept, fld).await;
                }
            }
        }
        count
    }

    fn push(&self, rank: ResumeRank, queued: Queued) {
        lock(&self.shared.queue).push(rank, queued);
    }

    /// Drops the waiting workers of the supervisor of `inner`.
    /// Drops the waiting starts of the supervisor of `inner` and gives the agents of its rows.
    fn forget(&self, inner: &Arc<Inner>) -> Vec<AgentName> {
        let mut dropped = Vec::new();
        lock(&self.shared.queue).retain(|queued| {
            let theirs = std::ptr::eq(queued.supervisor.as_ptr(), Arc::as_ptr(inner));
            if theirs && let Start::Row { agent, .. } = &queued.start {
                dropped.push(agent.clone());
            }
            !theirs
        });
        dropped
    }
}

impl Supervisor {
    /// Makes the project's daemon starts queue their workers in `groups`, the groups that every project of the daemon shares.
    pub fn set_resume_groups(&self, groups: ResumeGroups) {
        *lock(&self.inner.groups) = groups;
    }

    /// The groups in which this project's daemon start queues its workers.
    pub fn resume_groups(&self) -> ResumeGroups {
        lock(&self.inner.groups).clone()
    }

    /// Queues `row` for its resume group. Until its group runs it, the stall rule sees it as waiting for a place.
    pub(super) fn queue_resume(
        &self,
        row: &WorkerProcessRow,
        kept: &Arc<BTreeSet<AgentName>>,
        fld: bool,
    ) {
        let owes_turn = matches!(row.state, WorkerState::Working | WorkerState::Failed);
        self.inner.streams.queue(&row.agent, self.inner.clock.now());
        self.resume_groups().push(
            ResumeRank::of(row.role, owes_turn),
            Queued {
                supervisor: Arc::downgrade(&self.inner),
                start: Start::Row {
                    agent: row.agent.clone(),
                    kept: kept.clone(),
                    fld,
                },
            },
        );
    }

    /// Runs the owed-resume sweeps of a daemon start, each in a task of its own, and returns once each waits for its first ticket or has ended, so the first group finds the resumes fold owes in the queue.
    pub(super) async fn start_sweeps(&self, sweeps: Vec<SweepTask>) {
        let (left, mut settled) = watch::channel(sweeps.len());
        let left = Arc::new(left);
        let groups = self.resume_groups();
        for sweep in sweeps {
            let context = Sweep {
                groups: groups.clone(),
                settled: Arc::new(AtomicBool::new(false)),
                left: left.clone(),
            };
            tokio::spawn(SWEEP.scope(context.clone(), async move {
                sweep.await;
                context.settle();
            }));
        }
        let _ = settled.wait_for(|left| *left == 0).await;
    }

    /// Waits for the group of an owed resume when a sweep of a daemon start serves it, ranked before the rows; any other resume goes at once. False when the project stopped or paused before the group came: the work stays owed, and the next start or the resume of the pause serves it. Taken before any lock, so a resume that waits holds back no other.
    pub(super) async fn start_ticket(&self) -> bool {
        let Ok(sweep) = SWEEP.try_with(Clone::clone) else {
            return true;
        };
        let (go, wait) = oneshot::channel();
        sweep.groups.push(
            ResumeRank::Resume,
            Queued {
                supervisor: Arc::downgrade(&self.inner),
                start: Start::Ticket(go),
            },
        );
        sweep.settle();
        wait.await.is_ok()
    }

    /// Drops this project's workers that wait for their group, as its processes end: the next daemon start or the resume of a pause runs them again.
    pub(super) fn forget_queued(&self) {
        for agent in self.resume_groups().forget(&self.inner) {
            self.inner.streams.unqueue(&agent);
        }
    }

    /// Runs `agent` again as the daemon start would have when its group comes, from its row as it is now. A worker that runs already, such as one started for a message meanwhile, or that a person stopped is left as it is; in a paused project the resume of the pause runs it.
    async fn resume_queued(&self, agent: &AgentName, kept: &BTreeSet<AgentName>, fld: bool) {
        let served = self.serve_queued(agent, kept, fld).await;
        self.inner.streams.unqueue(agent);
        // The loop that runs waiting turns again reads only the processes that run, so it hears of each one a group starts.
        self.owed_turns_changed(Vec::new());
        if let Err(error) = served {
            tracing::warn!(%agent, code = code_of(&error), "could not serve a worker after the daemon start");
        }
    }

    async fn serve_queued(
        &self,
        agent: &AgentName,
        kept: &BTreeSet<AgentName>,
        fld: bool,
    ) -> Result<(), AgentError> {
        if self.is_running(agent) {
            return Ok(());
        }
        if self.is_paused().await? {
            self.inner
                .unreachable_owed
                .store(true, std::sync::atomic::Ordering::SeqCst);
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
        match row {
            Some(row)
                if matches!(
                    row.state,
                    WorkerState::Working | WorkerState::Idle | WorkerState::Failed
                ) =>
            {
                self.restart_row(&row, fld, kept).await
            }
            _ => Ok(()),
        }
    }
}
