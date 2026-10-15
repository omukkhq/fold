use fold_core::events::types;
use fold_core::{
    AgentName, Event, EventFilter, EventType, FoldError, MessageId, ProposalId, TaskId, codes,
    concerns,
};
use fold_store::{Store, StoreError, events};
use serde_json::json;
use thiserror::Error;

use super::EventBus;

/// The page size when a call names no `limit`.
pub const DEFAULT_LIMIT: u32 = 100;

/// The largest page one call returns; a larger `limit` is lowered to it.
pub const MAX_LIMIT: u32 = 500;

#[derive(Debug, Error)]
pub enum EventsError {
    #[error(transparent)]
    Fold(#[from] FoldError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// A request for stored events as a transport receives it. The service parses it, so every transport refuses the same input with the same `invalid_event_filter`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventQuery {
    /// Read after this `seq` and leave the cursor alone; `None` reads after the agent's cursor.
    pub after: Option<u64>,
    /// Exact declared event types; empty matches every type.
    pub types: Vec<String>,
    pub task: Option<String>,
    pub proposal: Option<String>,
    pub limit: Option<u32>,
}

/// Events in `seq` order and the `seq` to read after next time: the highest one returned, or the starting point when nothing matched.
#[derive(Debug, Clone, PartialEq)]
pub struct EventPage {
    pub events: Vec<Event>,
    pub next: u64,
}

fn invalid(message: impl Into<String>) -> FoldError {
    FoldError::new(codes::INVALID_EVENT_FILTER, message).with_hint("call get_events")
}

/// The parsed selection shared by `get_events` and `wait_for_events`.
pub(super) fn parse_filter(
    types: &[String],
    task: Option<&str>,
    proposal: Option<&str>,
) -> Result<EventFilter, FoldError> {
    let types = types
        .iter()
        .map(|name| {
            EventType::parse(name).map_err(|_| {
                invalid("an event type in types is not declared")
                    .with_details([("type", json!(name))])
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let task = task
        .map(|id| id.parse::<TaskId>())
        .transpose()
        .map_err(|_| invalid("task is not a task id such as T-12"))?;
    let proposal = proposal
        .map(|id| id.parse::<ProposalId>())
        .transpose()
        .map_err(|_| invalid("proposal is not a proposal id such as P-3"))?;
    Ok(EventFilter {
        types,
        task,
        proposal,
    })
}

fn parse_limit(limit: Option<u32>) -> Result<usize, FoldError> {
    match limit {
        Some(0) => Err(invalid("limit must be at least 1")),
        Some(limit) => Ok(limit.min(MAX_LIMIT) as usize),
        None => Ok(DEFAULT_LIMIT as usize),
    }
}

/// One read-pool query: the events after `query.after` (0 when absent) that match the filter. It never touches a cursor.
pub async fn query(store: &Store, query: &EventQuery) -> Result<EventPage, EventsError> {
    let filter = parse_filter(
        &query.types,
        query.task.as_deref(),
        query.proposal.as_deref(),
    )?;
    let limit = parse_limit(query.limit)?;
    let after = query.after.unwrap_or(0);
    let events = events::read_after(store.read(), after, &filter, limit).await?;
    Ok(page(events, after))
}

/// The events after the agent's cursor (0 when it has none), and the cursor moved to the highest `seq` returned in the same write transaction. A query with an explicit `after` is [`query`] and leaves the cursor alone.
pub async fn read_since_cursor(
    store: &Store,
    agent_id: i64,
    query: &EventQuery,
) -> Result<EventPage, EventsError> {
    if query.after.is_some() {
        return self::query(store, query).await;
    }
    let filter = parse_filter(
        &query.types,
        query.task.as_deref(),
        query.proposal.as_deref(),
    )?;
    let limit = parse_limit(query.limit)?;
    cursor_page(store, agent_id, &filter, limit, false).await
}

fn page(events: Vec<Event>, start: u64) -> EventPage {
    let next = events.last().map_or(start, |event| event.seq);
    EventPage { events, next }
}

/// Reads after the stored cursor, keeps at most `limit` matching events and moves the cursor to the last one kept, all in one write transaction. With `recipients_only`, a message event is kept only when a worker of the agent receives the message, and pages are read on until one is kept or the table ends.
pub(super) async fn cursor_page(
    store: &Store,
    agent_id: i64,
    filter: &EventFilter,
    limit: usize,
    recipients_only: bool,
) -> Result<EventPage, EventsError> {
    store
        .write(async |tx| {
            let start = events::cursor(tx.conn(), agent_id).await?;
            let mut from = start;
            let mut kept = Vec::new();
            loop {
                let page = events::read_after(tx.conn(), from, filter, limit).await?;
                let full = page.len() == limit;
                if let Some(last) = page.last() {
                    from = last.seq;
                }
                for event in page {
                    let keep = match recipient_check(&event) {
                        _ if !recipients_only => true,
                        RecipientCheck::Pass => true,
                        RecipientCheck::Fail => false,
                        RecipientCheck::Ask(message) => {
                            events::is_recipient(tx.conn(), message, agent_id).await?
                        }
                    };
                    if keep {
                        kept.push(event);
                    }
                }
                if !kept.is_empty() || !full {
                    break;
                }
            }
            kept.truncate(limit);
            let page = page(kept, start);
            if page.next > start {
                events::advance_cursor(tx, agent_id, page.next).await?;
            }
            Ok::<_, EventsError>(page)
        })
        .await
}

fn is_message_event(event: &Event) -> bool {
    event.event_type == types::MESSAGE_SENT || event.event_type == types::MESSAGE_ACKNOWLEDGED
}

/// What decides whether an event reaches an agent that waits.
pub(super) enum RecipientCheck {
    Pass,
    Fail,
    /// Passes only when a worker of the agent receives this message.
    Ask(MessageId),
}

/// A message event reaches an agent only when one of its workers is a recipient of the message in `data.message`; every other event passes.
pub(super) fn recipient_check(event: &Event) -> RecipientCheck {
    if !is_message_event(event) {
        return RecipientCheck::Pass;
    }
    match event
        .data
        .get("message")
        .and_then(serde_json::Value::as_str)
        .and_then(|id| id.parse::<MessageId>().ok())
    {
        Some(message) => RecipientCheck::Ask(message),
        None => RecipientCheck::Fail,
    }
}

/// Queries and cursors of one project's events. Reading never appends an event and never changes task state; the only write is the agent's cursor.
#[derive(Debug, Clone)]
pub struct EventService {
    store: Store,
    bus: EventBus,
}

impl EventService {
    pub fn new(store: Store, bus: EventBus) -> Self {
        Self { store, bus }
    }

    pub(super) fn store(&self) -> &Store {
        &self.store
    }

    pub(super) fn bus(&self) -> &EventBus {
        &self.bus
    }

    /// `get_events` for the agent with row id `agent_id`.
    pub async fn get_events(
        &self,
        agent_id: i64,
        query: &EventQuery,
    ) -> Result<EventPage, EventsError> {
        read_since_cursor(&self.store, agent_id, query).await
    }

    /// How many events after the agent's cursor concern it. It only reads, so building a footer never moves the cursor.
    pub async fn new_event_count(
        &self,
        agent_id: i64,
        agent: &AgentName,
    ) -> Result<u64, EventsError> {
        let mut tx = self.store.read().begin().await.map_err(StoreError::from)?;
        let after = events::cursor(&mut *tx, agent_id).await?;
        let facts = events::agent_facts(&mut tx, agent_id, agent).await?;
        let mut tasks = facts.claimed.clone();
        tasks.extend(facts.prerequisites.iter().copied());
        let mut proposals = facts.authored.clone();
        proposals.extend(facts.reviewing.iter().copied());
        let candidates =
            events::touching_after(&mut *tx, after, &tasks, &proposals, &[types::MAIN_ADVANCED])
                .await?;
        tx.commit().await.map_err(StoreError::from)?;
        let count = candidates
            .iter()
            .filter(|event| concerns(event, &facts))
            .count();
        Ok(u64::try_from(count).unwrap_or(u64::MAX))
    }
}
