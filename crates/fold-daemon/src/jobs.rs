use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use fold_core::checkpoint_policy::quiet_checkpoint_due;
use fold_core::events::types;
use fold_core::object::ObjectHash;
use fold_core::{
    Actor, FoldError, NewEvent, ProjectPaths, RevisionId, Timestamp, WorkspaceId, codes,
};
use fold_store::{Store, StoreError, workspaces};
use fold_vcs::git::Git;
use fold_vcs::main_view;
use fold_vcs::objects::ObjectStore;
use fold_vcs::paths::WorkspaceError;
use fold_vcs::platform::Capabilities;
use serde_json::json;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::sync::{Semaphore, broadcast, watch};
use tokio::task::{JoinHandle, JoinSet};

use crate::daemon::AppState;
use crate::projects::ProjectHandle;
use crate::services::agents::AgentError;
use crate::services::events::{BusMessage, EventBuffer, EventBus};
use crate::services::projects::{self, RemoteContext};
use crate::services::tasks::sweep_leases;
use crate::services::workspaces::{
    QuietCheckpoint, Reconciliation, WorkspaceService, capabilities, check_stalls,
};

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

/// How many workspaces [`reconcile_workspaces`] scans at once.
pub const RECONCILE_PARALLEL: usize = 4;

/// Reconciles every task workspace in state `active` or `missing` with its directory when a project opens, before it serves requests: see [`WorkspaceService::reconcile_workspace`]. Up to [`RECONCILE_PARALLEL`] run at once. A workspace whose reconciliation fails is logged and left as it was; the others go on. Returns what was done, by workspace id.
pub async fn reconcile_workspaces(handle: &ProjectHandle) -> Vec<Reconciliation> {
    let service = WorkspaceService::for_project(handle);
    let rows = match workspaces::list_live_task_workspaces(handle.store.read()).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(project = %handle.slug, code = error.code().unwrap_or("store_error"), "the workspaces were not reconciled");
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
                code = error.code().unwrap_or(codes::INTERNAL_ERROR),
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

/// Starts the live job: every `tick` it lets the live state of each open project look at the clock, so a workspace scans in full every `rescan_seconds` of clock time even when its watcher reports nothing. A cycle is a tick that reached at least one workspace. It stops when `stop` changes.
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
        let config = match projects::load_config(&project.paths).await {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!(project = %project.slug, code = error.code().unwrap_or(codes::INTERNAL_ERROR), "quiet checkpoints skipped; the next tick tries again");
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
                    tracing::warn!(project = %project.slug, workspace = %workspace, code = error.code().unwrap_or(codes::INTERNAL_ERROR), "a quiet checkpoint was not taken; the next editing burst tries again");
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
                    code = error.code().unwrap_or(codes::INTERNAL_ERROR),
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
                    code = error.code().unwrap_or(codes::INTERNAL_ERROR),
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
                    code = presence_code(&error),
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

fn presence_code(error: &AgentError) -> &'static str {
    match error {
        AgentError::Fold(error) => error.code(),
        AgentError::Store(error) => error.code().unwrap_or(codes::INTERNAL_ERROR),
        AgentError::Event(_) => codes::INTERNAL_ERROR,
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
                code = error.code().unwrap_or(codes::INTERNAL_ERROR),
                "daily garbage collection failed"
            ),
        }
    }
}

/// The revisions waiting for `main/`. Queuing a revision older than the one waiting changes nothing, so several queued revisions collapse to the newest.
#[derive(Debug, Clone)]
pub struct MainQueue(Arc<watch::Sender<Option<RevisionId>>>);

impl Default for MainQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl MainQueue {
    pub fn new() -> Self {
        Self(Arc::new(watch::channel(None).0))
    }

    pub fn queue(&self, revision: RevisionId) {
        self.0.send_if_modified(|newest| {
            if newest.is_some_and(|newest| newest >= revision) {
                return false;
            }
            *newest = Some(revision);
            true
        });
    }
}

/// The running `main_materialiser` of one project. Dropping it stops the task; a rewrite already in its blocking work finishes first.
#[derive(Debug)]
pub struct MainMaterialiser {
    queue: MainQueue,
    task: JoinHandle<()>,
    progress: watch::Receiver<JobProgress>,
}

impl MainMaterialiser {
    pub fn queue(&self) -> &MainQueue {
        &self.queue
    }

    /// A check is a queued revision the job looked at; a cycle is a rewrite that finished and moved the marker.
    pub fn progress(&self) -> watch::Receiver<JobProgress> {
        self.progress.clone()
    }
}

impl Drop for MainMaterialiser {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Starts the `main_materialiser` of one project: it queues the revision of each committed `main.advanced` on `bus`, a proposal integration and a pull alike, and rewrites `main/` to the newest queued revision with [`main_view::update`]. At start it queues the current head, so a revision committed before a crash or a stop that never reached `main/` is written then. A project without a marker shows `r1`, which `fld init` wrote. A failed or cut-off rewrite is logged and leaves the marker on the revision `main/` last finished showing, with the rewrite it began noted, so the next queued revision, or the head at the next start, starts from what `main/` holds.
pub fn spawn_main_materialiser(
    store: Store,
    bus: &EventBus,
    paths: ProjectPaths,
    caps: Arc<OnceLock<Capabilities>>,
    queue: MainQueue,
) -> MainMaterialiser {
    let (progress, watcher) = watch::channel(JobProgress::default());
    let run = MainRun {
        store,
        paths,
        caps,
        queue: queue.clone(),
        progress,
    };
    let task = tokio::spawn(run.run(bus.subscribe()));
    MainMaterialiser {
        queue,
        task,
        progress: watcher,
    }
}

struct MainRun {
    store: Store,
    paths: ProjectPaths,
    caps: Arc<OnceLock<Capabilities>>,
    queue: MainQueue,
    progress: watch::Sender<JobProgress>,
}

impl MainRun {
    async fn run(self, mut events: broadcast::Receiver<BusMessage>) {
        let mut queued = self.queue.0.subscribe();
        queued.mark_changed();
        self.queue_head().await;
        loop {
            tokio::select! {
                received = events.recv() => match received {
                    Ok(BusMessage::Durable(event)) if event.event_type == types::MAIN_ADVANCED => {
                        if let Some(revision) = event.revision {
                            self.queue.queue(revision);
                        }
                    }
                    Ok(_) => {}
                    Err(RecvError::Lagged(_)) => self.queue_head().await,
                    Err(RecvError::Closed) => return,
                },
                changed = queued.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let target = *queued.borrow_and_update();
                    if let Some(target) = target {
                        self.materialise(target).await;
                    }
                }
            }
        }
    }

    async fn queue_head(&self) {
        match workspaces::head(self.store.read()).await {
            Ok(Some(head)) => self.queue.queue(head.number),
            Ok(None) => {}
            Err(error) => tracing::warn!(
                code = error.code().unwrap_or(codes::INTERNAL_ERROR),
                "main view could not read the head; the next integration tries again"
            ),
        }
    }

    async fn materialise(&self, target: RevisionId) {
        let rewrote = self.rewrite(target).await;
        self.progress.send_modify(|seen| {
            seen.checks += 1;
            seen.cycles += u64::from(rewrote.as_ref().is_ok_and(|done| *done));
        });
        if let Err(code) = rewrote {
            tracing::warn!(revision = %target, code, "main view rewrite failed; the next integration tries again");
        }
    }

    /// False when `main/` already shows `target` or a later revision and no rewrite stopped part-way.
    async fn rewrite(&self, target: RevisionId) -> Result<bool, &'static str> {
        let fold_dir = self.paths.fold_dir();
        let marker = tokio::task::spawn_blocking(move || main_view::read_state(&fold_dir))
            .await
            .map_err(|_| codes::INTERNAL_ERROR)?
            .map_err(|error| FoldError::from(WorkspaceError::from(error)).code())?;
        let shown = marker.shown.or_else(|| RevisionId::from_number(1));
        if marker.rewriting.is_none() && shown.is_some_and(|shown| shown >= target) {
            return Ok(false);
        }
        let from = match shown {
            Some(shown) => match workspaces::get_by_number(self.store.read(), shown).await {
                Ok(row) => Some(row.tree_hash),
                Err(StoreError::RevisionNotFound(_)) => None,
                Err(error) => return Err(error.code().unwrap_or(codes::INTERNAL_ERROR)),
            },
            None => None,
        };
        let to = workspaces::get_by_number(self.store.read(), target)
            .await
            .map_err(|error| error.code().unwrap_or(codes::INTERNAL_ERROR))?
            .tree_hash;
        let (paths, caps) = (self.paths.clone(), Arc::clone(&self.caps));
        tokio::task::spawn_blocking(move || rewrite_main(&paths, &caps, from, to, target))
            .await
            .map_err(|_| codes::INTERNAL_ERROR)?
            .map_err(|error| error.code())?;
        tracing::info!(revision = %target, "main view rewritten");
        Ok(true)
    }
}

fn rewrite_main(
    paths: &ProjectPaths,
    caps: &OnceLock<Capabilities>,
    from: Option<ObjectHash>,
    to: ObjectHash,
    target: RevisionId,
) -> Result<(), FoldError> {
    let caps = capabilities(caps, paths)?;
    let objects = ObjectStore::open(&paths.fold_dir()).map_err(FoldError::from)?;
    let done = main_view::update(
        &objects,
        &paths.main_dir(),
        &paths.fold_dir(),
        from.as_ref(),
        &to,
        target,
        caps,
    )?;
    if !done.degraded.is_empty() {
        tracing::warn!(revision = %target, links = done.degraded.len(), "main view wrote symlinks as regular files; the platform cannot create symlinks");
    }
    Ok(())
}

/// The Git job of one project. Dropping it stops the task; a push already started finishes in its own task.
#[derive(Debug)]
pub struct AutoCommit(JoinHandle<()>);

impl Drop for AutoCommit {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Starts `auto_commit` for one project. It writes the commit of every revision that has none into `.fold/git` once at the start, which catches up an integration a stopped daemon never committed, and again after every committed integration of either kind, which the bus carries only after its commit. After an integration it then runs the push of `fld push` as `fold` to each remote whose `push_on_integrate` is true, read at that moment. A commit covers every revision so far, so the integrations that arrive while one runs need one more run at most. A failed commit appends `git.commit_failed` and a failed push `remote.push_failed`, each in its own transaction, and logs the project and the code only; the integration stands either way.
pub fn auto_commit(context: RemoteContext, git: Git) -> AutoCommit {
    let receiver = context.bus.subscribe();
    AutoCommit(tokio::spawn(run_auto_commit(context, git, receiver)))
}

async fn run_auto_commit(
    context: RemoteContext,
    git: Git,
    mut receiver: broadcast::Receiver<BusMessage>,
) {
    commit_after_integration(&context, &git).await;
    loop {
        match receiver.recv().await {
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

/// Whether every revision has its commit now.
async fn commit_after_integration(context: &RemoteContext, git: &Git) -> bool {
    let committed =
        projects::commit_main(&context.store, &context.paths, git, context.clock.as_ref()).await;
    let Err(error) = committed else {
        return true;
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

/// Pushes to every remote whose `push_on_integrate` is true, one after the other.
async fn push_after_integration(context: &RemoteContext, git: &Git) {
    let remotes = match projects::load_config(&context.paths).await {
        Ok(config) => config.remotes,
        Err(error) => {
            tracing::warn!(project = %context.slug, code = error.code().unwrap_or(codes::INTERNAL_ERROR), "the automatic push skipped an integration; the configuration is invalid");
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
        record_failure(context, event, "remote.push_failed").await;
    }
}

/// Appends a failure event in its own transaction and publishes it.
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
            tracing::warn!(project = %context.slug, event = name, code = error.code().unwrap_or(codes::INTERNAL_ERROR), "a failure event was not recorded");
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
