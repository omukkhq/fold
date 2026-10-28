use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use fold_core::events::types;
use fold_core::projects::notify::{
    EventFacts, Links, Lookup, MAX_NOTICES_PER_SEND, NeedsYouNotice, Notice, NoticeType,
    SEND_TIMEOUT, cause_of, classify, due_at, needs_you_notice, notice,
};
use fold_core::{EventFilter, FoldError, LiveEvent, NotifyTargetId, Timestamp, codes};
use fold_store::projects::notify::{self as outbox, NewSend, SeenItem, SendState};
use fold_store::{StoreError, events, tasks};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;

use super::items::{item_key, notice_item};
use super::settings::{Applying, NotifySettings};
use super::{attempt, body, links, off_items, webhook_id_prefix, with_notify_attention};
use crate::jobs::JobProgress;
use crate::projects::ProjectHandle;
use crate::services::agents::{NeedsYouKind, with_folder_attention};
use crate::services::error::from_store;
use crate::services::events::BusMessage;

/// How long a pass that failed holds back the next one, so a broken settings file does not fill the log.
const RETRY_AFTER: Duration = Duration::from_secs(60);

/// The events one write of the notifier passes.
const PAGE: usize = 500;

/// How long a try may stay in flight before a pass sends it again: well past [`SEND_TIMEOUT`], so only a try whose result was never recorded, as after a failed write, goes again.
pub const STALE_TRY: Duration = Duration::from_secs(SEND_TIMEOUT.as_secs() * 4);

/// The sends one pass makes to one target, so a long queue of a window-0 target does not hold the others back.
const SENDS_PER_PASS: usize = 20;

/// What one pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NotifyPass {
    /// Notices that went into the outbox.
    pub queued: u64,
    /// Tries that reached their receiver and got a 2xx.
    pub sent: u64,
    /// When a send is due next.
    pub next_due: Option<Timestamp>,
}

impl NotifyPass {
    fn due(&mut self, at: Timestamp) {
        self.next_due = Some(self.next_due.map_or(at, |next| next.min(at)));
    }
}

/// What the notifier of a project keeps between passes.
#[derive(Debug, Default)]
pub struct PassState {
    /// The active targets the last cleanup of the outbox kept; a change of them cleans again.
    cleaned_for: Option<BTreeSet<NotifyTargetId>>,
    /// Whether the first pass took back the tries that were in flight when the daemon stopped.
    resumed: bool,
}

fn store_error(error: StoreError) -> FoldError {
    from_store(error)
}

/// One pass of the project's notifier: it queues a notice for each new stored event and Needs you change that a target wants, in the same write as the cursor move, then makes and tries the sends that are due. The first pass puts a try that was in flight when the daemon stopped back to go again with the same webhook id, and every later pass does the same for a try whose record never came within [`STALE_TRY`]. A project without an active target reads nothing more and writes nothing.
pub async fn notify_pass(
    handle: &ProjectHandle,
    settings: &NotifySettings,
    state: &mut PassState,
) -> Result<NotifyPass, FoldError> {
    let now = handle.clock.now();
    let started_by = if state.resumed {
        now.checked_sub(STALE_TRY)
    } else {
        Some(now)
    };
    if let Some(started_by) = started_by
        && outbox::open_sends(handle.store.read())
            .await
            .map_err(store_error)?
            .iter()
            .any(|send| {
                send.state == SendState::Sending
                    && send.last_try_at.is_some_and(|at| at <= started_by)
            })
    {
        handle
            .store
            .write(async |tx| outbox::reset_sending(tx, started_by, now).await)
            .await
            .map_err(store_error)?;
    }
    state.resumed = true;
    let active: Vec<Applying> = settings
        .applying(&handle.slug)
        .await?
        .into_iter()
        .filter(|applying| applying.target.active())
        .collect();
    let ids: BTreeSet<NotifyTargetId> = active.iter().map(|applying| applying.target.id).collect();
    let keep: Vec<NotifyTargetId> = ids.iter().copied().collect();
    if state.cleaned_for.as_ref() != Some(&ids) {
        if outbox::has_inactive(handle.store.read(), &keep)
            .await
            .map_err(store_error)?
        {
            handle
                .store
                .write(async |tx| outbox::drop_inactive(tx, &keep).await)
                .await
                .map_err(store_error)?;
        }
        state.cleaned_for = Some(ids);
    }
    let mut pass = NotifyPass::default();
    if active.is_empty() {
        return Ok(pass);
    }
    let links = links(settings, &handle.slug).await?;
    let seq = queue_events(handle, &active, &links, &mut pass).await?;
    queue_needs_you(handle, settings, &active, &links, seq, &mut pass).await?;
    for applying in &active {
        send_due(handle, settings, applying, &links, &mut pass).await?;
    }
    Ok(pass)
}

/// The cursor, or where a cursor that never moved starts: before the first event since the oldest active target was added.
pub(super) async fn start_cursor(
    handle: &ProjectHandle,
    active: &[Applying],
) -> Result<u64, FoldError> {
    let read = handle.store.read();
    if let Some(cursor) = outbox::cursor(read).await.map_err(store_error)? {
        return Ok(cursor);
    }
    let since = active
        .iter()
        .map(|applying| applying.target.created_at)
        .min()
        .unwrap_or(Timestamp::EPOCH);
    match outbox::first_seq_since(read, since)
        .await
        .map_err(store_error)?
    {
        Some(first) => Ok(first.saturating_sub(1)),
        None => events::head_seq(read).await.map_err(store_error),
    }
}

async fn facts(
    handle: &ProjectHandle,
    lookup: Lookup,
    kind: NoticeType,
) -> Result<EventFacts, FoldError> {
    let mut facts = EventFacts::default();
    if let Some(task) = lookup.task
        && let Some(row) = tasks::get_by_number(handle.store.read(), task)
            .await
            .map_err(store_error)?
    {
        facts.task_title = Some(row.title.as_str().to_owned());
        if kind == NoticeType::TaskCreated {
            facts.task_status = Some(row.status);
        }
    }
    if let Some(request) = lookup.request {
        let mut conn = handle
            .store
            .read()
            .acquire()
            .await
            .map_err(|error| store_error(error.into()))?;
        facts.request_title = tasks::requests::fetch(&mut conn, request)
            .await
            .map_err(store_error)?
            .and_then(|row| row.title);
    }
    Ok(facts)
}

/// Queues the notices of the stored events after the cursor, one write per page with the cursor move. Returns the cursor after the last page.
async fn queue_events(
    handle: &ProjectHandle,
    active: &[Applying],
    links: &Links,
    pass: &mut NotifyPass,
) -> Result<u64, FoldError> {
    let mut cursor = start_cursor(handle, active).await?;
    let moved = outbox::cursor(handle.store.read())
        .await
        .map_err(store_error)?
        != Some(cursor);
    let mut first = moved;
    loop {
        let page = events::read_after(handle.store.read(), cursor, &EventFilter::default(), PAGE)
            .await
            .map_err(store_error)?;
        let Some(last) = page.last().map(|event| event.seq) else {
            if first {
                handle
                    .store
                    .write(async |tx| outbox::set_cursor(tx, cursor).await)
                    .await
                    .map_err(store_error)?;
            }
            return Ok(cursor);
        };
        first = false;
        let mut rows: Vec<(NotifyTargetId, u64, String, Timestamp)> = Vec::new();
        for (index, event) in page.iter().enumerate() {
            let Some((kind, lookup)) = classify(event) else {
                continue;
            };
            let wanting: Vec<&Applying> = active
                .iter()
                .filter(|applying| {
                    applying.target.filter.wants_type(kind)
                        && applying.target.created_at <= event.created_at
                })
                .collect();
            if wanting.is_empty() {
                continue;
            }
            let facts = facts(handle, lookup, kind).await?;
            let cause = (event.event_type == types::TASK_STATUS_CHANGED)
                .then(|| cause_of(event, &page[..index]))
                .flatten();
            let Some(built) = notice(kind, event, &facts, cause.as_ref(), links) else {
                continue;
            };
            let json = serialise(&built)?;
            for applying in wanting {
                if applying.target.filter.matches(&built) {
                    rows.push((
                        applying.target.id,
                        event.seq,
                        json.clone(),
                        event.created_at,
                    ));
                }
            }
        }
        handle
            .store
            .write(async |tx| {
                for (target, seq, notice, at) in &rows {
                    outbox::queue(tx, *target, *seq, notice, None, *at).await?;
                }
                outbox::set_cursor(tx, last).await
            })
            .await
            .map_err(store_error)?;
        pass.queued += rows.len() as u64;
        cursor = last;
        if page.len() < PAGE {
            return Ok(cursor);
        }
    }
}

fn serialise(notice: &Notice) -> Result<String, FoldError> {
    serde_json::to_string(notice)
        .map_err(|_| FoldError::new(codes::INTERNAL_ERROR, "a notice did not serialise"))
}

/// Compares the open Needs you items with those the notifier saw before: a new item queues `needs_you.opened` to each target added before it came, a closed one `needs_you.closed` to each target added before it was seen, except that an item that closes while its `needs_you.opened` still waits for the send takes that notice back and sends no close either. Until the folder's first look since the project opened, a missing branch or folder item may only be unseen yet, so its row stays.
async fn queue_needs_you(
    handle: &ProjectHandle,
    settings: &NotifySettings,
    active: &[Applying],
    links: &Links,
    seq: u64,
    pass: &mut NotifyPass,
) -> Result<(), FoldError> {
    let items = with_notify_attention(
        with_folder_attention(
            handle.agents().needs_you().await.map_err(FoldError::from)?,
            handle.git_attention.head_moved(),
            handle.git_attention.folder_held(),
        ),
        off_items(settings, &handle.slug).await?,
    );
    let mut open = BTreeMap::new();
    for item in items {
        open.entry(item_key(&item)).or_insert(item);
    }
    let unsure: &[&str] = if handle.git_attention.observed() {
        &[]
    } else {
        &[
            NeedsYouKind::HeadMoved.as_str(),
            NeedsYouKind::FolderHeld.as_str(),
        ]
    };
    let seen = outbox::seen_items(handle.store.read())
        .await
        .map_err(store_error)?;
    let known: BTreeSet<&str> = seen.iter().map(|item| item.key.as_str()).collect();
    let now = handle.clock.now();
    let mut added = Vec::new();
    let mut rows: Vec<(NotifyTargetId, String, String)> = Vec::new();
    for (key, item) in &open {
        if known.contains(key.as_str()) {
            continue;
        }
        let shown = notice_item(item);
        let built = serialise(&needs_you_notice(&shown, true, seq, now, links))?;
        for applying in active {
            if applying
                .target
                .filter
                .wants_type(NoticeType::NeedsYouOpened)
                && item.since >= applying.target.created_at
            {
                rows.push((applying.target.id, built.clone(), key.clone()));
            }
        }
        added.push(SeenItem {
            key: key.clone(),
            kind: shown.kind,
            item: shown.item,
            path: shown.path,
            seen_at: now,
        });
    }
    let mut removed = Vec::new();
    let mut closes: Vec<(NotifyTargetId, String, Option<String>)> = Vec::new();
    for item in &seen {
        let kind = item
            .key
            .split_once(':')
            .map_or(item.key.as_str(), |(kind, _)| kind);
        if open.contains_key(&item.key) || unsure.contains(&kind) {
            continue;
        }
        let shown = NeedsYouNotice {
            kind: item.kind.clone(),
            item: item.item.clone(),
            title: String::new(),
            humans: Vec::new(),
            path: item.path.clone(),
        };
        let built = serialise(&needs_you_notice(&shown, false, seq, now, links))?;
        for applying in active {
            let wanted = applying
                .target
                .filter
                .wants_type(NoticeType::NeedsYouClosed)
                && item.seen_at >= applying.target.created_at;
            closes.push((
                applying.target.id,
                item.key.clone(),
                wanted.then(|| built.clone()),
            ));
        }
        removed.push(item.key.clone());
    }
    if added.is_empty() && removed.is_empty() {
        return Ok(());
    }
    let queued = handle
        .store
        .write(async |tx| {
            let mut queued = 0_u64;
            for item in &added {
                outbox::add_seen(tx, item).await?;
            }
            for (target, notice, key) in &rows {
                outbox::queue(tx, *target, seq, notice, Some(key), now).await?;
                queued += 1;
            }
            for key in &removed {
                outbox::remove_seen(tx, key).await?;
            }
            for (target, key, notice) in &closes {
                if outbox::take_back_opened(tx, *target, key).await? {
                    queued = queued.saturating_sub(1);
                    continue;
                }
                if let Some(notice) = notice {
                    outbox::queue(tx, *target, seq, notice, None, now).await?;
                    queued += 1;
                }
            }
            Ok::<_, StoreError>(queued)
        })
        .await
        .map_err(store_error)?;
    pass.queued += queued;
    Ok(())
}

/// Makes and tries the sends of one target that are due: its oldest open send first, so its notices go out in the order of the events, else one new send of the notices that waited a whole window, at most [`MAX_NOTICES_PER_SEND`], or one alone for a window of 0.
async fn send_due(
    handle: &ProjectHandle,
    settings: &NotifySettings,
    applying: &Applying,
    links: &Links,
    pass: &mut NotifyPass,
) -> Result<(), FoldError> {
    let target = &applying.target;
    for _ in 0..SENDS_PER_PASS {
        let now = handle.clock.now();
        let open = outbox::open_sends(handle.store.read())
            .await
            .map_err(store_error)?
            .into_iter()
            .find(|send| send.target == target.id);
        if let Some(send) = open {
            if send.state == SendState::Sending {
                return Ok(());
            }
            match send.next_try_at {
                Some(at) if at > now => {
                    pass.due(at);
                    return Ok(());
                }
                _ => {}
            }
            let Some(tried) = attempt(handle, settings, applying, &send).await? else {
                return Ok(());
            };
            if tried.judgement == fold_core::projects::notify::Judgement::Delivered {
                pass.sent += 1;
            }
            if !tried.active {
                return Ok(());
            }
            continue;
        }
        let first = outbox::waiting(handle.store.read())
            .await
            .map_err(store_error)?
            .into_iter()
            .find(|(waiting, _)| *waiting == target.id)
            .map(|(_, first)| first);
        let Some(first) = first else {
            return Ok(());
        };
        let due = due_at(first, target.window());
        if due > now {
            pass.due(due);
            return Ok(());
        }
        let limit = if target.window_seconds == 0 {
            1
        } else {
            MAX_NOTICES_PER_SEND
        };
        let rows = outbox::outbox_of(handle.store.read(), target.id, limit)
            .await
            .map_err(store_error)?;
        let notices = rows
            .iter()
            .map(|row| serde_json::from_str::<Notice>(&row.notice))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| from_store(StoreError::Corrupt("notify_outbox.notice")))?;
        let body = body(target.kind, &links.project(&handle.name), &notices, now)?;
        let summary =
            serde_json::to_string(&notices.iter().map(Notice::summary).collect::<Vec<_>>())
                .map_err(|_| {
                    FoldError::new(codes::INTERNAL_ERROR, "the summary did not serialise")
                })?;
        let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
        let prefix = webhook_id_prefix(&handle.slug)?;
        let count = u32::try_from(notices.len()).unwrap_or(u32::MAX);
        handle
            .store
            .write(async |tx| {
                outbox::create_send(
                    tx,
                    &NewSend {
                        target: target.id,
                        webhook_id_prefix: &prefix,
                        body: &body,
                        summary: &summary,
                        notices: count,
                        test: false,
                        outbox: &ids,
                    },
                    now,
                )
                .await
            })
            .await
            .map_err(store_error)?;
    }
    pass.due(handle.clock.now());
    Ok(())
}

/// A turn of a worker ended, which can add or end the item of a worker whose turns fail before they reach the model.
fn ends_turn(event: &LiveEvent) -> bool {
    event.event_type == types::WORKER_OUTPUT
        && matches!(
            event.data.get("step").and_then(serde_json::Value::as_str),
            Some("result" | "result_error")
        )
}

/// Runs [`notify_pass`] for `handle` after committed events, ended turns and a person's change of a target, at most once per `tick`, and at each tick at which a send is due by the project's clock. Returns the project's slug once its project closes or the job stops.
pub async fn notify_project(
    handle: Arc<ProjectHandle>,
    settings: NotifySettings,
    tick: Duration,
    progress: Arc<watch::Sender<JobProgress>>,
    mut stop: watch::Receiver<bool>,
) -> String {
    let mut events = handle.bus.subscribe();
    let mut closing = handle.bus.closing();
    let wake = settings.waker(&handle.slug);
    let mut ticks = tokio::time::interval(tick);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut state = PassState::default();
    let mut dirty = true;
    let mut due: Option<Timestamp> = None;
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = closing.wait_for(|closed| *closed) => break,
            () = wake.notified() => {
                dirty = true;
                continue;
            }
            received = events.recv() => {
                match received {
                    Ok(BusMessage::Durable(_)) | Err(RecvError::Lagged(_)) => dirty = true,
                    Ok(BusMessage::Live(event)) if ends_turn(&event) => dirty = true,
                    Ok(BusMessage::Live(_)) => {}
                    Err(RecvError::Closed) => break,
                }
                continue;
            }
            _ = ticks.tick() => {}
        }
        let now = handle.clock.now();
        if !dirty && due.is_none_or(|due| now < due) {
            continue;
        }
        dirty = false;
        match notify_pass(&handle, &settings, &mut state).await {
            Ok(pass) => {
                due = pass.next_due;
                progress.send_modify(|seen| {
                    seen.checks += 1;
                    seen.cycles += pass.sent;
                });
            }
            Err(error) => {
                tracing::warn!(project = %handle.slug, code = error.code(), "the notifier skipped a pass");
                due = now.checked_add(RETRY_AFTER);
                progress.send_modify(|seen| seen.checks += 1);
            }
        }
    }
    handle.slug.clone()
}
