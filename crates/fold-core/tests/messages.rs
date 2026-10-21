use std::collections::HashSet;

use fold_core::messages::resolve;
use fold_core::{
    Actor, Address, AddressedAs, AgentActor, AgentName, Directory, DirectoryAgent, FoldError,
    HumanName, MessageBody, Recipient, ResolveError, Sender, Tag, Target, WorkerName, codes,
};
use proptest::prelude::*;
use serde_json::json;

#[cfg(test)]
fn worker(name: &str) -> WorkerName {
    WorkerName::parse(name).unwrap()
}

#[cfg(test)]
fn human(name: &str) -> HumanName {
    HumanName::parse(name).unwrap()
}

#[cfg(test)]
fn addresses(entries: &[&str]) -> Vec<Address> {
    Address::parse_all(entries).unwrap()
}

/// Agents `backend` (tag api), `frontend` (tags ui, review) with child `frontend/ui`, and `review` (tag review); humans `ana` and `tanim`.
#[cfg(test)]
fn directory() -> Directory {
    let agent = |name: &str, tags: &[&str]| DirectoryAgent {
        name: AgentName::parse(name).unwrap(),
        tags: tags.iter().map(|tag| Tag::parse(tag).unwrap()).collect(),
    };
    Directory {
        agents: vec![
            agent("backend", &["api"]),
            agent("frontend", &["ui", "review"]),
            agent("review", &["review"]),
        ],
        workers: vec![
            worker("backend"),
            worker("frontend"),
            worker("frontend/ui"),
            worker("review"),
        ],
        humans: vec![human("ana"), human("tanim")],
    }
}

#[cfg(test)]
fn to_worker(name: &str, addressed_as: AddressedAs) -> Recipient {
    Recipient {
        target: Target::Worker(worker(name)),
        addressed_as,
    }
}

#[cfg(test)]
fn to_human(name: &str, addressed_as: AddressedAs) -> Recipient {
    Recipient {
        target: Target::Human(human(name)),
        addressed_as,
    }
}

#[test]
fn every_address_form_parses_and_prints_back() {
    let table = [
        (
            "frontend",
            Address::Agent(AgentName::parse("frontend").unwrap()),
            AddressedAs::Agent,
        ),
        (
            "frontend/ui",
            Address::Worker(worker("frontend/ui")),
            AddressedAs::Worker,
        ),
        (
            "frontend/ui/forms",
            Address::Worker(worker("frontend/ui/forms")),
            AddressedAs::Worker,
        ),
        (
            "tag:review",
            Address::Tag(Tag::parse("review").unwrap()),
            AddressedAs::Tag,
        ),
        ("all", Address::All, AddressedAs::All),
        (
            "human:tanim",
            Address::Human(human("tanim")),
            AddressedAs::Human,
        ),
        ("humans", Address::Humans, AddressedAs::Humans),
    ];
    for (entry, address, addressed_as) in table {
        let parsed = Address::parse(entry).unwrap();
        assert_eq!(parsed, address, "{entry}");
        assert_eq!(parsed.addressed_as(), addressed_as, "{entry}");
        assert_eq!(parsed.to_string(), entry);
        assert_eq!(
            addressed_as.as_str().parse::<AddressedAs>(),
            Ok(addressed_as)
        );
    }
    assert_eq!("everyone".parse::<AddressedAs>(), Err(()));
}

#[test]
fn a_malformed_entry_is_invalid_name_naming_the_entry() {
    for entry in [
        "",
        "Frontend",
        "tag:",
        "tag:Review",
        "human:",
        "human:A B",
        "frontend/",
        "/ui",
        "a b",
    ] {
        let error = Address::parse(entry).unwrap_err();
        assert_eq!(error.entry(), entry);
        let error = FoldError::from(error);
        assert_eq!(error.code(), codes::INVALID_NAME, "{entry:?}");
        assert_eq!(
            error.details().cloned().map(serde_json::Value::Object),
            Some(json!({"to": [entry]}))
        );
    }
    let error = Address::parse_all(&["frontend", "BAD", "also bad"]).unwrap_err();
    assert_eq!(error.entry(), "BAD");
}

#[test]
fn of_worker_uses_the_agent_form_for_a_main_worker() {
    assert_eq!(
        Address::of_worker(&worker("frontend")).to_string(),
        "frontend"
    );
    assert_eq!(
        Address::of_worker(&worker("frontend/ui")),
        Address::Worker(worker("frontend/ui"))
    );
}

#[test]
fn agent_and_worker_addresses_reach_exactly_that_worker() {
    let rows = resolve(
        &addresses(&["frontend", "frontend/ui", "human:tanim"]),
        &directory(),
        &Sender::Fold,
    )
    .unwrap();
    assert_eq!(
        rows,
        vec![
            to_worker("frontend", AddressedAs::Agent),
            to_worker("frontend/ui", AddressedAs::Worker),
            to_human("tanim", AddressedAs::Human),
        ]
    );
}

#[test]
fn tag_and_all_leave_out_the_sender() {
    let sender = Sender::Worker(worker("frontend"));
    let rows = resolve(&addresses(&["tag:review"]), &directory(), &sender).unwrap();
    assert_eq!(rows, vec![to_worker("review", AddressedAs::Tag)]);
    let rows = resolve(&addresses(&["all"]), &directory(), &sender).unwrap();
    assert_eq!(
        rows,
        vec![
            to_worker("backend", AddressedAs::All),
            to_worker("review", AddressedAs::All),
        ]
    );
}

#[test]
fn a_name_the_sender_gives_explicitly_is_kept() {
    let sender = Sender::Worker(worker("frontend"));
    let rows = resolve(&addresses(&["frontend", "all"]), &directory(), &sender).unwrap();
    assert_eq!(
        rows,
        vec![
            to_worker("frontend", AddressedAs::Agent),
            to_worker("backend", AddressedAs::All),
            to_worker("review", AddressedAs::All),
        ]
    );
}

#[test]
fn a_child_worker_sender_leaves_its_main_worker_in_all() {
    let sender = Sender::Worker(worker("frontend/ui"));
    let rows = resolve(&addresses(&["all"]), &directory(), &sender).unwrap();
    let targets: Vec<String> = rows.iter().map(|row| row.target.to_string()).collect();
    assert_eq!(targets, ["backend", "frontend", "review"]);
}

#[test]
fn duplicates_collapse_and_the_first_entry_sets_addressed_as() {
    let rows = resolve(
        &addresses(&[
            "tag:review",
            "all",
            "review",
            "human:ana",
            "humans",
            "frontend",
        ]),
        &directory(),
        &Sender::Fold,
    )
    .unwrap();
    assert_eq!(
        rows,
        vec![
            to_worker("frontend", AddressedAs::Tag),
            to_worker("review", AddressedAs::Tag),
            to_worker("backend", AddressedAs::All),
            to_human("ana", AddressedAs::Human),
            to_human("tanim", AddressedAs::Humans),
        ]
    );
}

#[test]
fn humans_reaches_every_registry_human_except_the_sender() {
    let rows = resolve(&addresses(&["humans"]), &directory(), &Sender::Fold).unwrap();
    assert_eq!(
        rows,
        vec![
            to_human("ana", AddressedAs::Humans),
            to_human("tanim", AddressedAs::Humans),
        ]
    );
    let sender = Sender::Human(human("tanim"));
    let rows = resolve(&addresses(&["humans"]), &directory(), &sender).unwrap();
    assert_eq!(rows, vec![to_human("ana", AddressedAs::Humans)]);
    let rows = resolve(&addresses(&["human:tanim"]), &directory(), &sender).unwrap();
    assert_eq!(rows, vec![to_human("tanim", AddressedAs::Human)]);
}

#[test]
fn an_unknown_agent_or_worker_is_recipient_not_found() {
    for entry in ["ghost", "frontend/ghost"] {
        let error =
            resolve(&addresses(&["review", entry]), &directory(), &Sender::Fold).unwrap_err();
        assert_eq!(
            error,
            ResolveError::UnknownRecipient {
                to: entry.to_owned()
            }
        );
        let error = FoldError::from(error);
        assert_eq!(error.code(), codes::RECIPIENT_NOT_FOUND);
        assert_eq!(
            error.details().cloned().map(serde_json::Value::Object),
            Some(json!({"to": [entry]}))
        );
        assert!(error.hint().unwrap().contains("list_agents"));
    }
}

#[test]
fn an_unknown_human_is_unknown_human() {
    let error = resolve(&addresses(&["human:bob"]), &directory(), &Sender::Fold).unwrap_err();
    assert_eq!(
        error,
        ResolveError::UnknownHuman {
            to: "human:bob".to_owned()
        }
    );
    let error = FoldError::from(error);
    assert_eq!(error.code(), codes::UNKNOWN_HUMAN);
    assert_eq!(
        error.details().cloned().map(serde_json::Value::Object),
        Some(json!({"to": ["human:bob"]}))
    );
}

#[test]
fn an_empty_expansion_is_recipient_not_found_listing_every_entry() {
    let lonely = Directory {
        agents: vec![DirectoryAgent {
            name: AgentName::parse("frontend").unwrap(),
            tags: Vec::new(),
        }],
        workers: vec![worker("frontend")],
        humans: vec![human("tanim")],
    };
    let sender = Sender::Worker(worker("frontend"));
    let error = resolve(&addresses(&["tag:review", "all"]), &lonely, &sender).unwrap_err();
    assert_eq!(
        error,
        ResolveError::NoRecipient {
            to: vec!["tag:review".to_owned(), "all".to_owned()]
        }
    );
    let error = FoldError::from(error);
    assert_eq!(error.code(), codes::RECIPIENT_NOT_FOUND);
    assert_eq!(
        error.details().cloned().map(serde_json::Value::Object),
        Some(json!({"to": ["tag:review", "all"]}))
    );
    let error = resolve(
        &addresses(&["humans"]),
        &lonely,
        &Sender::Human(human("tanim")),
    )
    .unwrap_err();
    assert!(matches!(error, ResolveError::NoRecipient { .. }));
    let error = resolve(&[], &lonely, &Sender::Fold).unwrap_err();
    assert_eq!(error, ResolveError::NoRecipient { to: Vec::new() });
}

#[test]
fn the_sender_follows_the_actor() {
    let agent = Actor::Agent(AgentActor {
        agent: AgentName::parse("frontend").unwrap(),
        worker: worker("frontend/ui"),
        tags: Vec::new(),
    });
    let table = [
        (agent, "worker", "frontend/ui"),
        (Actor::human(human("tanim")), "human", "human:tanim"),
        (Actor::System, "fold", "fold"),
    ];
    for (actor, kind, label) in table {
        let sender = Sender::of(&actor);
        assert_eq!(sender.kind(), kind);
        assert_eq!(sender.to_string(), label);
        assert_eq!(sender.to_string(), actor.event_actor().as_str());
    }
    assert_eq!(Target::Human(human("ana")).to_string(), "human:ana");
}

#[test]
fn a_body_debug_form_holds_no_text() {
    let body = MessageBody::parse("secret plan").unwrap();
    let shown = format!("{body:?}");
    assert!(!shown.contains("secret"), "{shown}");
    assert!(shown.contains("11"), "{shown}");
}

#[test]
fn a_blank_body_is_refused() {
    assert_eq!(MessageBody::parse("hello").unwrap().as_str(), "hello");
    for blank in ["", "  \n\t"] {
        let error = MessageBody::parse(blank).unwrap_err();
        assert_eq!(error.code(), codes::INVALID_REQUEST);
        assert_eq!(
            error.details().cloned().map(serde_json::Value::Object),
            Some(json!({"reason": "empty_message"}))
        );
    }
}

#[cfg(test)]
fn entry() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("all".to_owned()),
        Just("humans".to_owned()),
        Just("human:ana".to_owned()),
        Just("human:tanim".to_owned()),
        Just("tag:review".to_owned()),
        Just("tag:ui".to_owned()),
        Just("tag:api".to_owned()),
        Just("tag:none".to_owned()),
        Just("backend".to_owned()),
        Just("frontend".to_owned()),
        Just("frontend/ui".to_owned()),
        Just("review".to_owned()),
    ]
}

#[cfg(test)]
fn sender() -> impl Strategy<Value = Sender> {
    prop_oneof![
        Just(Sender::Fold),
        Just(Sender::Worker(worker("frontend"))),
        Just(Sender::Worker(worker("frontend/ui"))),
        Just(Sender::Worker(worker("review"))),
        Just(Sender::Human(human("tanim"))),
    ]
}

proptest! {
    #[test]
    fn resolution_never_returns_a_duplicate_target(
        entries in proptest::collection::vec(entry(), 0..12),
        sender in sender(),
    ) {
        let list = Address::parse_all(&entries).unwrap();
        match resolve(&list, &directory(), &sender) {
            Ok(rows) => {
                prop_assert!(!rows.is_empty());
                let unique: HashSet<&Target> = rows.iter().map(|row| &row.target).collect();
                prop_assert_eq!(unique.len(), rows.len());
                // The first entry in input order that reaches a target decides its addressed_as.
                for row in &rows {
                    let first = list
                        .iter()
                        .find(|address| resolve(std::slice::from_ref(*address), &directory(), &sender)
                            .is_ok_and(|alone| alone.iter().any(|r| r.target == row.target)))
                        .map(Address::addressed_as);
                    prop_assert_eq!(first, Some(row.addressed_as));
                }
            }
            Err(error) => prop_assert_eq!(
                error,
                ResolveError::NoRecipient { to: entries.clone() }
            ),
        }
    }
}

#[test]
fn a_reply_goes_to_the_parent_sender() {
    use fold_core::messages::reply_addresses;
    let recipients = [Target::Human(human("tanim")), Target::Human(human("ana"))];
    let tanim = Sender::Human(human("tanim"));
    assert_eq!(
        reply_addresses(&Sender::Worker(worker("frontend")), &recipients, &tanim),
        addresses(&["frontend"])
    );
    assert_eq!(
        reply_addresses(&Sender::Worker(worker("frontend/ui")), &recipients, &tanim),
        addresses(&["frontend/ui"])
    );
    assert_eq!(
        reply_addresses(&Sender::Human(human("ana")), &recipients, &tanim),
        addresses(&["human:ana"])
    );
}

#[test]
fn a_reply_to_your_own_message_or_to_fold_goes_to_the_other_recipients() {
    use fold_core::messages::reply_addresses;
    let tanim = Sender::Human(human("tanim"));
    let recipients = [
        Target::Worker(worker("backend")),
        Target::Human(human("tanim")),
        Target::Worker(worker("frontend/ui")),
    ];
    assert_eq!(
        reply_addresses(&tanim, &recipients, &tanim),
        addresses(&["backend", "frontend/ui"])
    );
    assert_eq!(
        reply_addresses(
            &Sender::Fold,
            &recipients,
            &Sender::Worker(worker("backend"))
        ),
        addresses(&["human:tanim", "frontend/ui"])
    );
    assert!(reply_addresses(&tanim, &[Target::Human(human("tanim"))], &tanim).is_empty());
}

#[cfg(test)]
fn labels(labels: &[&str]) -> Vec<fold_core::Choice> {
    labels
        .iter()
        .copied()
        .map(fold_core::Choice::label)
        .collect()
}

#[test]
fn choices_are_two_to_six_distinct_single_lines() {
    use fold_core::messages::{MessageChoices, chosen_answer};
    let choices = MessageChoices::parse(&labels(&[" Red ", "Blue"]), None).unwrap();
    assert_eq!(choices.labels(), ["Red", "Blue"]);
    assert_eq!(choices.header(), None);
    let reason = |options: &[&str]| {
        MessageChoices::parse(&labels(options), None)
            .unwrap_err()
            .details()
            .and_then(|details| details.get("reason").cloned())
    };
    for options in [
        &["one"][..],
        &["a", "b", "c", "d", "e", "f", "g"][..],
        &["a", " "][..],
        &["a", "b\nc"][..],
        &["Red", "red"][..],
    ] {
        assert_eq!(
            reason(options),
            Some(serde_json::json!("choices")),
            "{options:?}"
        );
    }
    let long = "x".repeat(101);
    assert!(MessageChoices::parse(&labels(&["a", long.as_str()]), None).is_err());
    let labels = choices.labels();
    assert_eq!(chosen_answer(&labels, " blue \n"), "Blue");
    assert_eq!(chosen_answer(&labels, "green, please"), "green, please");
}

#[test]
fn choices_keep_one_line_descriptions_and_a_short_header() {
    use fold_core::Choice;
    use fold_core::messages::{MessageChoices, short_line};
    let option = |label: &str, description: Option<&str>| Choice {
        label: label.to_owned(),
        description: description.map(str::to_owned),
    };
    let choices = MessageChoices::parse(
        &[
            option("Recharts", Some(" Ready-made bar components. ")),
            option("Plain SVG", Some("  ")),
        ],
        Some(" Chart library "),
    )
    .unwrap();
    assert_eq!(
        choices.options(),
        [
            option("Recharts", Some("Ready-made bar components.")),
            option("Plain SVG", None),
        ]
    );
    assert_eq!(choices.header(), Some("Chart library"));
    assert_eq!(
        MessageChoices::parse(&[option("a", None), option("b", None)], Some(""))
            .unwrap()
            .header(),
        None
    );
    let long = "x".repeat(301);
    for (options, header) in [
        (vec![option("a", Some("one\ntwo")), option("b", None)], None),
        (
            vec![option("a", Some(long.as_str())), option("b", None)],
            None,
        ),
        (vec![option("a", None), option("b", None)], Some("a\nb")),
        (
            vec![option("a", None), option("b", None)],
            Some("a header longer than forty characters, really"),
        ),
    ] {
        let error = MessageChoices::parse(&options, header).unwrap_err();
        assert_eq!(
            error
                .details()
                .and_then(|details| details.get("reason").cloned()),
            Some(serde_json::json!("choices"))
        );
    }
    assert_eq!(
        short_line(" Fold probe ", 12),
        Some("Fold probe".to_owned())
    );
    assert_eq!(short_line("Fold probe header", 12), None);
    assert_eq!(short_line("a\nb", 12), None);
    assert_eq!(short_line(" ", 12), None);
}

#[test]
fn a_line_excerpt_holds_the_run_without_line_endings() {
    use fold_core::messages::line_excerpt;
    let text = "one\r\ntwo\nthree\n";
    assert_eq!(line_excerpt(text, 1, 1), Some(("one".to_owned(), false)));
    assert_eq!(
        line_excerpt(text, 2, 3),
        Some(("two\nthree".to_owned(), false))
    );
    assert_eq!(line_excerpt("a\n\nb", 2, 2), Some((String::new(), false)));
    assert_eq!(line_excerpt(text, 3, 4), None);
    assert_eq!(line_excerpt(text, 0, 1), None);
    assert_eq!(line_excerpt(text, 2, 1), None);
}

#[test]
fn a_long_line_excerpt_keeps_its_first_lines() {
    use fold_core::messages::{MAX_EXCERPT_CHARS, MAX_EXCERPT_LINES, line_excerpt};
    let many: String = (1..=150).map(|n| format!("line {n}\n")).collect();
    let (excerpt, cut) = line_excerpt(&many, 1, 150).unwrap();
    assert!(cut);
    assert_eq!(excerpt.lines().count(), MAX_EXCERPT_LINES);
    assert!(excerpt.ends_with("line 100"));

    let wide = format!("{}\n{}\n", "x".repeat(MAX_EXCERPT_CHARS - 2), "yy");
    let (excerpt, cut) = line_excerpt(&wide, 1, 2).unwrap();
    assert!(cut);
    assert_eq!(excerpt.chars().count(), MAX_EXCERPT_CHARS - 2);

    let huge = "z".repeat(MAX_EXCERPT_CHARS + 5);
    let (excerpt, cut) = line_excerpt(&huge, 1, 1).unwrap();
    assert!(cut);
    assert_eq!(excerpt.chars().count(), MAX_EXCERPT_CHARS);
}
