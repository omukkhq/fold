#![cfg(test)]

use fold_core::Timestamp;
use fold_core::merge::ConflictKind;
use fold_store::testing::TestProject;
use fold_store::workspaces::{
    ConflictRow, NewStash, StashState, apply_stash, conflict_stash, drop_stash, gc_roots,
    insert_stash, stash_stack,
};
use fold_store::{Store, StoreError};

use crate::common::exec;
use crate::seed;

async fn push(store: &Store, label: &str) -> i64 {
    store
        .write(async |tx| {
            insert_stash(
                tx,
                &NewStash {
                    base_tree: seed::tree("revision tree"),
                    tree: seed::tree(label),
                    revision: "r1".parse().unwrap(),
                    actor: "human:ana",
                    now: Timestamp::EPOCH,
                },
            )
            .await
        })
        .await
        .unwrap()
}

async fn states(store: &Store) -> Vec<(String, StashState)> {
    let mut conn = store.read().acquire().await.unwrap();
    stash_stack(&mut conn)
        .await
        .unwrap()
        .into_iter()
        .map(|entry| (entry.tree.to_string(), entry.state))
        .collect()
}

#[tokio::test]
async fn the_stack_is_newest_first_and_an_entry_only_moves_forward() {
    let project = TestProject::new().await;
    let store = project.store();
    seed::workspace(store, 1).await;
    let older = push(store, "older").await;
    let newer = push(store, "newer").await;
    assert_eq!(
        states(store).await,
        [
            (seed::tree("newer").to_string(), StashState::Stashed),
            (seed::tree("older").to_string(), StashState::Stashed),
        ]
    );
    let at = Timestamp::from_millis(5).unwrap();
    let rows = [ConflictRow {
        path: b"a.txt".to_vec(),
        kind: ConflictKind::BothModifiedText,
    }];
    let moved = store
        .write(async |tx| {
            Ok::<_, StoreError>((
                drop_stash(tx, newer, at).await?,
                conflict_stash(tx, newer, &rows, at).await?,
                apply_stash(tx, newer, at).await?,
            ))
        })
        .await
        .unwrap();
    // A stashed entry is not dropped, and a conflicted one is not applied.
    assert_eq!(moved, (false, true, false));
    let mut conn = store.read().acquire().await.unwrap();
    let stack = stash_stack(&mut conn).await.unwrap();
    drop(conn);
    assert_eq!(stack[0].conflicted_at, Some(at));
    assert_eq!(stack[0].conflicts, rows);
    assert_eq!(stack[1].conflicts, []);
    // One entry at most is conflicted.
    assert!(
        exec(
            store,
            &format!("UPDATE folder_stashes SET state = 'conflicted', conflicted_at = 1 WHERE id = {older}")
        )
        .await
        .is_err()
    );
    let roots = gc_roots(store.read()).await.unwrap();
    assert!(roots.contains(&seed::tree("newer")) && roots.contains(&seed::tree("older")));

    assert!(
        store
            .write(async |tx| drop_stash(tx, newer, at).await)
            .await
            .unwrap()
    );
    assert!(
        store
            .write(async |tx| apply_stash(tx, older, at).await)
            .await
            .unwrap()
    );
    assert!(states(store).await.is_empty());
    assert!(
        !gc_roots(store.read())
            .await
            .unwrap()
            .contains(&seed::tree("newer"))
    );
    for sql in [
        "UPDATE folder_stashes SET state = 'stashed', closed_at = NULL, conflicted_at = NULL",
        "UPDATE folder_stashes SET tree_hash = 'other'",
        "DELETE FROM folder_stashes",
        "UPDATE folder_stash_conflicts SET kind = 'add_add'",
        "DELETE FROM folder_stash_conflicts",
        "INSERT INTO folder_stashes (number, base_tree_hash, tree_hash, revision_id, actor, state, created_at) VALUES (9, 't', 't', 1, 'human:ana', 'stashed', 0)",
        "INSERT INTO folder_stashes (number, base_tree_hash, tree_hash, revision_id, actor, state, created_at) VALUES (9, 't', 'u', 1, 'frontend', 'stashed', 0)",
    ] {
        assert!(exec(store, sql).await.is_err(), "{sql}");
    }
    // An entry on a revision that does not exist is refused.
    let missing = store
        .write(async |tx| {
            insert_stash(
                tx,
                &NewStash {
                    base_tree: seed::tree("a"),
                    tree: seed::tree("b"),
                    revision: "r9".parse().unwrap(),
                    actor: "human:ana",
                    now: Timestamp::EPOCH,
                },
            )
            .await
        })
        .await;
    assert!(matches!(missing, Err(StoreError::RevisionNotFound(_))));
}
