use fold_core::object::ObjectHash;

use crate::error::StoreError;

pub(super) use crate::columns::time as timestamp;

pub(super) fn tree_hash(text: &str, column: &'static str) -> Result<ObjectHash, StoreError> {
    text.parse().map_err(|_| StoreError::Corrupt(column))
}

pub(super) fn number<T>(
    value: i64,
    convert: fn(i64) -> Option<T>,
    column: &'static str,
) -> Result<T, StoreError> {
    convert(value).ok_or(StoreError::Corrupt(column))
}
