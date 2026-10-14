use fold_core::IdKind;

use crate::error::StoreError;
use crate::store::WriteTx;

/// Takes the next public number of `kind`. The number is consumed only if the surrounding write commits, and a committed number is never handed out again.
pub async fn allocate(tx: &mut WriteTx, kind: IdKind) -> Result<u32, StoreError> {
    let key = kind.counter_key();
    let number = sqlx::query_scalar!(
        r#"INSERT INTO counters (kind, next) VALUES (?, 2)
           ON CONFLICT (kind) DO UPDATE SET next = next + 1
           RETURNING next - 1 AS "number!: i64""#,
        key
    )
    .fetch_one(tx.conn())
    .await?;
    u32::try_from(number).map_err(|_| StoreError::Corrupt("counters.next"))
}
