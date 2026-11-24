use crate::common::{count, exec, open};
use fold_core::object::{ObjectKind, hash_object};
use fold_core::requests::{DraftCard, DraftEdge, Requester, validate_draft};
use fold_core::{AreaName, RequestId, TaskId, TaskKind, Timestamp};
use fold_store::tasks::requests::{self, NewRequest, drafts, insert_draft, set_task_request};
use fold_store::testing::assert_append_only;
use fold_store::workspaces::gc_roots;
use fold_store::{Store, StoreError};

/// One agent, a task, request 1 with a draft waiting for the requester, and one row in every append-only request table.
const SEED: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'frontend', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'frontend', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'frontend', 0, 0);
INSERT INTO requests (id, number, kind, title, body, state, requester, drafter_worker_id, drafter_since, created_at, updated_at) VALUES (1, 1, 'bug', 'Crash', 'It crashes', 'draft_ready', 'human:tanim', 1, 0, 0, 0);
INSERT INTO request_drafts (id, request_id, version, author, created_at) VALUES (1, 1, 1, 'frontend', 0);
INSERT INTO request_draft_cards (draft_id, key, position, title, description) VALUES (1, 'c1', 1, 'One', 'd'), (1, 'c2', 2, 'Two', 'd');
INSERT INTO request_draft_edges (draft_id, prerequisite_key, dependent_key) VALUES (1, 'c1', 'c2');
INSERT INTO request_draft_card_needs (card_id, task_id) VALUES (1, 1);
INSERT INTO request_comments (request_id, version, author, body, created_at) VALUES (1, 1, 'human:tanim', 'more', 0);
";

#[cfg(test)]
async fn seeded() -> (tempfile::TempDir, Store) {
    let (dir, _paths, store) = open().await;
    for statement in SEED.lines().filter(|line| !line.is_empty()) {
        exec(&store, statement).await.unwrap();
    }
    let object = hash_object(ObjectKind::Blob, b"screenshot");
    exec(
        &store,
        &format!(
            "INSERT INTO request_attachments (request_id, position, name, media_type, size, object, created_at) VALUES (1, 1, 'shot.png', 'image/png', 10, '{object}', 0)"
        ),
    )
    .await
    .unwrap();
    (dir, store)
}

#[cfg(test)]
fn request_sql(number: u32, columns: &str, values: &str) -> String {
    format!(
        "INSERT INTO requests (number, body, requester, created_at, updated_at{columns}) VALUES ({number}, 'b', 'frontend', 0, 0{values})"
    )
}

#[cfg(test)]
async fn refused(store: &Store, sql: &str) -> StoreError {
    exec(store, sql).await.unwrap_err()
}

#[tokio::test]
async fn the_request_history_tables_are_append_only() {
    let (_dir, store) = seeded().await;
    for table in [
        "request_attachments",
        "request_drafts",
        "request_draft_cards",
        "request_draft_edges",
        "request_draft_card_needs",
        "request_comments",
    ] {
        assert_append_only(&store, table).await;
    }
}

#[tokio::test]
async fn state_kind_and_text_checks_hold() {
    let (_dir, store) = seeded().await;
    exec(&store, &request_sql(2, "", "")).await.unwrap();
    let cases = [
        (request_sql(3, ", state", ", 'waiting'"), "requests_state_values"),
        (
            request_sql(3, ", kind", ", 'chore'"),
            "requests_kind_values",
        ),
        (
            request_sql(3, ", title", ", 'two\nlines'"),
            "requests_title_single_line",
        ),
        (
            "INSERT INTO requests (number, body, requester, created_at, updated_at) VALUES (3, '  ', 'frontend', 0, 0)".to_owned(),
            "requests_body_present",
        ),
        (
            request_sql(3, ", state, drafter_worker_id, drafter_since", ", 'drafting', 1, 0"),
            "requests_sorted_once_claimed",
        ),
        (
            request_sql(
                3,
                ", kind, title, state",
                ", 'bug', 't', 'drafting'",
            ),
            "requests_drafter_matches_state",
        ),
        (
            request_sql(
                3,
                ", kind, title, state, drafter_worker_id",
                ", 'bug', 't', 'draft_ready', 1",
            ),
            "requests_drafter_since_with_drafter",
        ),
        (
            request_sql(3, ", state", ", 'closed'"),
            "requests_close_reason_iff_closed",
        ),
        (
            request_sql(3, ", close_reason", ", 'done'"),
            "requests_close_reason_iff_closed",
        ),
    ];
    for (sql, constraint) in cases {
        let error = refused(&store, &sql).await;
        assert_eq!(error.constraint().as_deref(), Some(constraint), "{sql}");
    }
    exec(
        &store,
        &request_sql(3, ", state, close_reason", ", 'closed', 'not needed'"),
    )
    .await
    .unwrap();
    let duplicate = refused(&store, &request_sql(2, "", "")).await;
    assert_eq!(duplicate.constraint().as_deref(), Some("requests.number"));
}

#[tokio::test]
async fn drafts_cards_and_edges_hold_their_shape() {
    let (_dir, store) = seeded().await;
    let version = refused(
        &store,
        "INSERT INTO request_drafts (request_id, version, author, created_at) VALUES (1, 1, 'frontend', 0)",
    )
    .await;
    assert_eq!(
        version.constraint().as_deref(),
        Some("request_drafts.request_id, request_drafts.version")
    );
    exec(
        &store,
        "INSERT INTO request_drafts (id, request_id, version, author, created_at) VALUES (2, 1, 2, 'human:tanim', 0)",
    )
    .await
    .unwrap();
    exec(
        &store,
        "INSERT INTO request_draft_cards (draft_id, key, position, title, description) VALUES (2, 'c1', 1, 'One', 'd')",
    )
    .await
    .unwrap();
    let self_edge = refused(
        &store,
        "INSERT INTO request_draft_edges (draft_id, prerequisite_key, dependent_key) VALUES (2, 'c1', 'c1')",
    )
    .await;
    assert_eq!(
        self_edge.constraint().as_deref(),
        Some("request_draft_edges_no_self_edge")
    );
    let unknown = refused(
        &store,
        "INSERT INTO request_draft_edges (draft_id, prerequisite_key, dependent_key) VALUES (2, 'c1', 'c2')",
    )
    .await;
    assert!(unknown.to_string().contains("FOREIGN KEY"), "{unknown}");
    let key = refused(
        &store,
        "INSERT INTO request_draft_cards (draft_id, key, position, title, description) VALUES (2, 'c1', 2, 'Again', 'd')",
    )
    .await;
    assert_eq!(
        key.constraint().as_deref(),
        Some("request_draft_cards.draft_id, request_draft_cards.key")
    );
    let comment = refused(
        &store,
        "INSERT INTO request_comments (request_id, version, author, body, created_at) VALUES (1, 9, 'human:tanim', 'x', 0)",
    )
    .await;
    assert!(comment.to_string().contains("FOREIGN KEY"), "{comment}");
}

#[tokio::test]
async fn attachments_keep_their_limits() {
    let (_dir, store) = seeded().await;
    let attachment = |position: u32, name: &str, size: u64| {
        format!(
            "INSERT INTO request_attachments (request_id, position, name, media_type, size, object, created_at) VALUES (1, {position}, '{name}', 'text/plain', {size}, 'h', 0)"
        )
    };
    for (sql, constraint) in [
        (
            attachment(11, "a.txt", 1),
            "request_attachments_position_range",
        ),
        (
            attachment(2, "a/b.txt", 1),
            "request_attachments_plain_name",
        ),
        (attachment(2, "a..txt", 1), "request_attachments_plain_name"),
        (attachment(2, "", 1), "request_attachments_plain_name"),
        (
            attachment(2, "big.bin", 10 * 1024 * 1024 + 1),
            "request_attachments_size_range",
        ),
    ] {
        let error = refused(&store, &sql).await;
        assert_eq!(error.constraint().as_deref(), Some(constraint), "{sql}");
    }
    exec(&store, &attachment(10, "last.txt", 10 * 1024 * 1024))
        .await
        .unwrap();
    let twice = refused(&store, &attachment(1, "again.txt", 1)).await;
    assert_eq!(
        twice.constraint().as_deref(),
        Some("request_attachments.request_id, request_attachments.position")
    );
}

#[tokio::test]
async fn a_task_keeps_the_request_that_made_it() {
    let (_dir, store) = seeded().await;
    let missing = refused(&store, "UPDATE tasks SET request_id = 9 WHERE number = 1").await;
    assert!(missing.to_string().contains("FOREIGN KEY"), "{missing}");
    exec(&store, "UPDATE tasks SET request_id = 1 WHERE number = 1")
        .await
        .unwrap();
    exec(&store, &request_sql(2, "", "")).await.unwrap();
    let moved = refused(&store, "UPDATE tasks SET request_id = 2 WHERE number = 1").await;
    assert!(
        moved.to_string().contains("tasks.request_id never changes"),
        "{moved}"
    );
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM tasks WHERE request_id = 1").await,
        1
    );
    exec(
        &store,
        "INSERT INTO messages (number, thread_id, sender_kind, body, created_at, request_id) VALUES (1, 1, 'fold', 'b', 0, 1)",
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn set_task_request_refuses_an_unknown_request() {
    let (_dir, store) = seeded().await;
    let task = TaskId::from_number(1).unwrap();
    let error = store
        .write(async |tx| set_task_request(tx, task, RequestId::from_number(9).unwrap()).await)
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::Invalid(_)), "{error:?}");
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM tasks WHERE request_id IS NULL"
        )
        .await,
        1
    );
    store
        .write(async |tx| set_task_request(tx, task, RequestId::from_number(1).unwrap()).await)
        .await
        .unwrap();
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM tasks WHERE request_id = 1").await,
        1
    );
}

#[tokio::test]
async fn an_attachment_blob_is_a_gc_root() {
    let (_dir, store) = seeded().await;
    let roots = gc_roots(store.read()).await.unwrap();
    assert_eq!(roots, vec![hash_object(ObjectKind::Blob, b"screenshot")]);
}

#[tokio::test]
async fn a_code_location_names_one_anchor_and_a_forward_range() {
    let (_dir, store) = seeded().await;
    for statement in [
        "INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 0)",
        "INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 1, 'task', 'ws/t-1', 1, 'active', 0)",
        "INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, created_by, created_at) VALUES (1, 1, 1, 1, 1, 't2', 'm', 'frontend', 0)",
        "INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 1, 'p', 0, 0)",
        "INSERT INTO proposal_versions (id, proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (1, 1, 1, 1, 't2', 1, 'frontend', 0)",
    ] {
        exec(&store, statement).await.unwrap();
    }
    let code = |columns: &str, values: &str| {
        format!(
            "INSERT INTO request_code_refs (request_id, path, start_line, end_line, excerpt, excerpt_cut{columns}) VALUES (1, 'a.rs', {values})"
        )
    };
    for (sql, constraint) in [
        (code("", "1, 1, 'a', 0"), "request_code_refs_one_anchor"),
        (
            code(
                ", revision_id, version_id, side",
                "1, 1, 'a', 0, 1, 1, 'new'",
            ),
            "request_code_refs_one_anchor",
        ),
        (
            code(", version_id", "1, 1, 'a', 0, 1"),
            "request_code_refs_side_with_version",
        ),
        (
            code(", revision_id, side", "1, 1, 'a', 0, 1, 'new'"),
            "request_code_refs_side_with_version",
        ),
        (
            code(", revision_id", "3, 2, 'a', 0, 1"),
            "request_code_refs_range_order",
        ),
        (
            code(", revision_id", "0, 0, 'a', 0, 1"),
            "request_code_refs_range_order",
        ),
    ] {
        let error = refused(&store, &sql).await;
        assert_eq!(error.constraint().as_deref(), Some(constraint), "{sql}");
    }
    exec(
        &store,
        &code(", version_id, side", "2, 4, 'b', 1, 1, 'old'"),
    )
    .await
    .unwrap();
    let again = refused(&store, &code(", revision_id", "1, 1, 'a', 0, 1")).await;
    assert!(again.unique_violation(), "{again}");
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM request_code_refs").await,
        1
    );
}

#[cfg(test)]
fn ts(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).unwrap()
}

/// Two areas, `web` and `api`, with labels 21 and 22.
#[cfg(test)]
async fn with_areas() -> (tempfile::TempDir, Store) {
    let (dir, store) = seeded().await;
    for statement in [
        "INSERT INTO labels (id, name, color, created_at) VALUES (21, 'web', 'sage', 0), (22, 'api', 'teal', 0)",
        "INSERT INTO areas (id, name, label_id, created_by, created_at) VALUES (31, 'web', 21, 'main', 0), (32, 'api', 22, 'main', 0)",
    ] {
        exec(&store, statement).await.unwrap();
    }
    (dir, store)
}

#[cfg(test)]
async fn insert_request(store: &Store, number: u32, area: Option<&str>, now: i64) {
    let area = area.map(|area| AreaName::parse(area).unwrap());
    let requester = Requester::parse("web-worker").unwrap();
    store
        .write(async |tx| {
            requests::insert(
                tx,
                &NewRequest {
                    number: RequestId::from_number(number).unwrap(),
                    body: "The board flickers",
                    requester: &requester,
                    area: area.as_ref(),
                    now: ts(now),
                },
            )
            .await
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn a_request_keeps_its_area_and_an_unknown_area_is_refused() {
    let (_dir, store) = with_areas().await;
    insert_request(&store, 2, Some("web"), 5).await;
    insert_request(&store, 3, None, 5).await;
    let mut conn = store.read().acquire().await.unwrap();
    let fetch = async |conn: &mut sqlx::SqliteConnection, number: u32| {
        requests::fetch(conn, RequestId::from_number(number).unwrap())
            .await
            .unwrap()
            .unwrap()
    };
    let web = fetch(&mut conn, 2).await;
    assert_eq!(web.area.as_ref().map(AreaName::as_str), Some("web"));
    assert_eq!(web.escalated_at, None);
    assert_eq!(fetch(&mut conn, 3).await.area, None);
    drop(conn);

    let requester = Requester::parse("web-worker").unwrap();
    let ghost = AreaName::parse("ghost").unwrap();
    let unknown = store
        .write(async |tx| {
            requests::insert(
                tx,
                &NewRequest {
                    number: RequestId::from_number(4).unwrap(),
                    body: "b",
                    requester: &requester,
                    area: Some(&ghost),
                    now: ts(5),
                },
            )
            .await
        })
        .await
        .unwrap_err();
    assert!(matches!(unknown, StoreError::Invalid(_)), "{unknown}");
    let foreign = refused(&store, &request_sql(4, ", area_id", ", 99")).await;
    assert!(foreign.to_string().contains("FOREIGN KEY"), "{foreign}");
    let moved = refused(&store, "UPDATE requests SET area_id = 32 WHERE number = 2").await;
    assert!(
        moved
            .to_string()
            .contains("requests.area_id never changes once set"),
        "{moved}"
    );
    exec(&store, "UPDATE requests SET area_id = 32 WHERE number = 3")
        .await
        .unwrap();
    let mut conn = store.read().acquire().await.unwrap();
    assert_eq!(
        fetch(&mut conn, 3)
            .await
            .area
            .as_ref()
            .map(AreaName::as_str),
        Some("api"),
        "a request without an area may get one"
    );
}

#[tokio::test]
async fn only_open_unclaimed_area_requests_older_than_the_time_wait() {
    let (_dir, store) = with_areas().await;
    insert_request(&store, 2, Some("web"), 10).await;
    insert_request(&store, 3, Some("api"), 20).await;
    insert_request(&store, 4, None, 10).await;
    insert_request(&store, 5, Some("web"), 30).await;
    insert_request(&store, 6, Some("web"), 10).await;
    insert_request(&store, 7, Some("web"), 10).await;
    exec(
        &store,
        "UPDATE requests SET state = 'drafting', kind = 'bug', title = 'T', drafter_worker_id = 1, drafter_since = 11 WHERE number = 6",
    )
    .await
    .unwrap();
    exec(
        &store,
        "UPDATE requests SET state = 'closed', close_reason = 'dup' WHERE number = 7",
    )
    .await
    .unwrap();
    let waiting = |before: i64| {
        let store = &store;
        async move {
            requests::waiting_area_requests(store.read(), ts(before))
                .await
                .unwrap()
                .into_iter()
                .map(|request| (request.id.to_string(), request.area.to_string()))
                .collect::<Vec<_>>()
        }
    };
    let pair = |id: &str, area: &str| (id.to_owned(), area.to_owned());
    assert_eq!(
        waiting(30).await,
        vec![pair("RQ-2", "web"), pair("RQ-3", "api")],
        "created exactly at the time is not older"
    );
    assert_eq!(waiting(10).await, Vec::new());
    let first = requests::waiting_area_requests(store.read(), ts(30))
        .await
        .unwrap();
    assert_eq!(first[0].requester, Requester::parse("web-worker").unwrap());
    assert_eq!(first[0].created_at, ts(10));

    let mark = |number: u32| {
        let store = &store;
        async move {
            store
                .write(async |tx| {
                    requests::mark_escalated(tx, RequestId::from_number(number).unwrap(), ts(40))
                        .await
                })
                .await
        }
    };
    assert!(mark(2).await.unwrap());
    assert!(!mark(2).await.unwrap(), "a request escalates once");
    assert!(matches!(mark(99).await, Err(StoreError::Invalid(_))));
    assert_eq!(
        waiting(50).await,
        vec![pair("RQ-3", "api"), pair("RQ-5", "web")]
    );
    let mut conn = store.read().acquire().await.unwrap();
    let escalated = requests::fetch(&mut conn, RequestId::from_number(2).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(escalated.escalated_at, Some(ts(40)));
    drop(conn);
    let again = refused(
        &store,
        "UPDATE requests SET escalated_at = 41 WHERE number = 2",
    )
    .await;
    assert!(
        again
            .to_string()
            .contains("requests.escalated_at is set once"),
        "{again}"
    );
}

#[cfg(test)]
fn card(key: &str, kind: TaskKind, needs: &[u32]) -> DraftCard {
    DraftCard {
        key: key.to_owned(),
        title: format!("Card {key}"),
        description: "d".to_owned(),
        label: None,
        kind,
        needs: needs
            .iter()
            .map(|n| TaskId::from_number(*n).unwrap())
            .collect(),
    }
}

#[tokio::test]
async fn a_draft_version_reads_back_with_its_kinds_and_needs() {
    let (_dir, store) = seeded().await;
    exec(
        &store,
        "INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (2, 2, 'two', 'frontend', 0, 0)",
    )
    .await
    .unwrap();
    let draft = validate_draft(
        vec![
            card("a", TaskKind::NoProposal, &[2, 1]),
            card("b", TaskKind::Human, &[]),
            card("c", TaskKind::Proposal, &[1]),
        ],
        vec![DraftEdge {
            prerequisite: "a".to_owned(),
            dependent: "c".to_owned(),
        }],
    )
    .unwrap();
    let request = RequestId::from_number(1).unwrap();
    store
        .write(async |tx| {
            insert_draft(
                tx,
                request,
                2,
                "frontend",
                None,
                &draft,
                Timestamp::from_millis(5).unwrap(),
            )
            .await
        })
        .await
        .unwrap();
    let mut conn = store.read().acquire().await.unwrap();
    let rows = drafts(&mut conn, request).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].cards[0].kind, TaskKind::Proposal);
    assert_eq!(rows[0].cards[0].needs, [TaskId::from_number(1).unwrap()]);
    assert!(rows[0].cards[1].needs.is_empty());
    assert_eq!(rows[1].cards, draft.cards());
    assert_eq!(rows[1].edges, draft.edges());
}

#[tokio::test]
async fn a_draft_that_needs_an_unknown_task_is_refused_whole() {
    let (_dir, store) = seeded().await;
    let draft = validate_draft(vec![card("a", TaskKind::Proposal, &[9])], Vec::new()).unwrap();
    let request = RequestId::from_number(1).unwrap();
    let error = store
        .write(async |tx| {
            insert_draft(
                tx,
                request,
                2,
                "frontend",
                None,
                &draft,
                Timestamp::from_millis(5).unwrap(),
            )
            .await
        })
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::Invalid(_)), "{error}");
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM request_drafts").await,
        1
    );
}

#[tokio::test]
async fn a_card_keeps_a_known_kind_and_each_need_once() {
    let (_dir, store) = seeded().await;
    let kind = refused(
        &store,
        "INSERT INTO request_draft_cards (draft_id, key, position, title, description, kind) VALUES (1, 'c3', 3, 'Three', 'd', 'design')",
    )
    .await;
    assert_eq!(
        kind.constraint().as_deref(),
        Some("request_draft_cards_kind_values")
    );
    let twice = refused(
        &store,
        "INSERT INTO request_draft_card_needs (card_id, task_id) VALUES (1, 1)",
    )
    .await;
    assert_eq!(
        twice.constraint().as_deref(),
        Some("request_draft_card_needs.card_id, request_draft_card_needs.task_id")
    );
    let missing = refused(
        &store,
        "INSERT INTO request_draft_card_needs (card_id, task_id) VALUES (2, 7)",
    )
    .await;
    assert!(missing.to_string().contains("FOREIGN KEY"), "{missing}");
    assert!(
        fold_store::tasks::history(
            &mut store.read().acquire().await.unwrap(),
            TaskId::from_number(1).unwrap()
        )
        .await
        .unwrap()
        .request_need
    );
}
