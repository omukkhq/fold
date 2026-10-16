use std::sync::Arc;
use std::time::Duration;

use fold_core::{
    Actor, AgentActor, AgentName, EventFilter, FoldError, HumanName, Tag, TaskId, WorkerName, codes,
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
    let main = service.resolve_worker(&ctx, None).await.unwrap();
    assert_eq!(main.name, worker("frontend"));
    assert_eq!(main.parent_id, None);
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
async fn the_integrator_tag_is_refused_before_anything_is_written() {
    let project = TestProject::new().await;
    let (service, _bus) = setup(&project);
    let mut ctx = service.start_session(1).await.unwrap();
    let error = fold_error(
        service
            .register_agent(&mut ctx, &agent("boss"), &[tag("integrator")])
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code(), codes::HUMAN_REQUIRED);
    assert_eq!(ctx.agent, None);
    assert!(stored(&project).await.is_empty());
    assert!(
        agents::find_agent(project.store().read(), &agent("boss"))
            .await
            .unwrap()
            .is_none()
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
async fn register_worker_builds_children_under_the_main_worker() {
    let project = TestProject::new().await;
    let (service, _bus) = setup(&project);
    let mut ctx = service.start_session(1).await.unwrap();
    service
        .register_agent(&mut ctx, &agent("frontend"), &[])
        .await
        .unwrap();
    let ui = service.register_worker(&ctx, "ui", None).await.unwrap();
    assert!(ui.created);
    assert_eq!(ui.worker.name, worker("frontend/ui"));
    let main = service.resolve_worker(&ctx, None).await.unwrap();
    assert_eq!(ui.worker.parent_id, Some(main.id));

    let again = service.register_worker(&ctx, "ui", None).await.unwrap();
    assert!(!again.created);
    assert_eq!(again.worker.id, ui.worker.id);

    let nested = service
        .register_worker(&ctx, "btn", Some(&worker("frontend/ui")))
        .await
        .unwrap();
    assert_eq!(nested.worker.name, worker("frontend/ui/btn"));
    assert_eq!(nested.worker.parent_id, Some(ui.worker.id));

    let types: Vec<String> = stored(&project).await.into_iter().map(|e| e.0).collect();
    assert_eq!(
        types.iter().filter(|t| *t == "worker.registered").count(),
        3
    );

    let bad = fold_error(
        service
            .register_worker(&ctx, "Bad", None)
            .await
            .unwrap_err(),
    );
    assert_eq!(bad.code(), codes::INVALID_NAME);
    let missing = fold_error(
        service
            .register_worker(&ctx, "x", Some(&worker("frontend/nope")))
            .await
            .unwrap_err(),
    );
    assert_eq!(missing.code(), codes::WORKER_NOT_FOUND);
}

#[tokio::test]
async fn register_worker_needs_a_registered_agent_and_never_adopts_a_foreign_parent() {
    let project = TestProject::new().await;
    let (service, _bus) = setup(&project);
    let unbound = service.start_session(1).await.unwrap();
    let error = fold_error(
        service
            .register_worker(&unbound, "ui", None)
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code(), codes::AGENT_NOT_REGISTERED);

    let mut front = service.start_session(2).await.unwrap();
    service
        .register_agent(&mut front, &agent("frontend"), &[])
        .await
        .unwrap();
    let mut back = service.start_session(3).await.unwrap();
    service
        .register_agent(&mut back, &agent("backend"), &[])
        .await
        .unwrap();
    let error = fold_error(
        service
            .register_worker(&back, "x", Some(&worker("frontend")))
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code(), codes::WORKER_NOT_FOUND);
}

#[tokio::test]
async fn resolve_worker_hides_workers_of_other_agents() {
    let project = TestProject::new().await;
    let (service, _bus) = setup(&project);
    let mut front = service.start_session(1).await.unwrap();
    service
        .register_agent(&mut front, &agent("frontend"), &[])
        .await
        .unwrap();
    service.register_worker(&front, "ui", None).await.unwrap();
    let mut back = service.start_session(2).await.unwrap();
    service
        .register_agent(&mut back, &agent("backend"), &[])
        .await
        .unwrap();

    let own = service
        .resolve_worker(&front, Some(&worker("frontend/ui")))
        .await
        .unwrap();
    assert_eq!(own.name, worker("frontend/ui"));
    for foreign in ["frontend/ui", "frontend", "backend/ghost"] {
        let error = fold_error(
            service
                .resolve_worker(&back, Some(&worker(foreign)))
                .await
                .unwrap_err(),
        );
        assert_eq!(error.code(), codes::WORKER_NOT_FOUND, "{foreign}");
        assert_eq!(error.hint(), Some("call register_worker"));
    }
    assert_eq!(
        service.resolve_worker(&back, None).await.unwrap().name,
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
    let error = fold_error(service.resolve_worker(&ctx, None).await.unwrap_err());
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
    let main = service.resolve_worker(&ctx, None).await.unwrap();
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
    let after = service.resolve_worker(&ctx, None).await.unwrap();
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
    let after = service.resolve_worker(&ctx, None).await.unwrap();
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
    let mut other = service.start_session(2).await.unwrap();
    let before = stored(&project).await.len();
    let error = service
        .register_agent(&mut other, &agent("frontend"), &[tag("integrator")])
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
fn human(name: &str) -> Actor {
    Actor::human(HumanName::parse(name).unwrap())
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
    service.register_worker(&ctx, "ui", None).await.unwrap();
    assert_eq!(
        service
            .actor(&ctx, Some(&worker("frontend/ui")))
            .await
            .unwrap(),
        Actor::Agent(AgentActor {
            agent: agent("frontend"),
            worker: worker("frontend/ui"),
            tags: vec![tag("web")],
        })
    );
    let main = service.actor(&ctx, None).await.unwrap();
    assert_eq!(main.event_actor().as_str(), "frontend");
    let unbound = service.start_session(8).await.unwrap();
    assert_eq!(
        fold_error(service.actor(&unbound, None).await.unwrap_err()).code(),
        codes::AGENT_NOT_REGISTERED
    );
}

#[tokio::test]
async fn an_agent_registering_with_integrator_gets_human_required_even_when_it_has_other_tags() {
    let project = TestProject::new().await;
    let (service, _bus) = setup(&project);
    let mut ctx = registered(&service, "review", &[tag("review")]).await;
    let before = stored(&project).await.len();
    let error = fold_error(
        service
            .register_agent(&mut ctx, &agent("review"), &[tag("ui"), tag("integrator")])
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code(), codes::HUMAN_REQUIRED);
    assert!(error.hint().is_some());
    assert_eq!(stored(&project).await.len(), before);
    let review = agents::find_agent(project.store().read(), &agent("review"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        agents::tags(project.store().read(), review.id)
            .await
            .unwrap(),
        vec![tag("review")]
    );
}

#[tokio::test]
async fn set_integrator_with_an_agent_or_system_actor_gets_human_required() {
    let project = TestProject::new().await;
    let (service, _bus) = setup(&project);
    let ctx = registered(&service, "review", &[]).await;
    let me = service.actor(&ctx, None).await.unwrap();
    for (actor, on) in [(me.clone(), true), (me, false), (Actor::System, true)] {
        let error = fold_error(
            service
                .set_integrator(&actor, &agent("review"), on)
                .await
                .unwrap_err(),
        );
        assert_eq!(error.code(), codes::HUMAN_REQUIRED);
    }
    // The policy runs before the lookup, so an agent learns nothing about other agents.
    let error = fold_error(
        service
            .set_integrator(&Actor::System, &agent("ghost"), true)
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code(), codes::HUMAN_REQUIRED);
    let types: Vec<String> = stored(&project).await.into_iter().map(|e| e.0).collect();
    assert!(
        !types.iter().any(|t| t.starts_with("agent.tag")),
        "{types:?}"
    );
}

#[tokio::test]
async fn a_human_grants_and_removes_integrator_once_each() {
    let project = TestProject::new().await;
    let (service, bus) = setup(&project);
    registered(&service, "review", &[tag("review")]).await;
    let mut receiver = bus.subscribe();
    let tanim = human("tanim");

    let granted = service
        .set_integrator(&tanim, &agent("review"), true)
        .await
        .unwrap();
    assert_eq!(granted.agent, agent("review"));
    assert_eq!(granted.tags, vec![tag("integrator"), tag("review")]);
    let Ok(BusMessage::Durable(event)) = receiver.try_recv() else {
        panic!("expected agent.tag_added on the bus");
    };
    assert_eq!(event.event_type.as_str(), "agent.tag_added");

    let again = service
        .set_integrator(&tanim, &agent("review"), true)
        .await
        .unwrap();
    assert_eq!(again, granted);
    assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));

    let removed = service
        .set_integrator(&tanim, &agent("review"), false)
        .await
        .unwrap();
    assert_eq!(removed.tags, vec![tag("review")]);
    let absent = service
        .set_integrator(&tanim, &agent("review"), false)
        .await
        .unwrap();
    assert_eq!(absent, removed);

    let tag_events: Vec<_> = stored(&project)
        .await
        .into_iter()
        .filter(|e| e.0.starts_with("agent.tag"))
        .collect();
    assert_eq!(
        tag_events,
        vec![
            (
                "agent.tag_added".to_owned(),
                "human:tanim".to_owned(),
                serde_json::json!({"agent": "review", "tag": "integrator"})
            ),
            (
                "agent.tag_removed".to_owned(),
                "human:tanim".to_owned(),
                serde_json::json!({"agent": "review", "tag": "integrator"})
            ),
        ]
    );
}

#[tokio::test]
async fn set_integrator_on_an_unknown_agent_is_agent_not_found() {
    let project = TestProject::new().await;
    let (service, _bus) = setup(&project);
    let error = fold_error(
        service
            .set_integrator(&human("tanim"), &agent("ghost"), true)
            .await
            .unwrap_err(),
    );
    assert_eq!(error.code(), codes::AGENT_NOT_FOUND);
    assert!(stored(&project).await.is_empty());
}
