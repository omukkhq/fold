use std::future::Future;
use std::pin::Pin;

use fold_core::{Actor, Address, FoldError, MessageBody, MessageId, ProposalId, TaskId, Timestamp};
use fold_store::agents::WorkerRow;
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, Tool};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::error::{from_agent, from_message};
use super::identity::parse;
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
    /// The message text. It is stored as plain text and never logged.
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
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
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
    /// The id of the thread's first message, such as m-31. Any message of the thread also names it.
    pub thread: String,
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
            "Send a message to agents, workers or humans, optionally linked to a task or proposal, or as a reply in a thread. A message never changes a task, a claim or a dependency.",
            schema_for_type::<SendMessageInput>(),
        ),
        Tool::new(
            GET_INBOX,
            "Read your unacknowledged messages in id order. Reading does not acknowledge: a message comes back on every call until acknowledge_message.",
            schema_for_type::<GetInboxInput>(),
        ),
        Tool::new(
            GET_THREAD,
            "Read every message of a thread in id order.",
            schema_for_type::<GetThreadInput>(),
        ),
        Tool::new(
            ACKNOWLEDGE_MESSAGE,
            "Mark a message as handled, so it leaves your inbox and the unread count. With up_to, also every earlier message of the same worker.",
            schema_for_type::<AcknowledgeMessageInput>(),
        ),
    ]
}

/// The acting agent of a call, with its tags as they are now.
async fn actor(server: &FoldServer, worker: &WorkerRow) -> Result<Actor, FoldError> {
    let session = server.session().await;
    server
        .agents
        .actor(&session, Some(&worker.name))
        .await
        .map_err(from_agent)
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
) {
    let references = [
        ("reply_to", reply_to.map(|id| id.to_string())),
        ("task", task.map(|id| id.to_string())),
        ("proposal", proposal.map(|id| id.to_string())),
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
    insert_references(&mut object, item.reply_to, item.task, item.proposal);
    object.insert("body".into(), json!(item.body));
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
    );
    object.insert("body".into(), json!(message.body));
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
    let actor = actor(server, worker).await?;
    let sent = server
        .messages
        .send(&actor, message)
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
    let actor = actor(server, worker).await?;
    let items = server
        .messages
        .inbox(&actor, input.include_workers)
        .await
        .map_err(from_message)?;
    Ok(json!({ "messages": items.iter().map(inbox_json).collect::<Vec<_>>() }))
}

pub async fn get_thread(server: &FoldServer, arguments: &JsonObject) -> Result<Value, FoldError> {
    let input: GetThreadInput = parse(arguments)?;
    let thread = message_id(&input.thread)?;
    let messages = server.messages.thread(thread).await.map_err(from_message)?;
    Ok(json!({ "messages": messages.iter().map(thread_json).collect::<Vec<_>>() }))
}

pub async fn acknowledge_message(
    server: &FoldServer,
    worker: &WorkerRow,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: AcknowledgeMessageInput = parse(arguments)?;
    let id = message_id(&input.id)?;
    let actor = actor(server, worker).await?;
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
