use fold_core::events::types;
use fold_core::{
    EventActor, EventFilter, EventType, NewEvent, ProposalId, RevisionId, TaskId, WorkspaceId,
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
