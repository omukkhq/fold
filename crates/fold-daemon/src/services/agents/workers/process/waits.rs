use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime};

use fold_core::accounts::AccountStamp;
use fold_core::turn_waits::{LimitKind, WaitGroup, WaitingTurn, rerun_plan, rerun_turn};
use fold_core::{AgentName, Timestamp, WorkerState};
use fold_store::StoreError;
use fold_store::agents::{self, WorkerProcessRow};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use super::{Inner, Supervisor, lock};
use crate::services::agents::workers::code_of;
use crate::services::agents::{AgentError, accounts};

/// How often the loop that runs owed turns again looks at the project's clock while it waits for the next one.
const OWED_POLL: Duration = Duration::from_secs(1);

/// The turns fold owes workers whose last turn ended for a cause outside them, as the supervisor follows them in memory. The waits themselves are on the rows, so a daemon start finds them again.
#[derive(Default)]
pub(super) struct OwedTurns {
    /// The loop that runs owed turns again, while any row waits.
    task: Option<JoinHandle<()>>,
    /// Wakes the loop: a turn waits, a wait ended, or a pause ended.
    wake: Arc<Notify>,
    /// The groups a turn showed to have passed, which the loop's next plan runs whole.
    released: Vec<WaitGroup>,
    /// The re-run sent to each agent, by the end of the turn it runs again, so a plan read before the worker shows `working` never sends it twice.
    sent: HashMap<AgentName, Timestamp>,
    /// A change came after the loop's last read of the rows, so a loop that found no wait reads them once more before it ends.
    changed: bool,
}

impl Drop for OwedTurns {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Supervisor {
    /// Whether the loop that runs owed turns again goes.
    pub(super) fn has_owed_turns(&self) -> bool {
        lock(&self.inner.owed)
            .task
            .as_ref()
            .is_some_and(|task| !task.is_finished())
    }

    /// Tells the loop that runs owed turns again that a wait started or ended, with the groups whose turns may run at once; starts the loop when it does not go.
    pub(in crate::services::agents::workers) fn owed_turns_changed(
        &self,
        released: Vec<WaitGroup>,
    ) {
        let mut owed = lock(&self.inner.owed);
        owed.changed = true;
        for group in released {
            if !owed.released.contains(&group) {
                owed.released.push(group);
            }
        }
        if owed.task.as_ref().is_none_or(JoinHandle::is_finished) {
            let wake = owed.wake.clone();
            owed.task = Some(tokio::spawn(run_owed_turns(
                Arc::downgrade(&self.inner),
                wake,
            )));
        } else {
            owed.wake.notify_one();
        }
    }

    /// Forgets the re-run fold sent `agent`, so the loop sends it again: the process it went to ended for a switch of the account.
    pub(super) fn forget_rerun(&self, agent: &AgentName) {
        lock(&self.inner.owed).sent.remove(agent);
    }

    /// Runs every turn that waits for a usage limit again at once, as after a change that ends the limit for the project.
    pub fn run_limit_turns_now(&self) {
        self.owed_turns_changed(LimitKind::ALL.map(WaitGroup::limit).to_vec());
    }

    /// Runs again, in its own running process, each owed turn that the plan of [`rerun_plan`] makes due, and returns when the plan must be read again, `Some(None)` for only when woken; `Ok(None)` once no row waits. While the project's agents are paused nothing runs: the resume of the pause gives each worker a turn of its own, and wakes the loop.
    async fn serve_owed_turns(&self) -> Result<Option<Option<Timestamp>>, AgentError> {
        // The releases are taken before the rows are read, so each one meets rows at least as new as itself: a release that comes later, such as at the end of a switch while this read sees the row still starting, stays for the next plan.
        let released = {
            let mut owed = lock(&self.inner.owed);
            owed.changed = false;
            std::mem::take(&mut owed.released)
        };
        let rows = match self.worker_rows().await {
            Ok(rows) => rows,
            Err(error) => {
                self.keep_released(released);
                return Err(error);
            }
        };
        if !rows
            .iter()
            .any(|row| row.wait.is_some() && row.state.is_running())
        {
            // A release ends the waits there are now; a turn that reaches a limit later waits for its own reset.
            return Ok(None);
        }
        match self.is_paused().await {
            Ok(false) => {}
            Ok(true) => {
                self.keep_released(released);
                return Ok(Some(None));
            }
            Err(error) => {
                self.keep_released(released);
                return Err(error.into());
            }
        }
        // A process that started with another account than the project's waits for its own switch, which runs its turn after it with the project's account. Nothing else switches a process whose turn waits, such as after `fld account update`, so the loop starts the switch.
        let current = self.current_account().await.ok();
        for row in &rows {
            if row.wait.is_some()
                && row.state == WorkerState::Idle
                && self.can_take_turn(&row.agent)
                && !lock(&self.inner.refreshing).contains(&row.agent)
                && !self.runs_account(&row.agent, current.as_ref())
            {
                self.switch_soon(&row.agent);
            }
        }
        let now = self.inner.clock.now();
        let mut waiting: Vec<(&WorkerProcessRow, WaitingTurn<AgentName>)> = rows
            .iter()
            .filter(|row| {
                row.state == WorkerState::Idle
                    && self.can_take_turn(&row.agent)
                    && self.runs_account(&row.agent, current.as_ref())
            })
            .filter_map(|row| {
                let wait = row.wait.as_ref()?;
                let sent = lock(&self.inner.owed).sent.get(&row.agent) == Some(&wait.tried_at);
                (!sent).then(|| {
                    (
                        row,
                        WaitingTurn {
                            key: row.agent.clone(),
                            group: wait.group(),
                            since: wait.since,
                            tried_at: wait.tried_at,
                            tries: wait.tries,
                            resets_at: wait.resets_at,
                        },
                    )
                })
            })
            .collect();
        waiting.sort_by(|a, b| (a.1.since, &a.1.key).cmp(&(b.1.since, &b.1.key)));
        let turns: Vec<WaitingTurn<AgentName>> =
            waiting.iter().map(|(_, turn)| turn.clone()).collect();
        let plan = rerun_plan(now, &turns, &released);
        for agent in &plan.due {
            let Some((row, turn)) = waiting.iter().find(|(_, turn)| &turn.key == agent) else {
                continue;
            };
            let text = rerun_turn(turn.group.reason).to_owned();
            let text = self
                .with_catch_up(row, Some(text), &BTreeSet::new())
                .await
                .unwrap_or_default();
            lock(&self.inner.owed)
                .sent
                .insert(agent.clone(), turn.tried_at);
            tracing::info!(%agent, reason = turn.group.reason.as_str(), "fold runs a worker's waiting turn again");
            self.send_turn(agent, &text);
        }
        Ok(Some(plan.next_at))
    }

    async fn worker_rows(&self) -> Result<Vec<WorkerProcessRow>, AgentError> {
        let mut conn = self
            .inner
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        Ok(agents::list_worker_processes(&mut conn).await?)
    }

    /// Puts back releases that a plan did not use, before any that came meanwhile.
    fn keep_released(&self, released: Vec<WaitGroup>) {
        let mut owed = lock(&self.inner.owed);
        for group in released {
            if !owed.released.contains(&group) {
                owed.released.push(group);
            }
        }
    }

    /// Whether the process of `agent` runs with the project's account `current`, or the account cannot be told (`None`), and no restart of it is under way.
    fn runs_account(&self, agent: &AgentName, current: Option<&Option<AccountStamp>>) -> bool {
        if lock(&self.inner.refreshing).contains(agent) {
            return false;
        }
        current.is_none_or(|current| {
            lock(&self.inner.running)
                .get(agent)
                .is_some_and(|run| &run.account == current)
        })
    }

    /// Whether `agent` has a running process with no user turn in its queue, which a turn written now starts at once.
    fn can_take_turn(&self, agent: &AgentName) -> bool {
        lock(&self.inner.running)
            .get(agent)
            .is_some_and(|run| run.open_turns.load(Ordering::SeqCst) == 0)
    }
}

/// The loop that runs owed turns again. It holds the supervisor weakly between its reads, so the project's handle can drop, and it ends once no row waits; the next wait starts it again.
async fn run_owed_turns(inner: Weak<Inner>, wake: Arc<Notify>) {
    loop {
        let Some(strong) = inner.upgrade() else {
            return;
        };
        let supervisor = Supervisor { inner: strong };
        let index = supervisor
            .accounts_dir()
            .map(|dir| accounts::index_path(&dir));
        let seen = index_mark(index.as_deref()).await;
        let next = match supervisor.serve_owed_turns().await {
            Ok(Some(next)) => next,
            Ok(None) => {
                // A wait recorded after the read finds this loop still going and only wakes it, so the loop ends only when nothing changed since.
                let mut owed = lock(&supervisor.inner.owed);
                if owed.changed {
                    continue;
                }
                drop(owed.task.take());
                return;
            }
            Err(error) => {
                tracing::warn!(
                    code = code_of(&error),
                    "could not run the waiting turns again; fold tries again soon"
                );
                supervisor
                    .inner
                    .clock
                    .now()
                    .checked_add(fold_core::turn_waits::NETWORK_FIRST_WAIT)
            }
        };
        let clock = supervisor.inner.clock.clone();
        drop(supervisor);
        loop {
            tokio::select! {
                () = wake.notified() => break,
                () = tokio::time::sleep(OWED_POLL) => {}
            }
            if next.is_some_and(|next| clock.now() >= next) || inner.strong_count() == 0 {
                break;
            }
            // `fld account update` changes no project and tells no daemon, so a new token shows only in the host's accounts index; a waiting turn then switches to it and runs again at once.
            if index_mark(index.as_deref()).await != seen {
                break;
            }
        }
    }
}

/// What tells one version of the accounts index from the next, which every change renames into place: its inode and its time of change.
async fn index_mark(index: Option<&Path>) -> Option<(u64, SystemTime)> {
    use std::os::unix::fs::MetadataExt as _;
    let meta = tokio::fs::metadata(index?).await.ok()?;
    Some((meta.ino(), meta.modified().ok()?))
}
