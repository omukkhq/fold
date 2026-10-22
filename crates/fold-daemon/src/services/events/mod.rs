use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use fold_core::{Event, EventFilter, LiveEvent, NewEvent, Timestamp};
use fold_store::{Store, StoreError, WriteTx, events};
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::sync::watch;

mod feed;
mod query;
mod wait;

pub use feed::{Feed, FeedItem, REPLAY_PAGE, parse_last_event_id};
pub use query::{
    DEFAULT_LIMIT, EventPage, EventQuery, EventService, EventsError, HistoryPage, MAX_LIMIT,
    NEW_EVENTS_CAP, history, query, read_since_cursor,
};
pub use wait::{
    DEFAULT_TIMEOUT_S, MAX_TIMEOUT_S, SHORT_CACHE_MAX_TIMEOUT_S, TICK, WaitQuery, WaitStep, Waiter,
};

const BUS_CAPACITY: usize = 1024;

/// What the bus carries: a stored event, or a live notice that is never stored.
#[derive(Debug, Clone)]
pub enum BusMessage {
    Durable(Arc<Event>),
    Live(Arc<LiveEvent>),
}

/// The events a service appended during one write. They go to the bus only through [`EventBus::publish`], after the write has committed; dropping the buffer after a rollback publishes nothing.
#[derive(Debug, Default)]
pub struct EventBuffer {
    events: Vec<Event>,
}

impl EventBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends inside the caller's write transaction and keeps the stored event.
    pub async fn append(
        &mut self,
        tx: &mut WriteTx,
        new: &NewEvent,
        now: Timestamp,
    ) -> Result<&Event, StoreError> {
        let stored = events::append(tx, new, now).await?;
        self.events.push(stored);
        Ok(&self.events[self.events.len() - 1])
    }

    /// Takes over the events another step of the same write appended.
    pub fn extend(&mut self, other: EventBuffer) {
        self.events.extend(other.events);
    }

    /// The stored events in the order they were appended.
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// The in-process fan-out of one project. Events of two projects never mix because each project owns its own bus.
#[derive(Debug, Clone)]
pub struct EventBus {
    sender: broadcast::Sender<BusMessage>,
    waiters: Arc<AtomicUsize>,
    /// Set once at daemon shutdown. Every clone of the project's bus lives as long as the project, so the channel itself never closes.
    closing: Arc<watch::Sender<bool>>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        Self::with_capacity(BUS_CAPACITY)
    }

    /// A smaller capacity makes a slow subscriber lag sooner; tests use it to exercise recovery.
    pub fn with_capacity(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            waiters: Arc::new(AtomicUsize::new(0)),
            closing: Arc::new(watch::Sender::new(false)),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BusMessage> {
        self.sender.subscribe()
    }

    /// How many receivers are subscribed right now. A waiter that ended has dropped its receiver.
    pub fn subscribers(&self) -> usize {
        self.sender.receiver_count()
    }

    /// How many waits have read their first page and wait on the bus right now.
    pub fn waiters(&self) -> usize {
        self.waiters.load(Ordering::SeqCst)
    }

    pub(crate) fn waiting(&self) -> WaitingGuard {
        self.waiters.fetch_add(1, Ordering::SeqCst);
        WaitingGuard(self.waiters.clone())
    }

    /// Sends the buffered events in `seq` order. A service calls this only after `Store::write` returned `Ok`.
    pub fn publish(&self, buffer: EventBuffer) {
        let mut stored = buffer.events;
        stored.sort_by_key(|event| event.seq);
        for event in stored {
            // A send fails only when nobody is subscribed, and then nobody needs the event.
            let _ = self.sender.send(BusMessage::Durable(Arc::new(event)));
        }
    }

    pub fn publish_live(&self, event: LiveEvent) {
        let _ = self.sender.send(BusMessage::Live(Arc::new(event)));
    }

    /// Ends every open event stream of the project; the daemon calls it when it shuts down.
    pub fn close(&self) {
        self.closing.send_replace(true);
    }

    /// Holds `true` once [`EventBus::close`] was called.
    pub fn closing(&self) -> watch::Receiver<bool> {
        self.closing.subscribe()
    }

    /// Yields every stored event with `seq > after` that matches `filter`, then the events committed later, each exactly once and in `seq` order. It subscribes before it reads the table, so nothing falls between the backlog and the live events.
    pub fn stream_after(&self, store: &Store, after: u64, filter: EventFilter) -> EventStream {
        EventStream {
            receiver: self.subscribe(),
            store: store.clone(),
            filter,
            cursor: after,
            floor: after,
            syncing: true,
            pending: VecDeque::new(),
        }
    }
}

/// Counts one wait in [`EventBus::waiters`] until it is dropped.
#[derive(Debug)]
pub struct WaitingGuard(Arc<AtomicUsize>);

impl Drop for WaitingGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A gap-free, duplicate-free view of the stored events. The table is the source of truth: the bus only says that something new exists, so a lagged receiver or a reordered publish is repaired by reading the table.
#[derive(Debug)]
pub struct EventStream {
    receiver: broadcast::Receiver<BusMessage>,
    store: Store,
    filter: EventFilter,
    /// Every event with a `seq` up to here has been yielded or filtered out.
    cursor: u64,
    /// The newest `seq` the bus announced; once the table is read up to it, the cursor may move there even when no row matched the filter.
    floor: u64,
    syncing: bool,
    pending: VecDeque<Arc<Event>>,
}

impl EventStream {
    /// The next matching stored event, or `None` once the bus is closed.
    pub async fn next(&mut self) -> Option<Result<Arc<Event>, StoreError>> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Some(Ok(event));
            }
            if self.syncing {
                if let Err(error) = self.read_page().await {
                    return Some(Err(error));
                }
                continue;
            }
            match self.receiver.recv().await {
                Ok(BusMessage::Durable(event)) => {
                    if event.seq <= self.cursor {
                        continue;
                    }
                    if event.seq == self.cursor + 1 {
                        self.cursor = event.seq;
                        if self.filter.matches(&event) {
                            return Some(Ok(event));
                        }
                        continue;
                    }
                    self.floor = self.floor.max(event.seq);
                    self.syncing = true;
                }
                Ok(BusMessage::Live(_)) => {}
                Err(RecvError::Lagged(_)) => self.syncing = true,
                Err(RecvError::Closed) => return None,
            }
        }
    }

    async fn read_page(&mut self) -> Result<(), StoreError> {
        let page = events::read_after(
            self.store.read(),
            self.cursor,
            &self.filter,
            events::MAX_PAGE,
        )
        .await?;
        let full = page.len() == events::MAX_PAGE;
        if let Some(last) = page.last() {
            self.cursor = last.seq;
        }
        self.pending.extend(page.into_iter().map(Arc::new));
        if !full {
            self.syncing = false;
            self.cursor = self.cursor.max(self.floor);
        }
        Ok(())
    }
}
