use crate::revisions_support::{append, child, count, exec, hash, id, init, integration, stored};
use fold_core::codes;
use fold_core::revision::Revision;
use fold_store::testing::{TestProject, assert_append_only};
use fold_store::{StoreError, workspaces};

#[tokio::test]
async fn initial_revision_has_no_parent_and_main_points_at_it() {
    let project = TestProject::new().await;
    let r1 = workspaces::initial_revision(hash(1), project.now()).unwrap();
    let row =
        workspaces::create_initial_revision(project.store(), &stored(&r1), Some("/imported/dir"))
            .await
            .unwrap();
    assert_eq!(row.number, id(1));
    assert_eq!(row.parent, None);
    assert_eq!(row.hash, r1.hash());
    assert_eq!(row.tree_hash, hash(1));
    assert_eq!(row.import_source.as_deref(), Some("/imported/dir"));
    assert_eq!(row.integration, None);

    let pool = project.store().read();
    let head = workspaces::head(pool).await.unwrap().unwrap();
    assert_eq!(head, row);
    let fetched = workspaces::get_by_number(pool, id(1)).await.unwrap();
    assert_eq!(fetched.import_source.as_deref(), Some("/imported/dir"));
}

#[tokio::test]
async fn plain_init_has_no_import_source() {
    let project = TestProject::new().await;
    init(&project).await;
    let row = workspaces::get_by_number(project.store().read(), id(1))
        .await
        .unwrap();
    assert_eq!(row.import_source, None);
}

#[tokio::test]
async fn initial_revision_is_created_once() {
    let project = TestProject::new().await;
    let r1 = init(&project).await;
    let again = workspaces::create_initial_revision(project.store(), &stored(&r1), None).await;
    assert!(matches!(again, Err(StoreError::Invalid(_))));
    assert_eq!(
        count(project.store(), "SELECT COUNT(*) FROM revisions").await,
        1
    );

    let with_parent = child(&r1, 2);
    let refused =
        workspaces::create_initial_revision(project.store(), &stored(&with_parent), None).await;
    assert!(matches!(refused, Err(StoreError::Invalid(_))));
}

#[tokio::test]
async fn a_second_main_is_refused() {
    let project = TestProject::new().await;
    init(&project).await;
    let second = project
        .store()
        .write(async |tx| workspaces::create_main(tx, id(1)).await)
        .await;
    assert!(matches!(second, Err(StoreError::Invalid(_))));
}

#[tokio::test]
async fn main_needs_an_existing_revision() {
    let project = TestProject::new().await;
    let missing = project
        .store()
        .write(async |tx| workspaces::create_main(tx, id(1)).await)
        .await
        .unwrap_err();
    assert_eq!(missing.code(), Some(codes::REVISION_NOT_FOUND));
    assert!(
        workspaces::head(project.store().read())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn revisions_are_append_only() {
    let project = TestProject::new().await;
    init(&project).await;
    assert_append_only(project.store(), "revisions").await;
}

#[tokio::test]
async fn numbers_are_gapless_and_a_wrong_number_is_refused() {
    let project = TestProject::new().await;
    let r1 = init(&project).await;
    let r2 = child(&r1, 2);
    append(&project, &r2).await.unwrap();
    let r3 = child(&r2, 3);
    append(&project, &r3).await.unwrap();

    let mut skipping = child(&r3, 4);
    skipping.number = id(5);
    assert!(matches!(
        append(&project, &skipping).await,
        Err(StoreError::Invalid(_))
    ));
    let mut repeating = child(&r3, 4);
    repeating.number = id(3);
    assert!(matches!(
        append(&project, &repeating).await,
        Err(StoreError::Invalid(_))
    ));

    let r4 = child(&r3, 4);
    append(&project, &r4).await.unwrap();
    let numbers: Vec<u32> = workspaces::list_after(project.store().read(), id(1))
        .await
        .unwrap()
        .iter()
        .map(|row| row.number.number())
        .collect();
    assert_eq!(numbers, [2, 3, 4], "a refused number is not consumed");
}

#[tokio::test]
async fn a_missing_parent_or_integration_is_refused() {
    let project = TestProject::new().await;
    let r1 = init(&project).await;
    let orphan = Revision {
        parent: Some(hash(77)),
        ..child(&r1, 2)
    };
    assert!(matches!(
        append(&project, &orphan).await,
        Err(StoreError::Invalid(_))
    ));
    let unintegrated = Revision {
        integration: integration(4),
        ..child(&r1, 2)
    };
    assert!(matches!(
        append(&project, &unintegrated).await,
        Err(StoreError::Invalid(_))
    ));
}

#[tokio::test]
async fn revision_one_and_a_parent_go_together() {
    let project = TestProject::new().await;
    init(&project).await;
    let with_parent = exec(
        project.store(),
        "INSERT INTO revisions (number, hash, tree_hash, parent_id, created_at) VALUES (1, 'a', 'b', 1, 0)",
    )
    .await
    .unwrap_err();
    assert!(
        with_parent
            .to_string()
            .contains("revisions_number_one_iff_no_parent")
    );
    let without_parent = exec(
        project.store(),
        "INSERT INTO revisions (number, hash, tree_hash, created_at) VALUES (2, 'a', 'b', 0)",
    )
    .await
    .unwrap_err();
    assert!(
        without_parent
            .to_string()
            .contains("revisions_number_one_iff_no_parent")
    );
}

#[tokio::test]
async fn import_source_belongs_to_the_first_revision() {
    let project = TestProject::new().await;
    init(&project).await;
    let refused = exec(
        project.store(),
        "INSERT INTO revisions (number, hash, tree_hash, parent_id, import_source, created_at) VALUES (2, 'a', 'b', 1, '/x', 0)",
    )
    .await
    .unwrap_err();
    assert_eq!(
        refused.constraint().as_deref(),
        Some("revisions_import_source_first_only")
    );
}

#[tokio::test]
async fn advance_main_moves_the_head_from_the_expected_revision() {
    let project = TestProject::new().await;
    let r1 = init(&project).await;
    let r2 = child(&r1, 2);
    append(&project, &r2).await.unwrap();
    project
        .store()
        .write(async |tx| workspaces::advance_main(tx, id(1), id(2)).await)
        .await
        .unwrap();
    let head = workspaces::head(project.store().read())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(head.number, id(2));
}

#[tokio::test]
async fn a_stale_expected_head_changes_nothing() {
    let project = TestProject::new().await;
    let r1 = init(&project).await;
    let r2 = child(&r1, 2);
    append(&project, &r2).await.unwrap();
    let r3 = child(&r2, 3);
    append(&project, &r3).await.unwrap();
    let stale = project
        .store()
        .write(async |tx| workspaces::advance_main(tx, id(2), id(3)).await)
        .await
        .unwrap_err();
    assert_eq!(stale.code(), Some(codes::HEAD_MOVED));
    let head = workspaces::head(project.store().read())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(head.number, id(1));

    let unknown = project
        .store()
        .write(async |tx| workspaces::advance_main(tx, id(1), id(9)).await)
        .await
        .unwrap_err();
    assert_eq!(unknown.code(), Some(codes::REVISION_NOT_FOUND));
}

#[tokio::test]
async fn two_concurrent_advances_from_one_head_give_one_success() {
    let project = TestProject::new().await;
    let r1 = init(&project).await;
    let r2 = child(&r1, 2);
    append(&project, &r2).await.unwrap();
    let r3 = child(&r2, 3);
    append(&project, &r3).await.unwrap();
    let mut handles = Vec::new();
    for target in [2, 3] {
        let store = project.store().clone();
        handles.push(tokio::spawn(async move {
            store
                .write(async |tx| workspaces::advance_main(tx, id(1), id(target)).await)
                .await
        }));
    }
    let mut outcomes = Vec::new();
    for handle in handles {
        outcomes.push(handle.await.unwrap());
    }
    assert_eq!(outcomes.iter().filter(|o| o.is_ok()).count(), 1);
    let failed = outcomes.into_iter().find_map(Result::err).unwrap();
    assert_eq!(failed.code(), Some(codes::HEAD_MOVED));
}

#[tokio::test]
async fn every_integration_has_its_own_number() {
    let project = TestProject::new().await;
    let unnumbered = "INSERT INTO integrations (kind, remote_commit, result, actor, created_at) VALUES ('pull', 'c', 'succeeded', 'human:ana', 0)";
    let numbered = "INSERT INTO integrations (number, kind, remote_commit, result, actor, created_at) VALUES (1, 'pull', 'c', 'succeeded', 'human:ana', 0)";
    let missing = exec(project.store(), unnumbered).await.unwrap_err();
    assert_eq!(
        missing.constraint().as_deref(),
        Some("integrations_number_positive")
    );
    exec(project.store(), numbered).await.unwrap();
    let repeated = exec(project.store(), numbered).await.unwrap_err();
    assert!(repeated.unique_violation());
}
