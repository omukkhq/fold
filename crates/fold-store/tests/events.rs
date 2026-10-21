use std::collections::BTreeSet;

use fold_core::events::types;
use fold_core::{
    AgentEventFacts, AgentName, EventActor, EventFilter, EventType, MessageId, NewEvent,
    ProposalId, RevisionId, TaskId, WorkspaceId, concerns,
};
use fold_store::testing::{TestProject, assert_append_only};
use fold_store::{StoreError, events};
use serde_json::json;

#[cfg(test)]
fn task(n: u32) -> TaskId {
    TaskId::from_number(n).unwrap()
}

#[cfg(test)]
fn new_event(event_type: EventType) -> NewEvent {
    NewEvent::new(event_type, EventActor::parse("frontend").unwrap()).unwrap()
}

#[cfg(test)]
async fn append_all(project: &TestProject, batch: Vec<NewEvent>) -> Vec<u64> {
    let now = project.now();
    project
        .store()
        .write(async |tx| {
            let mut seqs = Vec::new();
            for event in &batch {
                seqs.push(events::append(tx, event, now).await?.seq);
            }
            Ok::<_, StoreError>(seqs)
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn append_stores_the_reason_in_its_own_column_and_leaves_data_as_given() {
    let project = TestProject::new().await;
    let with = new_event(types::WORKSPACE_WATCH_DEGRADED)
        .reason("watch_limit")
        .data(json!({"reason": "watch_limit"}))
        .unwrap();
    append_all(&project, vec![with, new_event(types::TASK_CREATED)]).await;
    let rows: Vec<(Option<String>, String)> =
        sqlx::query_as("SELECT reason, data FROM events ORDER BY seq")
            .fetch_all(project.store().read())
            .await
            .unwrap();
    assert_eq!(
        rows,
        [
            (
                Some("watch_limit".to_owned()),
                r#"{"reason":"watch_limit"}"#.to_owned()
            ),
            (None, "{}".to_owned()),
        ]
    );
}

#[tokio::test]
async fn append_returns_increasing_seq_and_the_stored_event() {
    let project = TestProject::new().await;
    assert_eq!(events::head_seq(project.store().read()).await.unwrap(), 0);
    let full = new_event(types::TASK_STATUS_CHANGED)
        .task(task(12))
        .proposal(ProposalId::from_number(3).unwrap())
        .workspace(WorkspaceId::from_number(4).unwrap())
        .revision(RevisionId::from_number(5).unwrap())
        .data(json!({"to": "review"}))
        .unwrap();
    let seqs = append_all(&project, vec![full.clone(), new_event(types::TASK_CREATED)]).await;
    assert_eq!(seqs, vec![1, 2]);
    let more = append_all(&project, vec![new_event(types::TASK_UPDATED)]).await;
    assert_eq!(more, vec![3]);
    assert_eq!(events::head_seq(project.store().read()).await.unwrap(), 3);

    let stored = events::read_after(project.store().read(), 0, &EventFilter::default(), 10)
        .await
        .unwrap();
    assert_eq!(
        stored.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(stored[0].event_type, types::TASK_STATUS_CHANGED);
    assert_eq!(stored[0].actor.as_str(), "frontend");
    assert_eq!(stored[0].task, full.task);
    assert_eq!(stored[0].proposal, full.proposal);
    assert_eq!(stored[0].workspace, full.workspace);
    assert_eq!(stored[0].revision, full.revision);
    assert_eq!(stored[0].data, json!({"to": "review"}));
    assert_eq!(stored[0].created_at, project.now());
    assert_eq!(stored[1].task, None);
    assert_eq!(stored[1].data, json!({}));

    let after_one = events::read_after(project.store().read(), 1, &EventFilter::default(), 10)
        .await
        .unwrap();
    assert_eq!(
        after_one.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![2, 3]
    );
    let limited = events::read_after(project.store().read(), 0, &EventFilter::default(), 2)
        .await
        .unwrap();
    assert_eq!(limited.len(), 2);
}

#[tokio::test]
async fn after_pages_every_type_in_seq_order() {
    let project = TestProject::new().await;
    let batch = vec![
        new_event(types::TASK_CREATED),
        new_event(types::TASK_UPDATED).task(task(1)),
        new_event(types::TASK_STATUS_CHANGED),
        new_event(types::TASK_CREATED),
    ];
    append_all(&project, batch).await;
    let pool = project.store().read();
    let seqs = |events: Vec<fold_core::Event>| events.iter().map(|e| e.seq).collect::<Vec<_>>();
    assert_eq!(seqs(events::after(pool, 0, 2).await.unwrap()), vec![1, 2]);
    assert_eq!(seqs(events::after(pool, 2, 2).await.unwrap()), vec![3, 4]);
    assert_eq!(
        seqs(events::after(pool, 4, 2).await.unwrap()),
        Vec::<u64>::new()
    );
    assert_eq!(
        seqs(events::after(pool, 1, 10).await.unwrap()),
        vec![2, 3, 4]
    );
}

#[tokio::test]
async fn events_are_refused_an_update_and_a_delete() {
    let project = TestProject::new().await;
    append_all(&project, vec![new_event(types::TASK_CREATED)]).await;
    assert_append_only(project.store(), "events").await;
}

#[tokio::test]
async fn a_rolled_back_append_consumes_no_seq() {
    let project = TestProject::new().await;
    let rolled_back = project
        .store()
        .write(async |tx| {
            events::append(tx, &new_event(types::TASK_CREATED), project.now()).await?;
            Err::<(), _>(StoreError::Invalid("stop"))
        })
        .await;
    assert!(rolled_back.is_err());
    assert_eq!(events::head_seq(project.store().read()).await.unwrap(), 0);
    let seqs = append_all(&project, vec![new_event(types::TASK_CREATED)]).await;
    assert_eq!(seqs, vec![1]);
}

#[tokio::test]
async fn filters_by_type_task_and_proposal() {
    let project = TestProject::new().await;
    let proposal = ProposalId::from_number(7).unwrap();
    append_all(
        &project,
        vec![
            new_event(types::TASK_CREATED).task(task(1)),
            new_event(types::TASK_UPDATED).task(task(1)),
            new_event(types::TASK_CREATED).task(task(2)),
            new_event(types::PROPOSAL_OPENED)
                .task(task(2))
                .proposal(proposal),
            new_event(types::REVIEW_SUBMITTED).proposal(proposal),
            new_event(types::MAIN_ADVANCED),
        ],
    )
    .await;
    let pool = project.store().read();
    let seqs = |filter: EventFilter| async move {
        events::read_after(pool, 0, &filter, 100)
            .await
            .unwrap()
            .iter()
            .map(|e| e.seq)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        seqs(EventFilter {
            types: vec![types::TASK_CREATED],
            ..EventFilter::default()
        })
        .await,
        vec![1, 3]
    );
    assert_eq!(
        seqs(EventFilter {
            types: vec![types::TASK_UPDATED, types::MAIN_ADVANCED],
            ..EventFilter::default()
        })
        .await,
        vec![2, 6]
    );
    assert_eq!(
        seqs(EventFilter {
            task: Some(task(2)),
            ..EventFilter::default()
        })
        .await,
        vec![3, 4]
    );
    assert_eq!(
        seqs(EventFilter {
            proposal: Some(proposal),
            ..EventFilter::default()
        })
        .await,
        vec![4, 5]
    );
    assert_eq!(
        seqs(EventFilter {
            types: vec![types::TASK_CREATED],
            task: Some(task(2)),
            proposal: None,
        })
        .await,
        vec![3]
    );
    assert!(
        seqs(EventFilter {
            task: Some(task(99)),
            ..EventFilter::default()
        })
        .await
        .is_empty()
    );
}

#[tokio::test]
async fn an_event_for_a_task_without_a_row_is_accepted() {
    let project = TestProject::new().await;
    let tasks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE number = 12")
        .fetch_one(project.store().read())
        .await
        .unwrap();
    assert_eq!(tasks, 0);
    append_all(
        &project,
        vec![new_event(types::TASK_DELETED).task(task(12))],
    )
    .await;
    let stored = events::read_after(project.store().read(), 0, &EventFilter::default(), 10)
        .await
        .unwrap();
    assert_eq!(stored[0].task, Some(task(12)));
}

#[tokio::test]
async fn append_refuses_live_types_and_non_object_data() {
    let project = TestProject::new().await;
    let mut live = new_event(types::TASK_CREATED);
    live.event_type = types::WORKSPACE_CHANGED;
    let mut scalar = new_event(types::TASK_CREATED);
    scalar.data = json!(5);
    for bad in [live, scalar] {
        let result = project
            .store()
            .write(async |tx| events::append(tx, &bad, project.now()).await)
            .await;
        assert!(matches!(result, Err(StoreError::Invalid(_))), "{result:?}");
    }
    assert_eq!(events::head_seq(project.store().read()).await.unwrap(), 0);
}

#[tokio::test]
async fn newest_by_agent_reads_the_agent_and_its_children_only() {
    let project = TestProject::new().await;
    let by = |actor: &str| {
        NewEvent::new(types::TASK_UPDATED, EventActor::parse(actor).unwrap()).unwrap()
    };
    let batch = [
        "fe",
        "fe-two",
        "fe/ui",
        "fed",
        "fe-two/ui",
        "fe/ui/deep",
        "fold",
        "fe",
    ]
    .map(by)
    .to_vec();
    append_all(&project, batch).await;
    let agent = fold_core::AgentName::parse("fe").unwrap();
    let newest = events::newest_by_agent(project.store().read(), &agent, 10)
        .await
        .unwrap();
    let seen: Vec<(u64, &str)> = newest.iter().map(|e| (e.seq, e.actor.as_str())).collect();
    assert_eq!(
        seen,
        [(8, "fe"), (6, "fe/ui/deep"), (3, "fe/ui"), (1, "fe")]
    );
    let two = events::newest_by_agent(project.store().read(), &agent, 2)
        .await
        .unwrap();
    assert_eq!(two.iter().map(|e| e.seq).collect::<Vec<_>>(), [8, 6]);
}

#[tokio::test]
async fn read_after_caps_the_page_at_one_thousand() {
    let project = TestProject::new().await;
    let batch = (0..1005).map(|_| new_event(types::TASK_UPDATED)).collect();
    append_all(&project, batch).await;
    let page = events::read_after(project.store().read(), 0, &EventFilter::default(), 5000)
        .await
        .unwrap();
    assert_eq!(page.len(), events::MAX_PAGE);
    let rest = events::read_after(
        project.store().read(),
        page.last().unwrap().seq,
        &EventFilter::default(),
        5000,
    )
    .await
    .unwrap();
    assert_eq!(rest.len(), 5);
}

#[tokio::test]
async fn rows_that_do_not_parse_are_reported_as_corrupt() {
    let columns = [
        ("type", "'task.exploded', 'fold', '{}'", "events.type"),
        ("task_id", "'task.created', 'fold', '{}'", "events.task_id"),
        ("actor", "'task.created', 'bad actor', '{}'", "events.actor"),
    ];
    for (column, values, expected) in columns {
        let project = TestProject::new().await;
        let task_id = if column == "task_id" { "0" } else { "NULL" };
        let sql = format!(
            "INSERT INTO events (type, actor, data, task_id, created_at) VALUES ({values}, {task_id}, 0)"
        );
        project
            .store()
            .write(async |tx| {
                sqlx::query(sqlx::AssertSqlSafe(sql))
                    .execute(tx.conn())
                    .await?;
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
        let error = events::read_after(project.store().read(), 0, &EventFilter::default(), 10)
            .await
            .unwrap_err();
        assert!(
            matches!(&error, StoreError::Corrupt(found) if *found == expected),
            "{error:?}"
        );
    }
}

#[cfg(test)]
fn by(event_type: EventType, actor: &str) -> NewEvent {
    NewEvent::new(event_type, EventActor::parse(actor).unwrap()).unwrap()
}

#[cfg(test)]
fn proposal(n: u32) -> ProposalId {
    ProposalId::from_number(n).unwrap()
}

#[cfg(test)]
async fn exec(project: &TestProject, sql: &str) {
    project
        .store()
        .write(async |tx| {
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .execute(tx.conn())
                .await
                .map_err(StoreError::from)
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn count_concerning_counts_what_concerns_holds_and_stops_at_its_cap() {
    let project = TestProject::new().await;
    let agent = AgentName::parse("fe").unwrap();
    let mut facts = AgentEventFacts::new(agent.clone());
    facts.claimed.insert(task(1));
    facts.prerequisites.insert(task(2));
    facts.authored.insert(proposal(3));
    facts.reviewing.insert(proposal(4));
    let batch = vec![
        by(types::TASK_UPDATED, "be").task(task(1)),
        by(types::TASK_UPDATED, "be").task(task(2)),
        by(types::TASK_UPDATED, "be").task(task(5)),
        by(types::REVIEW_SUBMITTED, "be").proposal(proposal(3)),
        by(types::REVIEW_SUBMITTED, "human:ana").proposal(proposal(4)),
        by(types::REVIEW_SUBMITTED, "be").proposal(proposal(6)),
        by(types::MAIN_ADVANCED, "fold"),
        by(types::TASK_UPDATED, "fe").task(task(1)),
        by(types::TASK_UPDATED, "fe/ui").task(task(1)),
        by(types::TASK_UPDATED, "fe-two").task(task(1)),
        by(types::MAIN_ADVANCED, "fe"),
    ];
    append_all(&project, batch).await;
    let stored = events::after(project.store().read(), 0, 100).await.unwrap();
    let tasks: BTreeSet<TaskId> = facts.claimed.union(&facts.prerequisites).copied().collect();
    let proposals: BTreeSet<ProposalId> = facts.authored.union(&facts.reviewing).copied().collect();
    let count = |after: u64, cap: u64| {
        let (agent, tasks, proposals) = (agent.clone(), tasks.clone(), proposals.clone());
        let store = project.store().clone();
        async move {
            events::count_concerning(
                store.read(),
                after,
                &agent,
                &tasks,
                &proposals,
                &[types::MAIN_ADVANCED],
                cap,
            )
            .await
            .unwrap()
        }
    };
    for after in [0, 4, 9, 11] {
        let expected = stored
            .iter()
            .filter(|event| event.seq > after && concerns(event, &facts))
            .count();
        assert_eq!(count(after, 1000).await, expected as u64, "after {after}");
    }
    assert_eq!(count(0, 1000).await, 6);
    assert_eq!(count(0, 2).await, 2);

    let backlog = (0..1500)
        .map(|_| by(types::MAIN_ADVANCED, "fold"))
        .collect();
    append_all(&project, backlog).await;
    assert_eq!(
        count(0, 101).await,
        101,
        "the count stops at its cap however long the backlog is"
    );
}

#[cfg(test)]
fn reach(agent_id: i64) -> events::Reach<'static> {
    events::Reach {
        agent_id,
        message_types: &[types::MESSAGE_SENT],
        own_types: &[types::MESSAGE_ACKNOWLEDGED],
        wake: None,
    }
}

#[tokio::test]
async fn read_reaching_keeps_a_message_event_only_for_an_agent_whose_worker_receives_it() {
    let project = TestProject::new().await;
    exec(
        &project,
        "INSERT INTO agents (id, name, created_at) VALUES (1, 'fe', 0), (2, 'be', 0);
         INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'fe', 0), (2, 2, 'be', 0);
         INSERT INTO messages (number, thread_id, sender_kind, sender_human, body, created_at)
         VALUES (1, 1, 'human', 'ana', 'one', 0), (2, 2, 'human', 'ana', 'two', 0);
         INSERT INTO message_recipients (message_id, worker_id, addressed_as)
         SELECT m.id, 2, 'agent' FROM messages m WHERE m.number = 1;
         INSERT INTO message_recipients (message_id, worker_id, addressed_as)
         SELECT m.id, 1, 'agent' FROM messages m WHERE m.number = 2;",
    )
    .await;
    let to_be = MessageId::from_number(1).unwrap();
    let to_fe = MessageId::from_number(2).unwrap();
    let mut batch: Vec<NewEvent> = (0..1200)
        .map(|_| by(types::MESSAGE_SENT, "human:ana").message(to_be))
        .collect();
    batch.push(by(types::TASK_UPDATED, "be").task(task(1)));
    batch.push(by(types::MESSAGE_ACKNOWLEDGED, "be").message(to_be));
    batch.push(by(types::MESSAGE_SENT, "human:ana").message(to_fe));
    let seqs = append_all(&project, batch).await;
    let page = events::read_reaching(
        project.store().read(),
        0,
        &EventFilter::default(),
        reach(1),
        100,
    )
    .await
    .unwrap();
    let read: Vec<u64> = page.iter().map(|event| event.seq).collect();
    assert_eq!(
        read,
        [seqs[1200], seqs[1202]],
        "one read passes the 1200 messages to another agent"
    );
    let backend = events::read_reaching(
        project.store().read(),
        seqs[1199],
        &EventFilter::default(),
        reach(2),
        2,
    )
    .await
    .unwrap();
    let read: Vec<u64> = backend.iter().map(|event| event.seq).collect();
    assert_eq!(read, [seqs[1200], seqs[1201]]);

    exec(
        &project,
        "INSERT INTO message_recipients (message_id, worker_id, addressed_as)
         SELECT m.id, 1, 'agent' FROM messages m WHERE m.number = 1;",
    )
    .await;
    let acked = append_all(
        &project,
        vec![by(types::MESSAGE_ACKNOWLEDGED, "be/sub").message(to_be)],
    )
    .await;
    let reach = async |agent: i64| -> Vec<u64> {
        events::read_reaching(
            project.store().read(),
            seqs[1202],
            &EventFilter::default(),
            reach(agent),
            10,
        )
        .await
        .unwrap()
        .iter()
        .map(|event| event.seq)
        .collect()
    };
    assert_eq!(
        reach(2).await,
        acked,
        "the acknowledging agent's subworker counts"
    );
    assert!(
        reach(1).await.is_empty(),
        "another recipient of the acknowledged message is not woken"
    );
}

#[tokio::test]
async fn read_reaching_selects_a_message_to_the_waking_worker_past_the_filter() {
    let project = TestProject::new().await;
    exec(
        &project,
        "INSERT INTO agents (id, name, created_at) VALUES (1, 'be', 0), (2, 'fe', 0);
         INSERT INTO workers (id, agent_id, name, parent_worker_id, created_at)
         VALUES (1, 1, 'be', NULL, 0), (2, 1, 'be/sub', 1, 0), (3, 2, 'fe', NULL, 0);
         INSERT INTO messages (number, thread_id, sender_kind, sender_human, body, created_at)
         VALUES (1, 1, 'human', 'ana', 'one', 0), (2, 2, 'human', 'ana', 'two', 0),
                (3, 3, 'human', 'ana', 'three', 0);
         INSERT INTO message_recipients (message_id, worker_id, addressed_as)
         SELECT m.id, CASE m.number WHEN 1 THEN 1 WHEN 2 THEN 2 ELSE 3 END, 'worker' FROM messages m;",
    )
    .await;
    let seqs = append_all(
        &project,
        vec![
            by(types::MESSAGE_SENT, "human:ana").message(MessageId::from_number(1).unwrap()),
            by(types::MESSAGE_SENT, "human:ana").message(MessageId::from_number(2).unwrap()),
            by(types::MESSAGE_SENT, "human:ana").message(MessageId::from_number(3).unwrap()),
            by(types::MAIN_ADVANCED, "fold"),
        ],
    )
    .await;
    let filter = EventFilter {
        types: vec![types::PROPOSAL_OPENED],
        ..EventFilter::default()
    };
    let read = async |worker_id: i64, every_worker: bool| -> Vec<u64> {
        let reach = events::Reach {
            wake: Some(events::MessageWake {
                event_type: types::MESSAGE_SENT,
                worker_id,
                every_worker,
            }),
            ..reach(1)
        };
        events::read_reaching(project.store().read(), 0, &filter, reach, 10)
            .await
            .unwrap()
            .iter()
            .map(|event| event.seq)
            .collect()
    };
    assert_eq!(read(1, false).await, [seqs[0]], "only the worker's own row");
    assert_eq!(
        read(1, true).await,
        [seqs[0], seqs[1]],
        "every worker of the agent, never another agent's"
    );
    assert_eq!(read(2, false).await, [seqs[1]]);
    let unwoken = events::read_reaching(project.store().read(), 0, &filter, reach(1), 10)
        .await
        .unwrap();
    assert!(unwoken.is_empty(), "without a wake the filter decides");
}

#[tokio::test]
async fn read_reaching_wakes_only_for_an_unread_message_the_worker_did_not_send() {
    let project = TestProject::new().await;
    exec(
        &project,
        "INSERT INTO agents (id, name, created_at) VALUES (1, 'be', 0);
         INSERT INTO workers (id, agent_id, name, parent_worker_id, created_at)
         VALUES (1, 1, 'be', NULL, 0), (2, 1, 'be/sub', 1, 0);
         INSERT INTO messages (number, thread_id, sender_kind, sender_human, body, created_at)
         VALUES (1, 1, 'human', 'ana', 'read', 0), (2, 2, 'human', 'ana', 'acknowledged', 0),
                (3, 3, 'human', 'ana', 'to itself', 0), (4, 4, 'human', 'ana', 'to its child', 0),
                (5, 5, 'human', 'ana', 'from its child', 0), (6, 6, 'human', 'ana', 'new', 0);
         INSERT INTO message_recipients (message_id, worker_id, addressed_as, delivered_at, acked_at)
         SELECT m.id, CASE m.number WHEN 4 THEN 2 ELSE 1 END, 'worker',
                CASE m.number WHEN 1 THEN 5 END, CASE m.number WHEN 2 THEN 5 END
         FROM messages m;",
    )
    .await;
    let message = |n: u32| MessageId::from_number(n).unwrap();
    let seqs = append_all(
        &project,
        vec![
            by(types::MESSAGE_SENT, "human:ana").message(message(1)),
            by(types::MESSAGE_SENT, "human:ana").message(message(2)),
            by(types::MESSAGE_SENT, "be").message(message(3)),
            by(types::MESSAGE_SENT, "be").message(message(4)),
            by(types::MESSAGE_SENT, "be/sub").message(message(5)),
            by(types::MESSAGE_SENT, "human:ana").message(message(6)),
        ],
    )
    .await;
    let filter = EventFilter {
        types: vec![types::PROPOSAL_OPENED],
        ..EventFilter::default()
    };
    let read = async |worker_id: i64, every_worker: bool| -> Vec<u64> {
        let reach = events::Reach {
            wake: Some(events::MessageWake {
                event_type: types::MESSAGE_SENT,
                worker_id,
                every_worker,
            }),
            ..reach(1)
        };
        events::read_reaching(project.store().read(), 0, &filter, reach, 10)
            .await
            .unwrap()
            .iter()
            .map(|event| event.seq)
            .collect()
    };
    assert_eq!(
        read(1, true).await,
        [seqs[4], seqs[5]],
        "a read, an acknowledged and an own message never wake the main worker; its child's does"
    );
    assert_eq!(
        read(2, false).await,
        [seqs[3]],
        "the main worker's message wakes its child"
    );
}

#[tokio::test]
async fn move_cursor_moves_only_from_where_the_cursor_stands() {
    let project = TestProject::new().await;
    exec(
        &project,
        "INSERT INTO agents (id, name, created_at) VALUES (1, 'fe', 0)",
    )
    .await;
    let mv = async |from: u64, to: u64| {
        project
            .store()
            .write(async |tx| events::move_cursor(tx, 1, from, to).await)
            .await
            .unwrap()
    };
    let cursor = async || events::cursor(project.store().read(), 1).await.unwrap();
    assert!(mv(0, 5).await, "no cursor yet stands at 0");
    assert_eq!(cursor().await, 5);
    assert!(!mv(0, 7).await, "another read moved it first");
    assert!(mv(5, 9).await);
    assert!(!mv(5, 12).await);
    assert_eq!(cursor().await, 9);
}
