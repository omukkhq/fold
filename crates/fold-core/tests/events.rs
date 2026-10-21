use fold_core::events::types;
use fold_core::{
    Event, EventActor, EventError, EventFilter, EventType, LiveEvent, NewEvent, ProposalId,
    RevisionId, TaskId, Timestamp, WorkspaceId,
};
use serde_json::json;

/// The event types table of the plan, one string per type, in table order.
const PLAN: [&str; 57] = [
    "agent.registered",
    "worker.registered",
    "session.started",
    "session.ended",
    "agent.tag_added",
    "agent.tag_removed",
    "task.created",
    "task.updated",
    "task.deleted",
    "task.archived",
    "task.completed",
    "task.status_changed",
    "task.blocked_changed",
    "task.human_review_set",
    "task.human_review_cleared",
    "label.created",
    "dependency.added",
    "dependency.removed",
    "checklist.created",
    "checklist.item_added",
    "checklist.item_updated",
    "checklist.item_done",
    "checklist.item_removed",
    "claim.acquired",
    "claim.released",
    "claim.taken_over",
    "claim.stale",
    "workspace.created",
    "workspace.adopted",
    "workspace.abandoned",
    "workspace.archived",
    "workspace.synced",
    "workspace.missing",
    "workspace.recovered",
    "workspace.watch_degraded",
    "workspace.changed",
    "checkpoint.recorded",
    "checkpoint.restored",
    "checkpoint.pruned",
    "changeset.recorded",
    "proposal.opened",
    "proposal.version_added",
    "proposal.closed",
    "proposal.integrated",
    "review.submitted",
    "review.comment_added",
    "review.batch_created",
    "review.batch_submitted",
    "review.batch_closed",
    "integration.succeeded",
    "integration.conflicted",
    "integration.refused",
    "main.advanced",
    "remote.pushed",
    "remote.push_failed",
    "remote.pulled",
    "remote.pull_conflicted",
];

#[test]
fn all_equals_the_plans_event_types() {
    let declared: Vec<&str> = EventType::ALL.iter().map(|t| t.as_str()).collect();
    let mut expected: Vec<&str> = PLAN.to_vec();
    expected.extend([
        "message.sent",
        "message.acknowledged",
        "request.created",
        "request.claimed",
        "request.released",
        "request.draft_submitted",
        "request.changes_requested",
        "request.approved",
        "request.closed",
        "git.commit_failed",
        "git.outside_commits",
        "git.uncommitted_changes",
        "git.attention_cleared",
        "git.committed",
        "agent.presence_changed",
        "area.defined",
        "worker.started",
        "worker.stopped",
        "worker.failed",
        "worker.output",
        "worker.refreshed",
        "worker.resumed",
        "worker.permission_asked",
        "worker.permission_answered",
        "workers.limits_changed",
        "session.bind_refused",
        "area.updated",
        "task.reported",
        "task.sent_back",
        "proposal.author_changed",
        "project.paused",
        "project.resumed",
        "task.hold_set",
        "task.hold_cleared",
        "proposal.human_edited",
        "proposal.review_again_requested",
        "human_change.recorded",
        "human_change.conflicted",
        "human_change.resolved",
        "git.sync_enabled",
        "git.head_moved",
        "human_change.withdrawn",
        "proposal.reviewer_fixed",
        "stash.pushed",
        "stash.applied",
        "stash.conflicted",
        "stash.dropped",
        "checks.started",
        "checks.finished",
        "integration.queued",
        "integration.unqueued",
        "integration.held",
        "review.scope_chosen",
        "worker.model_unreachable",
        "message.pushed",
        "message.ask_closed",
        "review.plan_created",
        "review.plan_changed",
        "review.plan_joined",
        "review.plan_left",
        "review.plan_wait_ended",
        "review.plan_started",
        "review.plan_ended",
        "review.batch_integrated",
    ]);
    assert_eq!(declared, expected);
}

#[test]
fn declared_names_are_domain_dot_verb_in_snake_case() {
    for event_type in EventType::ALL {
        let name = event_type.as_str();
        let (domain, verb) = name.split_once('.').unwrap();
        for part in [domain, verb] {
            assert!(!part.is_empty(), "{name}");
            assert!(
                part.split('_')
                    .all(|word| !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase())),
                "{name}"
            );
        }
        assert!(!verb.contains('.'), "{name}");
        assert_eq!(event_type.to_string(), name);
    }
}

#[test]
fn parse_accepts_declared_names_and_refuses_the_rest() {
    for event_type in EventType::ALL {
        assert_eq!(EventType::parse(event_type.as_str()), Ok(*event_type));
    }
    assert_eq!(EventType::parse("task.created"), Ok(types::TASK_CREATED));
    for bad in [
        "",
        "task",
        "task.",
        ".created",
        "task.exploded",
        "Task.created",
        "task.created ",
        "task..created",
        "task.status-changed",
        "task.created.extra",
    ] {
        assert_eq!(
            EventType::parse(bad),
            Err(EventError::UnknownType),
            "{bad:?}"
        );
    }
}

#[test]
fn workspace_changed_presence_changed_worker_output_and_message_pushed_are_the_live_types() {
    let live: Vec<_> = EventType::ALL.iter().filter(|t| t.is_live()).collect();
    assert_eq!(
        live,
        vec![
            &types::WORKSPACE_CHANGED,
            &types::AGENT_PRESENCE_CHANGED,
            &types::WORKER_OUTPUT,
            &types::MESSAGE_PUSHED
        ]
    );
}

#[test]
fn types_serialise_as_names() {
    assert_eq!(
        serde_json::to_string(&types::MAIN_ADVANCED).unwrap(),
        "\"main.advanced\""
    );
    assert_eq!(
        serde_json::from_str::<EventType>("\"main.advanced\"").unwrap(),
        types::MAIN_ADVANCED
    );
    assert!(serde_json::from_str::<EventType>("\"main.retreated\"").is_err());
}

#[test]
fn actors_are_workers_humans_or_fold() {
    for good in [
        "fold",
        "frontend",
        "frontend/ui",
        "frontend/ui/deep",
        "a/b/c/d",
        "back-end_2.x/db",
        "human:ana",
        "human:Ana Lopez",
    ] {
        assert_eq!(EventActor::parse(good).unwrap().as_str(), good);
    }
    for bad in [
        "",
        "/ui",
        "frontend/",
        "a//b",
        "Front/ui/deep",
        "front end",
        "human:",
        "human: ana",
        "human:ana ",
        "human:a\nb",
        "Human:ana",
        "front:end",
        &"x".repeat(65),
        &format!("human:{}", "x".repeat(65)),
    ] {
        assert_eq!(
            EventActor::parse(bad),
            Err(EventError::InvalidActor),
            "{bad:?}"
        );
    }
    assert_eq!(EventActor::fold().to_string(), "fold");
    assert_eq!(
        serde_json::to_string(&EventActor::fold()).unwrap(),
        "\"fold\""
    );
    assert!(serde_json::from_str::<EventActor>("\"bad actor\"").is_err());
}

#[test]
fn new_events_refuse_live_types_and_non_object_data() {
    let fold = EventActor::fold();
    for live in [types::WORKSPACE_CHANGED, types::AGENT_PRESENCE_CHANGED] {
        assert_eq!(
            NewEvent::new(live, fold.clone()),
            Err(EventError::LiveNotStored)
        );
    }
    let event = NewEvent::new(types::TASK_CREATED, fold).unwrap();
    assert_eq!(event.data, json!({}));
    assert_eq!(
        event.clone().data(json!([1])),
        Err(EventError::DataNotObject)
    );
    let built = event
        .task(TaskId::from_number(1).unwrap())
        .proposal(ProposalId::from_number(2).unwrap())
        .workspace(WorkspaceId::from_number(3).unwrap())
        .revision(RevisionId::from_number(4).unwrap())
        .message(fold_core::MessageId::from_number(5).unwrap())
        .data(json!({"k": 1}))
        .unwrap();
    assert_eq!(built.task.map(TaskId::number), Some(1));
    assert_eq!(built.proposal.map(ProposalId::number), Some(2));
    assert_eq!(built.workspace.map(WorkspaceId::number), Some(3));
    assert_eq!(built.revision.map(RevisionId::number), Some(4));
    assert_eq!(built.message.map(fold_core::MessageId::number), Some(5));
    assert_eq!(built.data, json!({"k": 1}));
}

#[cfg(test)]
fn sample() -> Event {
    Event {
        seq: 41,
        event_type: types::TASK_STATUS_CHANGED,
        actor: EventActor::parse("frontend/ui").unwrap(),
        task: TaskId::from_number(12),
        proposal: ProposalId::from_number(3),
        workspace: None,
        revision: RevisionId::from_number(17),
        data: json!({"from": "in_progress", "to": "review"}),
        created_at: Timestamp::from_millis(1_767_225_600_000).unwrap(),
    }
}

#[test]
fn event_serialises_to_a_golden_json_with_public_ids() {
    const GOLDEN: &str = r#"{"seq":41,"type":"task.status_changed","actor":"frontend/ui","task":"T-12","proposal":"P-3","workspace":null,"revision":"r17","data":{"from":"in_progress","to":"review"},"created_at":1767225600000}"#;
    let event = sample();
    assert_eq!(serde_json::to_string(&event).unwrap(), GOLDEN);
    assert_eq!(serde_json::from_str::<Event>(GOLDEN).unwrap(), event);
}

#[test]
fn live_events_have_no_seq_and_need_a_live_type() {
    let workspace = WorkspaceId::from_number(9);
    let live = LiveEvent::new(types::WORKSPACE_CHANGED, workspace, json!({"files": 2})).unwrap();
    assert_eq!(
        serde_json::to_string(&live).unwrap(),
        r#"{"type":"workspace.changed","workspace":"ws-9","data":{"files":2}}"#
    );
    assert_eq!(
        LiveEvent::new(types::TASK_CREATED, workspace, json!({})),
        Err(EventError::NotLive)
    );
    assert_eq!(
        LiveEvent::new(types::WORKSPACE_CHANGED, workspace, json!(1)),
        Err(EventError::DataNotObject)
    );
    let presence = LiveEvent::new(
        types::AGENT_PRESENCE_CHANGED,
        None,
        json!({"agent": "frontend", "presence": "idle"}),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_string(&presence).unwrap(),
        r#"{"type":"agent.presence_changed","workspace":null,"data":{"agent":"frontend","presence":"idle"}}"#
    );
}

#[test]
fn filter_matches_type_task_and_proposal() {
    let event = sample();
    assert!(EventFilter::default().matches(&event));
    let by_type = EventFilter {
        types: vec![types::TASK_CREATED, types::TASK_STATUS_CHANGED],
        ..EventFilter::default()
    };
    assert!(by_type.matches(&event));
    let other_type = EventFilter {
        types: vec![types::TASK_CREATED],
        ..EventFilter::default()
    };
    assert!(!other_type.matches(&event));
    let task = |n| EventFilter {
        task: TaskId::from_number(n),
        ..EventFilter::default()
    };
    assert!(task(12).matches(&event));
    assert!(!task(13).matches(&event));
    let proposal = |n| EventFilter {
        proposal: ProposalId::from_number(n),
        ..EventFilter::default()
    };
    assert!(proposal(3).matches(&event));
    assert!(!proposal(4).matches(&event));
    let mut without = event;
    without.task = None;
    assert!(!task(12).matches(&without));
}
