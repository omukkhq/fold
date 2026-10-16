use fold_core::roles::PermissionBehavior;
use fold_core::{HumanName, MessageId, PermissionQuestionId, Role, Timestamp};
use fold_store::Store;
use fold_store::agents::{self, NewQuestion, NewWorkerProcess, QuestionKind};
use serde_json::json;

use crate::common::{count, exec, open};

const ROWS: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'review-lead', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'review-lead', 0);
INSERT INTO messages (id, number, thread_id, sender_kind, sender_worker_id, body, created_at) VALUES (1, 1, 1, 'worker', 1, 'Which colour?', 0);
INSERT INTO messages (id, number, thread_id, reply_to, sender_kind, sender_human, body, created_at) VALUES (2, 2, 1, 1, 'human', 'ana', ' red ', 5);
";

#[cfg(test)]
fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).unwrap()
}

/// A review lead's worker process, the question message `m-1` from it and the human reply `m-2`.
#[cfg(test)]
async fn seeded() -> (tempfile::TempDir, Store, i64) {
    let (dir, _paths, store) = open().await;
    for statement in ROWS.lines().filter(|line| !line.is_empty()) {
        exec(&store, statement).await.unwrap();
    }
    let process = store
        .write(async |tx| {
            agents::insert_worker_process(
                tx.conn(),
                &NewWorkerProcess {
                    agent_id: 1,
                    role: Role::ReviewLead,
                    area_id: None,
                    model: None,
                    session_id: "s-1",
                    started_by: "human:ana",
                    now: at(1),
                },
            )
            .await
        })
        .await
        .unwrap();
    (dir, store, process)
}

#[cfg(test)]
async fn question(store: &Store, process: i64, kind: QuestionKind, number: Option<u32>) -> i64 {
    let input = json!({"command": "make deploy"});
    store
        .write(async |tx| {
            agents::insert_question(
                tx.conn(),
                &NewQuestion {
                    worker_process_id: process,
                    kind,
                    number: number.and_then(PermissionQuestionId::from_number),
                    session_id: "s-1",
                    request_id: "r-1",
                    tool_use_id: Some("toolu_1"),
                    tool: "Bash",
                    input: &input,
                    now: at(2),
                },
            )
            .await
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn a_question_takes_one_answer_or_one_closure_and_never_changes() {
    let (_dir, store, process) = seeded().await;
    let ana = HumanName::parse("ana").unwrap();
    let first = question(&store, process, QuestionKind::Tool, Some(1)).await;
    let second = question(&store, process, QuestionKind::Tool, Some(2)).await;
    let open = agents::open_questions(store.read()).await.unwrap();
    assert_eq!(open.len(), 2);
    assert_eq!(open[0].input, json!({"command": "make deploy"}));
    assert_eq!(open[0].agent.as_str(), "review-lead");
    store
        .write(async |tx| {
            agents::insert_answer(
                tx.conn(),
                first,
                PermissionBehavior::Deny,
                &ana,
                Some("not now"),
                at(3),
            )
            .await
        })
        .await
        .unwrap();
    let again = store
        .write(async |tx| {
            agents::insert_answer(
                tx.conn(),
                first,
                PermissionBehavior::Allow,
                &ana,
                None,
                at(4),
            )
            .await
        })
        .await
        .unwrap_err();
    assert_eq!(
        again.constraint().as_deref(),
        Some("worker_permission_answers.question_id")
    );
    let closed = store
        .write(async |tx| agents::close_open_questions(tx.conn(), process, at(5)).await)
        .await
        .unwrap();
    assert_eq!(closed, 1, "only the unanswered question closes");
    let late = store
        .write(async |tx| {
            agents::insert_answer(
                tx.conn(),
                second,
                PermissionBehavior::Allow,
                &ana,
                None,
                at(6),
            )
            .await
        })
        .await
        .unwrap_err();
    assert!(
        late.to_string()
            .contains("worker_permission_answers_open_only"),
        "{late}"
    );
    let close_answered = exec(
        &store,
        &format!(
            "INSERT INTO worker_permission_closures (question_id, created_at) VALUES ({first}, 7)"
        ),
    )
    .await
    .unwrap_err();
    assert!(
        close_answered
            .to_string()
            .contains("worker_permission_closures_open_only")
    );
    let rows = agents::questions_of_worker(store.read(), process)
        .await
        .unwrap();
    let answer = rows[0].answer.as_ref().unwrap();
    assert_eq!(answer.behavior, PermissionBehavior::Deny);
    assert_eq!(answer.answered_by, "human:ana");
    assert_eq!(answer.message.as_deref(), Some("not now"));
    assert_eq!(rows[1].closed_at, Some(at(5)));
    assert!(!rows[1].is_open());
    let found = agents::find_question(store.read(), PermissionQuestionId::from_number(2).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, second);
    assert!(
        agents::open_questions(store.read())
            .await
            .unwrap()
            .is_empty()
    );
    for table in [
        "worker_permission_questions",
        "worker_permission_answers",
        "worker_permission_closures",
    ] {
        let expected = format!("{table} is append-only");
        let update = exec(&store, &format!("UPDATE {table} SET rowid = rowid"))
            .await
            .unwrap_err();
        assert!(update.to_string().contains(&expected), "{update}");
        let delete = exec(&store, &format!("DELETE FROM {table}"))
            .await
            .unwrap_err();
        assert!(delete.to_string().contains(&expected), "{delete}");
    }
    let unnumbered = exec(
        &store,
        &format!("INSERT INTO worker_permission_questions (kind, worker_process_id, session_id, request_id, tool, input, created_at) VALUES ('tool', {process}, 's', 'r', 'Bash', '{{}}', 0)"),
    )
    .await
    .unwrap_err();
    assert_eq!(
        unnumbered.constraint().as_deref(),
        Some("worker_permission_questions_number_of_tool")
    );
}

#[tokio::test]
async fn an_ask_question_links_its_messages_and_is_only_allowed() {
    let (_dir, store, process) = seeded().await;
    let ana = HumanName::parse("ana").unwrap();
    let ask = question(&store, process, QuestionKind::Ask, None).await;
    let tool = question(&store, process, QuestionKind::Tool, Some(1)).await;
    let m1 = MessageId::from_number(1).unwrap();
    store
        .write(async |tx| agents::insert_question_message(tx.conn(), ask, 1, m1).await)
        .await
        .unwrap();
    let on_tool = store
        .write(async |tx| {
            agents::insert_question_message(tx.conn(), tool, 1, MessageId::from_number(2).unwrap())
                .await
        })
        .await
        .unwrap_err();
    assert!(
        on_tool
            .to_string()
            .contains("worker_question_messages_ask_only")
    );
    assert_eq!(
        agents::question_messages(store.read(), ask).await.unwrap(),
        [m1]
    );
    let mut conn = store.read().acquire().await.unwrap();
    assert_eq!(
        agents::open_ask_of_thread(&mut conn, m1).await.unwrap(),
        Some(ask)
    );
    drop(conn);
    let reply = fold_store::messages::first_human_reply(store.read(), m1)
        .await
        .unwrap();
    assert_eq!(reply, Some((" red ".to_owned(), ana.clone())));
    assert_eq!(
        fold_store::messages::first_human_reply(store.read(), MessageId::from_number(2).unwrap())
            .await
            .unwrap(),
        None
    );
    let denied = store
        .write(async |tx| {
            agents::insert_answer(tx.conn(), ask, PermissionBehavior::Deny, &ana, None, at(3)).await
        })
        .await
        .unwrap_err();
    assert!(
        denied
            .to_string()
            .contains("worker_permission_answers_ask_allows")
    );
    store
        .write(async |tx| {
            agents::insert_answer(tx.conn(), ask, PermissionBehavior::Allow, &ana, None, at(3))
                .await
        })
        .await
        .unwrap();
    let mut conn = store.read().acquire().await.unwrap();
    assert_eq!(
        agents::open_ask_of_thread(&mut conn, m1).await.unwrap(),
        None
    );
    drop(conn);
    let row = agents::question_by_id(store.read(), ask)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.kind, QuestionKind::Ask);
    assert_eq!(row.number, None);
    for table in ["worker_question_messages", "message_choices"] {
        if table == "message_choices" {
            exec(
                &store,
                "INSERT INTO message_choices (message_id, position, label) VALUES (1, 1, 'Red')",
            )
            .await
            .unwrap();
        }
        let expected = format!("{table} is append-only");
        let update = exec(&store, &format!("UPDATE {table} SET rowid = rowid"))
            .await
            .unwrap_err();
        assert!(update.to_string().contains(&expected), "{update}");
        let delete = exec(&store, &format!("DELETE FROM {table}"))
            .await
            .unwrap_err();
        assert!(delete.to_string().contains(&expected), "{delete}");
    }
    let seventh = exec(
        &store,
        "INSERT INTO message_choices (message_id, position, label) VALUES (1, 7, 'x')",
    )
    .await
    .unwrap_err();
    assert_eq!(
        seventh.constraint().as_deref(),
        Some("message_choices_position_range")
    );
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM message_choices").await,
        1
    );
    exec(
        &store,
        "INSERT INTO message_choices (message_id, position, label, description) VALUES (1, 2, 'Blue', 'Use blue.')",
    )
    .await
    .unwrap();
    let blank = exec(
        &store,
        "INSERT INTO message_choices (message_id, position, label, description) VALUES (1, 3, 'Green', ' ')",
    )
    .await
    .unwrap_err();
    assert_eq!(
        blank.constraint().as_deref(),
        Some("message_choices_description_present")
    );
    exec(
        &store,
        "INSERT INTO message_question_headers (message_id, header) VALUES (1, 'Colour')",
    )
    .await
    .unwrap();
    let second = exec(
        &store,
        "INSERT INTO message_question_headers (message_id, header) VALUES (1, 'Again')",
    )
    .await
    .unwrap_err();
    assert!(second.unique_violation());
    for operation in [
        "UPDATE message_question_headers SET header = 'x'",
        "DELETE FROM message_question_headers",
    ] {
        let error = exec(&store, operation).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("message_question_headers is append-only"),
            "{error}"
        );
    }
}
