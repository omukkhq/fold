use std::time::Duration;

use crate::time::Timestamp;

/// What of a closed workspace's owning agent keeps fold from removing its directory before the `closed_workspace_days` window has passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerHold {
    /// A worker process of the agent has not ended.
    Process,
    /// The agent has an open session.
    Session,
}

/// Why fold removes the directory of a closed workspace now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalReason {
    /// Its owning agent has no worker process that has not ended and no open session.
    OwnerEnded,
    /// It closed longer than `closed_workspace_days` ago, past which its owner does not hold it back.
    PastWindow,
}

/// Why fold keeps the directory of a closed workspace for now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepReason {
    /// A worker process of its owning agent has not ended, and the window has not passed.
    OwnerProcess,
    /// Its owning agent has an open session, and the window has not passed.
    OwnerSession,
    /// An open session works in the directory.
    SessionInside,
    /// The path names no directory directly below `.fold/ws/`.
    InvalidPath,
    /// The directory, or one below it, is on another file system than `.fold/ws/`, such as a mount, whose files a delete would remove.
    OtherFilesystem,
    /// The removal failed with this code, such as `unrecorded_changes`; the next run tries again.
    Failed(&'static str),
}

/// What fold does, or would do, with the directory of one closed workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosedDecision {
    Remove(RemovalReason),
    Keep(KeepReason),
}

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// The instant `days` days before `now`; `None` when it lies before the earliest timestamp, so nothing is before it.
pub fn window_cutoff(now: Timestamp, days: u64) -> Option<Timestamp> {
    u32::try_from(days)
        .ok()
        .and_then(|days| DAY.checked_mul(days))
        .and_then(|window| now.checked_sub(window))
}

/// The instant before which a closed workspace's owner no longer holds its directory back, or `None` when the window never overrides the owner. `config.toml` refuses `closed_workspace_days = 0`; a configuration built with 0 keeps the owner's hold too, because a workspace can close in the middle of its worker's turn, such as when a lead releases the claim, and its directory must not go while that worker still runs in it.
pub fn owner_hold_cutoff(now: Timestamp, days: u64) -> Option<Timestamp> {
    if days == 0 {
        None
    } else {
        window_cutoff(now, days)
    }
}

/// What fold does with the directory of a closed workspace: an open session working in it keeps it; otherwise it goes once nothing of its owner holds it, or once it closed before the cutoff of [`owner_hold_cutoff`].
pub fn decide_closed(
    session_inside: bool,
    hold: Option<OwnerHold>,
    past_window: bool,
) -> ClosedDecision {
    if session_inside {
        return ClosedDecision::Keep(KeepReason::SessionInside);
    }
    match hold {
        None => ClosedDecision::Remove(RemovalReason::OwnerEnded),
        Some(_) if past_window => ClosedDecision::Remove(RemovalReason::PastWindow),
        Some(OwnerHold::Process) => ClosedDecision::Keep(KeepReason::OwnerProcess),
        Some(OwnerHold::Session) => ClosedDecision::Keep(KeepReason::OwnerSession),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis).unwrap()
    }

    #[test]
    fn a_session_inside_keeps_the_directory_whatever_the_owner_does() {
        for hold in [None, Some(OwnerHold::Process), Some(OwnerHold::Session)] {
            for past in [false, true] {
                assert_eq!(
                    decide_closed(true, hold, past),
                    ClosedDecision::Keep(KeepReason::SessionInside)
                );
            }
        }
    }

    #[test]
    fn the_owner_holds_the_directory_until_the_window_has_passed() {
        assert_eq!(
            decide_closed(false, None, false),
            ClosedDecision::Remove(RemovalReason::OwnerEnded)
        );
        assert_eq!(
            decide_closed(false, None, true),
            ClosedDecision::Remove(RemovalReason::OwnerEnded)
        );
        assert_eq!(
            decide_closed(false, Some(OwnerHold::Process), false),
            ClosedDecision::Keep(KeepReason::OwnerProcess)
        );
        assert_eq!(
            decide_closed(false, Some(OwnerHold::Session), false),
            ClosedDecision::Keep(KeepReason::OwnerSession)
        );
        assert_eq!(
            decide_closed(false, Some(OwnerHold::Session), true),
            ClosedDecision::Remove(RemovalReason::PastWindow)
        );
    }

    #[test]
    fn the_window_cutoff_lies_days_before_now() {
        let now = at(30 * 24 * 60 * 60 * 1000);
        assert_eq!(window_cutoff(now, 0), Some(now));
        assert_eq!(window_cutoff(now, 1), Some(at(29 * 24 * 60 * 60 * 1000)));
        assert_eq!(window_cutoff(now, u64::MAX), None);
    }

    #[test]
    fn zero_days_never_overrides_the_owner() {
        let now = at(30 * 24 * 60 * 60 * 1000);
        assert_eq!(owner_hold_cutoff(now, 0), None);
        assert_eq!(owner_hold_cutoff(now, 1), window_cutoff(now, 1));
        assert_eq!(owner_hold_cutoff(now, u64::MAX), None);
    }
}
