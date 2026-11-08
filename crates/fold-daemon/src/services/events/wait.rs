use std::time::Duration;

use std::collections::BTreeSet;

use fold_core::events::types;
use fold_core::push::request_of;
use fold_core::requests::{DRAFTER_WANTED, Requester};
use fold_core::roles::CacheTtl;
use fold_core::{AgentName, Event, EventFilter, EventType, MessageId, RequestId, Timestamp};
use fold_store::agents::{self, WorkerRow};
use fold_store::events::{self, MessageWake};
use fold_store::tasks::requests;
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

/// The wait of a caller whose prompt cache lives `ttl`: a five-minute cache cuts the requested wait, or the default one, to [`SHORT_CACHE_MAX_TIMEOUT_S`]. `None` is a caller that fold runs in no worker process, whose wait stays as requested.
pub fn cap_for_cache(requested: Option<u64>, ttl: Option<CacheTtl>) -> Option<u64> {
    match ttl {
        Some(CacheTtl::FiveMinutes) => Some(
            requested
                .unwrap_or(DEFAULT_TIMEOUT_S)
                .min(SHORT_CACHE_MAX_TIMEOUT_S),
        ),
        Some(CacheTtl::OneHour) | None => requested,
    }
}

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
    wakes: Wakes,
    /// The requests of the page that [`Waiter::next`] returned last that wait for a drafter.
    requests: Vec<RequestId>,
    /// The page that [`Waiter::next`] returned last holds a `worker.stalled` that fold told the waiting agent.
    stalled: bool,
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
        self.start_wait(worker, None, Wakes::default(), query).await
    }

    /// [`EventService::wait`] for `worker`, which a new message it receives and did not send itself also ends, whatever the filter says: a notice that fold pushes into a session waits there until the tool call returns. For the main agent and an area lead, a request that another agent created or released and that waits for a drafter ends it too: the push of `request.created` reaches a session only once its tool call returns, so a lead that waits for something else would see the request only at the end of its wait. A `worker.stalled` that names the agent among those fold told ends it for the same reason, so the lead hears of a stall before a person does.
    pub async fn wait_as(
        &self,
        worker: &WorkerRow,
        query: &WaitQuery,
        started: Timestamp,
    ) -> Result<Waiter, EventsError> {
        let agent = worker.name.agent();
        let claimant = agents::standing(self.store().read(), &agent)
            .await?
            .claims_requests()
            .then(|| agent.clone());
        let wakes = Wakes {
            claimant,
            told: Some(agent),
        };
        self.start_wait(worker, Some(wake_of(worker, started)), wakes, query)
            .await
    }

    /// The messages of `page` that wait for the wait of `worker` that started at `started`, by the rule that wakes it, so a page that the filter matched names no message the worker has read, acknowledged or sent.
    pub async fn waiting_messages(
        &self,
        worker: &WorkerRow,
        page: &EventPage,
        started: Timestamp,
    ) -> Result<Vec<MessageId>, EventsError> {
        let seqs: Vec<u64> = page
            .events
            .iter()
            .filter(|event| event.event_type == types::MESSAGE_SENT)
            .map(|event| event.seq)
            .collect();
        if seqs.is_empty() {
            return Ok(Vec::new());
        }
        Ok(events::waiting_messages(self.store().read(), &seqs, wake_of(worker, started)).await?)
    }

    async fn start_wait(
        &self,
        worker: &WorkerRow,
        wake: Option<MessageWake>,
        wakes: Wakes,
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
        let first = self.page(worker, &filter, wake, &wakes).await?;
        let now = Instant::now();
        let mut ticks = tokio::time::interval_at(now + TICK, TICK);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        Ok(Waiter {
            service: self.clone(),
            worker: worker.clone(),
            filter,
            wake,
            wakes,
            requests: first.requests,
            stalled: first.stalled,
            receiver,
            closing,
            ready: (!first.page.events.is_empty()).then_some(first.page),
            deadline: now + Duration::from_secs(timeout_s),
            ticks,
            tick_count: 0,
            _waiting: self.bus().waiting(),
        })
    }

    /// The next page of the wait, the requests in it that wait for a drafter and whether it holds a stall told to the waiting agent. An event that only a wake of [`Wakes`] selected stays in the page only while it concerns the waiting agent: a request while it waits for a drafter, a stall while fold told the agent of it. The cursor has moved past it either way, so a page that loses every event is read again from there at once.
    async fn page(
        &self,
        worker: &WorkerRow,
        filter: &EventFilter,
        wake: Option<MessageWake>,
        wakes: &Wakes,
    ) -> Result<WaitPage, EventsError> {
        let wake_types = wakes.types();
        loop {
            let page = cursor_page(
                self.store(),
                worker,
                filter,
                DEFAULT_LIMIT as usize,
                Some(wake),
                &wake_types,
            )
            .await?;
            if page.events.is_empty() || wake_types.is_empty() {
                return Ok(WaitPage::plain(page));
            }
            let kept = self.keep_woken(page, filter, wakes).await?;
            if !kept.page.events.is_empty() {
                return Ok(kept);
            }
        }
    }

    /// Drops the events of `page` that only a wake of `wakes` selected and that do not concern the waiting agent: a request it cannot claim now, a stall fold did not tell it of. An agent never wakes for what its own workers did, nor for a request it made itself.
    async fn keep_woken(
        &self,
        page: EventPage,
        filter: &EventFilter,
        wakes: &Wakes,
    ) -> Result<WaitPage, EventsError> {
        let named = |event: &Event| {
            wakes.claimant.as_ref().is_some_and(|agent| {
                DRAFTER_WANTED.contains(&event.event_type)
                    && event.actor.agent() != Some(agent.as_str())
            })
        };
        let waiting: BTreeSet<RequestId> = match &wakes.claimant {
            Some(agent) if page.events.iter().any(named) => {
                let own = Requester::Agent(agent.clone());
                requests::unfinished(self.store().read())
                    .await?
                    .into_iter()
                    .filter(|request| {
                        request.state.waits_for_drafter(request.has_drafter)
                            && request.requester != own
                    })
                    .map(|request| request.id)
                    .collect()
            }
            _ => BTreeSet::new(),
        };
        let told = |event: &Event| {
            wakes
                .told
                .as_ref()
                .is_some_and(|agent| tells_of_stall(event, agent))
        };
        let mut requests = Vec::new();
        let mut stalled = false;
        let events = page
            .events
            .into_iter()
            .filter(|event| {
                let claimable = named(event)
                    && request_of(event).is_some_and(|request| waiting.contains(&request));
                if claimable
                    && let Some(request) = request_of(event)
                    && !requests.contains(&request)
                {
                    requests.push(request);
                }
                let stall = told(event);
                stalled |= stall;
                claimable
                    || stall
                    || filter.matches(event)
                    || !(DRAFTER_WANTED.contains(&event.event_type)
                        || event.event_type == types::WORKER_STALLED)
            })
            .collect();
        Ok(WaitPage {
            page: EventPage {
                events,
                next: page.next,
            },
            requests,
            stalled,
        })
    }
}

/// The events that end an agent's wait whatever its filter says, besides its new messages.
#[derive(Debug, Clone, Default)]
struct Wakes {
    /// The agent of an orchestrator's wait, which a request it could claim also ends.
    claimant: Option<AgentName>,
    /// The agent of an agent's wait, which a stall that fold told it of also ends.
    told: Option<AgentName>,
}

impl Wakes {
    fn types(&self) -> Vec<EventType> {
        let mut woken = Vec::new();
        if self.claimant.is_some() {
            woken.extend(DRAFTER_WANTED);
        }
        if self.told.is_some() {
            woken.push(types::WORKER_STALLED);
        }
        woken
    }

    /// Whether `event`, which came on the bus, can end the wait past its filter.
    fn may_wake(&self, event: &Event) -> bool {
        (self.claimant.is_some() && DRAFTER_WANTED.contains(&event.event_type))
            || self
                .told
                .as_ref()
                .is_some_and(|agent| tells_of_stall(event, agent))
    }
}

/// `event` is a `worker.stalled` whose `told` names `agent`.
fn tells_of_stall(event: &Event, agent: &AgentName) -> bool {
    event.event_type == types::WORKER_STALLED
        && event
            .data
            .get("told")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|told| {
                told.iter()
                    .any(|name| name.as_str() == Some(agent.as_str()))
            })
}

/// A page of a wait with what its wakes found in it.
struct WaitPage {
    page: EventPage,
    requests: Vec<RequestId>,
    stalled: bool,
}

impl WaitPage {
    fn plain(page: EventPage) -> Self {
        Self {
            page,
            requests: Vec::new(),
            stalled: false,
        }
    }
}

fn wake_of(worker: &WorkerRow, since: Timestamp) -> MessageWake {
    MessageWake {
        event_type: types::MESSAGE_SENT,
        worker_id: worker.id,
        since,
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
                            || self.wakes.may_wake(&event)
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
            let found = self
                .service
                .page(&self.worker, &self.filter, self.wake, &self.wakes)
                .await?;
            if !found.page.events.is_empty() {
                self.requests = found.requests;
                self.stalled = found.stalled;
                return Ok(WaitStep::Events(found.page));
            }
        }
    }

    /// The requests of the page that [`Waiter::next`] returned last that wait for a drafter and that the waiting orchestrator may claim; always empty for any other agent.
    pub fn requests(&self) -> &[RequestId] {
        &self.requests
    }

    /// Whether the page that [`Waiter::next`] returned last holds a `worker.stalled` that fold told the waiting agent.
    pub fn stalled(&self) -> bool {
        self.stalled
    }

    /// How many events the waiter holds in memory: at most the one page its first read found.
    pub fn held(&self) -> usize {
        self.ready.as_ref().map_or(0, |page| page.events.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_five_minute_cache_cuts_a_long_or_default_wait_and_keeps_a_short_one() {
        let short = Some(CacheTtl::FiveMinutes);
        assert_eq!(cap_for_cache(Some(1500), short), Some(270));
        assert_eq!(cap_for_cache(None, short), Some(270));
        assert_eq!(cap_for_cache(Some(30), short), Some(30));
    }

    #[test]
    fn an_hour_cache_or_no_worker_process_keeps_the_requested_wait() {
        assert_eq!(
            cap_for_cache(Some(1500), Some(CacheTtl::OneHour)),
            Some(1500)
        );
        assert_eq!(cap_for_cache(None, Some(CacheTtl::OneHour)), None);
        assert_eq!(cap_for_cache(Some(1500), None), Some(1500));
        assert_eq!(cap_for_cache(None, None), None);
    }
}
