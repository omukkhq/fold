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
