use std::time::Duration;

use crate::agents::roles::WorkerState;
use crate::time::Timestamp;

/// Why a worker's turn ended for a cause outside the worker: Claude Code could not reach the model, or the account reached a usage limit. Such a turn is owed: fold runs it again in the same session once the cause has passed, and it is never a stall or a failed turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WaitReason {
    Network,
    UsageLimit,
}

impl WaitReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::UsageLimit => "usage_limit",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "network" => Some(Self::Network),
            "usage_limit" => Some(Self::UsageLimit),
            _ => None,
        }
    }
}

/// The usage limit a turn reached, as Claude Code names it. `Usage` is the older message, which names no kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LimitKind {
    Session,
    Weekly,
    Opus,
    Sonnet,
    Usage,
}

impl LimitKind {
    pub const ALL: [Self; 5] = [
        Self::Session,
        Self::Weekly,
        Self::Opus,
        Self::Sonnet,
        Self::Usage,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Weekly => "weekly",
            Self::Opus => "opus",
            Self::Sonnet => "sonnet",
            Self::Usage => "usage",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }

    /// The limit in words, as a sentence names it: `weekly limit`, `Opus limit`.
    pub fn words(self) -> &'static str {
        match self {
            Self::Session => "session limit",
            Self::Weekly => "weekly limit",
            Self::Opus => "Opus limit",
            Self::Sonnet => "Sonnet limit",
            Self::Usage => "usage limit",
        }
    }
}

/// The longest reset wording fold keeps, in bytes. Claude Code's is a weekday, a time and a zone name, such as `Mon 12:00am (Asia/Tokyo)`.
pub const MAX_RESET_LEN: usize = 64;

/// What the end of a turn says the worker waits for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnWait {
    Network,
    UsageLimit {
        kind: LimitKind,
        /// Claude Code's wording of the reset, after `resets`, such as `1:50am (Asia/Tokyo)` or `Mon 12:00am`. It is a local time without a date or an offset, so fold shows it and never computes with it.
        reset: Option<String>,
        /// The reset instant, which only the older message gives, as seconds since the epoch.
        resets_at: Option<Timestamp>,
    },
}

impl TurnWait {
    pub fn group(&self) -> WaitGroup {
        match self {
            Self::Network => WaitGroup::NETWORK,
            Self::UsageLimit { kind, .. } => WaitGroup::limit(*kind),
        }
    }
}

/// The documented text of a turn that could not reach the model, after the optional `API Error: ` that Claude Code puts first.
const NETWORK_TEXT: &str = "Can't reach the API server";

/// The documented texts of the usage limits, each the start of Claude Code's message.
const LIMIT_TEXTS: [(&str, LimitKind); 4] = [
    ("You've hit your session limit", LimitKind::Session),
    ("You've hit your weekly limit", LimitKind::Weekly),
    ("You've hit your Opus limit", LimitKind::Opus),
    ("You've hit your Sonnet limit", LimitKind::Sonnet),
];

/// The older usage limit message, `Claude AI usage limit reached|<epoch seconds>`.
const OLD_LIMIT_TEXT: &str = "Claude AI usage limit reached";

/// What a turn that ended with Claude Code's `text`, cleaned as [`crate::turn_errors::error_text`] cleans it, waits for; `None` for any other text. Only the start of the text counts, so a text that quotes one of these further on is no wait. A typographic apostrophe counts as a plain one.
pub fn turn_wait(text: &str) -> Option<TurnWait> {
    let text = text.trim_start().replace('\u{2019}', "'");
    let first = text.lines().next().unwrap_or_default();
    if first
        .strip_prefix("API Error: ")
        .unwrap_or(first)
        .starts_with(NETWORK_TEXT)
    {
        return Some(TurnWait::Network);
    }
    for (start, kind) in LIMIT_TEXTS {
        if let Some(rest) = first.strip_prefix(start) {
            return Some(TurnWait::UsageLimit {
                kind,
                reset: reset_wording(rest),
                resets_at: None,
            });
        }
    }
    let rest = first.strip_prefix(OLD_LIMIT_TEXT)?;
    let resets_at = rest
        .strip_prefix('|')
        .and_then(|seconds| seconds.trim().parse::<i64>().ok())
        .and_then(|seconds| seconds.checked_mul(1000))
        .and_then(|millis| Timestamp::from_millis(millis).ok());
    Some(TurnWait::UsageLimit {
        kind: LimitKind::Usage,
        reset: None,
        resets_at,
    })
}

/// The wording after `resets` in the rest of a limit message, such as `1:50am (Asia/Tokyo)` from ` · resets 1:50am (Asia/Tokyo)`; `None` when there is none or it is longer than [`MAX_RESET_LEN`].
fn reset_wording(rest: &str) -> Option<String> {
    let at = rest.find("resets ")?;
    let wording = rest[at + "resets ".len()..]
        .trim()
        .trim_end_matches('.')
        .trim_end();
    (!wording.is_empty() && wording.len() <= MAX_RESET_LEN).then(|| wording.to_owned())
}

/// The waits that one check settles together: every turn that could not reach the model, or every turn at one kind of usage limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WaitGroup {
    pub reason: WaitReason,
    /// The limit of a `UsageLimit` group; `None` for the network.
    pub limit: Option<LimitKind>,
}

impl WaitGroup {
    pub const NETWORK: Self = Self {
        reason: WaitReason::Network,
        limit: None,
    };

    pub fn limit(kind: LimitKind) -> Self {
        Self {
            reason: WaitReason::UsageLimit,
            limit: Some(kind),
        }
    }
}

/// The first re-run of a turn that could not reach the model comes this long after it, and each re-run that fails again waits twice as long, up to [`NETWORK_LONGEST_WAIT`].
pub const NETWORK_FIRST_WAIT: Duration = Duration::from_secs(30);
pub const NETWORK_LONGEST_WAIT: Duration = Duration::from_secs(10 * 60);

/// The same for a usage limit whose reset instant fold does not know. Claude Code names the reset as a local time without a date or an offset, so fold tries again within the hour after it.
pub const LIMIT_FIRST_WAIT: Duration = Duration::from_secs(10 * 60);
pub const LIMIT_LONGEST_WAIT: Duration = Duration::from_secs(60 * 60);

/// A known reset instant is taken this long after it, so the account's limit has surely reset.
pub const RESET_MARGIN: Duration = Duration::from_secs(60);

/// How long after its last try a group's next re-run comes, when it was tried `tries` times again already.
pub fn wait_after(reason: WaitReason, tries: u32) -> Duration {
    let (first, longest) = match reason {
        WaitReason::Network => (NETWORK_FIRST_WAIT, NETWORK_LONGEST_WAIT),
        WaitReason::UsageLimit => (LIMIT_FIRST_WAIT, LIMIT_LONGEST_WAIT),
    };
    first
        .checked_mul(2_u32.saturating_pow(tries.min(16)))
        .map_or(longest, |wait| wait.min(longest))
}

/// One idle worker whose last turn waits, as the re-run plan reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitingTurn<K> {
    pub key: K,
    pub group: WaitGroup,
    /// The end of the first turn of this wait.
    pub since: Timestamp,
    /// The end of the newest turn of this wait.
    pub tried_at: Timestamp,
    /// The re-runs of this wait that failed again.
    pub tries: u32,
    pub resets_at: Option<Timestamp>,
}

/// Which waiting turns fold runs again now, and when the plan must be read again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerunPlan<K> {
    pub due: Vec<K>,
    /// The earliest time a group becomes due; `None` when nothing waits.
    pub next_at: Option<Timestamp>,
}

/// Decides at `now` which of the `waiting` turns run again, `waiting` being ordered oldest wait first. Each group is settled on its own. A group in `released` runs whole at once: a turn showed that its cause has passed. A usage limit group with a reset instant that no try has reached yet runs whole [`RESET_MARGIN`] after that instant. Otherwise the group's oldest turn alone runs again, [`wait_after`] its newest try: that re-run is the cheap check, and its success releases the rest of the group (see [`released_by`]).
pub fn rerun_plan<K: Clone>(
    now: Timestamp,
    waiting: &[WaitingTurn<K>],
    released: &[WaitGroup],
) -> RerunPlan<K> {
    let mut groups: Vec<WaitGroup> = waiting.iter().map(|turn| turn.group).collect();
    groups.sort();
    groups.dedup();
    let mut due = Vec::new();
    let mut next_at: Option<Timestamp> = None;
    for group in groups {
        let members: Vec<&WaitingTurn<K>> =
            waiting.iter().filter(|turn| turn.group == group).collect();
        if released.contains(&group) {
            due.extend(members.iter().map(|turn| turn.key.clone()));
            continue;
        }
        let tried = members.iter().map(|turn| turn.tried_at).max();
        let tries = members.iter().map(|turn| turn.tries).max().unwrap_or(0);
        let reset = members
            .iter()
            .filter_map(|turn| turn.resets_at)
            .max()
            .and_then(|reset| reset.checked_add(RESET_MARGIN));
        let (at, whole) = match (reset, tried) {
            (Some(reset), Some(tried)) if reset > tried => (Some(reset), true),
            (_, Some(tried)) => (tried.checked_add(wait_after(group.reason, tries)), false),
            (_, None) => (None, false),
        };
        let Some(at) = at else {
            continue;
        };
        if at <= now {
            if whole {
                due.extend(members.iter().map(|turn| turn.key.clone()));
            } else if let Some(first) = members.first() {
                due.push(first.key.clone());
            }
        } else {
            next_at = Some(next_at.map_or(at, |next| next.min(at)));
        }
    }
    RerunPlan { due, next_at }
}

/// The groups whose waiting turns a finished turn releases. A turn that reached the model, or ended well, shows that the model answers, which releases every turn that could not reach it, and, when that turn was itself a usage limit's wait, the other turns at that limit. A worker of another model can work through an Opus or Sonnet limit, so only a turn of the same wait releases a limit. A turn that reached no model releases nothing.
pub fn released_by(answered: bool, cleared: Option<WaitGroup>) -> Vec<WaitGroup> {
    if !answered {
        return Vec::new();
    }
    let mut groups = vec![WaitGroup::NETWORK];
    if let Some(group) = cleared.filter(|group| group.reason == WaitReason::UsageLimit) {
        groups.push(group);
    }
    groups
}

/// Whether a worker in `state` whose row holds a wait shows it: one whose process runs. The first answer of the model ends the wait, so a working worker that still holds one runs a turn again that has not reached the model yet; an ended worker runs no turn.
pub fn shows_wait(state: WorkerState) -> bool {
    matches!(
        state,
        WorkerState::Starting | WorkerState::Working | WorkerState::Idle
    )
}

/// The turn fold sends to run a turn that could not reach the model again.
pub const NETWORK_RERUN_TURN: &str = "fold runs your turn again: your last turn ended because Claude Code could not reach the model. Messages that came meanwhile wait in your inbox, and a lease can have expired while you waited: check your inbox, your claims and your workspace, then continue your work.";

/// The turn fold sends to run a turn that reached a usage limit again.
pub const LIMIT_RERUN_TURN: &str = "fold runs your turn again: your last turn ended at Claude Code's usage limit. Messages that came meanwhile wait in your inbox, and a lease can have expired while you waited: check your inbox, your claims and your workspace, then continue your work.";

/// The re-run turn of a wait for `reason`.
pub fn rerun_turn(reason: WaitReason) -> &'static str {
    match reason {
        WaitReason::Network => NETWORK_RERUN_TURN,
        WaitReason::UsageLimit => LIMIT_RERUN_TURN,
    }
}

/// What a human knows of turns that wait for the network.
pub const NETWORK_WAIT_HINT: &str = "fold runs each waiting turn again once the model answers; check the internet connection of the daemon's machine";

/// What a human knows and can do of turns that wait for a usage limit.
pub const LIMIT_WAIT_HINT: &str = "fold runs each waiting turn again after the reset; pause the agents to start no new turns until then";

#[cfg(test)]
mod tests {
    use super::*;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis).unwrap()
    }

    fn limit(kind: LimitKind, reset: Option<&str>) -> Option<TurnWait> {
        Some(TurnWait::UsageLimit {
            kind,
            reset: reset.map(str::to_owned),
            resets_at: None,
        })
    }

    #[test]
    fn the_documented_texts_are_waits_and_nothing_else_is() {
        assert_eq!(
            turn_wait("API Error: Can't reach the API server — check your internet connection"),
            Some(TurnWait::Network)
        );
        assert_eq!(
            turn_wait("Can\u{2019}t reach the API server - check your internet connection"),
            Some(TurnWait::Network)
        );
        assert_eq!(
            turn_wait("You've hit your session limit · resets 1:50am (Asia/Tokyo)"),
            limit(LimitKind::Session, Some("1:50am (Asia/Tokyo)"))
        );
        assert_eq!(
            turn_wait("You've hit your weekly limit · resets Mon 12:00am"),
            limit(LimitKind::Weekly, Some("Mon 12:00am"))
        );
        assert_eq!(
            turn_wait("You\u{2019}ve hit your Opus limit · resets 3:45pm."),
            limit(LimitKind::Opus, Some("3:45pm"))
        );
        assert_eq!(
            turn_wait("You've hit your Sonnet limit"),
            limit(LimitKind::Sonnet, None)
        );
        assert_eq!(
            turn_wait(&format!(
                "You've hit your session limit · resets {}",
                "x".repeat(MAX_RESET_LEN + 1)
            )),
            limit(LimitKind::Session, None)
        );
        assert_eq!(
            turn_wait("Claude AI usage limit reached|1789999200"),
            Some(TurnWait::UsageLimit {
                kind: LimitKind::Usage,
                reset: None,
                resets_at: Some(at(1_789_999_200_000)),
            })
        );
        assert_eq!(
            turn_wait("Claude AI usage limit reached|soon"),
            limit(LimitKind::Usage, None)
        );
        for other in [
            "Not logged in · Please run /login",
            "API Error: 529",
            "No conversation found with session ID: x",
            "The tool said: Can't reach the API server",
            "Quoting: You've hit your weekly limit",
            "",
        ] {
            assert_eq!(turn_wait(other), None, "{other}");
        }
    }

    #[test]
    fn every_name_reads_back() {
        for kind in LimitKind::ALL {
            assert_eq!(LimitKind::parse(kind.as_str()), Some(kind));
            assert!(kind.words().ends_with("limit"));
        }
        assert_eq!(LimitKind::parse("daily"), None);
        for reason in [WaitReason::Network, WaitReason::UsageLimit] {
            assert_eq!(WaitReason::parse(reason.as_str()), Some(reason));
        }
        assert_eq!(WaitReason::parse("tired"), None);
        assert_eq!(TurnWait::Network.group(), WaitGroup::NETWORK);
        assert_eq!(
            limit(LimitKind::Weekly, None).unwrap().group(),
            WaitGroup::limit(LimitKind::Weekly)
        );
    }

    #[test]
    fn each_failed_try_doubles_the_wait_up_to_its_cap() {
        assert_eq!(wait_after(WaitReason::Network, 0), NETWORK_FIRST_WAIT);
        assert_eq!(
            wait_after(WaitReason::Network, 1),
            NETWORK_FIRST_WAIT.saturating_mul(2)
        );
        assert_eq!(wait_after(WaitReason::Network, 30), NETWORK_LONGEST_WAIT);
        assert_eq!(wait_after(WaitReason::UsageLimit, 0), LIMIT_FIRST_WAIT);
        assert_eq!(
            wait_after(WaitReason::UsageLimit, u32::MAX),
            LIMIT_LONGEST_WAIT
        );
    }

    fn waiting(
        key: &'static str,
        group: WaitGroup,
        since: i64,
        tried: i64,
        tries: u32,
    ) -> WaitingTurn<&'static str> {
        WaitingTurn {
            key,
            group,
            since: at(since),
            tried_at: at(tried),
            tries,
            resets_at: None,
        }
    }

    #[test]
    fn the_oldest_turn_of_a_group_is_its_check_after_the_backoff() {
        let first = NETWORK_FIRST_WAIT.as_millis() as i64;
        let turns = [
            waiting("a", WaitGroup::NETWORK, 0, 0, 0),
            waiting("b", WaitGroup::NETWORK, 10, 10, 0),
        ];
        let early = rerun_plan(at(first), &turns, &[]);
        assert!(early.due.is_empty());
        assert_eq!(early.next_at, Some(at(10 + first)));
        let due = rerun_plan(at(10 + first), &turns, &[]);
        assert_eq!(due.due, ["a"]);
        assert_eq!(due.next_at, None);
        let again = [
            waiting("a", WaitGroup::NETWORK, 0, 100, 1),
            waiting("b", WaitGroup::NETWORK, 10, 10, 0),
        ];
        assert_eq!(
            rerun_plan(at(100 + first), &again, &[]).next_at,
            Some(at(100 + 2 * first))
        );
        let released = rerun_plan(at(11), &turns, &[WaitGroup::NETWORK]);
        assert_eq!(released.due, ["a", "b"]);
        assert_eq!(
            rerun_plan(at(0), &[] as &[WaitingTurn<&str>], &[]),
            RerunPlan {
                due: Vec::new(),
                next_at: None
            }
        );
    }

    #[test]
    fn a_known_reset_runs_its_whole_group_after_the_margin() {
        let weekly = WaitGroup::limit(LimitKind::Weekly);
        let margin = RESET_MARGIN.as_millis() as i64;
        let mut a = waiting("a", weekly, 0, 0, 0);
        a.resets_at = Some(at(5_000_000));
        let b = waiting("b", weekly, 10, 10, 0);
        let opus = waiting("c", WaitGroup::limit(LimitKind::Opus), 0, 4_900_000, 0);
        let turns = [a.clone(), b, opus];
        let plan = rerun_plan(at(5_000_000), &turns, &[]);
        assert!(plan.due.is_empty());
        assert_eq!(plan.next_at, Some(at(5_000_000 + margin)));
        let plan = rerun_plan(at(5_000_000 + margin), &turns, &[]);
        assert_eq!(plan.due, ["a", "b"], "the Opus limit has no known reset");
        let mut tried = a;
        tried.tried_at = at(5_000_000 + margin);
        tried.tries = 1;
        let plan = rerun_plan(at(5_000_000 + margin + 1), &[tried], &[]);
        let wait = wait_after(WaitReason::UsageLimit, 1).as_millis() as i64;
        assert_eq!(plan.next_at, Some(at(5_000_000 + margin + wait)));
        let other = rerun_plan(
            at(1),
            &[waiting("a", weekly, 0, 0, 0)],
            &[WaitGroup::limit(LimitKind::Opus)],
        );
        assert!(other.due.is_empty(), "a release of another limit leaves it");
    }

    #[test]
    fn only_an_answered_turn_releases_and_a_limit_only_by_its_own_wait() {
        assert!(released_by(false, Some(WaitGroup::NETWORK)).is_empty());
        assert_eq!(released_by(true, None), [WaitGroup::NETWORK]);
        assert_eq!(
            released_by(true, Some(WaitGroup::limit(LimitKind::Opus))),
            [WaitGroup::NETWORK, WaitGroup::limit(LimitKind::Opus)]
        );
        assert_eq!(
            released_by(true, Some(WaitGroup::NETWORK)),
            [WaitGroup::NETWORK]
        );
    }

    #[test]
    fn only_a_worker_whose_process_runs_shows_its_wait() {
        for state in [
            WorkerState::Starting,
            WorkerState::Working,
            WorkerState::Idle,
        ] {
            assert!(shows_wait(state));
        }
        for state in [
            WorkerState::Stopping,
            WorkerState::Stopped,
            WorkerState::Failed,
        ] {
            assert!(!shows_wait(state));
        }
        assert_eq!(rerun_turn(WaitReason::Network), NETWORK_RERUN_TURN);
        assert_eq!(rerun_turn(WaitReason::UsageLimit), LIMIT_RERUN_TURN);
    }
}
