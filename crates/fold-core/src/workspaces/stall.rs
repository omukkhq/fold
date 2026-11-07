use std::collections::BTreeSet;
use std::time::Duration;

use crate::agents::AgentName;
use crate::layout::MCP_SERVER_NAME;
use crate::time::Timestamp;

/// The kind and state of a workspace as the stall rule sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceFact {
    /// A task workspace whose directory is in place.
    Active,
    /// A task workspace whose files are the tree of its open proposal's newest version while the proposal waits on others: the worker sent its work and waits for the review or the integration.
    Sent,
    /// A task workspace whose directory is gone.
    Missing,
    /// An archived or abandoned task workspace.
    Closed,
    /// A review checkout: nothing in it is recorded, so its files say nothing about the worker.
    ReviewCheckout,
}

/// Whose move the open proposal of a task waits for, as the stall rule sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalTurn {
    /// Reviewers or the integration. A new version always starts here: it has no verdict, and its conflict and check flags start clear.
    Others,
    /// Its author, since the proposal came back: changes were requested, or the version was flagged as conflicting with main or failing its integration checks.
    Author { since: Timestamp },
}

impl ProposalTurn {
    /// From the time the latest verdict on the current version requested changes and the time the proposal was flagged as conflicting or failing its checks, each `None` when it does not apply. These are the facts that put the next move with the author in the task status too; the later time is the one the author answers.
    pub fn of(changes_requested_at: Option<Timestamp>, flagged_at: Option<Timestamp>) -> Self {
        match changes_requested_at.max(flagged_at) {
            Some(since) => Self::Author { since },
            None => Self::Others,
        }
    }
}

/// The fact of a task workspace whose directory is in place: `Sent` while its files are the open proposal's newest version (`edits_after_version` is false) and the proposal waits on others, `Active` otherwise. A proposal that came back to its author makes the workspace active again, edited or not.
pub fn present_workspace(
    edits_after_version: Option<bool>,
    turn: Option<ProposalTurn>,
) -> WorkspaceFact {
    match (edits_after_version, turn) {
        (Some(false), Some(ProposalTurn::Others)) => WorkspaceFact::Sent,
        _ => WorkspaceFact::Active,
    }
}

/// The proposal-stage check runs of a task workspace as the stall rule sees them: a run in the workspace (a `run_checks` call or the gate of `create_proposal` or `update_proposal`) is work the worker waits for, while it goes and until it tells the worker its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceChecks {
    /// No run of the workspace ever ended and none goes.
    None,
    /// A run of the workspace goes.
    Running,
    /// No run goes, and the newest run of the workspace ended `at`.
    Ended { at: Timestamp },
}

/// What the worker of a task waits for from others and when they last gave it the turn, as the stall rule sees it beside its workspace and its checks. The default is a worker that waits for nobody and was never given the turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorkerTurn {
    /// An ask about the task or a permission question of the worker waits for a person.
    pub waits_for_person: bool,
    /// The newest answer of a person: a person's message about the task or to the worker, an answer for the user or the closure of an ask about the task, or the answer to a permission question of the worker.
    pub answered_at: Option<Timestamp>,
    /// The start of the worker's current process: a start, a resume or a hand-off.
    pub started_at: Option<Timestamp>,
}

impl WorkerTurn {
    /// The next move is someone else's, so the worker's silence says nothing.
    pub fn waits(&self) -> bool {
        self.waits_for_person
    }

    /// The latest time the worker was given the turn, or `None` when it never was.
    pub fn given_at(&self) -> Option<Timestamp> {
        self.answered_at.max(self.started_at)
    }
}

/// The time the stall window counts from: the last activity, the time the proposal came back to its author, the end of the workspace's newest check run, or the time the worker was last given the turn, whichever is latest. The author gets the whole window to answer a verdict, the worker the whole window to act on its check results, an answer or a restart.
pub fn stall_start(
    last_activity_at: Timestamp,
    turn: Option<ProposalTurn>,
    checks: WorkspaceChecks,
    worker: WorkerTurn,
) -> Timestamp {
    let start = match turn {
        Some(ProposalTurn::Author { since }) => last_activity_at.max(since),
        Some(ProposalTurn::Others) | None => last_activity_at,
    };
    let start = match checks {
        WorkspaceChecks::Ended { at } => start.max(at),
        WorkspaceChecks::None | WorkspaceChecks::Running => start,
    };
    worker.given_at().map_or(start, |given| start.max(given))
}

/// A worker is stalled when it holds the unreleased implementation claim of the task whose active workspace has had no tracked content change for `stall_minutes`, runs no checks and waits for nobody. The time counts from `last_activity_at`, which is the creation time of a workspace with no edits. Calls do not count: a stalled worker can hold a fresh lease. A `last_activity_at` later than `now` is not stalled. This is [`activity`] for a worker whose stream fold does not read.
pub fn is_stalled(
    now: Timestamp,
    has_impl_claim: bool,
    workspace: WorkspaceFact,
    checks: WorkspaceChecks,
    worker: WorkerTurn,
    last_activity_at: Timestamp,
    stall_minutes: u64,
) -> bool {
    let facts = StallFacts {
        has_impl_claim,
        workspace,
        checks,
        worker,
        ..StallFacts::default()
    };
    activity(
        now,
        &facts,
        WorkerStream::Unread,
        last_activity_at,
        stall_minutes,
    )
    .is_stall()
}

/// Claude Code's Bash tool ends a command at its timeout, which is ten minutes at most.
pub const COMMAND_LIMIT: Duration = Duration::from_secs(10 * 60);

/// How long a call stays open before it counts as long: twice [`COMMAND_LIMIT`]. By then Claude Code has ended any command and written its result, also on a slow machine whose process tree takes a while to end, so a command still open means its result was lost or the session hangs. A subagent or another server's tool has no limit of its own and writes lines while it works, so it counts as long once the worker wrote nothing for this time.
pub const LONG_CALL: Duration = Duration::from_secs(20 * 60);

/// How long a call is open before it shows as work in progress; a shorter call is an ordinary step.
pub const WORKING_AFTER: Duration = Duration::from_secs(60);

/// The kind of a worker's tool call, as the activity rule tells calls apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    /// A shell command, which Claude Code ends after [`COMMAND_LIMIT`] at most.
    Command,
    /// A fold tool. fold serves it and its longest call, `wait_for_events`, ends after 25 minutes at most, so a fold call never counts as long.
    Fold,
    /// A subagent, which writes its own lines while it works.
    Subagent,
    /// A question to a person, which waits as long as the person does.
    Question,
    /// Any other tool.
    Tool,
}

impl CallKind {
    /// The kind of a call of the Claude Code tool `tool`.
    pub fn of(tool: &str) -> Self {
        match tool {
            "Bash" => Self::Command,
            "Task" | "Agent" => Self::Subagent,
            "AskUserQuestion" => Self::Question,
            _ if tool
                .strip_prefix("mcp__")
                .and_then(|rest| rest.strip_prefix(MCP_SERVER_NAME))
                .is_some_and(|rest| rest.starts_with("__")) =>
            {
                Self::Fold
            }
            _ => Self::Tool,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Fold => "fold",
            Self::Subagent => "subagent",
            Self::Question => "question",
            Self::Tool => "tool",
        }
    }
}

/// A worker's oldest tool call that has no result yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenCall {
    pub kind: CallKind,
    pub since: Timestamp,
}

/// What fold read of the worker's own output, the stream of the process fold supervises, as the stall rule sees it beside the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkerStream {
    /// fold reads no output of the worker: it runs in a session that fold does not supervise, so the workspace and the worker's turn are all fold sees of it.
    #[default]
    Unread,
    /// The worker's process runs. `last_output_at` is the time of its newest line, or of the start of its process or of the daemon when it wrote none since; `open_call` is its oldest call without a result.
    Running {
        last_output_at: Timestamp,
        open_call: Option<OpenCall>,
    },
    /// The worker's process ended at `at`, and no process of it runs.
    Ended { at: Timestamp },
}

/// The facts of the claim, the workspace and the worker that the stall rule reads beside the worker's stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StallFacts {
    pub has_impl_claim: bool,
    pub workspace: WorkspaceFact,
    pub checks: WorkspaceChecks,
    pub worker: WorkerTurn,
    /// The task has an open report: the worker said its work is done and waits for the lead's check.
    pub reported: bool,
    /// Needs you already names why the worker cannot work: it cannot reach fold or the model, or the daemon finds no Claude Code.
    pub explained: bool,
}

impl Default for StallFacts {
    fn default() -> Self {
        Self {
            has_impl_claim: false,
            workspace: WorkspaceFact::Active,
            checks: WorkspaceChecks::None,
            worker: WorkerTurn::default(),
            reported: false,
            explained: false,
        }
    }
}

/// What the stall rule says about the worker of a workspace: what fold saw, and whether someone must act.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// Nothing to say: the worker works in short steps, waits for others, or holds no claim on an active workspace.
    Quiet,
    /// The call open since `since` has run for [`WORKING_AFTER`] or longer and is not long: the worker works.
    Working { call: CallKind, since: Timestamp },
    /// No call is open and nothing happened since `since` for the stall window: the session hangs between its steps or ended its turn without its work done.
    Silent { since: Timestamp },
    /// The call open since `since` ran long; see [`LONG_CALL`].
    LongCall { call: CallKind, since: Timestamp },
    /// The worker's process ended at `since` and the task has no report.
    Stopped { since: Timestamp },
}

impl Activity {
    /// The worker needs someone to act: it is silent, its call ran long, or its process ended.
    pub fn is_stall(self) -> bool {
        matches!(
            self,
            Self::Silent { .. } | Self::LongCall { .. } | Self::Stopped { .. }
        )
    }

    /// The name of a stall, as events and the live state write it; `None` for `Quiet` and `Working`.
    pub fn reason(self) -> Option<&'static str> {
        match self {
            Self::Silent { .. } => Some("silent"),
            Self::LongCall { .. } => Some("long_call"),
            Self::Stopped { .. } => Some("stopped"),
            Self::Quiet | Self::Working { .. } => None,
        }
    }
}

/// Decides what the worker of a workspace does at `now`. Nothing is said of a worker without the implementation claim, a workspace that is not active, a check run that goes, a worker that waits for a person, a task with an open report, or a failure that Needs you already names. A process that ended is `Stopped` at once, unless the worker was given the turn after it ended, such as a proposal that came back to its author, whose resume fold owes: then it is `Silent` once a stall window from `start` passed without a run. A worker whose stream fold does not read is `Silent` after `stall_minutes` from `start`, the time [`stall_start`] gives. A worker whose process runs counts from the later of `start` and its newest line: while a call is open it is `Working` from [`WORKING_AFTER`] and `LongCall` from [`LONG_CALL`] (a command from its start, a subagent or another tool from the newest line; a fold call or a question never), and with no call open it is `Silent` after `stall_minutes`.
pub fn activity(
    now: Timestamp,
    facts: &StallFacts,
    stream: WorkerStream,
    start: Timestamp,
    stall_minutes: u64,
) -> Activity {
    if !facts.has_impl_claim
        || facts.workspace != WorkspaceFact::Active
        || facts.checks == WorkspaceChecks::Running
        || facts.worker.waits()
        || facts.reported
        || facts.explained
    {
        return Activity::Quiet;
    }
    let silent = |since: Timestamp| {
        if crate::tasks::claims::is_stale(since, now, stall_minutes) {
            Activity::Silent { since }
        } else {
            Activity::Quiet
        }
    };
    let lasted = |since: Timestamp, span: Duration| {
        now.duration_since(since)
            .is_some_and(|elapsed| elapsed >= span)
    };
    match stream {
        WorkerStream::Unread => silent(start),
        WorkerStream::Ended { at } if at >= start => Activity::Stopped { since: at },
        WorkerStream::Ended { .. } => silent(start),
        WorkerStream::Running {
            last_output_at,
            open_call: None,
        } => silent(start.max(last_output_at)),
        WorkerStream::Running {
            last_output_at,
            open_call: Some(call),
        } => {
            let quiet_since = match call.kind {
                CallKind::Question => return Activity::Quiet,
                CallKind::Fold => None,
                CallKind::Command => Some(call.since),
                CallKind::Subagent | CallKind::Tool => Some(call.since.max(last_output_at)),
            };
            if quiet_since.is_some_and(|since| lasted(since, LONG_CALL)) {
                Activity::LongCall {
                    call: call.kind,
                    since: call.since,
                }
            } else if lasted(call.since, WORKING_AFTER) {
                Activity::Working {
                    call: call.kind,
                    since: call.since,
                }
            } else {
                Activity::Quiet
            }
        }
    }
}

/// Whether a stall needs a person: nobody was told because no agent can act on it (`told_at` is `None`), or the agent told at `told_at` let one more stall window pass without fixing it.
pub fn needs_person(now: Timestamp, told_at: Option<Timestamp>, stall_minutes: u64) -> bool {
    told_at.is_none_or(|told| crate::tasks::claims::is_stale(told, now, stall_minutes))
}

/// Who hears of the stall of `worker` first: the agent that started it, which is its lead, or else the main agents, such as for a worker a person started. A worker never hears of its own stall, so a main agent that builds a task itself has nobody to tell. An empty list means no agent can act.
pub fn stall_receivers(
    worker: &AgentName,
    starter: Option<&AgentName>,
    main_agents: &BTreeSet<AgentName>,
) -> Vec<AgentName> {
    let receivers: Vec<AgentName> = match starter {
        Some(starter) => vec![starter.clone()],
        None => main_agents.iter().cloned().collect(),
    };
    receivers
        .into_iter()
        .filter(|agent| agent != worker)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: i64 = 60_000;
    const NONE: WorkspaceChecks = WorkspaceChecks::None;
    const IDLE: WorkerTurn = WorkerTurn {
        waits_for_person: false,
        answered_at: None,
        started_at: None,
    };

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis).unwrap()
    }

    #[test]
    fn an_active_claimed_workspace_stalls_from_the_end_of_the_window() {
        let edited = at(1_000_000);
        let active = WorkspaceFact::Active;
        assert!(!is_stalled(
            at(1_000_000 + 4 * MINUTE),
            true,
            active,
            NONE,
            IDLE,
            edited,
            5
        ));
        assert!(!is_stalled(
            at(1_000_000 + 5 * MINUTE - 1),
            true,
            active,
            NONE,
            IDLE,
            edited,
            5
        ));
        assert!(is_stalled(
            at(1_000_000 + 5 * MINUTE),
            true,
            active,
            NONE,
            IDLE,
            edited,
            5
        ));
        assert!(is_stalled(
            at(1_000_000 + 60 * MINUTE),
            true,
            active,
            NONE,
            IDLE,
            edited,
            5
        ));
    }

    #[test]
    fn the_window_follows_stall_minutes() {
        let active = WorkspaceFact::Active;
        assert!(!is_stalled(
            at(9 * MINUTE),
            true,
            active,
            NONE,
            IDLE,
            at(0),
            10
        ));
        assert!(is_stalled(
            at(10 * MINUTE),
            true,
            active,
            NONE,
            IDLE,
            at(0),
            10
        ));
        assert!(!is_stalled(
            at(200_000_000_000_000),
            true,
            active,
            NONE,
            IDLE,
            at(0),
            u64::MAX
        ));
    }

    #[test]
    fn without_an_implementation_claim_nothing_stalls() {
        let late = at(60 * MINUTE);
        assert!(!is_stalled(
            late,
            false,
            WorkspaceFact::Active,
            NONE,
            IDLE,
            at(0),
            5
        ));
    }

    #[test]
    fn only_an_active_task_workspace_stalls() {
        let late = at(60 * MINUTE);
        for workspace in [
            WorkspaceFact::Missing,
            WorkspaceFact::Closed,
            WorkspaceFact::ReviewCheckout,
        ] {
            assert!(
                !is_stalled(late, true, workspace, NONE, IDLE, at(0), 5),
                "{workspace:?}"
            );
        }
    }

    #[test]
    fn a_workspace_that_holds_its_sent_version_does_not_stall() {
        let late = at(60 * MINUTE);
        assert!(!is_stalled(
            late,
            true,
            WorkspaceFact::Sent,
            NONE,
            IDLE,
            at(0),
            5
        ));
        assert!(is_stalled(
            late,
            true,
            WorkspaceFact::Active,
            NONE,
            IDLE,
            at(0),
            5
        ));
    }

    #[test]
    fn a_proposal_is_with_its_author_from_the_later_of_its_verdict_and_its_flag() {
        assert_eq!(ProposalTurn::of(None, None), ProposalTurn::Others);
        assert_eq!(
            ProposalTurn::of(Some(at(5)), None),
            ProposalTurn::Author { since: at(5) }
        );
        assert_eq!(
            ProposalTurn::of(None, Some(at(7))),
            ProposalTurn::Author { since: at(7) }
        );
        assert_eq!(
            ProposalTurn::of(Some(at(9)), Some(at(7))),
            ProposalTurn::Author { since: at(9) }
        );
    }

    #[test]
    fn only_a_sent_version_that_waits_on_others_is_sent() {
        let back = Some(ProposalTurn::Author { since: at(0) });
        let waiting = Some(ProposalTurn::Others);
        assert_eq!(present_workspace(Some(false), waiting), WorkspaceFact::Sent);
        assert_eq!(present_workspace(Some(false), back), WorkspaceFact::Active);
        assert_eq!(
            present_workspace(Some(true), waiting),
            WorkspaceFact::Active
        );
        assert_eq!(present_workspace(None, waiting), WorkspaceFact::Active);
        assert_eq!(present_workspace(Some(false), None), WorkspaceFact::Active);
    }

    #[test]
    fn requested_changes_stall_from_the_verdict_not_from_the_last_edit() {
        let verdict = at(30 * MINUTE);
        let turn = Some(ProposalTurn::Author { since: verdict });
        let start = stall_start(at(0), turn, NONE, IDLE);
        assert_eq!(start, verdict);
        let fact = present_workspace(Some(false), turn);
        assert!(!is_stalled(
            at(34 * MINUTE),
            true,
            fact,
            NONE,
            IDLE,
            start,
            5
        ));
        assert!(is_stalled(
            at(35 * MINUTE),
            true,
            fact,
            NONE,
            IDLE,
            start,
            5
        ));
        assert_eq!(
            stall_start(at(40 * MINUTE), turn, NONE, IDLE),
            at(40 * MINUTE),
            "a later edit counts"
        );
        assert_eq!(
            stall_start(at(3), Some(ProposalTurn::Others), NONE, IDLE),
            at(3)
        );
        assert_eq!(stall_start(at(3), None, NONE, IDLE), at(3));
    }

    #[test]
    fn activity_in_the_future_is_not_stalled() {
        let active = WorkspaceFact::Active;
        assert!(!is_stalled(
            at(0),
            true,
            active,
            NONE,
            IDLE,
            at(10 * MINUTE),
            1
        ));
        assert!(!is_stalled(at(0), true, active, NONE, IDLE, at(0), 5));
    }

    #[test]
    fn a_running_check_run_never_stalls_its_worker() {
        let active = WorkspaceFact::Active;
        let running = WorkspaceChecks::Running;
        for minutes in [5, 60, 24 * 60] {
            assert!(!is_stalled(
                at(minutes * MINUTE),
                true,
                active,
                running,
                IDLE,
                at(0),
                5
            ));
        }
        assert_eq!(stall_start(at(7), None, running, IDLE), at(7));
    }

    #[test]
    fn the_window_starts_again_at_the_end_of_the_newest_run() {
        let active = WorkspaceFact::Active;
        let ended = WorkspaceChecks::Ended {
            at: at(20 * MINUTE),
        };
        let start = stall_start(at(0), None, ended, IDLE);
        assert_eq!(start, at(20 * MINUTE));
        assert!(!is_stalled(
            at(24 * MINUTE),
            true,
            active,
            ended,
            IDLE,
            start,
            5
        ));
        assert_eq!(
            stall_start(at(30 * MINUTE), None, ended, IDLE),
            at(30 * MINUTE),
            "an edit after the run counts"
        );
        let back = Some(ProposalTurn::Author {
            since: at(40 * MINUTE),
        });
        assert_eq!(stall_start(at(0), back, ended, IDLE), at(40 * MINUTE));
    }

    #[test]
    fn a_failed_run_and_no_edits_for_the_window_after_it_stall() {
        let active = WorkspaceFact::Active;
        let ended = WorkspaceChecks::Ended {
            at: at(12 * MINUTE),
        };
        let start = stall_start(at(0), None, ended, IDLE);
        assert!(!is_stalled(
            at(17 * MINUTE - 1),
            true,
            active,
            ended,
            IDLE,
            start,
            5
        ));
        assert!(is_stalled(
            at(17 * MINUTE),
            true,
            active,
            ended,
            IDLE,
            start,
            5
        ));
    }

    #[test]
    fn a_worker_that_waits_for_a_person_never_stalls() {
        let active = WorkspaceFact::Active;
        let asked = WorkerTurn {
            waits_for_person: true,
            ..IDLE
        };
        assert!(asked.waits());
        for minutes in [5, 60, 24 * 60] {
            assert!(!is_stalled(
                at(minutes * MINUTE),
                true,
                active,
                NONE,
                asked,
                at(0),
                5
            ));
        }
        assert!(!IDLE.waits());
        assert!(is_stalled(
            at(5 * MINUTE),
            true,
            active,
            NONE,
            IDLE,
            at(0),
            5
        ));
    }

    #[test]
    fn an_answer_clears_the_stall_and_only_a_new_full_window_brings_it_back() {
        let active = WorkspaceFact::Active;
        let edited = at(0);
        let waiting = WorkerTurn {
            waits_for_person: true,
            ..IDLE
        };
        let start = stall_start(edited, None, NONE, waiting);
        assert!(!is_stalled(
            at(40 * MINUTE),
            true,
            active,
            NONE,
            waiting,
            start,
            5
        ));
        let answered = WorkerTurn {
            answered_at: Some(at(40 * MINUTE)),
            ..IDLE
        };
        let start = stall_start(edited, None, NONE, answered);
        assert_eq!(start, at(40 * MINUTE));
        assert!(
            !is_stalled(at(40 * MINUTE), true, active, NONE, answered, start, 5),
            "the answer clears the mark at once"
        );
        assert!(!is_stalled(
            at(45 * MINUTE - 1),
            true,
            active,
            NONE,
            answered,
            start,
            5
        ));
        assert!(
            is_stalled(at(45 * MINUTE), true, active, NONE, answered, start, 5),
            "a new full window of silence"
        );
    }

    #[test]
    fn a_restart_of_the_worker_starts_the_window_again() {
        let active = WorkspaceFact::Active;
        let worker = WorkerTurn {
            started_at: Some(at(20 * MINUTE)),
            ..IDLE
        };
        assert_eq!(worker.given_at(), Some(at(20 * MINUTE)));
        let start = stall_start(at(0), None, NONE, worker);
        assert_eq!(start, at(20 * MINUTE));
        assert!(!is_stalled(
            at(25 * MINUTE - 1),
            true,
            active,
            NONE,
            worker,
            start,
            5
        ));
        assert!(is_stalled(
            at(25 * MINUTE),
            true,
            active,
            NONE,
            worker,
            start,
            5
        ));
    }

    #[test]
    fn the_latest_of_every_restart_counts() {
        let worker = WorkerTurn {
            answered_at: Some(at(30)),
            started_at: Some(at(20)),
            ..IDLE
        };
        assert_eq!(worker.given_at(), Some(at(30)));
        assert_eq!(IDLE.given_at(), None);
        assert_eq!(WorkerTurn::default(), IDLE);
        assert_eq!(
            stall_start(at(50), None, NONE, worker),
            at(50),
            "an edit after the answer counts"
        );
        let back = Some(ProposalTurn::Author { since: at(40) });
        assert_eq!(stall_start(at(0), back, NONE, worker), at(40));
        let ended = WorkspaceChecks::Ended { at: at(45) };
        assert_eq!(stall_start(at(0), back, ended, worker), at(45));
    }

    const CLAIMED: StallFacts = StallFacts {
        has_impl_claim: true,
        workspace: WorkspaceFact::Active,
        checks: NONE,
        worker: IDLE,
        reported: false,
        explained: false,
    };

    fn running(last_output: i64, call: Option<(CallKind, i64)>) -> WorkerStream {
        WorkerStream::Running {
            last_output_at: at(last_output),
            open_call: call.map(|(kind, since)| OpenCall {
                kind,
                since: at(since),
            }),
        }
    }

    #[test]
    fn a_call_of_each_tool_has_its_kind() {
        assert_eq!(CallKind::of("Bash"), CallKind::Command);
        assert_eq!(CallKind::of("Task"), CallKind::Subagent);
        assert_eq!(CallKind::of("Agent"), CallKind::Subagent);
        assert_eq!(CallKind::of("AskUserQuestion"), CallKind::Question);
        assert_eq!(CallKind::of("mcp__fold__wait_for_events"), CallKind::Fold);
        assert_eq!(CallKind::of("mcp__folder__read"), CallKind::Tool);
        assert_eq!(CallKind::of("mcp__pencil__draw"), CallKind::Tool);
        assert_eq!(CallKind::of("Edit"), CallKind::Tool);
        let names: Vec<&str> = [
            CallKind::Command,
            CallKind::Fold,
            CallKind::Subagent,
            CallKind::Question,
            CallKind::Tool,
        ]
        .into_iter()
        .map(CallKind::as_str)
        .collect();
        assert_eq!(names, ["command", "fold", "subagent", "question", "tool"]);
    }

    #[test]
    fn an_open_call_is_work_and_never_silence() {
        let call = Some((CallKind::Command, 0));
        assert_eq!(
            activity(at(30_000), &CLAIMED, running(0, call), at(0), 5),
            Activity::Quiet,
            "a short call is an ordinary step"
        );
        for minutes in [1, 5, 8, 19] {
            assert_eq!(
                activity(at(minutes * MINUTE), &CLAIMED, running(0, call), at(0), 5),
                Activity::Working {
                    call: CallKind::Command,
                    since: at(0)
                },
                "{minutes} min"
            );
        }
        let working = activity(at(8 * MINUTE), &CLAIMED, running(0, call), at(0), 5);
        assert!(!working.is_stall());
        assert_eq!(working.reason(), None);
    }

    #[test]
    fn no_open_call_and_no_output_for_the_window_is_silence() {
        let stream = running(3 * MINUTE, None);
        assert_eq!(
            activity(at(8 * MINUTE - 1), &CLAIMED, stream, at(0), 5),
            Activity::Quiet
        );
        let silent = activity(at(8 * MINUTE), &CLAIMED, stream, at(0), 5);
        assert_eq!(
            silent,
            Activity::Silent {
                since: at(3 * MINUTE)
            },
            "the newest line starts the window"
        );
        assert!(silent.is_stall());
        assert_eq!(silent.reason(), Some("silent"));
        assert_eq!(
            activity(at(8 * MINUTE), &CLAIMED, stream, at(4 * MINUTE), 5),
            Activity::Quiet,
            "a later edit, answer or start counts too"
        );
    }

    #[test]
    fn a_command_open_past_twice_claude_codes_limit_is_a_long_call() {
        assert_eq!(LONG_CALL, COMMAND_LIMIT * 2);
        let call = Some((CallKind::Command, 0));
        let at_limit = at(i64::try_from(LONG_CALL.as_millis()).unwrap());
        assert!(matches!(
            activity(at(20 * MINUTE - 1), &CLAIMED, running(0, call), at(0), 5),
            Activity::Working { .. }
        ));
        let long = activity(at_limit, &CLAIMED, running(0, call), at(0), 5);
        assert_eq!(
            long,
            Activity::LongCall {
                call: CallKind::Command,
                since: at(0)
            }
        );
        assert_eq!(long.reason(), Some("long_call"));
        assert!(long.is_stall());
        assert_eq!(
            activity(
                at(25 * MINUTE),
                &CLAIMED,
                running(15 * MINUTE, call),
                at(0),
                5
            ),
            long,
            "the lines of a parallel call do not make a command shorter"
        );
    }

    #[test]
    fn a_subagent_or_another_tool_is_long_only_after_the_window_without_output() {
        for kind in [CallKind::Subagent, CallKind::Tool] {
            let call = Some((kind, 0));
            assert!(
                matches!(
                    activity(
                        at(45 * MINUTE),
                        &CLAIMED,
                        running(30 * MINUTE, call),
                        at(0),
                        5
                    ),
                    Activity::Working { .. }
                ),
                "{kind:?} that writes lines works"
            );
            assert_eq!(
                activity(
                    at(50 * MINUTE),
                    &CLAIMED,
                    running(30 * MINUTE, call),
                    at(0),
                    5
                ),
                Activity::LongCall {
                    call: kind,
                    since: at(0)
                }
            );
        }
    }

    #[test]
    fn a_fold_call_never_runs_long_and_a_question_is_never_work() {
        let fold = Some((CallKind::Fold, 0));
        assert_eq!(
            activity(at(24 * 60 * MINUTE), &CLAIMED, running(0, fold), at(0), 5),
            Activity::Working {
                call: CallKind::Fold,
                since: at(0)
            }
        );
        let question = Some((CallKind::Question, 0));
        assert_eq!(
            activity(at(60 * MINUTE), &CLAIMED, running(0, question), at(0), 5),
            Activity::Quiet
        );
    }

    #[test]
    fn a_process_that_ended_is_stopped_at_once() {
        let ended = WorkerStream::Ended {
            at: at(10 * MINUTE),
        };
        let stopped = activity(at(10 * MINUTE), &CLAIMED, ended, at(0), 5);
        assert_eq!(
            stopped,
            Activity::Stopped {
                since: at(10 * MINUTE)
            }
        );
        assert!(stopped.is_stall());
        assert_eq!(stopped.reason(), Some("stopped"));
        let reported = StallFacts {
            reported: true,
            ..CLAIMED
        };
        assert_eq!(
            activity(at(60 * MINUTE), &reported, ended, at(0), 5),
            Activity::Quiet,
            "a worker that reported its task is done"
        );
        let explained = StallFacts {
            explained: true,
            ..CLAIMED
        };
        assert_eq!(
            activity(at(60 * MINUTE), &explained, ended, at(0), 5),
            Activity::Quiet,
            "Needs you already names the failure"
        );
    }

    #[test]
    fn a_turn_given_after_the_end_waits_one_window_for_the_owed_resume() {
        let ended = WorkerStream::Ended {
            at: at(10 * MINUTE),
        };
        let back = at(12 * MINUTE);
        assert_eq!(
            activity(at(16 * MINUTE), &CLAIMED, ended, back, 5),
            Activity::Quiet
        );
        assert_eq!(
            activity(at(17 * MINUTE), &CLAIMED, ended, back, 5),
            Activity::Silent { since: back }
        );
    }

    #[test]
    fn a_worker_that_waits_for_a_person_never_stalls_whatever_its_stream() {
        let asked = StallFacts {
            worker: WorkerTurn {
                waits_for_person: true,
                ..IDLE
            },
            ..CLAIMED
        };
        for stream in [
            WorkerStream::Unread,
            running(0, None),
            running(0, Some((CallKind::Command, 0))),
            WorkerStream::Ended { at: at(0) },
        ] {
            assert_eq!(
                activity(at(24 * 60 * MINUTE), &asked, stream, at(0), 5),
                Activity::Quiet,
                "{stream:?}"
            );
        }
    }

    #[test]
    fn nothing_is_said_without_a_claim_on_an_active_workspace_or_during_a_check_run() {
        let stream = running(0, Some((CallKind::Command, 0)));
        let late = at(60 * MINUTE);
        let cases = [
            StallFacts {
                has_impl_claim: false,
                ..CLAIMED
            },
            StallFacts {
                workspace: WorkspaceFact::Sent,
                ..CLAIMED
            },
            StallFacts {
                checks: WorkspaceChecks::Running,
                ..CLAIMED
            },
        ];
        for facts in cases {
            assert_eq!(
                activity(late, &facts, stream, at(0), 5),
                Activity::Quiet,
                "{facts:?}"
            );
        }
        assert_eq!(StallFacts::default().workspace, WorkspaceFact::Active);
        assert!(!StallFacts::default().has_impl_claim);
    }

    #[test]
    fn a_stall_needs_a_person_one_window_after_the_agent_was_told_or_at_once_when_nobody_can_act() {
        assert!(needs_person(at(0), None, 5));
        assert!(!needs_person(at(5 * MINUTE - 1), Some(at(0)), 5));
        assert!(needs_person(at(5 * MINUTE), Some(at(0)), 5));
    }

    #[test]
    fn a_stall_goes_to_the_starter_or_else_to_the_main_agents() {
        let worker = AgentName::parse("web-worker").unwrap();
        let lead = AgentName::parse("web-lead").unwrap();
        let frontend = AgentName::parse("frontend").unwrap();
        let main: BTreeSet<AgentName> = [frontend.clone()].into();
        assert_eq!(
            stall_receivers(&worker, Some(&lead), &main),
            vec![lead.clone()]
        );
        assert_eq!(
            stall_receivers(&worker, None, &main),
            vec![frontend.clone()]
        );
        assert!(stall_receivers(&worker, None, &BTreeSet::new()).is_empty());
        assert!(
            stall_receivers(&frontend, None, &main).is_empty(),
            "the main agent never hears of its own stall"
        );
    }
}
