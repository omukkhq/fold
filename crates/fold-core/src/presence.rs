use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::time::Timestamp;

/// How long after its last call a connected session counts as active. It is fixed, so every reader agrees on presence without a setting.
pub const ACTIVE_WINDOW_MS: u64 = 120_000;

/// An agent's presence. It is never stored: every read calculates it from the agent's sessions and the time. The order runs from least to most present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    Offline,
    Idle,
    Active,
}

impl Presence {
    pub fn as_str(self) -> &'static str {
        match self {
            Presence::Offline => "offline",
            Presence::Idle => "idle",
            Presence::Active => "active",
        }
    }
}

impl fmt::Display for Presence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What presence reads from one session row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionFact {
    /// True once the session has an `ended_at`.
    pub ended: bool,
    pub last_call_at: Option<Timestamp>,
}

/// `active` when a connected session called less than [`ACTIVE_WINDOW_MS`] before `now`, `idle` when a session is connected but none is active, `offline` when no session is connected. A call later than `now` counts as recent, as a lease renewed in the future is not stale.
pub fn presence_of(now: Timestamp, sessions: &[SessionFact]) -> Presence {
    let window = Duration::from_millis(ACTIVE_WINDOW_MS);
    let mut presence = Presence::Offline;
    for session in sessions.iter().filter(|session| !session.ended) {
        let recent = session.last_call_at.is_some_and(|at| {
            now.duration_since(at)
                .is_none_or(|elapsed| elapsed < window)
        });
        if recent {
            return Presence::Active;
        }
        presence = Presence::Idle;
    }
    presence
}
