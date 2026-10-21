use crate::common::{count, exec, open};
use fold_store::Store;
use fold_store::testing::assert_append_only;
use tempfile::TempDir;

/// Agent `frontend` with main worker id 1, and message `m-1` from it that starts its own thread.
#[cfg(test)]
struct Seeded {
    _dir: TempDir,
    store: Store,
}

#[cfg(test)]
impl Seeded {
    fn store(&self) -> &Store {
        &self.store
    }
}

#[cfg(test)]
async fn seeded() -> Seeded {
    let (dir, _paths, store) = open().await;
    let project = Seeded { _dir: dir, store };
    for sql in [
        "INSERT INTO agents (id, name, created_at) VALUES (1, 'frontend', 0)",
        "INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'frontend', 0)",
        "INSERT INTO messages (id, number, thread_id, sender_kind, sender_worker_id, body, created_at)
         VALUES (1, 1, 1, 'worker', 1, 'hello', 0)",
    ] {
        exec(project.store(), sql).await.unwrap();
    }
    project
}

#[cfg(test)]
async fn message(store: &Store, number: u32, sender: &str) -> Result<(), String> {
    let (kind, worker, human) = match sender {
        "worker" => ("worker", "1", "NULL"),
        "worker-without-id" => ("worker", "NULL", "NULL"),
        "worker-with-human" => ("worker", "1", "'tanim'"),
        "human" => ("human", "NULL", "'tanim'"),
        "human-without-name" => ("human", "NULL", "NULL"),
        "human-with-worker" => ("human", "1", "'tanim'"),
        "fold" => ("fold", "NULL", "NULL"),
        "fold-with-worker" => ("fold", "1", "NULL"),
        "fold-with-human" => ("fold", "NULL", "'tanim'"),
        _ => ("nobody", "NULL", "NULL"),
    };
    exec(
        store,
        &format!(
            "INSERT INTO messages (number, thread_id, reply_to, sender_kind, sender_worker_id, sender_human, body, created_at)
             VALUES ({number}, 1, 1, '{kind}', {worker}, {human}, 'text', 0)"
        ),
    )
    .await
    .map_err(|error| error.constraint().unwrap_or_else(|| error.to_string()))
}

#[tokio::test]
async fn a_thread_root_names_itself_as_its_thread() {
    let project = seeded().await;
    assert_eq!(
        count(
            project.store(),
            "SELECT COUNT(*) FROM messages WHERE number = 1 AND thread_id = 1 AND reply_to IS NULL"
        )
        .await,
        1
    );
    let error = exec(
        project.store(),
        "INSERT INTO messages (number, thread_id, sender_kind, body, created_at) VALUES (2, 9, 'fold', 'x', 0)",
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("FOREIGN KEY"), "{error}");
}

#[tokio::test]
async fn messages_refuse_update_and_delete() {
    let project = seeded().await;
    assert_append_only(project.store(), "messages").await;
    for sql in [
        "UPDATE messages SET body = 'edited' WHERE number = 1",
        "DELETE FROM messages WHERE number = 1",
    ] {
        let error = exec(project.store(), sql).await.unwrap_err();
        assert!(
            error.to_string().contains("messages is append-only"),
            "{error}"
        );
    }
    assert_eq!(
        count(
            project.store(),
            "SELECT COUNT(*) FROM messages WHERE body = 'hello'"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn the_sender_check_keeps_exactly_the_column_its_kind_needs() {
    let project = seeded().await;
    let store = project.store();
    let mut number = 2;
    for accepted in ["worker", "human", "fold"] {
        assert_eq!(message(store, number, accepted).await, Ok(()), "{accepted}");
        number += 1;
    }
    for refused in [
        "worker-without-id",
        "worker-with-human",
        "human-without-name",
        "human-with-worker",
        "fold-with-worker",
        "fold-with-human",
    ] {
        assert_eq!(
            message(store, number, refused).await,
            Err("messages_sender_matches_kind".to_owned()),
            "{refused}"
        );
    }
    assert_eq!(
        message(store, number, "nobody").await,
        Err("messages_sender_kind_values".to_owned())
    );
}

#[tokio::test]
async fn a_recipient_row_has_exactly_one_target() {
    let project = seeded().await;
    let store = project.store();
    for (target, expected) in [
        ("1, 'tanim'", Err("message_recipients_exactly_one_target")),
        ("NULL, NULL", Err("message_recipients_exactly_one_target")),
        ("1, NULL", Ok(())),
        ("NULL, 'tanim'", Ok(())),
    ] {
        let result = exec(
            store,
            &format!(
                "INSERT INTO message_recipients (message_id, worker_id, human, addressed_as) VALUES (1, {target}, 'agent')"
            ),
        )
        .await
        .map_err(|error| error.constraint().unwrap_or_default());
        assert_eq!(result, expected.map_err(str::to_owned), "{target}");
    }
}

#[tokio::test]
async fn a_second_row_for_the_same_message_and_target_is_refused() {
    let project = seeded().await;
    let store = project.store();
    exec(
        store,
        "INSERT INTO message_recipients (message_id, worker_id, addressed_as) VALUES (1, 1, 'agent')",
    )
    .await
    .unwrap();
    exec(
        store,
        "INSERT INTO message_recipients (message_id, human, addressed_as) VALUES (1, 'tanim', 'human')",
    )
    .await
    .unwrap();
    for (sql, constraint) in [
        (
            "INSERT INTO message_recipients (message_id, worker_id, addressed_as) VALUES (1, 1, 'all')",
            "message_recipients.message_id, message_recipients.worker_id",
        ),
        (
            "INSERT INTO message_recipients (message_id, human, addressed_as) VALUES (1, 'tanim', 'humans')",
            "message_recipients.message_id, message_recipients.human",
        ),
    ] {
        let error = exec(store, sql).await.unwrap_err();
        assert!(error.unique_violation(), "{error}");
        assert_eq!(error.constraint().as_deref(), Some(constraint));
    }
    let error = exec(
        store,
        "INSERT INTO message_recipients (message_id, human, addressed_as) VALUES (1, 'ana', 'everyone')",
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.constraint().as_deref(),
        Some("message_recipients_addressed_as_values")
    );
}

#[tokio::test]
async fn an_event_message_must_name_a_stored_message() {
    let project = seeded().await;
    let store = project.store();
    exec(
        store,
        "INSERT INTO events (type, actor, message_id, created_at) VALUES ('message.sent', 'frontend', 1, 0)",
    )
    .await
    .unwrap();
    let error = exec(
        store,
        "INSERT INTO events (type, actor, message_id, created_at) VALUES ('message.sent', 'frontend', 9, 0)",
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("FOREIGN KEY"), "{error}");
    assert_eq!(
        count(
            store,
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'events_message'"
        )
        .await,
        1
    );
}

#[cfg(test)]
async fn close(store: &Store, message: i64) -> Result<(), String> {
    exec(
        store,
        &format!(
            "INSERT INTO message_ask_closures (message_id, reason, closed_by, created_at)
             VALUES ({message}, 'dismissed', 'human:ana', 0)"
        ),
    )
    .await
    .map_err(|error| error.to_string())
}

#[tokio::test]
async fn only_an_open_ask_that_no_worker_waits_on_takes_one_closure_that_never_changes() {
    let project = seeded().await;
    let store = project.store();
    for sql in [
        "INSERT INTO message_recipients (message_id, human, addressed_as) VALUES (1, 'ana', 'human')",
        "INSERT INTO messages (id, number, thread_id, sender_kind, sender_worker_id, body, created_at, asks_human)
         VALUES (2, 2, 2, 'worker', 1, 'which one?', 0, 1)",
        "INSERT INTO message_recipients (message_id, human, addressed_as) VALUES (2, 'ana', 'humans')",
        "INSERT INTO messages (id, number, thread_id, sender_kind, sender_worker_id, body, created_at, asks_human)
         VALUES (3, 3, 3, 'worker', 1, 'answered?', 0, 1)",
        "INSERT INTO message_recipients (message_id, human, addressed_as) VALUES (3, 'ana', 'human')",
        "INSERT INTO messages (id, number, thread_id, reply_to, sender_kind, sender_human, body, created_at)
         VALUES (4, 4, 3, 3, 'human', 'ana', 'yes', 0)",
        "INSERT INTO messages (id, number, thread_id, sender_kind, sender_worker_id, body, created_at)
         VALUES (5, 5, 5, 'worker', 1, 'Red or blue?', 0)",
        "INSERT INTO message_recipients (message_id, human, addressed_as) VALUES (5, 'ana', 'humans')",
        "INSERT INTO agents (id, name, created_at) VALUES (2, 'web-lead', 0)",
        "INSERT INTO worker_processes (id, agent_id, role, session_id, state, started_by, started_at)
         VALUES (1, 2, 'review_lead', 's-1', 'working', 'human:ana', 0)",
        "INSERT INTO worker_permission_questions (id, kind, worker_process_id, session_id, request_id, tool, input, created_at)
         VALUES (1, 'ask', 1, 's-1', 'r-1', 'AskUserQuestion', '{}', 0)",
        "INSERT INTO worker_question_messages (question_id, position, message_id) VALUES (1, 1, 5)",
    ] {
        exec(store, sql).await.unwrap();
    }
    let open = "SELECT COUNT(*) FROM open_human_questions";
    assert_eq!(count(store, open).await, 2, "m-2 and the worker's m-5");
    let not_open = close(store, 1).await.unwrap_err();
    assert!(
        not_open.contains("message_ask_closures_open_only"),
        "a report to a human is no ask: {not_open}"
    );
    let answered = close(store, 3).await.unwrap_err();
    assert!(
        answered.contains("message_ask_closures_open_only"),
        "a human's reply already closed m-3: {answered}"
    );
    let worker = close(store, 5).await.unwrap_err();
    assert!(
        worker.contains("message_ask_closures_not_worker_question"),
        "{worker}"
    );
    close(store, 2).await.unwrap();
    assert_eq!(count(store, open).await, 1, "only the worker's m-5 waits");
    let again = close(store, 2).await.unwrap_err();
    assert!(again.contains("message_ask_closures_open_only"), "{again}");
    let bad_flag = exec(
        store,
        "INSERT INTO messages (id, number, thread_id, sender_kind, sender_worker_id, body, created_at, asks_human)
         VALUES (6, 6, 6, 'worker', 1, 'x', 0, 2)",
    )
    .await
    .unwrap_err();
    assert_eq!(
        bad_flag.constraint().as_deref(),
        Some("messages_asks_human_flag")
    );
    assert_append_only(store, "message_ask_closures").await;
}

#[tokio::test]
async fn message_lines_name_a_version_of_the_message_proposal() {
    let project = seeded().await;
    let store = project.store();
    for sql in [
        "INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'frontend', 0, 0)",
        "INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 0)",
        "INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 1, 'task', 'ws/t-1', 1, 'active', 0)",
        "INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, created_by, created_at) VALUES (1, 1, 1, 1, 1, 't2', 'm', 'frontend', 0)",
        "INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 1, 'p', 0, 0)",
        "INSERT INTO proposals (id, number, task_id, author_worker_id, title, state, close_reason, created_at, updated_at) VALUES (2, 2, 1, 1, 'q', 'closed', 'gone', 0, 0)",
        "INSERT INTO proposal_versions (id, proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (1, 1, 1, 1, 't2', 1, 'frontend', 0), (2, 2, 1, 1, 't2', 1, 'frontend', 0)",
        "INSERT INTO messages (id, number, thread_id, sender_kind, sender_human, proposal_id, body, created_at) VALUES (2, 2, 2, 'human', 'tanim', 1, 'why', 0)",
    ] {
        exec(store, sql).await.unwrap();
    }
    let lines = |message: u32, version: u32, start: u32, end: u32| {
        format!(
            "INSERT INTO message_line_refs (message_id, version_id, path, side, start_line, end_line, excerpt, excerpt_cut)
             VALUES ({message}, {version}, 'a.rs', 'new', {start}, {end}, 'x', 0)"
        )
    };
    let other = exec(store, &lines(2, 2, 1, 1)).await.unwrap_err();
    assert!(
        other
            .to_string()
            .contains("message_line_refs_same_proposal"),
        "{other}"
    );
    let no_proposal = exec(store, &lines(1, 1, 1, 1)).await.unwrap_err();
    assert!(
        no_proposal
            .to_string()
            .contains("message_line_refs_same_proposal"),
        "{no_proposal}"
    );
    let backwards = exec(store, &lines(2, 1, 3, 2)).await.unwrap_err();
    assert_eq!(
        backwards.constraint().as_deref(),
        Some("message_line_refs_range_order")
    );
    exec(store, &lines(2, 1, 2, 3)).await.unwrap();
    assert_append_only(store, "message_line_refs").await;
}

#[test]
fn the_debug_form_of_message_lines_leaves_out_the_excerpt() {
    let lines = fold_store::messages::MessageLines {
        version: 2,
        path: "web/src/board.ts".to_owned(),
        side: fold_core::reviews::Side::New,
        start_line: 4,
        end_line: 5,
        excerpt: "const secret = 1;".to_owned(),
        excerpt_cut: false,
    };
    let debug = format!("{lines:?}");
    assert!(!debug.contains("secret"), "{debug}");
    assert!(debug.contains("web/src/board.ts"), "{debug}");
    assert!(debug.contains("excerpt_len: 17"), "{debug}");
}
