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
