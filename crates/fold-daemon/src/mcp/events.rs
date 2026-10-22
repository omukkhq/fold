use std::future::Future;
use std::pin::Pin;

use fold_core::{Event, FoldError, WorkerName};
use fold_store::agents::WorkerRow;
use fold_store::events::WaitingMessage;
use rmcp::RoleServer;
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, ProgressNotificationParam, Tool};
use rmcp::schemars::JsonSchema;
use rmcp::service::RequestContext;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::error::{from_agent, from_events};
use super::parse;
use super::{FoldServer, FooterPart, FooterSource};
use crate::services::agents::{AgentService, SessionContext};
use crate::services::events::{
    EventPage, EventQuery, EventService, WaitQuery, WaitStep, cap_for_cache,
};

pub const GET_EVENTS: &str = "get_events";
pub const WAIT_FOR_EVENTS: &str = "wait_for_events";

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetEventsInput {
    /// Return events after this seq and leave your cursor where it is. Leave it out to read after your cursor and move the cursor to the last event returned.
    #[serde(default)]
    pub after: Option<u64>,
    /// Only these exact event types, such as task.created or claim.acquired. Leave it out for every type.
    #[serde(default)]
    pub types: Vec<String>,
    /// Only events about this task, such as T-12.
    #[serde(default)]
    pub task: Option<String>,
    /// Only events about this proposal, such as P-3.
    #[serde(default)]
    pub proposal: Option<String>,
    /// The most events to return: 1 to 500, 100 when left out.
    #[serde(default)]
    pub limit: Option<u32>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct WaitForEventsInput {
    /// Wake only for these exact event types, such as proposal.integrated. Leave it out to wake for every type. A new message to you ends the wait whatever this says.
    #[serde(default)]
    pub types: Vec<String>,
    /// Wake only for events about this task, such as T-12.
    #[serde(default)]
    pub task: Option<String>,
    /// Wake only for events about this proposal, such as P-3.
    #[serde(default)]
    pub proposal: Option<String>,
    /// The longest wait in seconds: 300 when left out, at most 1500. A task worker or reviewer with the 5-minute prompt cache waits at most 270, whatever this says; call again when a wait returns with nothing new.
    #[serde(default)]
    pub timeout_s: Option<u64>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            GET_EVENTS,
            "Read the project's events in seq order, after your cursor or after a seq you name, filtered by type, task or proposal. A read after your cursor moves it to the last event returned, past the events a filter left out.",
            schema_for_type::<GetEventsInput>(),
        ),
        Tool::new(
            WAIT_FOR_EVENTS,
            "Block until a matching event arrives after your cursor, then return it with every match committed with it and move your cursor to the last one, past the events a filter left out. A new message to you also ends the wait, whatever the filter says; the result then names it, so call get_inbox. Use it instead of polling get_events.",
            schema_for_type::<WaitForEventsInput>(),
        ),
    ]
}

/// One event with public ids only; reference fields that are absent are left out.
fn event_json(event: &Event) -> Value {
    let mut object = Map::new();
    object.insert("seq".into(), json!(event.seq));
    object.insert("type".into(), json!(event.event_type.as_str()));
    object.insert("actor".into(), json!(event.actor.as_str()));
    let references = [
        ("task", event.task.map(|id| id.to_string())),
        ("proposal", event.proposal.map(|id| id.to_string())),
        ("workspace", event.workspace.map(|id| id.to_string())),
        ("revision", event.revision.map(|id| id.to_string())),
    ];
    for (key, value) in references {
        if let Some(value) = value {
            object.insert(key.into(), json!(value));
        }
    }
    object.insert("created_at".into(), json!(event.created_at));
    object.insert("data".into(), event.data.clone());
    Value::Object(object)
}

fn page_json(page: &EventPage) -> Value {
    json!({
        "events": page.events.iter().map(event_json).collect::<Vec<_>>(),
        "next": page.next,
    })
}

/// The page of a wait, with the messages in it that wait for the worker and the call that reads them.
fn wait_json(page: &EventPage, waiting: &[WaitingMessage]) -> Value {
    let mut result = page_json(page);
    if waiting.is_empty() {
        return result;
    }
    let ids: Vec<String> = waiting
        .iter()
        .map(|message| message.id.to_string())
        .collect();
    result["messages"] = json!(ids);
    result["hint"] = json!(if waiting.iter().all(|message| message.own) {
        "a message waits for you; call get_inbox"
    } else {
        "a message waits for a worker of your agent; call get_inbox with include_workers"
    });
    result
}

pub async fn get_events(
    server: &FoldServer,
    worker: &WorkerRow,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: GetEventsInput = parse(arguments)?;
    let query = EventQuery {
        after: input.after,
        types: input.types,
        task: input.task,
        proposal: input.proposal,
        limit: input.limit,
    };
    let page = server
        .events
        .get_events(worker, &query)
        .await
        .map_err(from_events)?;
    Ok(page_json(&page))
}

/// Waits on the bus until a match, the timeout or the client's cancellation. Each tick renews the worker and the session as any tool call does, and sends a progress notification when the request carries a progress token.
pub async fn wait_for_events(
    server: &FoldServer,
    worker: &WorkerRow,
    arguments: &JsonObject,
    context: &RequestContext<RoleServer>,
) -> Result<Value, FoldError> {
    let input: WaitForEventsInput = parse(arguments)?;
    let ttl = server
        .agents
        .session_cache_ttl(&server.session().await)
        .await;
    let timeout_s = cap_for_cache(input.timeout_s, ttl);
    let query = WaitQuery {
        types: input.types,
        task: input.task,
        proposal: input.proposal,
        timeout_s,
    };
    let mut waiter = server
        .events
        .wait_as(worker, &query)
        .await
        .map_err(from_events)?;
    let progress = context.meta.get_progress_token();
    loop {
        let step = tokio::select! {
            step = waiter.next() => step.map_err(from_events)?,
            () = context.ct.cancelled() => return Ok(json!({"cancelled": true})),
        };
        match step {
            WaitStep::Events(page) => {
                let waiting = server
                    .events
                    .waiting_messages(worker, &page)
                    .await
                    .map_err(from_events)?;
                return Ok(wait_json(&page, &waiting));
            }
            WaitStep::TimedOut => return Ok(json!({"timed_out": true})),
            WaitStep::Tick(count) => {
                let session = server.session().await;
                if let Err(error) = server
                    .agents
                    .record_call(&session, worker, WAIT_FOR_EVENTS, None)
                    .await
                {
                    tracing::warn!(
                        code = from_agent(error).code(),
                        "could not renew a waiting worker"
                    );
                }
                if let Some(token) = progress.clone() {
                    let notice = ProgressNotificationParam::new(token, count as f64)
                        .with_message("waiting for events");
                    // A client that has gone away also cancels the request, which ends the loop.
                    let _ = context.peer.notify_progress(notice).await;
                }
            }
        }
    }
}

/// The `new events` part of the footer: the events after the calling worker's cursor that concern its agent.
pub struct NewEventsFooter {
    agents: AgentService,
    events: EventService,
}

impl NewEventsFooter {
    pub fn new(agents: AgentService, events: EventService) -> Self {
        Self { agents, events }
    }
}

impl FooterSource for NewEventsFooter {
    fn part(&self) -> FooterPart {
        FooterPart::NewEvents
    }

    fn count<'a>(
        &'a self,
        session: &'a SessionContext,
    ) -> Pin<Box<dyn Future<Output = u64> + Send + 'a>> {
        self.count_as(session, None)
    }

    /// A call whose `as` names no worker of the agent fails with its own error, so its footer counts for the main worker.
    fn count_as<'a>(
        &'a self,
        session: &'a SessionContext,
        as_worker: Option<&'a WorkerName>,
    ) -> Pin<Box<dyn Future<Output = u64> + Send + 'a>> {
        Box::pin(async move {
            let Some(agent) = session.agent.as_ref() else {
                return 0;
            };
            let named = match as_worker {
                Some(name) => self.agents.resolve_worker(session, Some(name)).await.ok(),
                None => None,
            };
            let worker = match named {
                Some(worker) => worker,
                None => match self.agents.resolve_worker(session, None).await {
                    Ok(worker) => worker,
                    Err(error) => {
                        tracing::warn!(
                            code = from_agent(error).code(),
                            "could not count new events"
                        );
                        return 0;
                    }
                },
            };
            match self.events.new_event_count(&worker, agent).await {
                Ok(count) => count,
                Err(error) => {
                    tracing::warn!(
                        code = from_events(error).code(),
                        "could not count new events"
                    );
                    0
                }
            }
        })
    }
}
