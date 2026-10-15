use fold_core::RevisionId;

use sqlx::SqliteExecutor;

use crate::error::StoreError;
use crate::store::WriteTx;

/// Inserts the `main` ref. It is created once, together with `r1`.
pub async fn create_main(tx: &mut WriteTx, revision: RevisionId) -> Result<(), StoreError> {
    let number = revision.to_db();
    let inserted = sqlx::query!(
        "INSERT INTO refs (name, revision_id)
         SELECT 'main', id FROM revisions WHERE number = ?
         ON CONFLICT (name) DO NOTHING",
        number
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    if inserted == 1 {
        return Ok(());
    }
    let exists =
        sqlx::query_scalar!(r#"SELECT COUNT(*) AS "count!: i64" FROM refs WHERE name = 'main'"#)
            .fetch_one(tx.conn())
            .await?;
    if exists > 0 {
        Err(StoreError::Invalid("the main ref already exists"))
    } else {
        Err(StoreError::RevisionNotFound(revision))
    }
}

/// Moves `main` from `expected` to `new` by compare-and-swap. The only statement that updates `refs`. A head that is not `expected` returns `HeadMoved` and changes nothing.
pub async fn advance_main(
    tx: &mut WriteTx,
    expected: RevisionId,
    new: RevisionId,
) -> Result<(), StoreError> {
    let (expected_number, new_number) = (expected.to_db(), new.to_db());
    let known = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!: i64" FROM revisions WHERE number = ?"#,
        new_number
    )
    .fetch_one(tx.conn())
    .await?;
    if known == 0 {
        return Err(StoreError::RevisionNotFound(new));
    }
    let moved = sqlx::query!(
        "UPDATE refs
         SET revision_id = (SELECT id FROM revisions WHERE number = ?1)
         WHERE name = 'main'
           AND revision_id = (SELECT id FROM revisions WHERE number = ?2)",
        new_number,
        expected_number
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    if moved == 1 {
        Ok(())
    } else {
        Err(StoreError::HeadMoved { expected })
    }
}

/// The number of the revision `main` names, or `None` before `r1` exists. On the writer's connection it is the head a compare-and-swap in the same transaction will find.
pub async fn main_head(
    executor: impl SqliteExecutor<'_>,
) -> Result<Option<RevisionId>, StoreError> {
    let number = sqlx::query_scalar!(
        r#"SELECT r.number AS "number!: i64"
           FROM refs JOIN revisions r ON r.id = refs.revision_id
           WHERE refs.name = 'main'"#
    )
    .fetch_optional(executor)
    .await?;
    number
        .map(|number| {
            RevisionId::try_from_db(number).ok_or(StoreError::Corrupt("revisions.number"))
        })
        .transpose()
}
