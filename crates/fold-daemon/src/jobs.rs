use std::time::Duration;

use fold_core::{Timestamp, codes};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::daemon::AppState;
use crate::services::agents::AgentError;
use crate::services::projects;

/// How often a running daemon's jobs look at the clock.
pub const JOB_TICK: Duration = Duration::from_secs(60);

pub const GC_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// How often the presence job looks at the clock. It is short against the two-minute active window, so an agent shows idle a few seconds after its window passes.
pub const PRESENCE_TICK: Duration = Duration::from_secs(5);

/// How often the live job lets each workspace pipeline look at the clock for a due rescan.
pub const LIVE_TICK: Duration = Duration::from_secs(1);

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
