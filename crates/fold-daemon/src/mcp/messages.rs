use std::future::Future;
use std::pin::Pin;

use fold_core::{
    Address, Choice, FoldError, MessageBody, MessageChoices, MessageId, ProposalId, QuestionSet,
    RequestId, SetQuestion, TaskId, Timestamp, codes,
};
use fold_store::messages::MessageLines;
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, Tool};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::error::{from_agent, from_message};
use super::parse;
use super::{FoldServer, FooterPart, FooterSource};
use crate::services::agents::{AgentService, SessionContext};
use crate::services::messages::{InboxItem, MessageService, MessageView, OpenAskView, SendMessage};

pub const SEND_MESSAGE: &str = "send_message";
pub const GET_INBOX: &str = "get_inbox";
pub const GET_THREAD: &str = "get_thread";
pub const ACKNOWLEDGE_MESSAGE: &str = "acknowledge_message";
pub const MARK_ANSWERED: &str = "mark_answered";
pub const LIST_OPEN_ASKS: &str = "list_open_asks";

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SendMessageInput {
    /// The recipients: an agent, tag:<tag>, all, human:<name> or humans.
    pub to: Vec<String>,
    /// The message text. It is stored as plain text and never logged. In message text `#` names a fold item (#T-12) and `@` names a file (@web/src/board.ts); the daemon parses nothing.
    pub body: String,
    /// Link the message to this task, such as T-12. A reply takes its parent's task when left out.
    #[serde(default)]
    pub task: Option<String>,
    /// Link the message to this proposal, such as P-3. A reply takes its parent's proposal when left out.
    #[serde(default)]
    pub proposal: Option<String>,
    /// Reply to this message, such as m-31; the reply joins its thread and acknowledges the message for you when it was sent to you.
    #[serde(default)]
    pub reply_to: Option<String>,
    /// Link the message to this request, such as RQ-4, to ask or answer about it: the message joins the request's clarification thread. A reply takes its parent's request when left out.
    #[serde(default)]
    pub request: Option<String>,
    /// Make the message a multiple-choice question: 2 to 6 options, each a short single-line label or {label, description}. The recipient answers with a reply that names one option, or with free text. Ask a human with to: ["humans"] or ["human:<name>"].
    #[serde(default)]
    pub choices: Option<Vec<ChoiceInput>>,
    /// Mark the message as asking a human for an answer, so it waits for the human on the board, in Needs you and in the Messages count until a human replies in its thread or someone calls mark_answered. Set it only when you need the human's answer; a report or a thank-you waits for no one. Needs a human recipient; a question with choices or questions asks without it.
    #[serde(default)]
    pub asks_human: bool,
    /// Ask the humans 1 to 4 questions as one set in this one message, with the body as a short line of context. The human answers them one by one and sends every answer in one reply, which holds each question with its answer. Ask everything you need now in one set and wait for the answer before you send another. Needs a human recipient; give questions or choices, not both.
    #[serde(default)]
    #[schemars(length(min = 1, max = 4))]
    pub questions: Option<Vec<QuestionInput>>,
}

/// One question of a set.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct QuestionInput {
    /// The question, at most 500 characters, such as "Is the MCP server connected again?".
    pub text: String,
    /// A short header of at most 40 characters that names the topic, such as "MCP server".
    #[serde(default)]
    pub header: Option<String>,
    /// 2 to 6 options, each a short single-line label or {label, description}; leave out for a question that takes free text only. The human may always write an own answer.
    #[serde(default)]
    pub choices: Option<Vec<ChoiceInput>>,
}

impl From<QuestionInput> for SetQuestion {
    fn from(input: QuestionInput) -> Self {
        SetQuestion {
            text: input.text,
            header: input.header,
            choices: input
                .choices
                .unwrap_or_default()
                .into_iter()
                .map(Choice::from)
                .collect(),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListOpenAsksInput {}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct MarkAnsweredInput {
    /// The ask that the user answered outside its thread, such as m-31: a message your agent sent to a human with asks_human, choices or questions, or, for the main agent, any agent's ask from list_open_asks.
    pub message: String,
    /// Main agent only: the user's answers, one per question of the ask in order, each a choice's label or free text from the user's words. fold sends them as one reply from you in the ask's thread and stores them, so the asker gets them; a worker's AskUserQuestion (reply_only in list_open_asks) ends only this way and gets them as its tool's answer. Leave out to only record that the user answered.
    #[serde(default)]
    #[schemars(length(min = 1, max = 4))]
    pub answers: Option<Vec<String>>,
}

/// One option of a question: a plain label, or a label with a one-line description that the human sees under it.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(untagged)]
pub enum ChoiceInput {
    /// A short single-line label, such as "Recharts".
    Label(String),
    /// A label with a description the human sees under it, such as {"label": "Recharts", "description": "Ready-made bar components; adds about 90 kB"}.
    Described {
        /// A short single-line label, such as "Recharts".
        label: String,
        /// One line of at most 300 characters that says what the option means, such as "Ready-made bar components; adds about 90 kB".
        #[serde(default)]
        description: Option<String>,
    },
}

impl From<ChoiceInput> for Choice {
    fn from(input: ChoiceInput) -> Self {
        match input {
            ChoiceInput::Label(label) => Choice::label(label),
            ChoiceInput::Described { label, description } => Choice { label, description },
        }
    }
}

/// The options as agents read them: each `{label, description?}`.
fn choices_json(choices: &[Choice]) -> Value {
    json!(
        choices
            .iter()
            .map(|choice| match &choice.description {
                Some(description) => json!({"label": choice.label, "description": description}),
                None => json!({"label": choice.label}),
            })
            .collect::<Vec<_>>()
    )
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetInboxInput {}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetThreadInput {
    /// The id of the thread's first message, such as m-31. Any message of the thread also names it. Give thread or request, not both.
    #[serde(default)]
    pub thread: Option<String>,
    /// A request, such as RQ-4, to read its clarification thread; it has no messages before anyone wrote about it.
    #[serde(default)]
    pub request: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AcknowledgeMessageInput {
    /// The messages you handled, such as ["m-31", "m-32"]; give at least one. Each must be a message to you.
    #[schemars(length(min = 1))]
    pub ids: Vec<String>,
    /// Also acknowledge every earlier unacknowledged message of yours, for each id.
    #[serde(default)]
    pub up_to: bool,
}

pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            SEND_MESSAGE,
            "Send a message to agents, workers or humans, optionally linked to a task, a proposal or a request, or as a reply in a thread. A message never changes a task, a claim, a dependency or a request.",
            schema_for_type::<SendMessageInput>(),
        ),
        Tool::new(
            GET_INBOX,
            "Read your unacknowledged messages in id order. Reading does not acknowledge: a message comes back on every call until acknowledge_message. A message with `lines` is a human's question about those lines of your proposal; answer it with send_message and reply_to.",
            schema_for_type::<GetInboxInput>(),
        ),
        Tool::new(
            GET_THREAD,
            "Read every message of a thread, or of a request's clarification thread, in id order.",
            schema_for_type::<GetThreadInput>(),
        ),
        Tool::new(
            ACKNOWLEDGE_MESSAGE,
            "Mark messages as handled, so they leave your inbox and the unread count. Give every message you handled in one call. A reply with send_message reply_to already acknowledges the message it answers when it was sent to you. With up_to, also every earlier message of the same worker.",
            schema_for_type::<AcknowledgeMessageInput>(),
        ),
        Tool::new(
            MARK_ANSWERED,
            "Record that the user answered an ask to a human outside its thread, such as in the main agent's session, so it stops waiting for a human. Only the asking agent or the main agent calls it. The main agent may pass the user's answers, one per question: fold sends them to the asker as one reply in the thread. A worker's AskUserQuestion (reply_only) ends only with answers.",
            schema_for_type::<MarkAnsweredInput>(),
        ),
        Tool::new(
            LIST_OPEN_ASKS,
            "Main agent only: list every agent's open ask to the humans, oldest first, each with its id, agent, thread, links, first line and question count. When the user says something is done, find each ask this answers. For a reply_only ask, a worker's AskUserQuestion, call mark_answered with its id and the user's answers, one per question. For any other ask, reply to its agent in its thread with send_message reply_to, then call mark_answered with its id.",
            schema_for_type::<ListOpenAsksInput>(),
        ),
    ]
}

fn message_id(text: &str) -> Result<MessageId, FoldError> {
    Ok(text.parse::<MessageId>()?)
}

fn ids(list: &[MessageId]) -> Vec<String> {
    list.iter().map(ToString::to_string).collect()
}

/// Leaves out the references a message does not have, so a result names only public ids it holds.
fn insert_references(
    object: &mut Map<String, Value>,
    reply_to: Option<MessageId>,
    task: Option<TaskId>,
    proposal: Option<ProposalId>,
    request: Option<RequestId>,
) {
    let references = [
        ("reply_to", reply_to.map(|id| id.to_string())),
        ("task", task.map(|id| id.to_string())),
        ("proposal", proposal.map(|id| id.to_string())),
        ("request", request.map(|id| id.to_string())),
    ];
    for (key, value) in references {
        if let Some(value) = value {
            object.insert(key.into(), json!(value));
        }
    }
}

fn inbox_json(item: &InboxItem) -> Value {
    let mut object = Map::new();
    object.insert("id".into(), json!(item.id.to_string()));
    object.insert("thread".into(), json!(item.thread.to_string()));
    object.insert("from".into(), json!(item.sender.to_string()));
    object.insert("to".into(), json!(item.to.as_str()));
    object.insert("addressed_as".into(), json!(item.addressed_as.as_str()));
    insert_references(
        &mut object,
        item.reply_to,
        item.task,
        item.proposal,
        item.request,
    );
    object.insert("body".into(), json!(item.body));
    if !item.choices.is_empty() {
        object.insert("choices".into(), choices_json(&item.choices));
    }
    if let Some(header) = &item.header {
        object.insert("header".into(), json!(header));
    }
    if item.asks_human {
        object.insert("asks_human".into(), json!(true));
    }
    if let Some(lines) = &item.lines {
        object.insert("lines".into(), lines_json(lines));
    }
    object.insert("sent_at".into(), json!(millis(item.sent_at)));
    Value::Object(object)
}

fn thread_json(message: &MessageView) -> Value {
    let mut object = Map::new();
    object.insert("id".into(), json!(message.id.to_string()));
    object.insert("from".into(), json!(message.sender.to_string()));
    object.insert(
        "to".into(),
        json!(
            message
                .recipients
                .iter()
                .map(|recipient| recipient.name.to_string())
                .collect::<Vec<_>>()
        ),
    );
    insert_references(
        &mut object,
        message.reply_to,
        message.task,
        message.proposal,
        message.request,
    );
    object.insert("body".into(), json!(message.body));
    if !message.choices.is_empty() {
        object.insert("choices".into(), choices_json(&message.choices));
    }
    if let Some(header) = &message.header {
        object.insert("header".into(), json!(header));
    }
    if message.asks_human {
        object.insert("asks_human".into(), json!(true));
    }
    if let Some(lines) = &message.lines {
        object.insert("lines".into(), lines_json(lines));
    }
    if is_set(&message.choices, message.asks_human, &message.questions) {
        object.insert("questions".into(), questions_json(&message.questions));
    }
    if let Some(ask) = message.answers_ask {
        object.insert("answers_ask".into(), json!(ask.to_string()));
        object.insert("answers".into(), json!(message.answers));
    }
    object.insert("sent_at".into(), json!(millis(message.sent_at)));
    Value::Object(object)
}

/// A message holds a question set when its questions come from neither its choices nor its `asks_human` mark; a single question shows as its body and choices alone.
fn is_set(choices: &[Choice], asks_human: bool, questions: &[SetQuestion]) -> bool {
    choices.is_empty() && !asks_human && !questions.is_empty()
}

/// The questions of a set as agents read them: each `{text, header?, choices?}`.
fn questions_json(questions: &[SetQuestion]) -> Value {
    json!(
        questions
            .iter()
            .map(|question| {
                let mut object = Map::new();
                object.insert("text".into(), json!(question.text));
                if let Some(header) = &question.header {
                    object.insert("header".into(), json!(header));
                }
                if !question.choices.is_empty() {
                    object.insert("choices".into(), choices_json(&question.choices));
                }
                Value::Object(object)
            })
            .collect::<Vec<_>>()
    )
}

/// The lines of the message's proposal a human asks its author about, with their text; `excerpt_cut` appears only when the text holds just the first lines.
fn lines_json(lines: &MessageLines) -> Value {
    let mut object = json!({
        "version": lines.version,
        "path": lines.path,
        "side": lines.side.as_str(),
        "start_line": lines.start_line,
        "end_line": lines.end_line,
        "excerpt": lines.excerpt,
    });
    if lines.excerpt_cut {
        object["excerpt_cut"] = json!(true);
    }
    object
}

fn millis(time: Timestamp) -> i64 {
    time.as_millis()
}

pub async fn send_message(server: &FoldServer, arguments: &JsonObject) -> Result<Value, FoldError> {
    let input: SendMessageInput = parse(arguments)?;
    let message = SendMessage {
        to: Address::parse_all(&input.to)?,
        body: MessageBody::parse(input.body)?,
        task: input.task.as_deref().map(str::parse).transpose()?,
        proposal: input.proposal.as_deref().map(str::parse).transpose()?,
        reply_to: input.reply_to.as_deref().map(message_id).transpose()?,
    };
    let request = input
        .request
        .as_deref()
        .map(str::parse::<RequestId>)
        .transpose()?;
    let choices = input
        .choices
        .map(|options| {
            let options: Vec<Choice> = options.into_iter().map(Choice::from).collect();
            MessageChoices::parse(&options, None)
        })
        .transpose()?;
    let questions = input
        .questions
        .map(|questions| {
            let questions: Vec<SetQuestion> =
                questions.into_iter().map(SetQuestion::from).collect();
            QuestionSet::parse(&questions)
        })
        .transpose()?;
    let actor = server.actor().await?;
    let sent = match (choices, questions) {
        (Some(_), Some(_)) => {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                "give questions or choices, not both",
            )
            .with_details([("reason", json!("questions"))])
            .with_hint("put a single question's options in questions[0].choices"));
        }
        (None, Some(questions)) => server
            .messages
            .send_set(&actor, message, request, questions)
            .await
            .map_err(from_message)?,
        (choices, None) => server
            .messages
            .send_marked(&actor, message, request, choices, input.asks_human)
            .await
            .map_err(from_message)?,
    };
    Ok(json!({
        "id": sent.id.to_string(),
        "thread": sent.thread.to_string(),
        "recipients": sent
            .recipients
            .iter()
            .map(|recipient| recipient.name.to_string())
            .collect::<Vec<_>>(),
    }))
}

pub async fn get_inbox(server: &FoldServer, arguments: &JsonObject) -> Result<Value, FoldError> {
    let _: GetInboxInput = parse(arguments)?;
    let actor = server.actor().await?;
    let items = server.messages.inbox(&actor).await.map_err(from_message)?;
    Ok(json!({ "messages": items.iter().map(inbox_json).collect::<Vec<_>>() }))
}

pub async fn get_thread(server: &FoldServer, arguments: &JsonObject) -> Result<Value, FoldError> {
    let input: GetThreadInput = parse(arguments)?;
    let messages = match (input.thread.as_deref(), input.request.as_deref()) {
        (Some(thread), None) => server
            .messages
            .thread(message_id(thread)?)
            .await
            .map_err(from_message)?,
        (None, Some(request)) => server
            .messages
            .request_thread(request.parse()?)
            .await
            .map_err(from_message)?
            .map(|thread| thread.messages)
            .unwrap_or_default(),
        _ => {
            return Err(
                FoldError::new(codes::INVALID_REQUEST, "give thread or request, not both")
                    .with_hint(
                        "call get_thread with a thread such as m-31 or a request such as RQ-4",
                    ),
            );
        }
    };
    Ok(json!({ "messages": messages.iter().map(thread_json).collect::<Vec<_>>() }))
}

pub async fn acknowledge_message(
    server: &FoldServer,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: AcknowledgeMessageInput = parse(arguments)?;
    let targets = input
        .ids
        .iter()
        .map(|id| message_id(id))
        .collect::<Result<Vec<_>, _>>()?;
    if targets.is_empty() {
        return Err(
            FoldError::new(codes::INVALID_REQUEST, "give at least one id")
                .with_hint("call acknowledge_message with ids such as [\"m-31\"]"),
        );
    }
    let actor = server.actor().await?;
    let acknowledged = server
        .messages
        .acknowledge_all(&actor, &targets, input.up_to)
        .await
        .map_err(from_message)?;
    Ok(json!({ "acknowledged": ids(&acknowledged) }))
}

pub async fn mark_answered(
    server: &FoldServer,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: MarkAnsweredInput = parse(arguments)?;
    let id = message_id(&input.message)?;
    let actor = server.actor().await?;
    if let Some(answers) = input.answers {
        let reply = server
            .messages
            .answer(&actor, id, &answers)
            .await
            .map_err(from_message)?;
        return Ok(json!({ "answered": id.to_string(), "reply": reply.id.to_string() }));
    }
    let closed = server
        .messages
        .mark_answered(&actor, id)
        .await
        .map_err(from_message)?;
    Ok(json!({ "answered": closed.to_string() }))
}

pub async fn list_open_asks(
    server: &FoldServer,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let _: ListOpenAsksInput = parse(arguments)?;
    let actor = server.actor().await?;
    let asks = server
        .messages
        .open_asks(&actor)
        .await
        .map_err(from_message)?;
    Ok(json!({ "asks": asks.iter().map(open_ask_json).collect::<Vec<_>>() }))
}

/// One open ask, compact: its links only when it has them, and for a worker's `AskUserQuestion`, which ends only with its answers, `reply_only` with the questions to answer in order, each its text and its choices' labels.
fn open_ask_json(ask: &OpenAskView) -> Value {
    let mut object = Map::new();
    object.insert("id".into(), json!(ask.id.to_string()));
    object.insert("agent".into(), json!(ask.agent.to_string()));
    object.insert("thread".into(), json!(ask.thread.to_string()));
    insert_references(&mut object, None, ask.task, ask.proposal, ask.request);
    object.insert("first_line".into(), json!(ask.first_line));
    object.insert("questions".into(), json!(ask.questions.len()));
    if ask.worker_question {
        object.insert("reply_only".into(), json!(true));
        let questions: Vec<Value> = ask
            .questions
            .iter()
            .map(|question| {
                let mut object = Map::new();
                object.insert("text".into(), json!(question.text));
                if !question.choices.is_empty() {
                    let labels: Vec<&str> = question
                        .choices
                        .iter()
                        .map(|choice| choice.label.as_str())
                        .collect();
                    object.insert("choices".into(), json!(labels));
                }
                Value::Object(object)
            })
            .collect();
        object.insert("answer_these".into(), Value::Array(questions));
    }
    Value::Object(object)
}

/// The `unread` part of the footer: the unacknowledged messages of the calling agent's worker.
pub struct UnreadFooter {
    agents: AgentService,
    messages: MessageService,
}

impl UnreadFooter {
    pub fn new(agents: AgentService, messages: MessageService) -> Self {
        Self { agents, messages }
    }
}

impl FooterSource for UnreadFooter {
    fn part(&self) -> FooterPart {
        FooterPart::Unread
    }

    fn count<'a>(
        &'a self,
        session: &'a SessionContext,
    ) -> Pin<Box<dyn Future<Output = u64> + Send + 'a>> {
        Box::pin(async move {
            if session.agent.is_none() {
                return 0;
            }
            let agent = match self.agents.require_agent(session).await {
                Ok(agent) => agent,
                Err(error) => {
                    tracing::warn!(
                        code = from_agent(error).code(),
                        "could not count unread messages"
                    );
                    return 0;
                }
            };
            match self.messages.unread_count(agent.id).await {
                Ok(count) => count,
                Err(error) => {
                    tracing::warn!(
                        code = from_message(error).code(),
                        "could not count unread messages"
                    );
                    0
                }
            }
        })
    }
}
