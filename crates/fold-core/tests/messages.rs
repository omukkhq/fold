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

/// Agents `backend` (tag api), `frontend` (tags ui, review) and `review` (tag review); humans `ana` and `tanim`.
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
        "frontend/ui",
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
fn of_worker_reaches_the_workers_agent() {
    assert_eq!(
        Address::of_worker(&worker("frontend")).to_string(),
        "frontend"
    );
    assert_eq!(
        Address::of_worker(&worker("frontend/ui")),
        Address::Agent(AgentName::parse("frontend").unwrap())
    );
}

#[test]
fn agent_and_human_addresses_reach_exactly_that_worker_and_human() {
    let rows = resolve(
        &addresses(&["frontend", "human:tanim"]),
        &directory(),
        &Sender::Fold,
    )
    .unwrap();
    assert_eq!(
        rows,
        vec![
            to_worker("frontend", AddressedAs::Agent),
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
fn an_unknown_agent_is_recipient_not_found() {
    let entry = "ghost";
    let error = resolve(&addresses(&["review", entry]), &directory(), &Sender::Fold).unwrap_err();
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
        worker: worker("frontend"),
        tags: Vec::new(),
    });
    let table = [
        (agent, "worker", "frontend"),
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
        Just("review".to_owned()),
    ]
}

#[cfg(test)]
fn sender() -> impl Strategy<Value = Sender> {
    prop_oneof![
        Just(Sender::Fold),
        Just(Sender::Worker(worker("frontend"))),
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
        addresses(&["frontend"]),
        "a stored worker row below an agent is answered through its agent"
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
        Target::Worker(worker("frontend")),
    ];
    assert_eq!(
        reply_addresses(&tanim, &recipients, &tanim),
        addresses(&["backend", "frontend"])
    );
    assert_eq!(
        reply_addresses(
            &Sender::Fold,
            &recipients,
            &Sender::Worker(worker("backend"))
        ),
        addresses(&["human:tanim", "frontend"])
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

#[test]
fn a_lone_carriage_return_ends_a_line_of_an_excerpt() {
    use fold_core::messages::{line_excerpt, text_lines};
    let text = "one\rtwo\r\nthree\nfour";
    assert_eq!(text_lines(text), ["one", "two", "three", "four"]);
    assert_eq!(text_lines("a\n\r\n"), ["a", ""]);
    assert!(text_lines("").is_empty());
    assert_eq!(
        line_excerpt(text, 2, 3),
        Some(("two\nthree".to_owned(), false))
    );
    assert_eq!(line_excerpt(text, 4, 4), Some(("four".to_owned(), false)));
    assert_eq!(line_excerpt(text, 4, 5), None);
}

fn set_question(text: &str, choices: &[&str]) -> fold_core::SetQuestion {
    fold_core::SetQuestion {
        text: text.to_owned(),
        header: None,
        choices: labels(choices),
    }
}

fn reason(error: &FoldError) -> Option<&serde_json::Value> {
    error.details().and_then(|details| details.get("reason"))
}

#[test]
fn a_question_set_holds_one_to_four_trimmed_questions_with_or_without_choices() {
    use fold_core::QuestionSet;
    let set = QuestionSet::parse(&[
        set_question("  Is the MCP server connected again? ", &["Yes", "Not yet"]),
        fold_core::SetQuestion {
            header: Some("  Keys ".to_owned()),
            ..set_question("Which keys must the workers use?", &[])
        },
    ])
    .unwrap();
    assert_eq!(
        set.questions()[0].text,
        "Is the MCP server connected again?"
    );
    assert_eq!(set.questions()[0].choices.len(), 2);
    assert_eq!(set.questions()[1].header.as_deref(), Some("Keys"));
    assert!(set.questions()[1].choices.is_empty());

    let five: Vec<_> = (0..5)
        .map(|n| set_question(&format!("q{n}"), &[]))
        .collect();
    for questions in [Vec::new(), five] {
        let error = QuestionSet::parse(&questions).unwrap_err();
        assert_eq!(error.code(), codes::INVALID_REQUEST);
        assert_eq!(reason(&error), Some(&json!("questions")));
    }
    let long = "x".repeat(fold_core::messages::MAX_QUESTION_LEN + 1);
    for text in ["  ", long.as_str()] {
        let error = QuestionSet::parse(&[set_question(text, &[])]).unwrap_err();
        assert_eq!(reason(&error), Some(&json!("questions")));
    }
    let header = fold_core::SetQuestion {
        header: Some("a header far longer than forty characters in all".to_owned()),
        ..set_question("Which?", &[])
    };
    assert_eq!(
        reason(&QuestionSet::parse(&[header]).unwrap_err()),
        Some(&json!("questions"))
    );
    let one_choice = QuestionSet::parse(&[set_question("Which?", &["Only"])]).unwrap_err();
    assert_eq!(reason(&one_choice), Some(&json!("choices")));
}

#[test]
fn a_question_with_choices_and_a_marked_ask_are_sets_of_one_with_the_body_as_question() {
    use fold_core::messages::{AskParts, ask_questions};
    let stored = [set_question("Stored?", &[])];
    let choices = labels(&["Red", "Blue"]);
    let parts = |stored, choices, marked| AskParts {
        stored,
        body: " Which colour? ",
        choices,
        header: Some("Colour"),
        marked,
    };
    assert_eq!(ask_questions(parts(&stored, &[], false)), stored.to_vec());
    let single = ask_questions(parts(&[], &choices, false));
    assert_eq!(single.len(), 1);
    assert_eq!(single[0].text, "Which colour?");
    assert_eq!(single[0].header.as_deref(), Some("Colour"));
    assert_eq!(single[0].choices, choices);
    let marked = ask_questions(parts(&[], &[], true));
    assert_eq!(marked.len(), 1);
    assert!(marked[0].choices.is_empty());
    assert!(ask_questions(parts(&[], &[], false)).is_empty());
}

#[test]
fn set_answers_take_one_answer_per_question_and_the_labels_own_spelling() {
    use fold_core::messages::set_answers;
    let questions = [
        set_question("Connected?", &["Yes, it is reconnected", "Not yet"]),
        set_question("Keys?", &[]),
    ];
    let answers = set_answers(
        &questions,
        &[
            " yes, IT is reconnected ".to_owned(),
            " mock the calls ".to_owned(),
        ],
    )
    .unwrap();
    assert_eq!(answers, ["Yes, it is reconnected", "mock the calls"]);

    for wrong in [
        vec!["Yes".to_owned()],
        vec!["Yes".to_owned(), "a".to_owned(), "b".to_owned()],
        vec!["Yes".to_owned(), "  ".to_owned()],
    ] {
        let error = set_answers(&questions, &wrong).unwrap_err();
        assert_eq!(error.code(), codes::INVALID_REQUEST);
        assert_eq!(reason(&error), Some(&json!("answers")));
        assert_eq!(
            error.details().and_then(|details| details.get("questions")),
            Some(&json!(2))
        );
    }
    assert!(set_answers(&[], &[]).is_err());
}

#[test]
fn the_reply_of_a_set_of_one_is_its_answer_and_of_a_larger_set_each_question_with_its_answer() {
    use fold_core::messages::answers_body;
    let one = [set_question("Connected?", &["Yes", "No"])];
    assert_eq!(answers_body(&one, &["Yes".to_owned()]), "Yes");
    let two = [set_question("Connected?", &[]), set_question("Keys?", &[])];
    assert_eq!(
        answers_body(&two, &["Yes".to_owned(), "Mock them".to_owned()]),
        "1. Connected?\nYes\n\n2. Keys?\nMock them"
    );
}

#[test]
fn the_first_line_of_an_ask_skips_blank_lines_and_is_cut_short() {
    use fold_core::messages::{FIRST_LINE_LEN, first_line};
    assert_eq!(
        first_line("\n  \n  Both wait for the server.\nmore"),
        "Both wait for the server."
    );
    let long = "y".repeat(FIRST_LINE_LEN + 10);
    let cut = first_line(&long);
    assert_eq!(cut.chars().count(), FIRST_LINE_LEN);
    assert!(cut.ends_with('…'));
    assert_eq!(first_line(""), "");
}
