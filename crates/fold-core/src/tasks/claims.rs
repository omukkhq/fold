use std::collections::BTreeSet;
use std::time::Duration;

use crate::AgentName;
use crate::time::{MIN_MILLIS, Timestamp};

/// A claim is stale once `lease_minutes` have passed since its last renewal. Staleness is computed when a claim is read and never stored by this rule. A `renewed_at` later than `now` is not stale.
pub fn is_stale(renewed_at: Timestamp, now: Timestamp, lease_minutes: u64) -> bool {
    let lease = Duration::from_secs(lease_minutes.saturating_mul(60));
    now.duration_since(renewed_at)
        .is_some_and(|elapsed| elapsed >= lease)
}

/// The proposal-stage check runs that a claim's holder started, as its lease sees them: the holder waits for such a run and makes no calls while it goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HolderRuns {
    /// No run of the holder goes or ever ended.
    #[default]
    None,
    /// A run of the holder goes.
    Running,
    /// No run of the holder goes, and its newest run ended `at`.
    Ended { at: Timestamp },
}

/// The time a claim's lease counts from at `now`: its last renewal, or the end of its holder's newest check run when that is later, and `now` while a run of its holder goes. A claim never runs out during its holder's own run, and a full lease follows the run's end.
pub fn lease_start(renewed_at: Timestamp, runs: HolderRuns, now: Timestamp) -> Timestamp {
    match runs {
        HolderRuns::None => renewed_at,
        HolderRuns::Running => renewed_at.max(now),
        HolderRuns::Ended { at } => renewed_at.max(at),
    }
}

/// What one claim of a holder waits for from others, as its lease sees it. The holder makes no calls while it waits, and a person has nothing to do about its claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClaimWait {
    /// An implementation claim whose task has an open proposal that waits on reviewers or the integration: not back with its author by requested changes, a conflict or failed checks.
    pub sent: bool,
    /// An implementation claim whose task has an ask that waits for a person.
    pub asks_person: bool,
    /// An implementation claim whose task has an open report that waits for the lead's check.
    pub reported: bool,
    /// A review claim whose holder gave its verdict on the proposal's newest version.
    pub verdict_given: bool,
}

impl ClaimWait {
    pub fn waits_on_others(self) -> bool {
        self.sent || self.asks_person || self.reported || self.verdict_given
    }
}

/// Why a claim's holder waits for others as its lease sees it: the holder makes no calls, and its next move is someone else's. A new kind of wait, such as a turn that waits for the network, is one more field here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HolderWait {
    /// An agent parked the holder with `stop_worker`'s `wait_for`, and that agent runs.
    pub parked_by_running_agent: bool,
    /// Every claim the holder holds waits on others, by [`ClaimWait::waits_on_others`].
    pub claims_wait_on_others: bool,
    /// The holder's last turn waits for the network or a usage limit while its process runs (see [`crate::turn_waits::shows_wait`]): fold runs the turn again itself once the cause has passed.
    pub turn_waits: bool,
}

impl HolderWait {
    pub fn waits(self) -> bool {
        self.parked_by_running_agent || self.claims_wait_on_others || self.turn_waits
    }
}

/// The holder's runs as its lease sees them: while the holder waits for others (see [`HolderWait`]) its lease holds as during a run of its own, and once the wait ends, the lease counts again from the last renewal, which the sweeper made while the wait lasted.
pub fn holding(runs: HolderRuns, wait: HolderWait) -> HolderRuns {
    if wait.waits() {
        HolderRuns::Running
    } else {
        runs
    }
}

/// Whether a stale claim, marked at `stale_at`, needs a person at `now`: no agent was told of it (`told` is false), or the agent told, the holder's lead or else the main agent, let one more `window_minutes` pass without renewing, releasing or taking over the claim.
pub fn stale_needs_person(
    now: Timestamp,
    stale_at: Timestamp,
    told: bool,
    window_minutes: u64,
) -> bool {
    !told || is_stale(stale_at, now, window_minutes)
}

/// A claim lapsed: it is stale by [`is_stale`] counted from [`lease_start`], so a takeover is allowed.
pub fn is_lapsed(
    renewed_at: Timestamp,
    runs: HolderRuns,
    now: Timestamp,
    lease_minutes: u64,
) -> bool {
    is_stale(lease_start(renewed_at, runs, now), now, lease_minutes)
}

/// A claim is near stale once three quarters of its lease have passed since its last renewal, so its holder hears of it while a quarter of the lease is left. A claim already stale and not yet taken over is near stale too: its holder still keeps it. A claim renewed at or before the returned time is near stale; `None` when the cutoff falls before the first representable time, and then no claim is.
pub fn near_stale_cutoff(now: Timestamp, lease_minutes: u64) -> Option<Timestamp> {
    let lease_millis = lease_minutes.saturating_mul(60_000);
    let warned_after = Duration::from_millis(lease_millis - lease_millis / 4);
    now.checked_sub(warned_after)
}

/// The renewal time at or before which a claim is stale at `now`, by the rule of `is_stale`, so a query can tell stale claims apart without reading each one. A lease longer than the timestamp range reaches back to its earliest value.
pub fn stale_cutoff(now: Timestamp, lease_minutes: u64) -> Timestamp {
    let lease = Duration::from_secs(lease_minutes.saturating_mul(60));
    now.checked_sub(lease)
        .unwrap_or(Timestamp::from_valid_millis(MIN_MILLIS))
}

/// The agents that hear of the stale claim of `holder` first and can act on it now: the agent that started the holder while it runs (`runs`), or else the main agents that run. An agent without a running process or an open session hears no push, so it counts as told only while it runs; an empty list means that no agent can act and the claim needs a person at once. The holder never hears of its own claim.
pub fn stale_claim_receivers(
    holder: &AgentName,
    starter: Option<&AgentName>,
    main_agents: &BTreeSet<AgentName>,
    runs: impl Fn(&AgentName) -> bool,
) -> Vec<AgentName> {
    let receivers: Vec<AgentName> = match starter.filter(|starter| runs(starter)) {
        Some(starter) => vec![starter.clone()],
        None => main_agents
            .iter()
            .filter(|agent| runs(agent))
            .cloned()
            .collect(),
    };
    receivers
        .into_iter()
        .filter(|agent| agent != holder)
        .collect()
}

/// The renewal time at or before which the sweeper renews the claims of a holder that waits at `now`: half a lease ago, so a waiting holder's claim is never older than half its lease and one sweep, without a renewal at every sweep.
pub fn renewal_due(now: Timestamp, lease_minutes: u64) -> Timestamp {
    let half = Duration::from_millis(lease_minutes.saturating_mul(30_000));
    now.checked_sub(half)
        .unwrap_or(Timestamp::from_valid_millis(MIN_MILLIS))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: i64 = 60_000;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis).unwrap()
    }

    #[test]
    fn a_claim_is_stale_from_the_end_of_its_lease() {
        let renewed = at(1_000_000);
        assert!(!is_stale(renewed, at(1_000_000 + 29 * MINUTE), 30));
        assert!(!is_stale(renewed, at(1_000_000 + 30 * MINUTE - 1), 30));
        assert!(is_stale(renewed, at(1_000_000 + 30 * MINUTE), 30));
        assert!(is_stale(renewed, at(1_000_000 + 31 * MINUTE), 30));
    }

    #[test]
    fn the_lease_follows_lease_minutes() {
        let renewed = at(0);
        assert!(!is_stale(renewed, at(4 * MINUTE), 5));
        assert!(is_stale(renewed, at(5 * MINUTE), 5));
        assert!(!is_stale(renewed, at(5 * MINUTE), 6));
        assert!(!is_stale(renewed, at(200_000_000_000_000), u64::MAX));
    }

    #[test]
    fn a_claim_is_near_stale_in_the_last_quarter_of_its_lease() {
        let now = at(100 * MINUTE);
        assert_eq!(near_stale_cutoff(now, 40), Some(at(70 * MINUTE)));
        assert_eq!(near_stale_cutoff(now, 1), Some(at(100 * MINUTE - 45_000)));
        assert_eq!(near_stale_cutoff(now, 0), Some(now));
        assert_eq!(
            near_stale_cutoff(at(10 * MINUTE), 40),
            Some(at(-20 * MINUTE))
        );
        assert_eq!(near_stale_cutoff(now, u64::MAX), None);
    }

    #[test]
    fn the_cutoff_splits_stale_renewals_from_live_ones() {
        let now = at(1_000_000 + 30 * MINUTE);
        let cutoff = stale_cutoff(now, 30);
        assert_eq!(cutoff, at(1_000_000));
        for renewed in [
            cutoff,
            at(1_000_000 - 1),
            at(1_000_000 + 1),
            now,
            at(now.as_millis() + 1),
        ] {
            assert_eq!(renewed <= cutoff, is_stale(renewed, now, 30), "{renewed}");
        }
        assert_eq!(stale_cutoff(now, 0), now);
        assert_eq!(
            stale_cutoff(now, u64::MAX),
            Timestamp::from_millis(MIN_MILLIS).unwrap()
        );
    }

    #[test]
    fn a_renewal_in_the_future_is_not_stale() {
        assert!(!is_stale(at(10 * MINUTE), at(0), 1));
        assert!(!is_stale(at(0), at(0), 30));
    }

    #[test]
    fn a_claim_never_lapses_while_its_holders_run_goes() {
        let renewed = at(0);
        for minutes in [30, 60, 24 * 60] {
            let now = at(minutes * MINUTE);
            assert!(is_stale(renewed, now, 30));
            assert!(!is_lapsed(renewed, HolderRuns::Running, now, 30));
            assert_eq!(lease_start(renewed, HolderRuns::Running, now), now);
        }
    }

    #[test]
    fn a_full_lease_follows_the_end_of_the_holders_newest_run() {
        let renewed = at(0);
        let ended = HolderRuns::Ended {
            at: at(50 * MINUTE),
        };
        assert_eq!(
            lease_start(renewed, ended, at(60 * MINUTE)),
            at(50 * MINUTE)
        );
        assert!(!is_lapsed(renewed, ended, at(80 * MINUTE - 1), 30));
        assert!(is_lapsed(renewed, ended, at(80 * MINUTE), 30));
        assert_eq!(
            lease_start(at(70 * MINUTE), ended, at(90 * MINUTE)),
            at(70 * MINUTE),
            "a call after the run counts"
        );
    }

    #[test]
    fn without_a_run_the_lease_counts_from_the_renewal() {
        let renewed = at(0);
        assert_eq!(HolderRuns::default(), HolderRuns::None);
        assert_eq!(lease_start(renewed, HolderRuns::None, at(MINUTE)), renewed);
        assert!(!is_lapsed(
            renewed,
            HolderRuns::None,
            at(30 * MINUTE - 1),
            30
        ));
        assert!(is_lapsed(renewed, HolderRuns::None, at(30 * MINUTE), 30));
        assert_eq!(
            lease_start(at(10 * MINUTE), HolderRuns::Running, at(5 * MINUTE)),
            at(10 * MINUTE),
            "a renewal later than now stays"
        );
    }

    #[test]
    fn a_parked_holders_claim_never_lapses_while_the_agent_that_parked_it_runs() {
        let renewed = at(0);
        for minutes in [30, 60, 24 * 60] {
            let now = at(minutes * MINUTE);
            let held = holding(
                HolderRuns::None,
                HolderWait {
                    parked_by_running_agent: true,
                    ..HolderWait::default()
                },
            );
            assert_eq!(held, HolderRuns::Running);
            assert!(!is_lapsed(renewed, held, now, 30), "{minutes} min");
        }
        assert_eq!(
            holding(HolderRuns::None, HolderWait::default()),
            HolderRuns::None
        );
        let ended = HolderRuns::Ended { at: at(5) };
        assert_eq!(holding(ended, HolderWait::default()), ended);
        assert!(
            is_lapsed(
                renewed,
                holding(HolderRuns::None, HolderWait::default()),
                at(30 * MINUTE),
                30
            ),
            "once the agent that parked it ends, the lease counts again"
        );
    }

    #[test]
    fn a_claim_that_waits_on_others_never_lapses() {
        for wait in [
            ClaimWait {
                sent: true,
                ..ClaimWait::default()
            },
            ClaimWait {
                asks_person: true,
                ..ClaimWait::default()
            },
            ClaimWait {
                reported: true,
                ..ClaimWait::default()
            },
            ClaimWait {
                verdict_given: true,
                ..ClaimWait::default()
            },
        ] {
            assert!(wait.waits_on_others(), "{wait:?}");
            let held = holding(
                HolderRuns::None,
                HolderWait {
                    claims_wait_on_others: wait.waits_on_others(),
                    ..HolderWait::default()
                },
            );
            assert!(!is_lapsed(at(0), held, at(24 * 60 * MINUTE), 30));
        }
        assert!(!ClaimWait::default().waits_on_others());
    }

    #[test]
    fn a_stale_claim_goes_to_the_lead_first_and_to_a_person_one_window_later() {
        let stale = at(30 * MINUTE);
        assert!(!stale_needs_person(stale, stale, true, 5));
        assert!(!stale_needs_person(at(35 * MINUTE - 1), stale, true, 5));
        assert!(stale_needs_person(at(35 * MINUTE), stale, true, 5));
        assert!(
            stale_needs_person(stale, stale, false, 5),
            "no agent can act, so a person at once"
        );
    }

    #[test]
    fn a_holder_whose_turn_waits_for_the_network_or_a_usage_limit_keeps_its_lease_and_counts_again_from_the_end()
     {
        let waits = HolderWait {
            turn_waits: true,
            ..HolderWait::default()
        };
        assert!(waits.waits());
        let renewed = at(0);
        for minutes in [30, 60, 5 * 60] {
            let now = at(minutes * MINUTE);
            assert_eq!(holding(HolderRuns::None, waits), HolderRuns::Running);
            assert!(!is_lapsed(
                renewed,
                holding(HolderRuns::None, waits),
                now,
                30
            ));
        }
        // The sweeper renewed the claim while the wait lasted; once it ends, the lease counts from that renewal.
        let wait_ended = at(5 * 60 * MINUTE);
        let after = holding(HolderRuns::None, HolderWait::default());
        assert!(!is_lapsed(
            wait_ended,
            after,
            at(5 * 60 * MINUTE + 30 * MINUTE - 1),
            30
        ));
        assert!(is_lapsed(
            wait_ended,
            after,
            at(5 * 60 * MINUTE + 30 * MINUTE),
            30
        ));
        assert!(!HolderWait::default().waits());
    }

    #[test]
    fn a_stale_claim_goes_to_a_running_starter_or_else_to_the_running_main_agents() {
        let name = |text: &str| AgentName::parse(text).unwrap();
        let (worker, lead, main) = (name("web-worker"), name("web-lead"), name("frontend"));
        let mains: BTreeSet<AgentName> = [main.clone()].into();
        let all = |_: &AgentName| true;
        assert_eq!(
            stale_claim_receivers(&worker, Some(&lead), &mains, all),
            vec![lead.clone()]
        );
        assert_eq!(
            stale_claim_receivers(&worker, Some(&lead), &mains, |agent| agent != &lead),
            vec![main.clone()],
            "a lead that does not run hears nothing, so the main agent is told"
        );
        assert!(stale_claim_receivers(&worker, Some(&lead), &mains, |_| false).is_empty());
        assert_eq!(
            stale_claim_receivers(&worker, None, &mains, all),
            vec![main.clone()]
        );
        assert!(stale_claim_receivers(&main, None, &mains, all).is_empty());
    }

    #[test]
    fn a_waiting_claim_is_renewed_after_half_its_lease() {
        let now = Timestamp::from_millis(10 * 60_000).unwrap();
        assert_eq!(renewal_due(now, 10).as_millis(), 5 * 60_000);
        assert_eq!(renewal_due(now, 0), now);
        assert_eq!(renewal_due(now, 60).as_millis(), -20 * 60_000);
        assert_eq!(renewal_due(now, u64::MAX).as_millis(), MIN_MILLIS);
    }
}
