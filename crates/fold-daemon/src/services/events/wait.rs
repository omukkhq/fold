use std::time::Duration;

use fold_core::EventFilter;
use fold_core::events::types;
use fold_store::agents::WorkerRow;
use fold_store::events::{self, MessageWake, WaitingMessage};
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::sync::watch;
use tokio::time::{Instant, Interval, MissedTickBehavior};

use super::query::{
    DEFAULT_LIMIT, EventPage, EventService, EventsError, cursor_page, parse_filter,
};
use super::{BusMessage, WaitingGuard};

/// The wait when a call names no `timeout_s`.
pub const DEFAULT_TIMEOUT_S: u64 = 300;

/// The longest wait; a larger `timeout_s` is lowered to it.
pub const MAX_TIMEOUT_S: u64 = 1500;

/// The longest wait of a worker whose prompt cache lives five minutes: a longer wait lets the cache expire, and the next turn pays to write it again.
pub const SHORT_CACHE_MAX_TIMEOUT_S: u64 = 270;

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

/// One worker waiting for matching events. It subscribed to the bus before it read the table, so no event commits unseen between the two; dropping it drops the subscription. The bus only says that something new exists: each read is one bounded page from the table after the worker's cursor, so a wait never loads the backlog.
#[derive(Debug)]
pub struct Waiter {
    service: EventService,
    worker: WorkerRow,
    filter: EventFilter,
    wake: Option<MessageWake>,
    receiver: broadcast::Receiver<BusMessage>,
    closing: watch::Receiver<bool>,
    /// The page the first read found, returned by the first [`Waiter::next`].
    ready: Option<EventPage>,
    deadline: Instant,
    ticks: Interval,
    tick_count: u64,
    _waiting: WaitingGuard,
}

impl EventService {
    /// Starts a wait for `worker` and reads the first page after its cursor. The deadline and the ticks run on `tokio::time`, so tests drive them with a paused clock.
    pub async fn wait(&self, worker: &WorkerRow, query: &WaitQuery) -> Result<Waiter, EventsError> {
        self.start_wait(worker, None, query).await
    }

    /// [`EventService::wait`] for `worker`, which a new message it receives and did not send itself also ends, whatever the filter says: a notice that fold pushes into a session waits there until the tool call returns. A main worker also wakes for the messages to its agent's other workers, because its session takes their notices and its inbox can read them.
    pub async fn wait_as(
        &self,
        worker: &WorkerRow,
        query: &WaitQuery,
    ) -> Result<Waiter, EventsError> {
        self.start_wait(worker, Some(wake_of(worker)), query).await
    }

    /// The messages of `page` that wait for `worker` by the rule that wakes its wait, so a page that the filter matched names no message the worker has read, acknowledged or sent.
    pub async fn waiting_messages(
        &self,
        worker: &WorkerRow,
        page: &EventPage,
    ) -> Result<Vec<WaitingMessage>, EventsError> {
        let seqs: Vec<u64> = page
            .events
            .iter()
            .filter(|event| event.event_type == types::MESSAGE_SENT)
            .map(|event| event.seq)
            .collect();
        if seqs.is_empty() {
            return Ok(Vec::new());
        }
        Ok(
            events::waiting_messages(self.store().read(), &seqs, worker.agent_id, wake_of(worker))
                .await?,
        )
    }

    async fn start_wait(
        &self,
        worker: &WorkerRow,
        wake: Option<MessageWake>,
        query: &WaitQuery,
    ) -> Result<Waiter, EventsError> {
        let filter = parse_filter(
            &query.types,
            query.task.as_deref(),
            query.proposal.as_deref(),
        )?;
        let timeout_s = query
            .timeout_s
            .unwrap_or(DEFAULT_TIMEOUT_S)
            .clamp(1, MAX_TIMEOUT_S);
        let receiver = self.bus().subscribe();
        let closing = self.bus().closing();
        let first = self.page(worker, &filter, wake).await?;
        let now = Instant::now();
        let mut ticks = tokio::time::interval_at(now + TICK, TICK);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        Ok(Waiter {
            service: self.clone(),
            worker: worker.clone(),
            filter,
            wake,
            receiver,
            closing,
            ready: (!first.events.is_empty()).then_some(first),
            deadline: now + Duration::from_secs(timeout_s),
            ticks,
            tick_count: 0,
            _waiting: self.bus().waiting(),
        })
    }

    async fn page(
        &self,
        worker: &WorkerRow,
        filter: &EventFilter,
        wake: Option<MessageWake>,
    ) -> Result<EventPage, EventsError> {
        cursor_page(
            self.store(),
            worker,
            filter,
            DEFAULT_LIMIT as usize,
            Some(wake),
        )
        .await
    }
}

/// A main worker also wakes for the messages to its agent's other workers, because its session takes their notices and its inbox can read them.
fn wake_of(worker: &WorkerRow) -> MessageWake {
    MessageWake {
        event_type: types::MESSAGE_SENT,
        worker_id: worker.id,
        every_worker: worker.name.is_main(),
    }
}

impl Waiter {
    /// Returns the matching events, and the sent messages of the wake, as soon as there are any, a tick every [`TICK`], or `TimedOut` at the deadline or once the project's bus closes. A matching event brings every match committed with it, because the page is read from the table after the cursor. The bus is polled before the deadline, so a match that is ready when the deadline passes is still returned.
    pub async fn next(&mut self) -> Result<WaitStep, EventsError> {
        if let Some(page) = self.ready.take() {
            return Ok(WaitStep::Events(page));
        }
        loop {
            let read = tokio::select! {
                biased;
                _ = self.closing.wait_for(|closed| *closed) => return Ok(WaitStep::TimedOut),
                message = self.receiver.recv() => match message {
                    Ok(BusMessage::Durable(event)) => {
                        self.filter.matches(&event)
                            || self.wake.is_some_and(|wake| wake.event_type == event.event_type)
                    }
                    Ok(BusMessage::Live(_)) => false,
                    // The table holds whatever the bus dropped.
                    Err(RecvError::Lagged(_)) => true,
                    Err(RecvError::Closed) => return Ok(WaitStep::TimedOut),
                },
                () = tokio::time::sleep_until(self.deadline) => return Ok(WaitStep::TimedOut),
                _ = self.ticks.tick() => {
                    self.tick_count += 1;
                    return Ok(WaitStep::Tick(self.tick_count));
                }
            };
            if !read {
                continue;
            }
            let page = self
                .service
                .page(&self.worker, &self.filter, self.wake)
                .await?;
            if !page.events.is_empty() {
                return Ok(WaitStep::Events(page));
            }
        }
    }

    /// How many events the waiter holds in memory: at most the one page its first read found.
    pub fn held(&self) -> usize {
        self.ready.as_ref().map_or(0, |page| page.events.len())
    }
}
