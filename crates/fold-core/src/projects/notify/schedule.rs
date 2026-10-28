use std::time::Duration;

use crate::time::Timestamp;

/// The longest a send waits for its receiver.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(15);

/// The waits before the second to the seventh try of a send; after the seventh fold gives the send up, about 8.5 hours after the first.
pub const RETRIES: [Duration; 6] = [
    Duration::from_secs(10),
    Duration::from_secs(60),
    Duration::from_secs(5 * 60),
    Duration::from_secs(30 * 60),
    Duration::from_secs(2 * 60 * 60),
    Duration::from_secs(6 * 60 * 60),
];

/// The longest `Retry-After` fold waits for.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(6 * 60 * 60);

/// Failed tries in a row that turn a target off.
pub const OFF_AFTER_FAILURES: u32 = 20;

/// When the pending notices of a target go out: `window` after the first of them, however many come after it, so a steady stream still sends once per window.
pub fn due_at(first_pending: Timestamp, window: Duration) -> Timestamp {
    first_pending.checked_add(window).unwrap_or(first_pending)
}

/// How a receiver answered one try.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// An HTTP status, with the wait a `Retry-After` header asked for.
    Status {
        code: u16,
        retry_after: Option<Duration>,
    },
    Timeout,
    HostNotFound,
    Tls,
    Unreachable,
}

impl Answer {
    /// The log's word for an answer without a status.
    pub fn error_kind(self) -> Option<&'static str> {
        match self {
            Answer::Status { .. } => None,
            Answer::Timeout => Some("timeout"),
            Answer::HostNotFound => Some("host_not_found"),
            Answer::Tls => Some("tls"),
            Answer::Unreachable => Some("unreachable"),
        }
    }

    pub fn status(self) -> Option<u16> {
        match self {
            Answer::Status { code, .. } => Some(code),
            _ => None,
        }
    }
}

/// What a try's answer means for its send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Judgement {
    /// A 2xx: the receiver took the send.
    Delivered,
    /// 410 Gone: the receiver is gone, so the target turns off at once.
    Gone,
    /// The send tries again at this time.
    Retry(Timestamp),
    /// The last try failed.
    GiveUp,
}

/// When the next try of a send whose `tries` tries failed goes, honouring a `Retry-After` up to [`MAX_RETRY_AFTER`]; `None` once every retry is spent.
pub fn next_try(tries: u32, retry_after: Option<Duration>, now: Timestamp) -> Option<Timestamp> {
    let index = usize::try_from(tries.checked_sub(1)?).ok()?;
    let wait = *RETRIES.get(index)?;
    let wait = retry_after.map_or(wait, |asked| wait.max(asked.min(MAX_RETRY_AFTER)));
    now.checked_add(wait)
}

/// The meaning of `answer` to the try number `tries` of a send: a redirect, a 4xx, a 5xx and no answer are failures.
pub fn judge(answer: Answer, tries: u32, now: Timestamp) -> Judgement {
    match answer {
        Answer::Status { code, .. } if (200..300).contains(&code) => Judgement::Delivered,
        Answer::Status { code: 410, .. } => Judgement::Gone,
        Answer::Status { retry_after, .. } => {
            next_try(tries, retry_after, now).map_or(Judgement::GiveUp, Judgement::Retry)
        }
        _ => next_try(tries, None, now).map_or(Judgement::GiveUp, Judgement::Retry),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_millis(seconds * 1000).unwrap()
    }

    #[test]
    fn the_window_starts_with_the_first_notice() {
        assert_eq!(due_at(at(100), Duration::from_secs(15)), at(115));
        assert_eq!(due_at(at(100), Duration::ZERO), at(100));
    }

    #[test]
    fn a_send_tries_seven_times_and_honours_retry_after() {
        let waits: Vec<i64> = (1..=6)
            .map(|tries| next_try(tries, None, at(0)).unwrap().as_millis() / 1000)
            .collect();
        assert_eq!(waits, [10, 60, 300, 1800, 7200, 21600]);
        assert_eq!(next_try(7, None, at(0)), None);
        assert_eq!(next_try(0, None, at(0)), None);
        assert_eq!(
            next_try(1, Some(Duration::from_secs(120)), at(0)),
            Some(at(120))
        );
        assert_eq!(
            next_try(2, Some(Duration::from_secs(5)), at(0)),
            Some(at(60))
        );
        assert_eq!(
            next_try(1, Some(Duration::from_secs(10 * 86_400)), at(0)),
            Some(at(21_600))
        );
    }

    #[test]
    fn answers_are_judged_by_their_status() {
        let status = |code| Answer::Status {
            code,
            retry_after: None,
        };
        assert_eq!(judge(status(204), 1, at(0)), Judgement::Delivered);
        assert_eq!(judge(status(410), 1, at(0)), Judgement::Gone);
        for code in [301, 404, 503] {
            assert_eq!(judge(status(code), 1, at(0)), Judgement::Retry(at(10)));
            assert_eq!(judge(status(code), 7, at(0)), Judgement::GiveUp);
        }
        assert_eq!(judge(Answer::Timeout, 2, at(0)), Judgement::Retry(at(60)));
        assert_eq!(judge(Answer::Tls, 7, at(0)), Judgement::GiveUp);
        assert_eq!(status(503).status(), Some(503));
        assert_eq!(status(503).error_kind(), None);
        assert_eq!(Answer::Timeout.status(), None);
        for (answer, kind) in [
            (Answer::Timeout, "timeout"),
            (Answer::HostNotFound, "host_not_found"),
            (Answer::Tls, "tls"),
            (Answer::Unreachable, "unreachable"),
        ] {
            assert_eq!(answer.error_kind(), Some(kind));
        }
    }

    proptest! {
        #[test]
        fn a_later_try_never_comes_sooner(tries in 1u32..7, asked in proptest::option::of(0u64..100_000)) {
            let asked = asked.map(Duration::from_secs);
            let this = next_try(tries, asked, at(1000)).unwrap();
            prop_assert!(this > at(1000));
            if let Some(next) = next_try(tries + 1, None, at(1000)) {
                prop_assert!(next >= next_try(tries, None, at(1000)).unwrap());
            }
            prop_assert!(this <= at(1000).checked_add(MAX_RETRY_AFTER).unwrap());
        }

        #[test]
        fn the_window_never_moves_back(first in 0i64..1_000_000_000, window in 0u64..3601) {
            let first = Timestamp::from_millis(first).unwrap();
            prop_assert!(due_at(first, Duration::from_secs(window)) >= first);
        }
    }
}
