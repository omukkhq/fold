use fold_core::{
    AgentName, EventActor, FoldError, HumanName, INTEGRATOR_TAG, NameError, Tag, WorkerName, codes,
};
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
fn the_integrator_tag_is_recognised() {
    assert!(Tag::parse(INTEGRATOR_TAG).unwrap().is_integrator());
    assert!(!Tag::parse("review").unwrap().is_integrator());
}

#[test]
fn worker_names_are_slash_separated_segments() {
    let main = WorkerName::parse("frontend").unwrap();
    assert!(main.is_main());
    assert_eq!(main.parent(), None);
    let child = WorkerName::parse("frontend/ui").unwrap();
    assert!(!child.is_main());
    assert_eq!(child.parent(), Some(main.clone()));
    assert_eq!(child.agent().as_str(), "frontend");
    assert_eq!(main.child("ui").unwrap(), child);
    let grandchild = child.child("btn").unwrap();
    assert_eq!(grandchild.as_str(), "frontend/ui/btn");
    assert_eq!(grandchild.parent(), Some(child));
    assert_eq!(
        WorkerName::main(&AgentName::parse("be").unwrap()).as_str(),
        "be"
    );
    assert_eq!(grandchild.to_string(), "frontend/ui/btn");
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
    let main = WorkerName::parse("a").unwrap();
    assert_eq!(main.child("Bad"), Err(NameError::InvalidSegment));
    assert_eq!(main.child(""), Err(NameError::InvalidSegment));
    let deep = WorkerName::parse(&vec!["a".repeat(32); 3].join("/")).unwrap();
    assert_eq!(deep.child(&"b".repeat(32)), Err(NameError::InvalidWorker));
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
