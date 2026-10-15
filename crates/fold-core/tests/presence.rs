use fold_core::{ACTIVE_WINDOW_MS, Presence, SessionFact, Timestamp, presence_of};
use proptest::prelude::*;

const NOW: i64 = 1_800_000_000_000;
const WINDOW: i64 = ACTIVE_WINDOW_MS as i64;

#[cfg(test)]
fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).unwrap()
}

#[cfg(test)]
fn open(ago: i64) -> SessionFact {
    SessionFact {
        ended: false,
        last_call_at: Some(at(NOW - ago)),
    }
}

#[cfg(test)]
fn ended(ago: i64) -> SessionFact {
    SessionFact {
        ended: true,
        last_call_at: Some(at(NOW - ago)),
    }
}

#[test]
fn the_window_is_two_minutes() {
    assert_eq!(ACTIVE_WINDOW_MS, 120_000);
}

#[test]
fn presence_follows_the_connected_sessions_and_their_last_call() {
    let never_called = SessionFact {
        ended: false,
        last_call_at: None,
    };
    let cases: Vec<(&str, Vec<SessionFact>, Presence)> = vec![
        ("no session", vec![], Presence::Offline),
        ("a call just now", vec![open(0)], Presence::Active),
        (
            "a call one ms inside the window",
            vec![open(WINDOW - 1)],
            Presence::Active,
        ),
        (
            "a call exactly two minutes ago",
            vec![open(WINDOW)],
            Presence::Idle,
        ),
        (
            "a call older than the window",
            vec![open(WINDOW + 1)],
            Presence::Idle,
        ),
        (
            "a connected session that never called",
            vec![never_called],
            Presence::Idle,
        ),
        (
            "an ended session with a recent call",
            vec![ended(0)],
            Presence::Offline,
        ),
        (
            "only ended sessions",
            vec![ended(0), ended(10 * WINDOW)],
            Presence::Offline,
        ),
        (
            "an ended recent call beside an old open one",
            vec![ended(0), open(5 * WINDOW)],
            Presence::Idle,
        ),
        (
            "one recent among several open",
            vec![open(5 * WINDOW), open(1_000), open(9 * WINDOW)],
            Presence::Active,
        ),
        (
            "a call later than now",
            vec![open(-5_000)],
            Presence::Active,
        ),
    ];
    for (name, sessions, expected) in cases {
        assert_eq!(presence_of(at(NOW), &sessions), expected, "{name}");
    }
}

#[test]
fn presence_moves_to_idle_as_time_passes() {
    let sessions = [SessionFact {
        ended: false,
        last_call_at: Some(at(NOW)),
    }];
    assert_eq!(
        presence_of(at(NOW + WINDOW - 1), &sessions),
        Presence::Active
    );
    assert_eq!(presence_of(at(NOW + WINDOW), &sessions), Presence::Idle);
}

#[test]
fn presence_has_stable_names_and_order() {
    assert_eq!(Presence::Active.as_str(), "active");
    assert_eq!(Presence::Idle.to_string(), "idle");
    assert_eq!(
        serde_json::to_value(Presence::Offline).unwrap(),
        serde_json::json!("offline")
    );
    assert_eq!(
        serde_json::from_value::<Presence>(serde_json::json!("active")).unwrap(),
        Presence::Active
    );
    assert!(Presence::Offline < Presence::Idle && Presence::Idle < Presence::Active);
}

#[cfg(test)]
fn session() -> impl Strategy<Value = SessionFact> {
    (any::<bool>(), proptest::option::of(-WINDOW..10 * WINDOW)).prop_map(|(ended, ago)| {
        SessionFact {
            ended,
            last_call_at: ago.map(|ago| at(NOW - ago)),
        }
    })
}

proptest! {
    #[test]
    fn adding_an_ended_session_never_improves_presence(
        sessions in proptest::collection::vec(session(), 0..6),
        ago in proptest::option::of(-WINDOW..10 * WINDOW),
        position in 0usize..7,
    ) {
        let before = presence_of(at(NOW), &sessions);
        let mut more = sessions.clone();
        let extra = SessionFact { ended: true, last_call_at: ago.map(|ago| at(NOW - ago)) };
        more.insert(position.min(more.len()), extra);
        prop_assert_eq!(presence_of(at(NOW), &more), before);
    }

    #[test]
    fn presence_does_not_depend_on_session_order(
        mut sessions in proptest::collection::vec(session(), 0..6),
    ) {
        let before = presence_of(at(NOW), &sessions);
        sessions.reverse();
        prop_assert_eq!(presence_of(at(NOW), &sessions), before);
    }
}
