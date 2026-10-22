use std::sync::Arc;
use std::time::Duration;

use fold_core::{
    Actor, AgentActor, AgentName, EventFilter, FoldError, Tag, TaskId, WorkerName, codes,
};
use fold_daemon::services::agents::{AgentError, AgentService, SessionContext};
use fold_daemon::services::events::{BusMessage, EventBus};
use fold_store::testing::TestProject;
use fold_store::{agents, events};
use tokio::sync::broadcast::error::TryRecvError;

#[cfg(test)]
fn setup(project: &TestProject) -> (AgentService, EventBus) {
    let bus = EventBus::new();
    let service = AgentService::new(
        project.store().clone(),
        bus.clone(),
        Arc::new(project.clock().clone()),
    );
    (service, bus)
}

#[cfg(test)]
fn agent(name: &str) -> AgentName {
    AgentName::parse(name).unwrap()
}

#[cfg(test)]
fn tag(name: &str) -> Tag {
    Tag::parse(name).unwrap()
}

#[cfg(test)]
fn worker(name: &str) -> WorkerName {
    WorkerName::parse(name).unwrap()
}

#[cfg(test)]
fn fold_error(error: AgentError) -> FoldError {
    match error {
        AgentError::Fold(error) => error,
        other => panic!("expected a fold error, got {other:?}"),
    }
}

#[cfg(test)]
async fn stored(project: &TestProject) -> Vec<(String, String, serde_json::Value)> {
    events::read_after(project.store().read(), 0, &EventFilter::default(), 1000)
        .await
        .unwrap()
        .into_iter()
        .map(|e| (e.event_type.to_string(), e.actor.to_string(), e.data))
        .collect()
}

#[tokio::test]
async fn register_agent_is_idempotent_and_merges_tags() {
    let project = TestProject::new().await;
    let (service, _bus) = setup(&project);
    let mut ctx = service.start_session(100).await.unwrap();
    assert_eq!(ctx.agent, None);
    assert!(stored(&project).await.is_empty());

    let first = service
        .register_agent(&mut ctx, &agent("frontend"), &[tag("web"), tag("ui")])
        .await
        .unwrap();
    assert!(first.created);
    assert_eq!(first.main_worker, worker("frontend"));
    assert_eq!(first.tags, vec![tag("ui"), tag("web")]);
    assert_eq!(ctx.agent, Some(agent("frontend")));

    let second = service
        .register_agent(&mut ctx, &agent("frontend"), &[tag("web"), tag("review")])
        .await
        .unwrap();
    assert!(!second.created);
    assert_eq!(second.tags, vec![tag("review"), tag("ui"), tag("web")]);

    let types: Vec<String> = stored(&project).await.into_iter().map(|e| e.0).collect();
    assert_eq!(
        types,
        ["agent.registered", "worker.registered", "session.started"]
    );
    let main = service.resolve_worker(&ctx).await.unwrap();
    assert_eq!(main.name, worker("frontend"));
}

#[tokio::test]
async fn a_second_session_binds_to_the_existing_agent_without_new_agent_events() {
    let project = TestProject::new().await;
    let (service, _bus) = setup(&project);
    let mut first = service.start_session(1).await.unwrap();
    service
        .register_agent(&mut first, &agent("frontend"), &[])
        .await
        .unwrap();
    let mut second = service.start_session(2).await.unwrap();
    let again = service
        .register_agent(&mut second, &agent("frontend"), &[])
        .await
        .unwrap();
    assert!(!again.created);
    let types: Vec<String> = stored(&project).await.into_iter().map(|e| e.0).collect();
    assert_eq!(
        types,
        [
            "agent.registered",
            "worker.registered",
            "session.started",
            "session.started"
        ]
    );
}

#[tokio::test]
async fn any_tag_but_main_is_a_label_the_agent_sets_on_itself() {
    let project = TestProject::new().await;
    let (service, _bus) = setup(&project);
    let mut ctx = service.start_session(1).await.unwrap();
    service
        .register_agent(&mut ctx, &agent("boss"), &[tag("ops"), tag("ui")])
        .await
        .unwrap();
    let boss = agents::find_agent(project.store().read(), &agent("boss"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        agents::tags(project.store().read(), boss.id).await.unwrap(),
        vec![tag("ops"), tag("ui")]
    );
}

#[tokio::test]
async fn a_bound_session_cannot_register_another_agent() {
    let project = TestProject::new().await;
    let (service, _bus) = setup(&project);
    let mut ctx = service.start_session(1).await.unwrap();
    service
        .register_agent(&mut ctx, &agent("frontend"), &[])
        .await
        .unwrap();
    let error = fold_error(
        service
            .register_agent(&mut ctx, &agent("backend"), &[])
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code(), codes::AGENT_ALREADY_BOUND);
    assert_eq!(ctx.agent, Some(agent("frontend")));
    assert!(
        agents::find_agent(project.store().read(), &agent("backend"))
            .await
            .unwrap()
            .is_none()
    );
    // A stale context cannot dodge the check: the stored binding decides.
    let mut stale = SessionContext {
        session_id: ctx.session_id,
        agent: None,
        peer: Default::default(),
    };
    let error = fold_error(
        service
            .register_agent(&mut stale, &agent("backend"), &[])
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code(), codes::AGENT_ALREADY_BOUND);
}

#[tokio::test]
async fn an_unknown_or_ended_session_cannot_register() {
    let project = TestProject::new().await;
    let (service, _bus) = setup(&project);
    let mut ghost = SessionContext {
        session_id: 999,
        agent: None,
        peer: Default::default(),
    };
    assert!(matches!(
        service.register_agent(&mut ghost, &agent("a"), &[]).await,
        Err(AgentError::Store(_))
    ));
    let mut ended = service.start_session(1).await.unwrap();
    service.end_session(&ended).await.unwrap();
    assert!(matches!(
        service.register_agent(&mut ended, &agent("a"), &[]).await,
        Err(AgentError::Store(_))
    ));
}

#[tokio::test]
async fn each_session_resolves_to_the_one_worker_of_its_agent() {
    let project = TestProject::new().await;
    let (service, _bus) = setup(&project);
    let mut front = service.start_session(1).await.unwrap();
    service
        .register_agent(&mut front, &agent("frontend"), &[])
        .await
        .unwrap();
    let mut back = service.start_session(2).await.unwrap();
    service
        .register_agent(&mut back, &agent("backend"), &[])
        .await
        .unwrap();
    let mut again = service.start_session(3).await.unwrap();
    service
        .register_agent(&mut again, &agent("frontend"), &[])
        .await
        .unwrap();

    let first = service.resolve_worker(&front).await.unwrap();
    assert_eq!(first.name, worker("frontend"));
    assert_eq!(service.resolve_worker(&again).await.unwrap().id, first.id);
    assert_eq!(
        service.resolve_worker(&back).await.unwrap().name,
        worker("backend")
    );
}

#[tokio::test]
async fn require_agent_fails_before_binding() {
    let project = TestProject::new().await;
    let (service, _bus) = setup(&project);
    let mut ctx = service.start_session(1).await.unwrap();
    let error = fold_error(service.require_agent(&ctx).await.unwrap_err());
    assert_eq!(error.code(), codes::AGENT_NOT_REGISTERED);
    assert_eq!(error.hint(), Some("call register_agent"));
    let error = fold_error(service.resolve_worker(&ctx).await.unwrap_err());
    assert_eq!(error.code(), codes::AGENT_NOT_REGISTERED);

    service
        .register_agent(&mut ctx, &agent("frontend"), &[])
        .await
        .unwrap();
    assert_eq!(
        service.require_agent(&ctx).await.unwrap().name,
        agent("frontend")
    );

    let dangling = SessionContext {
        session_id: ctx.session_id,
        agent: Some(agent("ghost")),
        peer: Default::default(),
    };
    let error = fold_error(service.require_agent(&dangling).await.unwrap_err());
    assert_eq!(error.code(), codes::AGENT_NOT_REGISTERED);
}

#[tokio::test]
async fn record_call_updates_activity_and_times() {
    let project = TestProject::new().await;
    let (service, _bus) = setup(&project);
    let mut ctx = service.start_session(1).await.unwrap();
    service
        .register_agent(&mut ctx, &agent("frontend"), &[])
        .await
        .unwrap();
    let main = service.resolve_worker(&ctx).await.unwrap();
    assert_eq!(main.activity, None);
    assert_eq!(main.last_call_at, None);

    project.clock().advance(Duration::from_secs(5)).unwrap();
    service
        .record_call(
            &ctx,
            &main,
            "claim_task",
            Some(TaskId::from_number(12).unwrap()),
        )
        .await
        .unwrap();
    let after = service.resolve_worker(&ctx).await.unwrap();
    assert_eq!(after.activity.as_deref(), Some("claim_task T-12"));
    assert_eq!(after.last_call_at, Some(project.now()));
    let session = agents::find_session(project.store().read(), ctx.session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session.last_call_at, Some(project.now()));

    project.clock().advance(Duration::from_secs(1)).unwrap();
    service
        .record_call(&ctx, &main, "list_tasks", None)
        .await
        .unwrap();
    let after = service.resolve_worker(&ctx).await.unwrap();
    assert_eq!(after.activity.as_deref(), Some("list_tasks"));
    assert_eq!(after.last_call_at, Some(project.now()));
}

#[tokio::test]
async fn end_session_is_safe_twice_and_stores_one_event() {
    let project = TestProject::new().await;
    let (service, _bus) = setup(&project);
    let unbound = service.start_session(1).await.unwrap();
    service.end_session(&unbound).await.unwrap();
    assert!(stored(&project).await.is_empty());

    let mut ctx = service.start_session(2).await.unwrap();
    service
        .register_agent(&mut ctx, &agent("frontend"), &[])
        .await
        .unwrap();
    project.clock().advance(Duration::from_secs(9)).unwrap();
    service.end_session(&ctx).await.unwrap();
    let ended_at = project.now();
    project.clock().advance(Duration::from_secs(9)).unwrap();
    service.end_session(&ctx).await.unwrap();
    service
        .end_session(&SessionContext {
            session_id: 999,
            agent: None,
            peer: Default::default(),
        })
        .await
        .unwrap();

    let session = agents::find_session(project.store().read(), ctx.session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session.ended_at, Some(ended_at));
    let ended: Vec<_> = stored(&project)
        .await
        .into_iter()
        .filter(|e| e.0 == "session.ended")
        .collect();
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].1, "frontend");
}

#[tokio::test]
async fn close_orphan_sessions_ends_open_sessions_at_the_clock_time() {
    let project = TestProject::new().await;
    let (service, _bus) = setup(&project);
    let mut a = service.start_session(1).await.unwrap();
    service
        .register_agent(&mut a, &agent("frontend"), &[])
        .await
        .unwrap();
    let mut b = service.start_session(2).await.unwrap();
    service
        .register_agent(&mut b, &agent("backend"), &[])
        .await
        .unwrap();
    let unbound = service.start_session(3).await.unwrap();
    let mut done = service.start_session(4).await.unwrap();
    service
        .register_agent(&mut done, &agent("review"), &[])
        .await
        .unwrap();
    project.clock().advance(Duration::from_secs(1)).unwrap();
    service.end_session(&done).await.unwrap();

    project.clock().advance(Duration::from_secs(60)).unwrap();
    let restart = project.now();
    assert_eq!(service.close_orphan_sessions().await.unwrap(), 3);
    assert_eq!(service.close_orphan_sessions().await.unwrap(), 0);

    for id in [a.session_id, b.session_id, unbound.session_id] {
        let session = agents::find_session(project.store().read(), id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(session.ended_at, Some(restart));
    }
    let orphaned: Vec<_> = stored(&project)
        .await
        .into_iter()
        .filter(|e| e.0 == "session.ended" && e.2.get("reason").is_some())
        .collect();
    assert_eq!(orphaned.len(), 2);
    for event in &orphaned {
        assert_eq!(event.1, "fold");
        assert_eq!(event.2["reason"], "daemon_restart");
    }
    assert_eq!(orphaned[0].2["agent"], "frontend");
    assert_eq!(orphaned[1].2["agent"], "backend");
}

#[tokio::test]
async fn events_reach_the_bus_only_after_the_commit() {
    let project = TestProject::new().await;
    let (service, bus) = setup(&project);
    let mut receiver = bus.subscribe();
    let mut ctx = service.start_session(1).await.unwrap();
    assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));

    service
        .register_agent(&mut ctx, &agent("frontend"), &[])
        .await
        .unwrap();
    let mut seqs = Vec::new();
    while let Ok(BusMessage::Durable(event)) = receiver.try_recv() {
        // Everything the bus carried is already stored.
        let rows = events::read_after(
            project.store().read(),
            event.seq - 1,
            &EventFilter::default(),
            1,
        )
        .await
        .unwrap();
        assert_eq!(rows[0].seq, event.seq);
        seqs.push(event.seq);
    }
    assert_eq!(seqs.len(), 3);

    // A refused command rolls back and publishes nothing.
    let mut holder = service.start_session(2).await.unwrap();
    service
        .register_agent(&mut holder, &agent("lead"), &[tag("main")])
        .await
        .unwrap();
    while receiver.try_recv().is_ok() {}
    let mut other = service.start_session(3).await.unwrap();
    let before = stored(&project).await.len();
    let error = service
        .register_agent(&mut other, &agent("frontend"), &[tag("main")])
        .await
        .unwrap_err();
    assert!(matches!(error, AgentError::Fold(_)));
    let _ = service
        .register_agent(&mut ctx, &agent("backend"), &[])
        .await;
    assert_eq!(stored(&project).await.len(), before);
    assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
}

#[tokio::test]
async fn a_failed_write_after_an_append_stores_and_publishes_nothing() {
    let project = TestProject::new().await;
    let (service, bus) = setup(&project);
    let mut receiver = bus.subscribe();
    let mut ctx = service.start_session(1).await.unwrap();
    // The tag insert fails after the agent events were appended, so the whole write must roll back.
    project
        .store()
        .write(async |tx| {
            sqlx::query(
                "CREATE TRIGGER refuse_tags BEFORE INSERT ON agent_tags BEGIN SELECT RAISE(ABORT, 'refused'); END",
            )
            .execute(tx.conn())
            .await?;
            Ok::<_, fold_store::StoreError>(())
        })
        .await
        .unwrap();
    let error = service
        .register_agent(&mut ctx, &agent("frontend"), &[tag("web")])
        .await
        .unwrap_err();
    assert!(matches!(error, AgentError::Store(_)));
    assert_eq!(ctx.agent, None);
    assert!(stored(&project).await.is_empty());
    assert!(
        agents::find_agent(project.store().read(), &agent("frontend"))
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
}

#[cfg(test)]
async fn registered(service: &AgentService, name: &str, tags: &[Tag]) -> SessionContext {
    let mut ctx = service.start_session(7).await.unwrap();
    service
        .register_agent(&mut ctx, &agent(name), tags)
        .await
        .unwrap();
    ctx
}

#[tokio::test]
async fn actor_carries_the_resolved_worker_and_the_current_tags() {
    let project = TestProject::new().await;
    let (service, _bus) = setup(&project);
    let ctx = registered(&service, "frontend", &[tag("web")]).await;
    let actor = service.actor(&ctx).await.unwrap();
    assert_eq!(
        actor,
        Actor::Agent(AgentActor {
            agent: agent("frontend"),
            worker: worker("frontend"),
            tags: vec![tag("web")],
        })
    );
    assert_eq!(actor.event_actor().as_str(), "frontend");
    let unbound = service.start_session(8).await.unwrap();
    assert_eq!(
        fold_error(service.actor(&unbound).await.unwrap_err()).code(),
        codes::AGENT_NOT_REGISTERED
    );
}
