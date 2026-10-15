use fold_core::policy::can_set_tag;
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

#[cfg(test)]
fn tag(name: &str) -> Tag {
    Tag::parse(name).unwrap()
}

#[test]
fn only_a_human_sets_the_integrator_tag() {
    let table = [
        ("agent adds ui", agent("frontend", &[]), "ui", None),
        (
            "agent adds integrator",
            agent("review", &["review"]),
            "integrator",
            Some(codes::HUMAN_REQUIRED),
        ),
        ("human adds integrator", human("tanim"), "integrator", None),
        (
            "system adds integrator",
            Actor::System,
            "integrator",
            Some(codes::HUMAN_REQUIRED),
        ),
    ];
    for (case, actor, name, expected) in table {
        let result = can_set_tag(&actor, &tag(name), true);
        assert_eq!(result.as_ref().err().map(|d| d.code()), expected, "{case}");
    }
}

#[test]
fn removing_integrator_also_needs_a_human() {
    let integrator = agent("review", &["integrator"]);
    let denied = can_set_tag(&integrator, &tag("integrator"), false).unwrap_err();
    assert_eq!(denied.code(), codes::HUMAN_REQUIRED);
    assert_eq!(
        denied.to_string(),
        "only a human can remove the integrator tag"
    );
    assert!(can_set_tag(&human("tanim"), &tag("integrator"), false).is_ok());
    assert!(can_set_tag(&agent("frontend/ui", &[]), &tag("ui"), false).is_ok());
}

#[test]
fn a_denial_becomes_a_fold_error_with_its_code_and_hint() {
    let denied = can_set_tag(&Actor::System, &tag("integrator"), true).unwrap_err();
    let hint = denied.hint();
    let error = FoldError::from(denied);
    assert_eq!(error.code(), codes::HUMAN_REQUIRED);
    assert_eq!(error.message(), "only a human can grant the integrator tag");
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
    assert!(can_claim(&agent("frontend/ui/deep", &[])).is_ok());
    for actor in [human("tanim"), Actor::System] {
        let denied = can_claim(&actor).unwrap_err();
        assert_eq!(denied.code(), codes::ACTOR_CANNOT_CLAIM);
        assert!(denied.hint().contains("claim_task"));
    }
}

#[test]
fn only_the_owning_worker_abandons_a_workspace() {
    use fold_core::policy::can_abandon_workspace;
    let owner = WorkerName::parse("frontend/ui").unwrap();
    assert!(can_abandon_workspace(&agent("frontend/ui", &[]), Some(&owner)).is_ok());
    for actor in [
        agent("frontend", &[]),
        agent("backend", &["integrator"]),
        human("tanim"),
        Actor::System,
    ] {
        let denied = can_abandon_workspace(&actor, Some(&owner)).unwrap_err();
        assert_eq!(denied.code(), codes::NOT_WORKSPACE_OWNER, "{actor:?}");
        assert!(denied.hint().contains("get_workspace"));
    }
    assert!(can_abandon_workspace(&agent("frontend/ui", &[]), None).is_err());
}

#[test]
fn only_a_human_sends_feedback() {
    use fold_core::policy::can_send_feedback;
    assert!(can_send_feedback(&human("tanim")).is_ok());
    for actor in [agent("frontend", &["integrator"]), Actor::System] {
        let denied = can_send_feedback(&actor).unwrap_err();
        assert_eq!(denied.code(), codes::HUMAN_REQUIRED);
        assert!(denied.hint().contains("send_message"));
    }
}

#[test]
fn only_a_human_or_the_daemon_pushes() {
    use fold_core::policy::can_push;
    assert!(can_push(&human("tanim")).is_ok());
    assert!(can_push(&Actor::System).is_ok());
    let denied = can_push(&agent("frontend/ui", &["integrator"])).unwrap_err();
    assert_eq!(denied.code(), codes::HUMAN_REQUIRED);
    assert!(denied.hint().contains("fold push"));
}

#[test]
fn only_the_owning_worker_changes_a_workspace() {
    use fold_core::policy::can_change_workspace;
    let owner = WorkerName::parse("frontend/ui").unwrap();
    assert!(can_change_workspace(&agent("frontend/ui", &[]), Some(&owner)).is_ok());
    for actor in [
        agent("frontend", &[]),
        agent("backend", &["integrator"]),
        human("tanim"),
        Actor::System,
    ] {
        let denied = can_change_workspace(&actor, Some(&owner)).unwrap_err();
        assert_eq!(denied.code(), codes::NOT_WORKSPACE_OWNER, "{actor:?}");
        assert!(denied.hint().contains("get_workspace"));
    }
    assert!(can_change_workspace(&agent("frontend/ui", &[]), None).is_err());
}
