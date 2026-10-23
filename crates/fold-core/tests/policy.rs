use fold_core::policy::{TagOrigin, can_take_main_tag};
use fold_core::{
    Actor, AgentActor, EventActor, FoldError, HumanName, NameError, Tag, WorkerName, codes,
};

#[cfg(test)]
fn agent(worker: &str, tags: &[&str]) -> Actor {
    let worker = WorkerName::parse(worker).unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: tags.iter().map(|tag| Tag::parse(tag).unwrap()).collect(),
    })
}

#[cfg(test)]
fn human(name: &str) -> Actor {
    Actor::human(HumanName::parse(name).unwrap())
}

#[test]
fn a_denial_becomes_a_fold_error_with_its_code_and_hint() {
    use fold_core::policy::can_push;
    let denied = can_push(&agent("review", &[])).unwrap_err();
    let hint = denied.hint();
    let error = FoldError::from(denied);
    assert_eq!(error.code(), codes::HUMAN_REQUIRED);
    assert_eq!(
        error.message(),
        "only a human can push main to the Git remote"
    );
    assert_eq!(error.hint(), Some(hint));
}

#[test]
fn event_actor_labels_round_trip() {
    for (actor, label) in [
        (agent("frontend", &["ui"]), "frontend"),
        (agent("frontend/ui", &[]), "frontend/ui"),
        (agent("frontend/ui/forms", &[]), "frontend/ui/forms"),
        (human("tanim"), "human:tanim"),
        (Actor::System, "fold"),
    ] {
        let event_actor = actor.event_actor();
        assert_eq!(event_actor.as_str(), label);
        assert_eq!(EventActor::parse(label).unwrap(), event_actor);
        let json = serde_json::to_string(&event_actor).unwrap();
        assert_eq!(
            serde_json::from_str::<EventActor>(&json).unwrap(),
            event_actor
        );
    }
}

#[test]
fn only_humans_are_human() {
    assert!(human("tanim").is_human());
    assert!(!agent("frontend", &[]).is_human());
    assert!(!Actor::System.is_human());
}

#[test]
fn human_names_follow_the_agent_name_pattern() {
    let name = HumanName::parse("tanim_2").unwrap();
    assert_eq!(name.as_str(), "tanim_2");
    assert_eq!(name.to_string(), "tanim_2");
    for bad in ["", "Tanim", "a b", "human:x", &"a".repeat(33)] {
        assert_eq!(HumanName::parse(bad), Err(NameError::InvalidSegment));
        assert_eq!(
            FoldError::from(HumanName::parse(bad).unwrap_err()).code(),
            codes::INVALID_NAME
        );
    }
}

#[test]
fn only_an_agent_can_claim() {
    use fold_core::policy::can_claim;
    assert!(can_claim(&agent("frontend", &[])).is_ok());
    for actor in [human("tanim"), Actor::System] {
        let denied = can_claim(&actor).unwrap_err();
        assert_eq!(denied.code(), codes::ACTOR_CANNOT_CLAIM);
        assert!(denied.hint().contains("claim_task"));
    }
}

#[test]
fn only_the_owning_worker_abandons_a_workspace() {
    use fold_core::policy::can_abandon_workspace;
    let owner = WorkerName::parse("design").unwrap();
    assert!(can_abandon_workspace(&agent("design", &[]), Some(&owner)).is_ok());
    for actor in [
        agent("frontend", &[]),
        agent("backend", &["review"]),
        human("tanim"),
        Actor::System,
    ] {
        let denied = can_abandon_workspace(&actor, Some(&owner)).unwrap_err();
        assert_eq!(denied.code(), codes::NOT_WORKSPACE_OWNER, "{actor:?}");
        assert!(denied.hint().contains("get_workspace"));
    }
    assert!(can_abandon_workspace(&agent("design", &[]), None).is_err());
}

#[test]
fn only_a_human_sends_feedback() {
    use fold_core::policy::can_send_feedback;
    assert!(can_send_feedback(&human("tanim")).is_ok());
    for actor in [agent("frontend", &["review"]), Actor::System] {
        let denied = can_send_feedback(&actor).unwrap_err();
        assert_eq!(denied.code(), codes::HUMAN_REQUIRED);
        assert!(denied.hint().contains("send_message"));
    }
}

#[test]
fn only_an_agents_worker_has_an_inbox() {
    use fold_core::policy::can_use_inbox;
    let worker = agent("design", &[]);
    assert_eq!(can_use_inbox(&worker).unwrap().worker.as_str(), "design");
    for actor in [human("tanim"), Actor::System] {
        let denied = can_use_inbox(&actor).unwrap_err();
        assert_eq!(denied.code(), codes::INVALID_REQUEST);
        assert!(denied.hint().contains("GET /messages"));
    }
}

#[test]
fn only_a_human_marks_a_thread_read() {
    use fold_core::policy::can_mark_read;
    assert_eq!(can_mark_read(&human("tanim")).unwrap().as_str(), "tanim");
    for actor in [agent("design", &[]), Actor::System] {
        let denied = can_mark_read(&actor).unwrap_err();
        assert_eq!(denied.code(), codes::HUMAN_REQUIRED);
        assert!(denied.hint().contains("acknowledge_message"));
    }
}

#[test]
fn only_a_human_or_the_daemon_pushes() {
    use fold_core::policy::can_push;
    assert!(can_push(&human("tanim")).is_ok());
    assert!(can_push(&Actor::System).is_ok());
    let denied = can_push(&agent("design", &["review"])).unwrap_err();
    assert_eq!(denied.code(), codes::HUMAN_REQUIRED);
    assert!(denied.hint().contains("fld push"));
}

#[test]
fn only_the_owning_worker_changes_a_workspace() {
    use fold_core::policy::can_change_workspace;
    let owner = WorkerName::parse("design").unwrap();
    assert!(can_change_workspace(&agent("design", &[]), Some(&owner)).is_ok());
    for actor in [
        agent("frontend", &[]),
        agent("backend", &["review"]),
        human("tanim"),
        Actor::System,
    ] {
        let denied = can_change_workspace(&actor, Some(&owner)).unwrap_err();
        assert_eq!(denied.code(), codes::NOT_WORKSPACE_OWNER, "{actor:?}");
        assert!(denied.hint().contains("get_workspace"));
    }
    assert!(can_change_workspace(&agent("design", &[]), None).is_err());
}

#[test]
fn only_the_owning_worker_runs_the_checks_of_a_workspace() {
    use fold_core::policy::can_run_checks;
    let owner = WorkerName::parse("design").unwrap();
    assert!(can_run_checks(&agent("design", &[]), Some(&owner)).is_ok());
    for actor in [
        agent("frontend", &[]),
        agent("review", &["main"]),
        human("tanim"),
        Actor::System,
    ] {
        let denied = can_run_checks(&actor, Some(&owner)).unwrap_err();
        assert_eq!(denied.code(), codes::NOT_WORKSPACE_OWNER, "{actor:?}");
    }
    assert!(can_run_checks(&agent("design", &[]), None).is_err());
}

#[test]
fn the_owning_worker_or_the_daemon_takes_an_automatic_checkpoint() {
    use fold_core::policy::can_take_auto_checkpoint;
    let owner = WorkerName::parse("design").unwrap();
    assert!(can_take_auto_checkpoint(&agent("design", &[]), Some(&owner)).is_ok());
    assert!(can_take_auto_checkpoint(&Actor::System, Some(&owner)).is_ok());
    assert!(can_take_auto_checkpoint(&Actor::System, None).is_ok());
    for actor in [
        agent("frontend", &[]),
        agent("backend", &["review"]),
        human("tanim"),
    ] {
        let denied = can_take_auto_checkpoint(&actor, Some(&owner)).unwrap_err();
        assert_eq!(denied.code(), codes::NOT_WORKSPACE_OWNER, "{actor:?}");
    }
}

#[test]
fn only_one_agent_that_fold_did_not_start_takes_the_main_tag() {
    use TagOrigin::{Handshake, RegisterTool};
    assert!(can_take_main_tag(Handshake, false, false, false).is_ok());
    assert!(
        can_take_main_tag(RegisterTool, true, false, false).is_ok(),
        "the holder repeats its own tag"
    );
    for (origin, holds_main, supervised, held_by_other, message) in [
        (
            Handshake,
            false,
            true,
            false,
            "a worker that fold started cannot take the main tag",
        ),
        (
            Handshake,
            false,
            false,
            true,
            "another agent holds the main tag",
        ),
        (
            Handshake,
            false,
            true,
            true,
            "a worker that fold started cannot take the main tag",
        ),
        (
            RegisterTool,
            false,
            false,
            false,
            "only the session that fld claude starts takes the main tag",
        ),
        (
            RegisterTool,
            false,
            true,
            false,
            "a worker that fold started cannot take the main tag",
        ),
    ] {
        let denied = can_take_main_tag(origin, holds_main, supervised, held_by_other).unwrap_err();
        assert_eq!(denied.code(), codes::MAIN_TAG_DENIED);
        assert_eq!(denied.to_string(), message);
        assert!(denied.hint().contains("register_agent"));
    }
}
