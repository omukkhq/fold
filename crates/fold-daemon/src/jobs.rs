use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use fold_core::checkpoint_policy::quiet_checkpoint_due;
use fold_core::events::{EventType, types};
use fold_core::{Actor, NewEvent, Timestamp, WorkspaceId, codes};
use fold_store::{StoreError, workspaces};
use fold_vcs::git::{CommitId, Git};
use serde_json::json;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::sync::{Semaphore, broadcast, watch};
use tokio::task::{JoinHandle, JoinSet};

use crate::daemon::AppState;
use crate::projects::ProjectHandle;
use crate::services::events::{BusMessage, EventBuffer};
use crate::services::projects::{self, AttentionChange, ProjectAttention, RemoteContext};
use crate::services::tasks::sweep_leases;
use crate::services::workspaces::{
    QuietCheckpoint, Reconciliation, WorkspaceService, check_stalls,
};
use crate::support::lock;
use crate::support::unpoisoned;

/// How often a running daemon's jobs look at the clock.
pub const JOB_TICK: Duration = Duration::from_secs(60);

pub const GC_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// How often the presence job looks at the clock. It is short against the two-minute active window, so an agent shows idle a few seconds after its window passes.
pub const PRESENCE_TICK: Duration = Duration::from_secs(5);

/// How often the live job lets each workspace pipeline look at the clock for a due rescan.
pub const LIVE_TICK: Duration = Duration::from_secs(1);

/// How often the quiet-period job looks at each workspace's live state. It is short against the two-minute quiet period, so a burst's checkpoint comes at most this much late.
pub const AUTO_CHECKPOINT_TICK: Duration = Duration::from_secs(10);

/// How often the lease sweeper looks for claims whose lease ran out.
pub const LEASE_TICK: Duration = Duration::from_secs(60);

/// How often the stall job decides the stalled flag of every live workspace.
pub const STALL_TICK: Duration = Duration::from_secs(30);

/// How often the automatic integration job looks for projects it does not listen to yet. Each project's listener reacts to its events, so this only bounds how late a project opened while the daemon runs gets its first look.
pub const AUTO_INTEGRATE_TICK: Duration = Duration::from_secs(5);

/// How often the idle job looks for short-lived workers idle past their role's `idle_minutes`.
pub const WORKER_IDLE_TICK: Duration = Duration::from_secs(30);

/// How many workspaces [`reconcile_workspaces`] scans at once.
pub const RECONCILE_PARALLEL: usize = 4;

/// Reconciles every task workspace in state `active` or `missing` with its directory when a project opens, before it serves requests: see [`WorkspaceService::reconcile_workspace`]. Up to [`RECONCILE_PARALLEL`] run at once. A workspace whose reconciliation fails is logged and left as it was; the others go on. Returns what was done, by workspace id.
pub async fn reconcile_workspaces(handle: &ProjectHandle) -> Vec<Reconciliation> {
    let service = WorkspaceService::for_project(handle);
    let rows = match workspaces::list_live_task_workspaces(handle.store.read()).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(project = %handle.slug, code = error.log_code(), "the workspaces were not reconciled");
            return Vec::new();
        }
    };
    let permits = Arc::new(Semaphore::new(RECONCILE_PARALLEL));
    let mut running = JoinSet::new();
    for row in rows {
        let (service, permits) = (service.clone(), Arc::clone(&permits));
        running.spawn(async move {
            let _permit = permits.acquire_owned().await;
            (row.id, service.reconcile_workspace(row.id).await)
        });
    }
    let mut done = Vec::new();
    while let Some(joined) = running.join_next().await {
        match joined {
            Ok((_, Ok(reconciled))) => done.push(reconciled),
            Ok((workspace, Err(error))) => tracing::error!(
                project = %handle.slug,
                workspace = %workspace,
                code = error.log_code(),
                "a workspace was not reconciled"
            ),
            Err(_) => tracing::error!(project = %handle.slug, "a workspace reconciliation stopped"),
        }
    }
    done.sort_by_key(|reconciled| reconciled.workspace);
    done
}

/// When the daily collection is due: every [`GC_INTERVAL`] of clock time after the start, never at the start itself. A clock that jumps past several due times runs once, and the next due time stays on the grid of the start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DailySchedule {
    next: Option<Timestamp>,
}

impl DailySchedule {
    pub fn starting_at(start: Timestamp) -> Self {
        Self {
            next: start.checked_add(GC_INTERVAL),
        }
    }

    /// Whether a run is due at `now`; when it is, the next due time moves past `now`.
    pub fn take_due(&mut self, now: Timestamp) -> bool {
        let Some(mut next) = self.next.filter(|next| *next <= now) else {
            return false;
        };
        while next <= now {
            match next.checked_add(GC_INTERVAL) {
                Some(later) => next = later,
                None => {
                    self.next = None;
                    return true;
                }
            }
        }
        self.next = Some(next);
        true
    }
}

/// How far a job has got: each look at the clock, and each cycle that did the job's work: a collection over the open projects, or a presence check that sent a frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JobProgress {
    pub checks: u64,
    pub cycles: u64,
}

/// A running job.
#[derive(Debug)]
pub struct Job {
    task: JoinHandle<()>,
    progress: watch::Receiver<JobProgress>,
}

impl Job {
    pub fn progress(&self) -> watch::Receiver<JobProgress> {
        self.progress.clone()
    }

    /// Waits up to `timeout` for the job to stop after its stop signal, then aborts it. An aborted run rolls back its open write; the next run finishes what it left.
    pub async fn stopped(self, timeout: Duration) {
        let abort = self.task.abort_handle();
        if tokio::time::timeout(timeout, self.task).await.is_err() {
            abort.abort();
        }
    }
}

/// Starts `gc_daily`: every 24 hours of the daemon's clock from now, it runs a full collection of every project open at that moment, one after the other. It looks at the clock every `tick` and stops when `stop` changes.
pub fn spawn_gc_daily(state: AppState, tick: Duration, stop: watch::Receiver<bool>) -> Job {
    let (progress, watcher) = watch::channel(JobProgress::default());
    let task = tokio::spawn(gc_daily(state, tick, stop, progress));
    Job {
        task,
        progress: watcher,
    }
}

/// Starts the presence job: every `tick` it checks each open project for agents whose presence left `active` with the clock's time, and sends a live `agent.presence_changed` for each. It stops when `stop` changes.
pub fn spawn_presence(state: AppState, tick: Duration, stop: watch::Receiver<bool>) -> Job {
    let (progress, watcher) = watch::channel(JobProgress::default());
    let task = tokio::spawn(presence(state, tick, stop, progress));
    Job {
        task,
        progress: watcher,
    }
}

/// Starts the live job: every `tick` it closes each open project whose database was deleted or replaced, through [`Projects::close_stale`], then lets the live state of each open project look at the clock, so a workspace scans in full every `rescan_seconds` of clock time even when its watcher reports nothing. A cycle is a tick that reached at least one workspace. It stops when `stop` changes.
///
/// [`Projects::close_stale`]: crate::projects::Projects::close_stale
pub fn spawn_live(state: AppState, tick: Duration, stop: watch::Receiver<bool>) -> Job {
    let (progress, watcher) = watch::channel(JobProgress::default());
    let task = tokio::spawn(live(state, tick, stop, progress));
    Job {
        task,
        progress: watcher,
    }
}

/// Starts the quiet-period job: every `tick` it runs [`QuietCheckpoints::run`] for each open project, one after the other. A cycle is a tick that recorded at least one checkpoint. It stops when `stop` changes.
pub fn spawn_auto_checkpoints(state: AppState, tick: Duration, stop: watch::Receiver<bool>) -> Job {
    let (progress, watcher) = watch::channel(JobProgress::default());
    let task = tokio::spawn(auto_checkpoints(state, tick, stop, progress));
    Job {
        task,
        progress: watcher,
    }
}

async fn auto_checkpoints(
    state: AppState,
    tick: Duration,
    mut stop: watch::Receiver<bool>,
    progress: watch::Sender<JobProgress>,
) {
    let mut quiet = QuietCheckpoints::default();
    let mut ticks = tokio::time::interval(tick);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = ticks.tick() => {}
        }
        let mut recorded = 0;
        for handle in state.projects.handles() {
            recorded += quiet.run(&handle).await;
        }
        progress.send_modify(|seen| {
            seen.checks += 1;
            seen.cycles += u64::from(recorded > 0);
        });
    }
}

/// The quiet-period checkpoints of the daemon's projects. The live state gives each workspace's last activity and the time of its newest automatic checkpoint; this keeps, per workspace, the last activity a run already handled without a new row, so a workspace that equals its latest checkpoint is not snapshotted again on every tick.
#[derive(Debug, Default)]
pub struct QuietCheckpoints {
    handled: HashMap<String, HashMap<WorkspaceId, Timestamp>>,
}

impl QuietCheckpoints {
    /// Takes a `Quiet` checkpoint, as the daemon, of every live workspace of `project` that [`quiet_checkpoint_due`] finds due at the project clock's `now`, and returns how many it recorded. A workspace another operation holds is left for a later run. A workspace that still equals its base and has no checkpoint has nothing to go back to, so it gets none.
    pub async fn run(&mut self, project: &ProjectHandle) -> usize {
        // A checkpoint writes objects, which would make the deleted `.fold/` again.
        if project.database_replaced().await {
            return 0;
        }
        let config = match projects::load_config(&project.paths).await {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!(project = %project.slug, code = error.log_code(), "quiet checkpoints skipped; the next tick tries again");
                return 0;
            }
        };
        let live = project.live.all();
        let handled = self.handled.entry(project.slug.clone()).or_default();
        handled.retain(|workspace, _| live.iter().any(|state| state.view.workspace == *workspace));
        let now = project.clock.now();
        let service = WorkspaceService::for_project(project);
        let mut recorded = 0;
        for state in live {
            let view = &state.view;
            let workspace = view.workspace;
            let activity = view.last_activity_at;
            let newest_auto = view
                .checkpoints
                .iter()
                .filter(|checkpoint| checkpoint.automatic)
                .map(|checkpoint| checkpoint.created_at)
                .max();
            let covered = newest_auto.max(handled.get(&workspace).copied());
            if !quiet_checkpoint_due(
                now,
                Some(activity),
                covered,
                config.auto_checkpoint_quiet_seconds,
            ) {
                continue;
            }
            if view.files.is_empty() && view.checkpoints.is_empty() {
                handled.insert(workspace, activity);
                continue;
            }
            match service.take_quiet_checkpoint(workspace).await {
                Ok(QuietCheckpoint::Busy) => {}
                Ok(QuietCheckpoint::Taken(taken)) => {
                    handled.insert(workspace, activity);
                    recorded += usize::from(taken.recorded);
                }
                Err(error) => {
                    handled.insert(workspace, activity);
                    tracing::warn!(project = %project.slug, workspace = %workspace, code = error.log_code(), "a quiet checkpoint was not taken; the next editing burst tries again");
                }
            }
        }
        recorded
    }
}

/// Starts the lease sweeper: every `tick` it runs [`sweep_leases`] on each open project with the time of that project's clock. A cycle is a tick that marked at least one claim. It stops when `stop` changes.
pub fn spawn_lease_sweeper(state: AppState, tick: Duration, stop: watch::Receiver<bool>) -> Job {
    let (progress, watcher) = watch::channel(JobProgress::default());
    let task = tokio::spawn(lease_sweeper(state, tick, stop, progress));
    Job {
        task,
        progress: watcher,
    }
}

/// Starts the stall job: every `tick` it runs [`check_stalls`] on each open project with the time of that project's clock. A cycle is a tick that changed at least one flag. It stops when `stop` changes.
pub fn spawn_stalls(state: AppState, tick: Duration, stop: watch::Receiver<bool>) -> Job {
    let (progress, watcher) = watch::channel(JobProgress::default());
    let task = tokio::spawn(stalls(state, tick, stop, progress));
    Job {
        task,
        progress: watcher,
    }
}

/// Stops the short-lived workers of every project that stayed idle past their role's `idle_minutes`, on every tick.
pub fn spawn_worker_idle(state: AppState, tick: Duration, stop: watch::Receiver<bool>) -> Job {
    let (progress, watcher) = watch::channel(JobProgress::default());
    let task = tokio::spawn(worker_idle(state, tick, stop, progress));
    Job {
        task,
        progress: watcher,
    }
}

async fn worker_idle(
    state: AppState,
    tick: Duration,
    mut stop: watch::Receiver<bool>,
    progress: watch::Sender<JobProgress>,
) {
    let mut ticks = tokio::time::interval(tick);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = ticks.tick() => {}
        }
        let mut stopped = 0;
        for handle in state.projects.handles() {
            match handle.workers.sweep_idle(handle.clock.now()).await {
                Ok(agents) => stopped += agents.len(),
                Err(error) => tracing::warn!(
                    project = %handle.slug,
                    code = crate::services::agents::code_of(&error),
                    "the idle worker sweep failed; the next tick tries again"
                ),
            }
        }
        progress.send_modify(|seen| {
            seen.checks += 1;
            seen.cycles += u64::from(stopped > 0);
        });
    }
}

/// Starts the automatic integration job: every `tick` it starts a listener for each open project that has none. A listener runs [`ReviewService::integrate_ready`] once when it starts, which retries what a stopped daemon left, and again after each committed event that can make a proposal ready, while `[integration].automatic` is true in the project's configuration, read on each run. Each run is a check; a run that integrated at least one proposal, cleanly or with a conflict, is a cycle. It stops when `stop` changes: each listener finishes the run it is in, so a stop never cuts an integration short unless the drain times out.
///
/// [`ReviewService::integrate_ready`]: crate::services::reviews::ReviewService::integrate_ready
pub fn spawn_auto_integrate(state: AppState, tick: Duration, stop: watch::Receiver<bool>) -> Job {
    let (progress, watcher) = watch::channel(JobProgress::default());
    let task = tokio::spawn(auto_integrate(state, tick, stop, Arc::new(progress)));
    Job {
        task,
        progress: watcher,
    }
}

async fn auto_integrate(
    state: AppState,
    tick: Duration,
    mut stop: watch::Receiver<bool>,
    progress: Arc<watch::Sender<JobProgress>>,
) {
    let mut listening = HashSet::new();
    let mut listeners = JoinSet::new();
    let mut ticks = tokio::time::interval(tick);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = ticks.tick() => {}
        }
        // A listener ends when its project closes, so a project opened again under the same slug gets a new one.
        while let Some(ended) = listeners.try_join_next() {
            if let Ok(slug) = ended {
                listening.remove(&slug);
            }
        }
        for handle in state.projects.handles() {
            if listening.insert(handle.slug.clone()) {
                listeners.spawn(integrate_project(
                    state.clone(),
                    handle,
                    Arc::clone(&progress),
                    stop.clone(),
                ));
            }
        }
    }
    while listeners.join_next().await.is_some() {}
}

/// One run after each event of a kind that can make a proposal ready, rather than one run for a burst: a run that is already past a proposal does not see an approval that lands on it during the run. Returns the project's slug once its project closes or the job stops. The listener holds the handle and so its bus, whose channel never closes, so a closed project ends it through [`EventBus::closing`].
///
/// [`EventBus::closing`]: crate::services::events::EventBus::closing
async fn integrate_project(
    state: AppState,
    handle: Arc<ProjectHandle>,
    progress: Arc<watch::Sender<JobProgress>>,
    mut stop: watch::Receiver<bool>,
) -> String {
    let mut events = handle.bus.subscribe();
    let mut closing = handle.bus.closing();
    integrate_ready(&state, &handle, &progress).await;
    loop {
        let received = tokio::select! {
            _ = stop.changed() => break,
            _ = closing.wait_for(|closed| *closed) => break,
            received = events.recv() => received,
        };
        match received {
            Ok(BusMessage::Durable(event)) if can_make_ready(event.event_type) => {}
            Err(RecvError::Lagged(_)) => {}
            Ok(_) => continue,
            Err(RecvError::Closed) => break,
        }
        integrate_ready(&state, &handle, &progress).await;
    }
    handle.slug.clone()
}

/// A verdict, alone or in a batch review, a changed or cleared mark, a removed dependency, a status change, which covers a prerequisite becoming Done, and an added tag, because an approval counts by the `main` tag and the role its agent holds now.
fn can_make_ready(event_type: EventType) -> bool {
    [
        types::REVIEW_SUBMITTED,
        types::REVIEW_BATCH_SUBMITTED,
        types::TASK_HUMAN_REVIEW_SET,
        types::TASK_HUMAN_REVIEW_CLEARED,
        types::AGENT_TAG_ADDED,
        types::DEPENDENCY_REMOVED,
        types::TASK_STATUS_CHANGED,
    ]
    .contains(&event_type)
}

async fn integrate_ready(
    state: &AppState,
    handle: &ProjectHandle,
    progress: &watch::Sender<JobProgress>,
) {
    let integrated = match projects::load_config(&handle.paths).await {
        Ok(config) if config.integration.automatic => {
            match handle.reviews(&state.registry).integrate_ready().await {
                Ok(done) => done.len(),
                Err(error) => {
                    tracing::warn!(project = %handle.slug, code = error.log_code(), "the automatic integration failed; the next event tries again");
                    0
                }
            }
        }
        Ok(_) => 0,
        Err(error) => {
            tracing::warn!(project = %handle.slug, code = error.log_code(), "the automatic integration skipped a run; the configuration is invalid");
            0
        }
    };
    progress.send_modify(|seen| {
        seen.checks += 1;
        seen.cycles += u64::from(integrated > 0);
    });
}

async fn lease_sweeper(
    state: AppState,
    tick: Duration,
    mut stop: watch::Receiver<bool>,
    progress: watch::Sender<JobProgress>,
) {
    let mut ticks = tokio::time::interval(tick);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = ticks.tick() => {}
        }
        let mut marked = 0;
        for handle in state.projects.handles() {
            match sweep_leases(&handle, handle.clock.now()).await {
                Ok(claims) => marked += claims.len(),
                Err(error) => tracing::warn!(
                    project = %handle.slug,
                    code = error.log_code(),
                    "the lease sweep failed; the next tick tries again"
                ),
            }
        }
        progress.send_modify(|seen| {
            seen.checks += 1;
            seen.cycles += u64::from(marked > 0);
        });
    }
}

async fn stalls(
    state: AppState,
    tick: Duration,
    mut stop: watch::Receiver<bool>,
    progress: watch::Sender<JobProgress>,
) {
    let mut ticks = tokio::time::interval(tick);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = ticks.tick() => {}
        }
        let mut changed = 0;
        for handle in state.projects.handles() {
            match check_stalls(&handle, handle.clock.now()).await {
                Ok(count) => changed += count,
                Err(error) => tracing::warn!(
                    project = %handle.slug,
                    code = error.log_code(),
                    "the stall check failed; the next tick tries again"
                ),
            }
        }
        progress.send_modify(|seen| {
            seen.checks += 1;
            seen.cycles += u64::from(changed > 0);
        });
    }
}

async fn live(
    state: AppState,
    tick: Duration,
    mut stop: watch::Receiver<bool>,
    progress: watch::Sender<JobProgress>,
) {
    let mut ticks = tokio::time::interval(tick);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = ticks.tick() => {}
        }
        state.projects.close_stale().await;
        let reached: usize = state
            .projects
            .handles()
            .iter()
            .map(|handle| handle.live.tick())
            .sum();
        progress.send_modify(|seen| {
            seen.checks += 1;
            seen.cycles += u64::from(reached > 0);
        });
    }
}

async fn presence(
    state: AppState,
    tick: Duration,
    mut stop: watch::Receiver<bool>,
    progress: watch::Sender<JobProgress>,
) {
    let mut ticks = tokio::time::interval(tick);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = ticks.tick() => {}
        }
        let mut sent = 0;
        for handle in state.projects.handles() {
            match handle.agents().sweep_presence().await {
                Ok(count) => sent += count,
                Err(error) => tracing::warn!(
                    project = %handle.slug,
                    code = error.log_code(),
                    "presence check failed; the next tick tries again"
                ),
            }
        }
        progress.send_modify(|seen| {
            seen.checks += 1;
            seen.cycles += u64::from(sent > 0);
        });
    }
}

async fn gc_daily(
    state: AppState,
    tick: Duration,
    mut stop: watch::Receiver<bool>,
    progress: watch::Sender<JobProgress>,
) {
    let mut schedule = DailySchedule::starting_at(state.clock.now());
    let mut ticks = tokio::time::interval(tick);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = ticks.tick() => {}
        }
        let due = schedule.take_due(state.clock.now());
        if due {
            collect_all(&state).await;
        }
        progress.send_modify(|seen| {
            seen.checks += 1;
            seen.cycles += u64::from(due);
        });
    }
}

async fn collect_all(state: &AppState) {
    match projects::purge_expired_web_sessions(&state.registry, state.clock.now()).await {
        Ok(purged) => tracing::info!(purged, "expired web sessions purged"),
        Err(error) => tracing::error!(
            code = error.log_code(),
            "expired web sessions could not be purged"
        ),
    }
    for handle in state.projects.handles() {
        match projects::gc(&handle, false).await {
            Ok(report) => tracing::info!(
                project = %handle.slug,
                deleted = report.deleted,
                bytes_freed = report.bytes_freed,
                workspaces_removed = report.workspaces_removed,
                checkpoints_pruned = report.checkpoints_pruned,
                "daily garbage collection finished"
            ),
            Err(error) if error.code() == Some(codes::GC_IN_PROGRESS) => tracing::info!(
                project = %handle.slug,
                code = codes::GC_IN_PROGRESS,
                "daily garbage collection skipped; the next cycle tries again"
            ),
            Err(error) => tracing::error!(
                project = %handle.slug,
                code = error.log_code(),
                "daily garbage collection failed"
            ),
        }
    }
}

/// A task of one project that follows its bus until the bus closes at the daemon's stop. Dropping it aborts the task.
#[derive(Debug)]
pub struct ProjectTask(std::sync::Mutex<Option<JoinHandle<()>>>);

impl ProjectTask {
    pub fn spawn(task: impl Future<Output = ()> + Send + 'static) -> Self {
        Self(std::sync::Mutex::new(Some(tokio::spawn(task))))
    }

    /// Waits up to `timeout` for the task to end after the project's bus closed, then aborts it, so the task never outlives the project's database. A second call returns at once.
    pub async fn stopped(&self, timeout: Duration) {
        let task = lock(&self.0).take();
        if let Some(task) = task {
            let abort = task.abort_handle();
            if tokio::time::timeout(timeout, task).await.is_err() {
                abort.abort();
            }
        }
    }
}

impl Drop for ProjectTask {
    fn drop(&mut self) {
        let task = unpoisoned(self.0.get_mut()).take();
        if let Some(task) = task {
            task.abort();
        }
    }
}

/// The Git job of one project. A push already started finishes in its own task.
pub type AutoCommit = ProjectTask;

/// Starts `auto_commit` for one project. It writes the commit of every revision that has none into the project's repository, and brings the project folder to it, once at the start, which catches up an integration a stopped daemon never committed, and again after every committed integration of either kind, which the bus carries only after its commit. After an integration it then runs the push of `fld push` as `fold` to each remote whose `push_on_integrate` is true, read at that moment. A commit covers every revision so far, so the integrations that arrive while one runs need one more run at most. Each commit recorded in `revision_commits` gets its `git.committed` event in the same transaction, published once the run returns, so the web interface shows the commit without a reload. A failed commit appends `git.commit_failed`, and a failed push appends `remote.push_failed` with the remote's failure in `remote_states`, each in its own transaction, and logs the project and the code only; the integration stands either way.
pub fn auto_commit(context: RemoteContext, git: Git) -> AutoCommit {
    let receiver = context.bus.subscribe();
    ProjectTask::spawn(run_auto_commit(context, git, receiver))
}

/// It ends once the project's bus closes; a commit or push it is running finishes first.
async fn run_auto_commit(
    context: RemoteContext,
    git: Git,
    mut receiver: broadcast::Receiver<BusMessage>,
) {
    let mut closing = context.bus.closing();
    commit_after_integration(&context, &git).await;
    loop {
        let received = tokio::select! {
            biased;
            _ = closing.wait_for(|closed| *closed) => return,
            received = receiver.recv() => received,
        };
        match received {
            Ok(BusMessage::Durable(event)) if event.event_type == types::INTEGRATION_SUCCEEDED => {}
            // A missed event may have been an integration; a commit is cheap when nothing is new.
            Err(RecvError::Lagged(_)) => {}
            Ok(_) => continue,
            Err(RecvError::Closed) => return,
        }
        // Everything queued so far was committed before it was sent, so the run below covers it.
        while !matches!(
            receiver.try_recv(),
            Err(TryRecvError::Empty | TryRecvError::Closed)
        ) {}
        if commit_after_integration(&context, &git).await {
            push_after_integration(&context, &git).await;
        }
    }
}

/// Whether every revision has its commit now. What the run found that needs a person goes to the project's Git attention, and each change of it appends an event in its own transaction: `git.outside_commits` with the branch and its tip, `git.uncommitted_changes` with the branch, or `git.attention_cleared` with the kind that ended. The first run after the project opens compares with nothing found, so it appends the events of what it finds again.
async fn commit_after_integration(context: &RemoteContext, git: &Git) -> bool {
    let committed =
        projects::commit_main(&context.store, &context.paths, git, context.clock.as_ref()).await;
    let error = match committed {
        Ok(mut committed) => {
            context.bus.publish(std::mem::take(&mut committed.events));
            let change = context
                .git_attention
                .set(committed.outside.is_some(), committed.uncommitted);
            if change != AttentionChange::default() {
                record_attention(context, &change, committed.outside.as_ref()).await;
            }
            return true;
        }
        Err(error) => error,
    };
    let code = projects::error_code(&error);
    tracing::warn!(project = %context.slug, code, "the Git commit of main failed");
    let head = workspaces::head(context.store.read())
        .await
        .ok()
        .flatten()
        .map(|row| row.number);
    let event = NewEvent::new(types::GIT_COMMIT_FAILED, Actor::System.event_actor())
        .and_then(|event| event.data(json!({"code": code})))
        .map(|event| match head {
            Some(head) => event.revision(head),
            None => event,
        });
    record_failure(context, event, "git.commit_failed").await;
    false
}

async fn record_attention(
    context: &RemoteContext,
    change: &AttentionChange,
    tip: Option<&CommitId>,
) {
    let branch = projects::load_config(&context.paths)
        .await
        .map(|config| config.git.branch)
        .unwrap_or_default();
    let actor = Actor::System.event_actor();
    for kind in &change.raised {
        let event = match kind {
            ProjectAttention::OutsideCommits => {
                tracing::warn!(project = %context.slug, "the Git branch of main has commits fold did not write; the project folder waits for fld pull");
                NewEvent::new(types::GIT_OUTSIDE_COMMITS, actor.clone()).and_then(|event| {
                    event.data(json!({"branch": branch, "commit": tip.map(CommitId::as_str)}))
                })
            }
            ProjectAttention::UncommittedChanges => {
                tracing::info!(project = %context.slug, "the project folder has changes that are not committed; it follows main once they are committed or stashed");
                NewEvent::new(types::GIT_UNCOMMITTED_CHANGES, actor.clone())
                    .and_then(|event| event.data(json!({"branch": branch})))
            }
        };
        record_failure(context, event, "git attention").await;
    }
    for kind in &change.cleared {
        let event = NewEvent::new(types::GIT_ATTENTION_CLEARED, actor.clone())
            .and_then(|event| event.data(json!({"branch": branch, "kind": kind.as_str()})));
        record_failure(context, event, "git.attention_cleared").await;
    }
}

/// Pushes to every remote whose `push_on_integrate` is true, one after the other.
async fn push_after_integration(context: &RemoteContext, git: &Git) {
    let remotes = match projects::load_config(&context.paths).await {
        Ok(config) => config.remotes,
        Err(error) => {
            tracing::warn!(project = %context.slug, code = error.log_code(), "the automatic push skipped an integration; the configuration is invalid");
            return;
        }
    };
    for (name, remote) in remotes {
        if !remote.push_on_integrate {
            continue;
        }
        let pushed =
            projects::push_with(context.clone(), &Actor::System, git, Some(name.clone())).await;
        let Err(error) = pushed else {
            continue;
        };
        let code = projects::error_code(&error);
        tracing::warn!(project = %context.slug, remote = %name, code, "the automatic push failed");
        let event = NewEvent::new(types::REMOTE_PUSH_FAILED, Actor::System.event_actor()).and_then(
            |event| event.data(json!({"remote": name, "branch": remote.branch, "code": code})),
        );
        record_failed_push(context, event, &name, code).await;
    }
}

/// Appends `remote.push_failed` and the remote's failure in `remote_states` in one transaction of their own, and publishes the event.
async fn record_failed_push(
    context: &RemoteContext,
    event: Result<NewEvent, fold_core::EventError>,
    name: &str,
    code: &str,
) {
    let Ok(event) = event else {
        return;
    };
    let now = context.clock.now();
    let recorded = context
        .store
        .write(async |tx| -> Result<EventBuffer, StoreError> {
            workspaces::record_push_failure(tx, name, code, now).await?;
            let mut buffer = EventBuffer::new();
            buffer.append(tx, &event, now).await?;
            Ok(buffer)
        })
        .await;
    match recorded {
        Ok(buffer) => context.bus.publish(buffer),
        Err(error) => {
            tracing::warn!(project = %context.slug, event = "remote.push_failed", code = error.log_code(), "an event was not recorded");
        }
    }
}

/// Appends a failure or attention event in its own transaction and publishes it.
async fn record_failure(
    context: &RemoteContext,
    event: Result<NewEvent, fold_core::EventError>,
    name: &'static str,
) {
    let Ok(event) = event else {
        return;
    };
    let now = context.clock.now();
    let recorded = context
        .store
        .write(async |tx| -> Result<EventBuffer, StoreError> {
            let mut buffer = EventBuffer::new();
            buffer.append(tx, &event, now).await?;
            Ok(buffer)
        })
        .await;
    match recorded {
        Ok(buffer) => context.bus.publish(buffer),
        Err(error) => {
            tracing::warn!(project = %context.slug, event = name, code = error.log_code(), "an event was not recorded");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hours: u64) -> Timestamp {
        Timestamp::EPOCH
            .checked_add(Duration::from_secs(hours * 60 * 60))
            .unwrap()
    }

    #[test]
    fn runs_every_24_hours_after_the_start_and_never_at_it() {
        let mut schedule = DailySchedule::starting_at(at(0));
        assert!(!schedule.take_due(at(0)));
        assert!(!schedule.take_due(at(23)));
        assert!(schedule.take_due(at(24)));
        assert!(!schedule.take_due(at(24)));
        assert!(!schedule.take_due(at(47)));
        assert!(schedule.take_due(at(48)));
    }

    #[test]
    fn a_jump_over_several_days_runs_once_and_keeps_the_grid() {
        let mut schedule = DailySchedule::starting_at(at(0));
        assert!(schedule.take_due(at(100)));
        assert!(!schedule.take_due(at(119)));
        assert!(schedule.take_due(at(120)));
    }

    #[test]
    fn the_end_of_time_stops_the_schedule() {
        let last = Timestamp::from_millis(253_402_300_799_999).unwrap();
        let mut never = DailySchedule::starting_at(last);
        assert!(!never.take_due(last));
        let near_end = last.checked_sub(GC_INTERVAL).unwrap();
        let mut once = DailySchedule::starting_at(near_end);
        assert!(once.take_due(last));
        assert!(!once.take_due(last));
    }
}
