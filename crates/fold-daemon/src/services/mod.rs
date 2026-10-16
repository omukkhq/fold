pub mod agents;
pub mod error;
pub mod events;
pub mod messages;
pub mod projects;
pub mod reviews;
pub mod tasks;
pub mod workspaces;

/// The error a fired failpoint returns, as a crash at that point would stop the command.
#[cfg(feature = "failpoints")]
pub(crate) fn fired(name: &str) -> fold_core::FoldError {
    fold_core::FoldError::new(fold_core::codes::FAILPOINT, format!("{name} fired"))
}

/// A listing's `limit`: 0 is refused with `invalid_request`, a value over `max` is lowered to it, and `None` stays `None` for the listing's own default.
pub(crate) fn page_limit(
    limit: Option<u32>,
    max: u32,
) -> Result<Option<u32>, fold_core::FoldError> {
    match limit {
        Some(0) => Err(fold_core::FoldError::new(
            fold_core::codes::INVALID_REQUEST,
            "limit must be at least 1",
        )
        .with_details([("field", serde_json::json!("limit"))])
        .with_hint("leave out limit for the default page, or pass a limit of 1 or more")),
        limit => Ok(limit.map(|limit| limit.min(max))),
    }
}
