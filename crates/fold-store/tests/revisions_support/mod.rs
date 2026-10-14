use fold_core::object::ObjectHash;
use fold_core::revision::{Revision, StoredRevision};
use fold_core::{IntegrationId, RevisionId, Timestamp};
use fold_store::testing::TestProject;
use fold_store::{Store, StoreError, workspaces};

pub fn hash(seed: u8) -> ObjectHash {
    ObjectHash::from_bytes([seed; 32])
}

pub fn id(n: u32) -> RevisionId {
    RevisionId::from_number(n).unwrap()
}

pub fn child(parent: &Revision, tree: u8) -> Revision {
    Revision {
        tree: hash(tree),
        parent: Some(parent.hash()),
        number: id(parent.number.number() + 1),
        integration: None,
        created_at: Timestamp::from_millis(1_000 + i64::from(parent.number.number())).unwrap(),
    }
}

pub fn integration(n: u32) -> Option<IntegrationId> {
    IntegrationId::from_number(n)
}

/// These tests have no object store, so they vouch for the objects themselves.
pub fn stored(revision: &Revision) -> StoredRevision {
    StoredRevision::confirmed(*revision)
}

pub async fn init(project: &TestProject) -> Revision {
    let r1 = workspaces::initial_revision(hash(1), project.now()).unwrap();
    workspaces::create_initial_revision(project.store(), &stored(&r1), None)
        .await
        .unwrap();
    r1
}

pub async fn append(project: &TestProject, revision: &Revision) -> Result<(), StoreError> {
    project
        .store()
        .write(async |tx| {
            workspaces::insert_revision(tx, &stored(revision)).await?;
            Ok(())
        })
        .await
}

pub async fn exec(store: &Store, sql: &str) -> Result<(), StoreError> {
    store
        .write(async |tx| {
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .execute(tx.conn())
                .await?;
            Ok::<_, StoreError>(())
        })
        .await
}

pub async fn count(store: &Store, sql: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .fetch_one(store.read())
        .await
        .unwrap()
}
