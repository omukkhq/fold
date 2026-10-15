use std::collections::VecDeque;
use std::sync::Arc;

use fold_core::events::types;
use fold_core::{Event, FoldError, LiveEvent, WorkspaceId, codes};
use fold_store::{Store, StoreError, events};
use serde_json::Value;
use tokio::sync::broadcast::{self, error::RecvError, error::TryRecvError};
use tokio::sync::watch;

use super::{BUS_CAPACITY, BusMessage, EventBus, EventsError};
use crate::live::{LiveState, WorkspaceLive};

/// How many stored events one replay read returns.
pub const REPLAY_PAGE: usize = 500;

const MAX_PENDING: usize = BUS_CAPACITY;

/// One frame of a project's event stream, before a transport encodes it.
#[derive(Debug, Clone)]
pub enum FeedItem {
    Durable(Arc<Event>),
    /// `seq` is the newest stored event the feed has sent; every newer one follows the snapshot.
    Snapshot {
        seq: u64,
        workspaces: Vec<Arc<WorkspaceLive>>,
    },
    Live(Arc<LiveEvent>),
}

/// The unsent live frame that a newer one replaces: a live frame is a whole state, so only the newest of each workspace or agent matters.
#[derive(Debug, PartialEq, Eq)]
enum Slot<'a> {
    Workspace(WorkspaceId),
    Agent(&'a str),
}

fn slot_of(event: &LiveEvent) -> Option<Slot<'_>> {
    if event.event_type == types::WORKSPACE_CHANGED {
        event.workspace.map(Slot::Workspace)
    } else if event.event_type == types::AGENT_PRESENCE_CHANGED {
        event
            .data
            .get("agent")
            .and_then(Value::as_str)
            .map(Slot::Agent)
    } else {
        None
    }
}

fn invalid_last_event_id(message: &str) -> FoldError {
    FoldError::new(codes::INVALID_LAST_EVENT_ID, message)
        .with_hint("reconnect without Last-Event-ID and reload the project")
}

/// Reads a `Last-Event-ID` value. The value never goes into the error, because it is the client's input.
pub fn parse_last_event_id(raw: &[u8]) -> Result<u64, FoldError> {
    std::str::from_utf8(raw)
        .ok()
        .filter(|text| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| invalid_last_event_id("Last-Event-ID is not an event sequence number"))
}

/// One client's view of a project's events: the stored events after its last id, a snapshot of the live workspaces, then everything the bus carries. No stored event is skipped or sent twice, because the feed subscribes before it reads the table and drops a bus event it already sent. The feed owns its subscription, so dropping it leaves nothing behind.
#[derive(Debug)]
pub struct Feed {
    receiver: broadcast::Receiver<BusMessage>,
    closing: watch::Receiver<bool>,
    store: Store,
    live: LiveState,
    /// Every stored event up to here is sent or queued.
    sent: u64,
    /// The table holds events the feed has not queued: the replay, or a hole in what the bus delivered.
    replaying: bool,
    snapshot_due: bool,
    pending: VecDeque<FeedItem>,
}

impl Feed {
    /// Refuses a `last_event_id` above the newest stored event with `invalid_last_event_id`: the client holds events of another database.
    pub async fn open(
        store: &Store,
        bus: &EventBus,
        live: &LiveState,
        last_event_id: Option<u64>,
    ) -> Result<Self, EventsError> {
        let receiver = bus.subscribe();
        let head = events::head_seq(store.read()).await?;
        let sent = match last_event_id {
            Some(id) if id > head => {
                return Err(invalid_last_event_id(
                    "Last-Event-ID is above the newest event of the project",
                )
                .with_details([("head", head.into())])
                .into());
            }
            Some(id) => id,
            None => head,
        };
        Ok(Self {
            receiver,
            closing: bus.closing(),
            store: store.clone(),
            live: live.clone(),
            sent,
            replaying: last_event_id.is_some(),
            snapshot_due: true,
            pending: VecDeque::new(),
        })
    }

    /// The next frame, or `None` once the stream must end: the bus is closed, or the client fell more than the bus capacity behind. A client that lagged reconnects with its last id, and the replay fills the gap.
    pub async fn next(&mut self) -> Option<Result<FeedItem, StoreError>> {
        loop {
            // Until the snapshot is sent, the bus keeps what arrives, so nothing goes ahead of the replay or the snapshot.
            if !self.snapshot_due
                && let Err(skipped) = self.drain()
            {
                return lagged(skipped);
            }
            if let Some(item) = self.pending.pop_front() {
                return Some(Ok(item));
            }
            if self.replaying {
                if let Err(error) = self.read_page().await {
                    return Some(Err(error));
                }
                continue;
            }
            if self.snapshot_due {
                let seq = self.sent;
                if let Err(skipped) = self.drain_before_snapshot() {
                    return lagged(skipped);
                }
                self.snapshot_due = false;
                return Some(Ok(FeedItem::Snapshot {
                    seq,
                    workspaces: self.live.all(),
                }));
            }
            let received = tokio::select! {
                biased;
                _ = self.closing.wait_for(|closed| *closed) => return None,
                received = self.receiver.recv() => received,
            };
            match received {
                Ok(message) => self.take(message),
                Err(RecvError::Lagged(skipped)) => return lagged(skipped),
                Err(RecvError::Closed) => return None,
            }
        }
    }

    /// Takes everything the bus already holds before each frame, so a live state the client has not read yet is replaced by a newer one. The queue stops taking at the bus capacity; a client slower than that lags and its stream ends. A hole in the stored events stops it too, because the table read that fills the hole comes first.
    fn drain(&mut self) -> Result<(), u64> {
        while self.pending.len() < MAX_PENDING && !self.replaying {
            match self.receiver.try_recv() {
                Ok(message) => self.take(message),
                Err(TryRecvError::Lagged(skipped)) => return Err(skipped),
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            }
        }
        Ok(())
    }

    /// Drops every workspace state queued before the snapshot: the live state publishes under its write guard, so the snapshot read after this is at least as new. Sent after the snapshot, such a state would bring back a workspace whose end the replay already sent. Presence frames and stored events are kept; the snapshot holds neither.
    fn drain_before_snapshot(&mut self) -> Result<(), u64> {
        for _ in 0..self.receiver.len() {
            match self.receiver.try_recv() {
                Ok(BusMessage::Live(event)) if event.event_type == types::WORKSPACE_CHANGED => {}
                Ok(message) => self.take(message),
                Err(TryRecvError::Lagged(skipped)) => return Err(skipped),
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            }
        }
        Ok(())
    }

    fn take(&mut self, message: BusMessage) {
        match message {
            BusMessage::Durable(event) => {
                if event.seq <= self.sent || self.replaying {
                    return;
                }
                // Two writes can publish in the other order than they committed; the table has both.
                if event.seq > self.sent + 1 {
                    self.replaying = true;
                    return;
                }
                self.sent = event.seq;
                self.pending.push_back(FeedItem::Durable(event));
            }
            BusMessage::Live(event) => {
                if let Some(slot) = slot_of(&event) {
                    self.pending.retain(|item| match item {
                        FeedItem::Live(older) => slot_of(older).as_ref() != Some(&slot),
                        FeedItem::Durable(_) | FeedItem::Snapshot { .. } => true,
                    });
                }
                self.pending.push_back(FeedItem::Live(event));
            }
        }
    }

    async fn read_page(&mut self) -> Result<(), StoreError> {
        let page = events::after(self.store.read(), self.sent, REPLAY_PAGE).await?;
        if page.len() < REPLAY_PAGE {
            self.replaying = false;
        }
        if let Some(last) = page.last() {
            self.sent = last.seq;
        }
        self.pending.extend(
            page.into_iter()
                .map(|event| FeedItem::Durable(Arc::new(event))),
        );
        Ok(())
    }
}

fn lagged(skipped: u64) -> Option<Result<FeedItem, StoreError>> {
    tracing::debug!(skipped, "an event stream fell behind the bus and ends");
    None
}
