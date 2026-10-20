use fold_core::messages::resolve;
use fold_core::{
    Actor, Address, AgentName, FoldError, MessageBody, MessageId, Sender, codes, policy,
};
use fold_store::messages;
use fold_store::{StoreError, agents, projects};
use serde_json::json;

use super::{Draft, MessageError, MessageService, MessageView, message_view, write_message};
use crate::services::events::EventBuffer;
use crate::services::page_limit;

/// The page size of a chat read that names no `limit`.
pub const DEFAULT_CHAT_LIMIT: u32 = 50;

/// The largest chat page one read returns; a larger `limit` is lowered to it.
pub const MAX_CHAT_LIMIT: u32 = 200;

/// The newest messages of the direct chat between the humans and one agent, in number order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatPage {
    pub agent: AgentName,
    pub messages: Vec<MessageView>,
    /// The `before` of the page of older messages; `None` when this page holds the first message.
    pub next: Option<MessageId>,
}

impl MessageService {
    /// The direct chat with `agent`: the messages without a task, proposal or request that the humans sent to the agent's workers and that those workers sent to humans. Every human reads the same chat. An unknown agent is `agent_not_found`.
    pub async fn chat(
        &self,
        agent: &AgentName,
        before: Option<MessageId>,
        limit: Option<u32>,
    ) -> Result<ChatPage, MessageError> {
        let limit = page_limit(limit, MAX_CHAT_LIMIT)?.unwrap_or(DEFAULT_CHAT_LIMIT);
        let mut tx = self.store.read().begin().await.map_err(StoreError::from)?;
        if agents::find_agent(&mut *tx, agent).await?.is_none() {
            return Err(agent_not_found(agent).into());
        }
        let mut rows = messages::chat_page(&mut tx, agent, before, limit + 1).await?;
        tx.commit().await.map_err(StoreError::from)?;
        let more = rows.len() > limit as usize;
        if more {
            rows.remove(0);
        }
        let messages: Vec<MessageView> = rows.into_iter().map(message_view).collect();
        let next = more.then(|| messages.first().map(|message| message.id)).flatten();
        Ok(ChatPage {
            agent: agent.clone(),
            messages,
            next,
        })
    }

    /// A named human's message in the direct chat with `agent`, to the agent's main worker and linked to nothing. It joins the thread of the chat's newest message, so the agent reads the whole chat with `get_thread`, and starts a thread when the chat is empty. fold pushes it like any message: a supervised worker gets it as a turn, an interactive session as a notice. It changes no task state.
    pub async fn send_chat(
        &self,
        actor: &Actor,
        agent: &AgentName,
        body: MessageBody,
    ) -> Result<MessageView, MessageError> {
        policy::can_send_feedback(actor).map_err(FoldError::from)?;
        let humans = projects::list_humans(self.registry.read()).await?;
        let sender = Sender::of(actor);
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let view = self
            .store
            .write(async |tx| {
                if agents::find_agent(tx.conn(), agent).await?.is_none() {
                    return Err(agent_not_found(agent).into());
                }
                let join = messages::chat_page(tx.conn(), agent, None, 1)
                    .await?
                    .pop()
                    .map(|newest| newest.thread);
                let directory = messages::directory(tx.conn(), humans).await?;
                let recipients = resolve(&[Address::Agent(agent.clone())], &directory, &sender)
                    .map_err(FoldError::from)?;
                let draft = Draft {
                    actor,
                    sender: &sender,
                    task: None,
                    proposal: None,
                    request: None,
                    join,
                    parent: None,
                    body: &body,
                    choices: None,
                    recipients,
                    now,
                };
                write_message(tx, &mut buffer, draft).await
            })
            .await?;
        self.bus.publish(buffer);
        Ok(view)
    }
}

fn agent_not_found(agent: &AgentName) -> FoldError {
    FoldError::new(
        codes::AGENT_NOT_FOUND,
        format!("agent {agent} does not exist"),
    )
    .with_details([("agent", json!(agent.as_str()))])
    .with_hint("open the Agents screen, or call list_agents, to see the agents")
}
