use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use fold_core::events::types;
use fold_core::projects::notify::{Notice, NoticeEvent, NoticeItem, send_at};
use fold_core::{FoldError, LiveEvent, Timestamp, codes};
use fold_store::projects::notify::{
    notify_state, record_notify_send, sync_notify_items, sync_notify_targets,
};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;

use super::deliver::deliver;
use super::items::{item_key, notice_item};
use super::settings::NotifySettings;
use super::{link_base, notice_project};
use crate::jobs::JobProgress;
use crate::projects::ProjectHandle;
use crate::services::agents::with_folder_attention;
use crate::services::error::from_store;
use crate::services::events::BusMessage;

/// How long a pass that failed holds back the next one, so a broken settings file does not fill the log.
const RETRY_AFTER: Duration = Duration::from_secs(60);

/// What one pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NotifyPass {
    /// The sends that reached their receiver.
    pub sent: u64,
    /// When the next send is due, if an item waits for one.
    pub next_due: Option<Timestamp>,
}

/// One look at the project's Needs you: records which items are open, makes the targets that apply now the known ones, and sends each target whose items are due one notice of all its pending items, oldest first. A pass with no target that applies reads and writes nothing more. A target that starts to apply counts as sent every open item that an earlier pass recorded or that came before the target was added, so it hears only of items that come after. A send that fails leaves its items pending until the interval after it. Every send is logged with the target's id and the item count, never a title, address or secret.
pub async fn notify_pass(
    handle: &ProjectHandle,
    settings: &NotifySettings,
    listen: SocketAddr,
) -> Result<NotifyPass, FoldError> {
    let (targets, _) = settings.applying(&handle.slug).await?;
    let mut pass = NotifyPass::default();
    if targets.is_empty() {
        return Ok(pass);
    }
    let items = with_folder_attention(
        handle.agents().needs_you().await.map_err(FoldError::from)?,
        handle.git_attention.head_moved(),
        handle.git_attention.folder_held(),
    );
    let mut open = BTreeMap::new();
    for item in items {
        open.entry(item_key(&item)).or_insert(item);
    }
    let keys: BTreeSet<String> = open.keys().cloned().collect();
    let now = handle.clock.now();
    handle
        .store
        .write(async |tx| {
            let recorded = sync_notify_items(tx, &keys, now).await?;
            let applying = targets
                .iter()
                .map(|target| {
                    let seen = open
                        .iter()
                        .filter(|(key, item)| {
                            recorded.contains(*key) || item.since < target.created_at
                        })
                        .map(|(key, _)| key.clone())
                        .collect();
                    (target.id, seen)
                })
                .collect();
            sync_notify_targets(tx, &applying).await
        })
        .await
        .map_err(from_store)?;
    let mut conn = handle
        .store
        .read()
        .acquire()
        .await
        .map_err(|error| from_store(error.into()))?;
    let state = notify_state(&mut conn).await.map_err(from_store)?;
    drop(conn);
    let base = link_base(settings, listen).await?;
    for target in targets {
        let mut pending: Vec<(&String, Timestamp)> = state
            .items
            .iter()
            .filter(|(key, _)| !state.sent.contains(&(target.id, (*key).clone())))
            .map(|(key, seen)| (key, *seen))
            .collect();
        let Some(oldest) = pending.iter().map(|(_, seen)| *seen).min() else {
            continue;
        };
        let last = state.targets.get(&target.id).copied().flatten();
        let due = send_at(oldest, last, target.interval());
        if now < due {
            pass.next_due = Some(pass.next_due.map_or(due, |next| next.min(due)));
            continue;
        }
        pending.sort_by_key(|(key, seen)| (open.get(*key).map(|item| item.since), *seen));
        let notice_items: Vec<NoticeItem> = pending
            .iter()
            .filter_map(|(key, _)| open.get(*key))
            .map(|item| notice_item(&base, &handle.slug, item))
            .collect();
        let sent_keys: Vec<String> = pending.iter().map(|(key, _)| (*key).clone()).collect();
        let notice = Notice {
            event: NoticeEvent::NeedsYou,
            project: Some(notice_project(&base, &handle.slug, &handle.name)),
            items: notice_items,
            sent_at: now.as_millis(),
        };
        let channel = target.channel.clone();
        let delivered = tokio::task::spawn_blocking(move || deliver(&channel, &notice))
            .await
            .map_err(|_| FoldError::new(codes::INTERNAL_ERROR, "a notice send stopped"))?;
        match &delivered {
            Ok(()) => {
                pass.sent += 1;
                tracing::info!(project = %handle.slug, notify_target = %target.id, items = sent_keys.len(), "a notice went out");
            }
            Err(error) => {
                tracing::warn!(project = %handle.slug, notify_target = %target.id, code = error.code(), items = sent_keys.len(), "a notice was not sent; the interval holds back the next try");
                if let Some(retry) = now.checked_add(target.interval()) {
                    pass.next_due = Some(pass.next_due.map_or(retry, |next| next.min(retry)));
                }
            }
        }
        handle
            .store
            .write(async |tx| {
                record_notify_send(tx, target.id, &sent_keys, delivered.is_ok(), now).await
            })
            .await
            .map_err(from_store)?;
    }
    Ok(pass)
}

/// A turn of a worker ended, which can add or end the item of a worker whose turns fail before they reach the model.
fn ends_turn(event: &LiveEvent) -> bool {
    event.event_type == types::WORKER_OUTPUT
        && matches!(
            event.data.get("step").and_then(serde_json::Value::as_str),
            Some("result" | "result_error")
        )
}

/// Runs [`notify_pass`] for `handle` after each committed event or ended turn, at most once per `tick`, and at each tick at which a send is due by the project's clock. Returns the project's slug once its project closes or the job stops.
pub async fn notify_project(
    handle: Arc<ProjectHandle>,
    settings: NotifySettings,
    listen: SocketAddr,
    tick: Duration,
    progress: Arc<watch::Sender<JobProgress>>,
    mut stop: watch::Receiver<bool>,
) -> String {
    let mut events = handle.bus.subscribe();
    let mut closing = handle.bus.closing();
    let mut ticks = tokio::time::interval(tick);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut dirty = true;
    let mut due: Option<Timestamp> = None;
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = closing.wait_for(|closed| *closed) => break,
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
        match notify_pass(&handle, &settings, listen).await {
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
