use fold_core::reviews::ProposalIntent;

use crate::error::StoreError;
use crate::store::WriteTx;

/// Writes the parts, their files and the other sections of the version with row id `version_id`, whose row already holds the summary, in the caller's write. The database refuses a file in two parts of the version.
pub(super) async fn insert(
    tx: &mut WriteTx,
    version_id: i64,
    intent: &ProposalIntent,
) -> Result<(), StoreError> {
    for (position, part) in (1_i64..).zip(&intent.parts) {
        let part_id = sqlx::query_scalar!(
            r#"INSERT INTO proposal_intent_parts (version_id, position, name, does)
               VALUES (?1, ?2, ?3, ?4) RETURNING id AS "id!: i64""#,
            version_id,
            position,
            part.name,
            part.does
        )
        .fetch_one(tx.conn())
        .await?;
        for (file_position, path) in (1_i64..).zip(&part.files) {
            sqlx::query!(
                "INSERT INTO proposal_intent_files (part_id, version_id, position, path)
                 VALUES (?1, ?2, ?3, ?4)",
                part_id,
                version_id,
                file_position,
                path
            )
            .execute(tx.conn())
            .await?;
        }
    }
    let kinds = intent
        .contracts
        .by_kind()
        .map(|(kind, entries)| (kind.as_str(), entries));
    let sections = kinds.into_iter().chain([
        ("decisions", intent.decisions.as_slice()),
        ("not_changed", intent.not_changed.as_slice()),
    ]);
    for (section, entries) in sections {
        for (position, text) in (1_i64..).zip(entries) {
            sqlx::query!(
                "INSERT INTO proposal_intent_entries (version_id, section, position, text)
                 VALUES (?1, ?2, ?3, ?4)",
                version_id,
                section,
                position,
                text
            )
            .execute(tx.conn())
            .await?;
        }
    }
    Ok(())
}

/// The intent as a check run carries it to the version it waits to make.
pub(crate) fn encode(intent: &ProposalIntent) -> Result<String, StoreError> {
    serde_json::to_string(intent).map_err(|_| StoreError::Invalid("the intent does not encode"))
}

/// An intent stored as JSON, by a check run or the `proposal_version_intents` view; `column` names it when it no longer decodes.
pub(crate) fn decode(
    text: Option<&str>,
    column: &'static str,
) -> Result<Option<ProposalIntent>, StoreError> {
    text.map(|text| serde_json::from_str(text).map_err(|_| StoreError::Corrupt(column)))
        .transpose()
}
