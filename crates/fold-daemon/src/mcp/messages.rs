use std::future::Future;
use std::pin::Pin;

use fold_core::{
    Address, Choice, FoldError, MessageBody, MessageChoices, MessageId, ProposalId, RequestId,
    TaskId, Timestamp, codes,
};
use fold_store::agents::WorkerRow;
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, Tool};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::error::{from_agent, from_message};
use super::parse;
use super::{FoldServer, FooterPart, FooterSource};
use crate::services::agents::{AgentService, SessionContext};
use crate::services::messages::{InboxItem, MessageService, MessageView, SendMessage};

pub const SEND_MESSAGE: &str = "send_message";
pub const GET_INBOX: &str = "get_inbox";
pub const GET_THREAD: &str = "get_thread";
pub const ACKNOWLEDGE_MESSAGE: &str = "acknowledge_message";

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SendMessageInput {
    /// The recipients: an agent (its main worker), an agent/worker path, tag:<tag>, all, human:<name> or humans.
    pub to: Vec<String>,
    /// The message text. It is stored as plain text and never logged. In message text `#` names a fold item (#T-12) and `@` names a file (@web/src/board.ts); the daemon parses nothing.
    pub body: String,
    /// Link the message to this task, such as T-12. A reply takes its parent's task when left out.
    #[serde(default)]
    pub task: Option<String>,
    /// Link the message to this proposal, such as P-3. A reply takes its parent's proposal when left out.
    #[serde(default)]
    pub proposal: Option<String>,
    /// Reply to this message, such as m-31; the reply joins its thread.
    #[serde(default)]
    pub reply_to: Option<String>,
    /// Link the message to this request, such as RQ-4, to ask or answer about it: the message joins the request's clarification thread. A reply takes its parent's request when left out.
    #[serde(default)]
    pub request: Option<String>,
    /// Make the message a multiple-choice question: 2 to 6 options, each a short single-line label or {label, description}. The recipient answers with a reply that names one option, or with free text. Ask a human with to: ["humans"] or ["human:<name>"].
    #[serde(default)]
    pub choices: Option<Vec<ChoiceInput>>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
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
pub struct GetInboxInput {
    /// Also return the unacknowledged messages of every other worker of your agent. Each message names its worker in `to`.
    #[serde(default)]
    pub include_workers: bool,
    /// The worker whose inbox to read, such as frontend/ui. Leave it out to read the main worker's inbox.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetThreadInput {
    /// The id of the thread's first message, such as m-31. Any message of the thread also names it. Give thread or request, not both.
    #[serde(default)]
    pub thread: Option<String>,
    /// A request, such as RQ-4, to read its clarification thread; it has no messages before anyone wrote about it.
    #[serde(default)]
    pub request: Option<String>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AcknowledgeMessageInput {
    /// The message you handled, such as m-31.
    pub id: String,
    /// Also acknowledge every earlier unacknowledged message of the same worker.
    #[serde(default)]
    pub up_to: bool,
    /// The worker whose message to acknowledge, such as frontend/ui. Leave it out for the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
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
            "Read your unacknowledged messages in id order. Reading does not acknowledge: a message comes back on every call until acknowledge_message.",
            schema_for_type::<GetInboxInput>(),
        ),
        Tool::new(
            GET_THREAD,
            "Read every message of a thread, or of a request's clarification thread, in id order.",
            schema_for_type::<GetThreadInput>(),
        ),
        Tool::new(
            ACKNOWLEDGE_MESSAGE,
            "Mark a message as handled, so it leaves your inbox and the unread count. With up_to, also every earlier message of the same worker.",
            schema_for_type::<AcknowledgeMessageInput>(),
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
    object.insert("sent_at".into(), json!(millis(message.sent_at)));
    Value::Object(object)
}

fn millis(time: Timestamp) -> i64 {
    time.as_millis()
}

pub async fn send_message(
    server: &FoldServer,
    worker: &WorkerRow,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
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
    let actor = server.actor(Some(&worker.name)).await?;
    let sent = server
        .messages
        .send_with_choices(&actor, message, request, choices)
        .await
        .map_err(from_message)?;
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

pub async fn get_inbox(
    server: &FoldServer,
    worker: &WorkerRow,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: GetInboxInput = parse(arguments)?;
    let actor = server.actor(Some(&worker.name)).await?;
    let items = server
        .messages
        .inbox(&actor, input.include_workers)
        .await
        .map_err(from_message)?;
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
    worker: &WorkerRow,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: AcknowledgeMessageInput = parse(arguments)?;
    let id = message_id(&input.id)?;
    let actor = server.actor(Some(&worker.name)).await?;
    let acknowledged = server
        .messages
        .acknowledge(&actor, id, input.up_to)
        .await
        .map_err(from_message)?;
    Ok(json!({ "acknowledged": ids(&acknowledged) }))
}

/// The `unread` part of the footer: the unacknowledged messages of every worker of the calling agent.
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
