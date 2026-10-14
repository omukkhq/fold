use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::{FoldError, codes};

/// `0000-01-01T00:00:00.000Z`.
const MIN_MILLIS: i64 = -62_167_219_200_000;
/// `9999-12-31T23:59:59.999Z`.
const MAX_MILLIS: i64 = 253_402_300_799_999;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TimeError {
    #[error("timestamp is outside 0000-01-01 to 9999-12-31")]
    OutOfRange,
    #[error("text is not an RFC 3339 timestamp")]
    Unparseable,
}

impl TimeError {
    pub fn code(&self) -> &'static str {
        codes::INVALID_TIMESTAMP
    }
}

impl From<TimeError> for FoldError {
    fn from(error: TimeError) -> Self {
        FoldError::new(error.code(), error.to_string())
    }
}

/// UTC milliseconds since the epoch. The range is limited to the years 0000 to 9999 so `Display` never fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(i64);

impl Timestamp {
    pub fn from_millis(millis: i64) -> Result<Self, TimeError> {
        if (MIN_MILLIS..=MAX_MILLIS).contains(&millis) {
            Ok(Self(millis))
        } else {
            Err(TimeError::OutOfRange)
        }
    }

    /// For values that were checked when they were stored.
    pub(crate) fn from_valid_millis(millis: i64) -> Self {
        Self(millis)
    }

    pub fn as_millis(self) -> i64 {
        self.0
    }

    /// Converts a value the caller already read; `fold-core` never reads the system clock itself. Sub-millisecond precision is dropped.
    pub fn from_system_time(time: SystemTime) -> Result<Self, TimeError> {
        let millis = match time.duration_since(UNIX_EPOCH) {
            Ok(after) => i64::try_from(after.as_millis()).map_err(|_| TimeError::OutOfRange)?,
            Err(before) => {
                let back = i64::try_from(before.duration().as_nanos().div_ceil(1_000_000))
                    .map_err(|_| TimeError::OutOfRange)?;
                -back
            }
        };
        Self::from_millis(millis)
    }

    pub fn checked_add(self, duration: Duration) -> Option<Self> {
        let delta = i64::try_from(duration.as_millis()).ok()?;
        Self::from_millis(self.0.checked_add(delta)?).ok()
    }

    pub fn checked_sub(self, duration: Duration) -> Option<Self> {
        let delta = i64::try_from(duration.as_millis()).ok()?;
        Self::from_millis(self.0.checked_sub(delta)?).ok()
    }

    /// `None` when `earlier` is later than `self`.
    pub fn duration_since(self, earlier: Timestamp) -> Option<Duration> {
        let delta = self.0.checked_sub(earlier.0)?;
        u64::try_from(delta).ok().map(Duration::from_millis)
    }

    /// Accepts any offset and converts to UTC; sub-millisecond digits are truncated.
    pub fn parse_rfc3339(text: &str) -> Result<Self, TimeError> {
        let parsed = OffsetDateTime::parse(text, &Rfc3339).map_err(|_| TimeError::Unparseable)?;
        let millis = parsed.unix_timestamp_nanos().div_euclid(1_000_000);
        let millis = i64::try_from(millis).map_err(|_| TimeError::OutOfRange)?;
        Self::from_millis(millis)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let nanos = i128::from(self.0) * 1_000_000;
        let moment = OffsetDateTime::from_unix_timestamp_nanos(nanos).map_err(|_| fmt::Error)?;
        write!(
            f,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            moment.year(),
            u8::from(moment.month()),
            moment.day(),
            moment.hour(),
            moment.minute(),
            moment.second(),
            moment.millisecond(),
        )
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(self.0)
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let millis = i64::deserialize(deserializer)?;
        Self::from_millis(millis).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn ts(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis).unwrap()
    }

    #[test]
    fn timestamp_formats_utc_millis() {
        assert_eq!(ts(0).to_string(), "1970-01-01T00:00:00.000Z");
        assert_eq!(ts(86_400_000).to_string(), "1970-01-02T00:00:00.000Z");
        assert_eq!(ts(951_782_400_000).to_string(), "2000-02-29T00:00:00.000Z");
        assert_eq!(
            ts(1_760_000_000_123).to_string(),
            "2025-10-09T08:53:20.123Z"
        );
        assert_eq!(ts(-1).to_string(), "1969-12-31T23:59:59.999Z");
        assert_eq!(ts(MIN_MILLIS).to_string(), "0000-01-01T00:00:00.000Z");
        assert_eq!(ts(MAX_MILLIS).to_string(), "9999-12-31T23:59:59.999Z");
    }

    #[test]
    fn timestamp_rejects_out_of_range() {
        assert_eq!(
            Timestamp::from_millis(MAX_MILLIS + 1),
            Err(TimeError::OutOfRange)
        );
        assert_eq!(
            Timestamp::from_millis(MIN_MILLIS - 1),
            Err(TimeError::OutOfRange)
        );
        assert_eq!(Timestamp::from_millis(i64::MAX), Err(TimeError::OutOfRange));
        assert_eq!(Timestamp::from_millis(i64::MIN), Err(TimeError::OutOfRange));
        assert!(serde_json::from_str::<Timestamp>("253402300800000").is_err());
        assert_eq!(serde_json::from_str::<Timestamp>("5").unwrap(), ts(5));
        assert_eq!(serde_json::to_string(&ts(5)).unwrap(), "5");
        assert_eq!(
            Timestamp::parse_rfc3339("10000-01-01T00:00:00Z"),
            Err(TimeError::Unparseable)
        );
        assert_eq!(
            Timestamp::parse_rfc3339("nope"),
            Err(TimeError::Unparseable)
        );
        let fold = FoldError::from(TimeError::OutOfRange);
        assert_eq!(fold.code(), "invalid_timestamp");
    }

    #[test]
    fn rfc3339_accepts_offsets_and_truncates() {
        let parsed = Timestamp::parse_rfc3339("1970-01-01T01:00:00.0019+01:00").unwrap();
        assert_eq!(parsed, ts(1));
        let before = Timestamp::parse_rfc3339("1969-12-31T23:59:59.9995Z").unwrap();
        assert_eq!(before, ts(-1));
    }

    proptest! {
        #[test]
        fn timestamp_round_trips_through_rfc3339(millis in MIN_MILLIS..=MAX_MILLIS) {
            let original = ts(millis);
            prop_assert_eq!(Timestamp::parse_rfc3339(&original.to_string()), Ok(original));
        }
    }

    #[test]
    fn timestamp_arithmetic_checks_overflow() {
        let base = ts(1_000);
        assert_eq!(
            base.checked_add(Duration::from_millis(500)),
            Some(ts(1_500))
        );
        assert_eq!(base.checked_sub(Duration::from_millis(500)), Some(ts(500)));
        assert_eq!(ts(MAX_MILLIS).checked_add(Duration::from_millis(1)), None);
        assert_eq!(ts(MIN_MILLIS).checked_sub(Duration::from_millis(1)), None);
        assert_eq!(base.checked_add(Duration::MAX), None);
        assert_eq!(base.checked_sub(Duration::MAX), None);
        assert_eq!(
            ts(MAX_MILLIS).duration_since(ts(MIN_MILLIS)),
            Some(Duration::from_millis((MAX_MILLIS - MIN_MILLIS) as u64))
        );
        assert_eq!(ts(5).duration_since(ts(2)), Some(Duration::from_millis(3)));
        assert_eq!(ts(2).duration_since(ts(5)), None);
    }

    #[test]
    fn system_time_converts_to_timestamp() {
        let after = UNIX_EPOCH + Duration::from_millis(1_500);
        assert_eq!(Timestamp::from_system_time(after), Ok(ts(1_500)));
        let before = UNIX_EPOCH - Duration::from_millis(1_500);
        assert_eq!(Timestamp::from_system_time(before), Ok(ts(-1_500)));
        let far = UNIX_EPOCH + Duration::from_secs(u64::MAX / 2);
        assert_eq!(Timestamp::from_system_time(far), Err(TimeError::OutOfRange));
    }
}
