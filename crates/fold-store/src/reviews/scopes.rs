use fold_core::reviews::{ReviewScope, ReviewScopeRecord, ScopeRule};
use fold_core::{ProposalId, Timestamp};
use sqlx::SqliteExecutor;

use super::time;
use crate::error::StoreError;
use crate::store::WriteTx;

/// The review lead's newest choice of scope for one proposal version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeChoiceRow {
    pub scope: ReviewScope,
    pub reason: String,
    pub chosen_by: String,
    pub chosen_at: Timestamp,
}

/// Stores a choice of scope for `version` of `proposal` in the caller's write. A version that does not exist inserts nothing and fails with `StoreError::Invalid`.
pub async fn insert_scope_choice(
    tx: &mut WriteTx,
    proposal: ProposalId,
    version: u32,
    scope: ReviewScope,
    reason: &str,
    chosen_by: &str,
    now: Timestamp,
) -> Result<(), StoreError> {
    let (number, scope, now) = (proposal.to_db(), scope.as_str(), now.as_millis());
    let inserted = sqlx::query!(
        "INSERT INTO review_scope_choices (version_id, scope, reason, chosen_by, chosen_at)
         SELECT v.id, ?3, ?4, ?5, ?6 FROM proposal_versions v JOIN proposals p ON p.id = v.proposal_id
         WHERE p.number = ?1 AND v.version = ?2",
        number,
        version,
        scope,
        reason,
        chosen_by,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    if inserted == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid("the proposal version does not exist"))
    }
}

/// The newest choice of scope for `version` of `proposal`, or `None` when the review lead made none.
pub async fn latest_scope_choice(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
    version: u32,
) -> Result<Option<ScopeChoiceRow>, StoreError> {
    let number = proposal.to_db();
    let row = sqlx::query!(
        r#"SELECT c.scope, c.reason, c.chosen_by, c.chosen_at
           FROM review_scope_choices c
           JOIN proposal_versions v ON v.id = c.version_id
           JOIN proposals p ON p.id = v.proposal_id
           WHERE p.number = ?1 AND v.version = ?2
           ORDER BY c.id DESC LIMIT 1"#,
        number,
        version
    )
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(ScopeChoiceRow {
            scope: scope(&row.scope, "review_scope_choices.scope")?,
            reason: row.reason,
            chosen_by: row.chosen_by,
            chosen_at: time(row.chosen_at, "review_scope_choices.chosen_at")?,
        })
    })
    .transpose()
}

/// Stores the scope of the review `review_id` in the caller's write, beside the review row it belongs to.
pub async fn insert_review_scope(
    tx: &mut WriteTx,
    review_id: i64,
    record: &ReviewScopeRecord,
) -> Result<(), StoreError> {
    let rules = serde_json::Value::from(
        record
            .rules
            .iter()
            .map(|rule| rule.as_str())
            .collect::<Vec<_>>(),
    )
    .to_string();
    let (suggested, scope) = (record.suggested.as_str(), record.scope.as_str());
    sqlx::query!(
        "INSERT INTO review_scopes (review_id, since_version, suggested, rules, scope, reason)
         VALUES (?, ?, ?, ?, ?, ?)",
        review_id,
        record.since,
        suggested,
        rules,
        scope,
        record.reason
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

pub(super) fn scope(text: &str, column: &'static str) -> Result<ReviewScope, StoreError> {
    ReviewScope::parse(text).ok_or(StoreError::Corrupt(column))
}

/// The stored scope of a review from its columns, all present or all absent.
pub(super) fn stored_scope(
    since: Option<i64>,
    suggested: Option<&str>,
    rules: Option<&str>,
    chosen: Option<&str>,
    reason: Option<String>,
) -> Result<Option<ReviewScopeRecord>, StoreError> {
    let (Some(since), Some(suggested), Some(rules), Some(chosen)) =
        (since, suggested, rules, chosen)
    else {
        return Ok(None);
    };
    let names: Vec<String> =
        serde_json::from_str(rules).map_err(|_| StoreError::Corrupt("review_scopes.rules"))?;
    let rules = names
        .iter()
        .map(|name| ScopeRule::parse(name).ok_or(StoreError::Corrupt("review_scopes.rules")))
        .collect::<Result<_, _>>()?;
    Ok(Some(ReviewScopeRecord {
        since: u32::try_from(since)
            .map_err(|_| StoreError::Corrupt("review_scopes.since_version"))?,
        suggested: scope(suggested, "review_scopes.suggested")?,
        rules,
        scope: scope(chosen, "review_scopes.scope")?,
        reason,
    }))
}
