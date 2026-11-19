use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime};

use fold_core::accounts::AccountStamp;
use fold_core::turn_waits::{
    LOGIN_RELEASES, WaitGroup, WaitReason, WaitingTurn, relogin_at, relogin_due, rerun_plan,
    rerun_turn,
};
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
    /// The groups a release met while the agent's process still started, which the plan that finds it idle runs: one worker's start never runs the others' turns again.
    pending: HashMap<AgentName, Vec<WaitGroup>>,
    /// The mark of Claude Code's own login that the loop saw last, `None` before its first look; it outlives each plan, so no change between two looks is lost.
    login_seen: Option<Option<accounts::LoginMark>>,
    /// How many changes of Claude Code's own login the loop saw since it started.
    logins: u64,
    /// Each process fold started again for a new login of Claude Code: the change it read, counted as [`OwedTurns::logins`] counts, and when it started (see [`relogin_due`]).
    relogins: HashMap<AgentName, (u64, Timestamp)>,
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

    /// Runs every turn that waits for a usage limit or a working login again at once, as after a change of the project's login: a switch of the account, or a new token or login of it.
    pub fn run_login_turns_now(&self) {
        self.owed_turns_changed(LOGIN_RELEASES.to_vec());
    }

    /// Notes `mark`, the mark of Claude Code's own login now, and returns whether it changed since the loop's last look. A change makes each process on that login whose turn waits for a working login start again in its session, whose restart then runs that turn again: a running process may not read the new login.
    fn note_login(&self, mark: Option<accounts::LoginMark>) -> bool {
        let mut owed = lock(&self.inner.owed);
        match &owed.login_seen {
            Some(seen) if *seen == mark => false,
            Some(_) => {
                owed.login_seen = Some(mark);
                owed.logins += 1;
                true
            }
            None => {
                owed.login_seen = Some(mark);
                false
            }
        }
    }

    /// Records that the process of `agent` started again for the `login`th change of Claude Code's own login.
    pub(super) fn relogin_ran(&self, agent: &AgentName, login: u64) {
        let now = self.inner.clock.now();
        lock(&self.inner.owed)
            .relogins
            .insert(agent.clone(), (login, now));
    }

    /// Runs the turn of `agent` that waits in `group` again once its process is idle, and no other turn.
    pub(super) fn release_for(&self, agent: &AgentName, group: WaitGroup) {
        {
            let mut owed = lock(&self.inner.owed);
            let groups = owed.pending.entry(agent.clone()).or_default();
            if !groups.contains(&group) {
                groups.push(group);
            }
        }
        self.owed_turns_changed(Vec::new());
    }

    /// Runs again, in its own running process, each owed turn that the plan of [`rerun_plan`] makes due, and returns when the plan must be read again, `Some(None)` for only when woken; `Ok(None)` once no row waits. While the project's agents are paused nothing runs: the resume of the pause gives each worker a turn of its own, and wakes the loop.
    async fn serve_owed_turns(&self) -> Result<Option<Option<Timestamp>>, AgentError> {
        // The releases are taken before the rows are read, so each one meets rows at least as new as itself: a release that comes later, such as at the end of a switch while this read sees the row still starting, stays for the next plan.
        let (released, pending, logins) = {
            let mut owed = lock(&self.inner.owed);
            owed.changed = false;
            (
                std::mem::take(&mut owed.released),
                std::mem::take(&mut owed.pending),
                owed.logins,
            )
        };
        let keep = |supervisor: &Self, released: Vec<WaitGroup>| {
            supervisor.keep_released(released);
            supervisor.keep_pending(pending.clone());
        };
        let rows = match self.worker_rows().await {
            Ok(rows) => rows,
            Err(error) => {
                keep(self, released);
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
                keep(self, released);
                return Ok(Some(None));
            }
            Err(error) => {
                keep(self, released);
                return Err(error.into());
            }
        }
        // A process that started with another account than the project's waits for its own switch, which runs its turn after it with the project's account. Nothing else switches a process whose turn waits, such as after `fld account update`, so the loop starts the switch. A new login of Claude Code restarts each process on it whose login was refused the same way, once per change and with the backoff of [`relogin_due`]; a process that cannot start again now, such as a working one, is looked at again in the next plan, and only a restart that ran counts.
        let current = self.current_account().await.ok();
        let on_own_login = matches!(current, Some(None));
        let now = self.inner.clock.now();
        let mut restarting = BTreeSet::new();
        let mut relogin_next: Option<Timestamp> = None;
        for row in &rows {
            if row.wait.is_none()
                || row.state != WorkerState::Idle
                || !self.can_take_turn(&row.agent)
                || lock(&self.inner.refreshing).contains(&row.agent)
            {
                continue;
            }
            if !self.runs_account(&row.agent, current.as_ref()) {
                self.switch_soon(&row.agent);
            } else if let Some(wait) = row
                .wait
                .as_ref()
                .filter(|wait| logins > 0 && on_own_login && wait.reason == WaitReason::Login)
            {
                let done = lock(&self.inner.owed).relogins.get(&row.agent).copied();
                if done.is_some_and(|(login, _)| login >= logins) {
                    continue;
                }
                let last = done.map(|(_, at)| at);
                if relogin_due(last, wait.tries, now) {
                    restarting.insert(row.agent.clone());
                    self.restart_soon(&row.agent, logins);
                } else if let Some(at) = relogin_at(last, wait.tries) {
                    relogin_next = Some(relogin_next.map_or(at, |next| next.min(at)));
                }
            }
        }
        let mut waiting: Vec<(&WorkerProcessRow, WaitingTurn<AgentName>)> = rows
            .iter()
            .filter(|row| {
                row.state == WorkerState::Idle
                    && !restarting.contains(&row.agent)
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
        let mut plan = rerun_plan(now, &turns, &released);
        // A release that comes while a waiting worker's process still starts, such as at a daemon start, stays for that worker alone, and the plan that finds it idle runs its turn.
        let mut still: HashMap<AgentName, Vec<WaitGroup>> = HashMap::new();
        for row in &rows {
            let Some(wait) = row.wait.as_ref() else {
                continue;
            };
            let group = wait.group();
            if lock(&self.inner.owed).sent.get(&row.agent) == Some(&wait.tried_at) {
                // Its re-run is on its way already.
                continue;
            }
            let owed = released.contains(&group)
                || pending
                    .get(&row.agent)
                    .is_some_and(|groups| groups.contains(&group));
            if !owed {
                continue;
            }
            let ready = turns
                .iter()
                .any(|turn| turn.key == row.agent && turn.group == group);
            if ready {
                if !plan.due.contains(&row.agent) {
                    plan.due.push(row.agent.clone());
                }
            } else if row.state == WorkerState::Starting
                || (row.state == WorkerState::Idle && !restarting.contains(&row.agent))
            {
                // An idle row that cannot take the turn now, such as one with a turn still queued or under a switch, takes it in a later plan.
                still.entry(row.agent.clone()).or_default().push(group);
            }
        }
        self.keep_pending(still);
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
        let next_at = match (plan.next_at, relogin_next) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        Ok(Some(next_at))
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

    /// Puts back the releases of single workers that a plan did not use.
    fn keep_pending(&self, pending: HashMap<AgentName, Vec<WaitGroup>>) {
        let mut owed = lock(&self.inner.owed);
        for (agent, groups) in pending {
            let kept = owed.pending.entry(agent).or_default();
            for group in groups {
                if !kept.contains(&group) {
                    kept.push(group);
                }
            }
        }
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
        supervisor.note_login(supervisor.login_mark().await);
        let next = match supervisor.serve_owed_turns().await {
            Ok(Some(next)) => next,
            Ok(None) => {
                // A wait recorded after the read finds this loop still going and only wakes it, so the loop ends only when nothing changed since.
                let mut owed = lock(&supervisor.inner.owed);
                if owed.changed {
                    continue;
                }
                drop(owed.task.take());
                // The next wait starts from the login there is then.
                owed.login_seen = None;
                owed.logins = 0;
                owed.relogins.clear();
                owed.pending.clear();
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
            // `claude login` tells no daemon either: a new login shows only in its credentials.
            let Some(strong) = inner.upgrade() else {
                return;
            };
            let supervisor = Supervisor { inner: strong };
            if supervisor.note_login(supervisor.login_mark().await) {
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
