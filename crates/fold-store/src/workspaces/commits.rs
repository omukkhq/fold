use fold_core::{RevisionId, Timestamp};
use sqlx::SqlitePool;

use crate::error::StoreError;
use crate::store::WriteTx;

/// The revisions without a `revision_commits` row, oldest first.
pub async fn uncommitted_revisions(pool: &SqlitePool) -> Result<Vec<RevisionId>, StoreError> {
    let numbers = sqlx::query_scalar!(
        r#"SELECT r.number AS "number!: i64"
           FROM revisions r
           LEFT JOIN revision_commits c ON c.revision_id = r.id
           WHERE c.revision_id IS NULL
           ORDER BY r.number"#
    )
    .fetch_all(pool)
    .await?;
    numbers
        .into_iter()
        .map(|number| {
            RevisionId::try_from_db(number).ok_or(StoreError::Corrupt("revisions.number"))
        })
        .collect()
}

/// Records each revision's commit on `branch` in the caller's write transaction. A revision that has a row already keeps it, so recording a commit twice changes nothing.
pub async fn insert_revision_commits(
    tx: &mut WriteTx,
    branch: &str,
    commits: &[(RevisionId, String)],
    now: Timestamp,
) -> Result<(), StoreError> {
    let created_at = now.as_millis();
    for (revision, commit) in commits {
        let number = revision.to_db();
        let inserted = sqlx::query!(
            r#"INSERT INTO revision_commits (revision_id, branch, "commit", created_at)
               SELECT id, ?2, ?3, ?4 FROM revisions WHERE number = ?1
               ON CONFLICT (revision_id) DO NOTHING"#,
            number,
            branch,
            commit,
            created_at
        )
        .execute(tx.conn())
        .await?;
        if inserted.rows_affected() == 0 {
            let known = sqlx::query_scalar!(
                r#"SELECT COUNT(*) AS "count!: i64" FROM revisions WHERE number = ?"#,
                number
            )
            .fetch_one(tx.conn())
            .await?;
            if known == 0 {
                return Err(StoreError::RevisionNotFound(*revision));
            }
        }
    }
    Ok(())
}
