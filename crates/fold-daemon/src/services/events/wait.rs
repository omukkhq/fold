use std::time::Duration;

use fold_core::EventFilter;
use tokio::time::{Instant, Interval, MissedTickBehavior};

use super::query::{
    DEFAULT_LIMIT, EventPage, EventService, EventsError, RecipientCheck, cursor_page, parse_filter,
    recipient_check,
};
use super::{EventStream, WaitingGuard};

/// The wait when a call names no `timeout_s`.
pub const DEFAULT_TIMEOUT_S: u64 = 300;

/// The longest wait; a larger `timeout_s` is lowered to it.
pub const MAX_TIMEOUT_S: u64 = 1500;

/// How often a waiter reports that it is still alive.
pub const TICK: Duration = Duration::from_secs(60);

/// A `wait_for_events` request as a transport receives it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WaitQuery {
    pub types: Vec<String>,
    pub task: Option<String>,
    pub proposal: Option<String>,
    pub timeout_s: Option<u64>,
}

/// What one [`Waiter::next`] call ends with.
#[derive(Debug, Clone, PartialEq)]
pub enum WaitStep {
    /// The matching events; the cursor has moved past them.
    Events(EventPage),
    /// The `n`th tick of [`TICK`]; the caller renews the worker and calls `next` again.
    Tick(u64),
    TimedOut,
}

/// One agent waiting for matching events. It subscribed to the bus before it read the backlog, so no event commits unseen between the two; dropping it drops the subscription.
#[derive(Debug)]
pub struct Waiter {
    service: EventService,
    agent_id: i64,
    filter: EventFilter,
    stream: EventStream,
    deadline: Instant,
    ticks: Interval,
    tick_count: u64,
    _waiting: WaitingGuard,
}

impl EventService {
    /// Starts a wait for the agent with row id `agent_id`. The deadline and the ticks run on `tokio::time`, so tests drive them with a paused clock.
    pub async fn wait(&self, agent_id: i64, query: &WaitQuery) -> Result<Waiter, EventsError> {
        let filter = parse_filter(
            &query.types,
            query.task.as_deref(),
            query.proposal.as_deref(),
        )?;
        let timeout_s = query
            .timeout_s
            .unwrap_or(DEFAULT_TIMEOUT_S)
            .clamp(1, MAX_TIMEOUT_S);
        let cursor = fold_store::events::cursor(self.store().read(), agent_id).await?;
        let mut stream = self
            .bus()
            .stream_after(self.store(), cursor, filter.clone());
        stream.catch_up().await?;
        let now = Instant::now();
        let mut ticks = tokio::time::interval_at(now + TICK, TICK);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        Ok(Waiter {
            service: self.clone(),
            agent_id,
            filter,
            stream,
            deadline: now + Duration::from_secs(timeout_s),
            ticks,
            tick_count: 0,
            _waiting: self.bus().waiting(),
        })
    }
}

impl Waiter {
    /// Returns the matching events as soon as there are any, a tick every [`TICK`], or `TimedOut` at the deadline. A matching event brings every match committed with it, because the page is read from the table after the cursor. The stream is polled first, so a match that is ready when the deadline passes is still returned.
    pub async fn next(&mut self) -> Result<WaitStep, EventsError> {
        loop {
            tokio::select! {
                biased;
                next = self.stream.next() => match next {
                    // The bus closes only when its project closes; the waiter has nothing more to wait for.
                    None => return Ok(WaitStep::TimedOut),
                    Some(Err(error)) => return Err(error.into()),
                    Some(Ok(event)) => {
                        let reaches = match recipient_check(&event) {
                            RecipientCheck::Pass => true,
                            RecipientCheck::Ask(seq) => {
                                fold_store::events::is_recipient(
                                    self.service.store().read(),
                                    seq,
                                    self.agent_id,
                                )
                                .await?
                            }
                        };
                        if !reaches {
                            continue;
                        }
                        let page = cursor_page(
                            self.service.store(),
                            self.agent_id,
                            &self.filter,
                            DEFAULT_LIMIT as usize,
                            true,
                        )
                        .await?;
                        if !page.events.is_empty() {
                            return Ok(WaitStep::Events(page));
                        }
                    }
                },
                () = tokio::time::sleep_until(self.deadline) => return Ok(WaitStep::TimedOut),
                _ = self.ticks.tick() => {
                    self.tick_count += 1;
                    return Ok(WaitStep::Tick(self.tick_count));
                }
            }
        }
    }
}
