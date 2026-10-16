use fold_core::Timestamp;

use crate::error::StoreError;

/// A stored UTC millisecond time; a value out of range is `Corrupt(column)`.
pub(crate) fn time(millis: i64, column: &'static str) -> Result<Timestamp, StoreError> {
    Timestamp::from_millis(millis).map_err(|_| StoreError::Corrupt(column))
}

/// A stored nullable UTC millisecond time; a value out of range is `Corrupt(column)`.
pub(crate) fn optional_time(
    millis: Option<i64>,
    column: &'static str,
) -> Result<Option<Timestamp>, StoreError> {
    millis.map(|value| time(value, column)).transpose()
}

/// A stored count or other non-negative integer that must fit `T`.
pub(crate) fn count<T: TryFrom<i64>>(value: i64, column: &'static str) -> Result<T, StoreError> {
    T::try_from(value).map_err(|_| StoreError::Corrupt(column))
}
