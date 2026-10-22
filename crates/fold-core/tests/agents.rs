use fold_core::{AgentName, EventActor, FoldError, HumanName, NameError, Tag, WorkerName, codes};
use proptest::prelude::*;

#[test]
fn agent_names_and_tags_follow_one_pattern() {
    for good in ["a", "0", "frontend", "front-end_2", &"a".repeat(32)] {
        assert_eq!(AgentName::parse(good).unwrap().as_str(), good);
        assert_eq!(Tag::parse(good).unwrap().to_string(), good);
    }
    for bad in [
        "",
        "-a",
        "_a",
        "Frontend",
        "a b",
        "a/b",
        "a.b",
        "é",
        &"a".repeat(33),
    ] {
        assert_eq!(AgentName::parse(bad), Err(NameError::InvalidSegment));
        assert_eq!(Tag::parse(bad), Err(NameError::InvalidSegment));
    }
}

#[test]
fn a_worker_is_named_as_its_agent_and_a_stored_name_reads_as_that_agents() {
    let main = WorkerName::parse("frontend").unwrap();
    assert!(main.is_main());
    assert_eq!(
        WorkerName::main(&AgentName::parse("frontend").unwrap()),
        main
    );
    let stored = WorkerName::parse("frontend/ui").unwrap();
    assert!(!stored.is_main());
    assert_eq!(stored.agent().as_str(), "frontend");
    assert_eq!(stored.to_string(), "frontend/ui");
}

#[test]
fn bad_worker_names_are_refused() {
    for bad in ["", "/", "a/", "/a", "a//b", "A/b", "a/B", "a b/c", "a/-b"] {
        assert_eq!(
            WorkerName::parse(bad),
            Err(NameError::InvalidWorker),
            "{bad}"
        );
    }
    assert!(WorkerName::parse(&format!("{}/{}", "a".repeat(32), "b".repeat(32))).is_ok());
    let long = vec!["a".repeat(32); 5].join("/");
    assert_eq!(WorkerName::parse(&long), Err(NameError::InvalidWorker));
}

#[test]
fn the_agent_name_fold_is_reserved_because_it_is_the_system_actor() {
    assert_eq!(AgentName::parse(EventActor::FOLD), Err(NameError::Reserved));
    for worker in ["fold", "fold/ui", "fold/ui/deep"] {
        assert_eq!(
            WorkerName::parse(worker),
            Err(NameError::Reserved),
            "{worker}"
        );
    }
    let error = FoldError::from(NameError::Reserved);
    assert_eq!(error.code(), codes::INVALID_NAME);
    assert!(error.message().contains("reserved"));
    for allowed in ["folder", "fold-ui", "my-fold"] {
        assert!(AgentName::parse(allowed).is_ok(), "{allowed}");
    }
    assert!(WorkerName::parse("frontend/fold").is_ok());
    // A human is recorded as `human:<name>` and a tag is never an actor, so neither collides with `fold`.
    assert!(HumanName::parse("fold").is_ok());
    assert!(Tag::parse("fold").is_ok());
}

#[test]
fn the_broadcast_addresses_all_and_humans_are_reserved_agent_names() {
    for reserved in ["all", "humans"] {
        assert_eq!(AgentName::parse(reserved), Err(NameError::Reserved));
        assert_eq!(
            WorkerName::parse(&format!("{reserved}/ui")),
            Err(NameError::Reserved),
            "{reserved}"
        );
        // `human:<name>` and `tag:<tag>` carry a prefix, so these words stay free for humans and tags.
        assert!(HumanName::parse(reserved).is_ok());
        assert!(Tag::parse(reserved).is_ok());
    }
    for allowed in ["allison", "humanses", "all-hands"] {
        assert!(AgentName::parse(allowed).is_ok(), "{allowed}");
    }
}

#[test]
fn a_name_error_becomes_invalid_name() {
    let error = FoldError::from(NameError::InvalidWorker);
    assert_eq!(error.code(), codes::INVALID_NAME);
    assert!(!error.message().is_empty());
}

proptest! {
    #[test]
    fn parse_accepts_exactly_the_pattern(text in "[ -~]{0,40}") {
        let expected = !text.is_empty()
            && text.len() <= 32
            && text.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && text.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
            && text != "fold";
        prop_assert_eq!(AgentName::parse(&text).is_ok(), expected);
    }
}
