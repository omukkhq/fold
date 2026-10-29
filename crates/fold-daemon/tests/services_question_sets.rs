use std::sync::Arc;

use fold_core::{
    Actor, Address, AgentActor, AgentName, Choice, HumanName, MessageBody, MessageChoices,
    MessageId, QuestionSet, Sender, SetQuestion, Tag, Target, WorkerName, codes,
};
use fold_daemon::services::agents::{AgentService, NeedsYouKind, needs_you};
use fold_daemon::services::events::EventBus;
use fold_daemon::services::messages::{
    DismissScope, MessageError, MessageService, MessageView, SendMessage,
};
use fold_store::projects;
use fold_store::testing::TestProject;

use crate::common::awaiting_human;

#[cfg(test)]
fn agent(name: &str) -> Actor {
    let worker = WorkerName::parse(name).unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: Vec::new(),
    })
}

#[cfg(test)]
fn human(name: &str) -> Actor {
    Actor::human(HumanName::parse(name).unwrap())
}

#[cfg(test)]
fn message(to: &[&str], text: &str, reply_to: Option<MessageId>) -> SendMessage {
    SendMessage {
        to: Address::parse_all(to).unwrap(),
        body: MessageBody::parse(text).unwrap(),
        task: None,
        proposal: None,
        reply_to,
    }
}

#[cfg(test)]
fn question(text: &str, choices: &[&str]) -> SetQuestion {
    SetQuestion {
        text: text.to_owned(),
        header: None,
        choices: choices.iter().copied().map(Choice::label).collect(),
    }
}

#[cfg(test)]
fn answers(texts: &[&str]) -> Vec<String> {
    texts.iter().map(|text| (*text).to_owned()).collect()
}

#[cfg(test)]
fn code(error: MessageError) -> Option<&'static str> {
    error.code()
}

/// Agents `frontend`, `backend` and `main` with the `main` tag; humans `ana` and `tanim`.
#[cfg(test)]
struct Fixture {
    project: TestProject,
    messages: MessageService,
}

#[cfg(test)]
impl Fixture {
    async fn new() -> Self {
        let project = TestProject::new().await;
        let bus = EventBus::new();
        let clock = Arc::new(project.clock().clone());
        let agents = AgentService::new(project.store().clone(), bus.clone(), clock.clone());
        for (name, tags) in [
            ("frontend", Vec::new()),
            ("backend", Vec::new()),
            ("main", vec![Tag::parse("main").unwrap()]),
        ] {
            let mut ctx = agents.start_session(1).await.unwrap();
            agents
                .register_agent(&mut ctx, &AgentName::parse(name).unwrap(), &tags)
                .await
                .unwrap();
        }
        for name in ["ana", "tanim"] {
            let now = project.now();
            let name = HumanName::parse(name).unwrap();
            project
                .registry()
                .write(async |tx| projects::insert_human(tx, &name, now).await)
                .await
                .unwrap();
        }
        let messages = MessageService::new(
            project.store().clone(),
            project.registry().clone(),
            bus,
            clock,
        );
        Self { project, messages }
    }

    /// `by` asks the humans `questions` as one set, in a new thread or as a reply to `reply_to`.
    async fn ask(
        &self,
        by: &str,
        questions: &[SetQuestion],
        reply_to: Option<MessageId>,
    ) -> MessageView {
        self.messages
            .send_set(
                &agent(by),
                message(
                    &["humans"],
                    "Both workers wait for the MCP server.",
                    reply_to,
                ),
                None,
                QuestionSet::parse(questions).unwrap(),
            )
            .await
            .unwrap()
    }

    async fn waiting(&self) -> Vec<MessageId> {
        awaiting_human(self.project.store())
            .await
            .into_iter()
            .map(|row| row.id)
            .collect()
    }
}

#[tokio::test]
async fn a_set_is_one_ask_that_holds_its_questions_and_counts_once_in_needs_you() {
    let fixture = Fixture::new().await;
    let set = fixture
        .ask(
            "frontend",
            &[
                question("Is the MCP server connected again?", &["Yes", "Not yet"]),
                question("Which keys must the workers use?", &[]),
            ],
            None,
        )
        .await;
    assert_eq!(set.questions.len(), 2);
    assert!(set.choices.is_empty());
    assert_eq!(fixture.waiting().await, [set.id]);

    let thread = fixture.messages.thread(set.id).await.unwrap();
    assert_eq!(thread[0].questions, set.questions);
    assert_eq!(thread[0].questions[0].choices.len(), 2);

    let items = needs_you(fixture.project.store()).await.unwrap();
    let asks: Vec<_> = items
        .iter()
        .filter(|item| item.kind == NeedsYouKind::Question)
        .collect();
    assert_eq!(asks.len(), 1);
    assert_eq!(asks[0].questions, 2);
    assert_eq!(asks[0].message.map(|(id, _)| id), Some(set.id));
}

#[tokio::test]
async fn a_set_needs_a_human_recipient() {
    let fixture = Fixture::new().await;
    let error = fixture
        .messages
        .send_set(
            &agent("frontend"),
            message(&["backend"], "context", None),
            None,
            QuestionSet::parse(&[question("Which?", &[])]).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(code(error), Some(codes::INVALID_REQUEST));
}

#[tokio::test]
async fn one_reply_answers_every_question_of_a_set_and_goes_to_the_asker() {
    let fixture = Fixture::new().await;
    let set = fixture
        .ask(
            "frontend",
            &[
                question("Connected?", &["Yes, it is reconnected", "Not yet"]),
                question("Which keys?", &[]),
            ],
            None,
        )
        .await;
    for wrong in [answers(&["Yes"]), answers(&["Yes", " "])] {
        let error = fixture
            .messages
            .answer(&human("ana"), set.id, &wrong)
            .await
            .unwrap_err();
        assert_eq!(code(error), Some(codes::INVALID_REQUEST));
    }
    let by_agent = fixture
        .messages
        .answer(&agent("main"), set.id, &answers(&["Yes", "x"]))
        .await
        .unwrap_err();
    assert_eq!(code(by_agent), Some(codes::HUMAN_REQUIRED));

    let reply = fixture
        .messages
        .answer(
            &human("ana"),
            set.id,
            &answers(&["yes, it is reconnected", "Mock the calls"]),
        )
        .await
        .unwrap();
    assert_eq!(reply.reply_to, Some(set.id));
    assert_eq!(reply.thread, set.thread);
    assert_eq!(
        reply.body,
        "1. Connected?\nYes, it is reconnected\n\n2. Which keys?\nMock the calls"
    );
    assert_eq!(reply.answers, ["Yes, it is reconnected", "Mock the calls"]);
    assert_eq!(reply.answers_ask, Some(set.id));
    let to: Vec<String> = reply
        .recipients
        .iter()
        .map(|recipient| recipient.name.to_string())
        .collect();
    assert_eq!(to, ["frontend"]);
    assert!(fixture.waiting().await.is_empty());

    let stored = fixture.messages.thread(set.id).await.unwrap();
    assert_eq!(stored[1].answers, reply.answers);
    assert_eq!(stored[1].answers_ask, Some(set.id));

    let again = fixture
        .messages
        .answer(&human("tanim"), set.id, &answers(&["Not yet", "x"]))
        .await
        .unwrap_err();
    assert_eq!(code(again), Some(codes::ASK_NOT_OPEN));
}

#[tokio::test]
async fn the_answer_to_one_set_leaves_the_other_set_of_its_thread_waiting() {
    let fixture = Fixture::new().await;
    let first = fixture
        .ask("frontend", &[question("Connected?", &[])], None)
        .await;
    let second = fixture
        .ask(
            "frontend",
            &[question("Run the suite?", &["Yes", "No"])],
            Some(first.id),
        )
        .await;
    assert_eq!(second.thread, first.thread);
    let reply = fixture
        .messages
        .answer(&human("ana"), second.id, &answers(&["no"]))
        .await
        .unwrap();
    assert_eq!(reply.body, "No");
    assert_eq!(fixture.waiting().await, [first.id]);
    fixture
        .messages
        .answer(&human("ana"), first.id, &answers(&["Yes"]))
        .await
        .unwrap();
    assert!(fixture.waiting().await.is_empty());
}

#[tokio::test]
async fn a_question_with_choices_and_a_marked_ask_are_sets_of_one() {
    let fixture = Fixture::new().await;
    let choices =
        MessageChoices::parse(&[Choice::label("Red"), Choice::label("Blue")], None).unwrap();
    let single = fixture
        .messages
        .send_marked(
            &agent("frontend"),
            message(&["humans"], "Which colour?", None),
            None,
            Some(choices),
            false,
        )
        .await
        .unwrap();
    let marked = fixture
        .messages
        .send_marked(
            &agent("backend"),
            message(&["human:tanim"], "Is the token renewed?", None),
            None,
            None,
            true,
        )
        .await
        .unwrap();
    assert_eq!(single.questions.len(), 1);
    assert_eq!(single.questions[0].text, "Which colour?");
    assert_eq!(single.questions[0].choices.len(), 2);
    assert_eq!(marked.questions.len(), 1);
    assert!(marked.questions[0].choices.is_empty());

    let reply = fixture
        .messages
        .answer(&human("ana"), single.id, &answers(&[" blue "]))
        .await
        .unwrap();
    assert_eq!(reply.body, "Blue");
    let reply = fixture
        .messages
        .answer(&human("tanim"), marked.id, &answers(&["Yes, today"]))
        .await
        .unwrap();
    assert_eq!(reply.body, "Yes, today");
    assert!(fixture.waiting().await.is_empty());

    let report = fixture
        .messages
        .send(&agent("frontend"), message(&["humans"], "done", None))
        .await
        .unwrap();
    assert!(report.questions.is_empty());
    let none = fixture
        .messages
        .answer(&human("ana"), report.id, &answers(&["ok"]))
        .await
        .unwrap_err();
    assert_eq!(code(none), Some(codes::ASK_NOT_OPEN));
}

#[tokio::test]
async fn the_main_agent_lists_every_open_set_and_closes_each_it_answered() {
    let fixture = Fixture::new().await;
    let frontend = fixture
        .ask(
            "frontend",
            &[question("Connected?", &[]), question("Keys?", &[])],
            None,
        )
        .await;
    let backend = fixture
        .ask("backend", &[question("Connected?", &["Yes", "No"])], None)
        .await;
    let denied = fixture
        .messages
        .open_asks(&agent("frontend"))
        .await
        .unwrap_err();
    assert_eq!(code(denied), Some(codes::MAIN_AGENT_REQUIRED));

    let asks = fixture.messages.open_asks(&agent("main")).await.unwrap();
    let listed: Vec<(MessageId, String, usize)> = asks
        .iter()
        .map(|ask| (ask.id, ask.agent.to_string(), ask.questions))
        .collect();
    assert_eq!(
        listed,
        [
            (frontend.id, "frontend".to_owned(), 2),
            (backend.id, "backend".to_owned(), 1),
        ]
    );
    assert_eq!(asks[0].first_line, "Both workers wait for the MCP server.");
    assert_eq!(asks[0].thread, frontend.thread);
    assert!(!asks[0].worker_question);
    assert_eq!(
        fixture
            .messages
            .open_asks(&human("ana"))
            .await
            .unwrap()
            .len(),
        2
    );

    for ask in [&frontend, &backend] {
        let Sender::Worker(asker) = &ask.sender else {
            panic!("an agent sent the set");
        };
        let reply = fixture
            .messages
            .send(
                &agent("main"),
                message(
                    &[asker.agent().as_str()],
                    "The MCP server is connected again.",
                    Some(ask.id),
                ),
            )
            .await
            .unwrap();
        assert_eq!(reply.thread, ask.thread);
        assert_eq!(reply.recipients[0].name, Target::Worker(asker.clone()));
        fixture
            .messages
            .mark_answered(&agent("main"), ask.id)
            .await
            .unwrap();
    }
    assert!(
        fixture
            .messages
            .open_asks(&agent("main"))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_human_marks_one_ask_answered_and_another_ask_of_its_thread_keeps_waiting() {
    let fixture = Fixture::new().await;
    let first = fixture
        .ask("frontend", &[question("Connected?", &[])], None)
        .await;
    let second = fixture
        .ask("frontend", &[question("Keys?", &[])], Some(first.id))
        .await;
    let closed = fixture
        .messages
        .dismiss_asks(&human("ana"), DismissScope::Ask(second.id))
        .await
        .unwrap();
    assert_eq!(closed, [second.id]);
    assert_eq!(fixture.waiting().await, [first.id]);
    let again = fixture
        .messages
        .dismiss_asks(&human("ana"), DismissScope::Ask(second.id))
        .await
        .unwrap();
    assert!(again.is_empty());
    let missing = fixture
        .messages
        .dismiss_asks(
            &human("ana"),
            DismissScope::Ask(MessageId::from_number(99).unwrap()),
        )
        .await
        .unwrap_err();
    assert_eq!(code(missing), Some(codes::MESSAGE_NOT_FOUND));
}
