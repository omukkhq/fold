use std::collections::BTreeMap;

use fold_core::Timestamp;
use fold_vcs::watch::DegradedReason;
use serde_json::Value;

use super::{Check, sysctl_fix};

pub(crate) const QUEUED_EVENTS_KEY: &str = "fs.inotify.max_queued_events";
pub(crate) const INSTANCES_KEY: &str = "fs.inotify.max_user_instances";
/// The kernel's default queue length. Each watched workspace has its own queue; a queue that overflows makes the watcher scan that whole workspace again, so a short queue costs time, never a change.
pub(crate) const MIN_QUEUED_EVENTS: u64 = 16_384;
pub(crate) const RECOMMENDED_QUEUED_EVENTS: u64 = 65_536;
/// The inotify instances doctor keeps free for the user's other programs, such as editors, file managers and the desktop session. The limit counts every instance of the user, and fold's daemon holds one for each watched workspace and no other.
pub(crate) const INSTANCE_MARGIN: u64 = 64;
pub(crate) const RECOMMENDED_INSTANCES: u64 = 1024;
/// What a degraded workspace does instead of following inotify.
const TIMER_SCAN: &str = "a workspace without an instance scans every 10 s and keeps working";

const QUEUE_CHECK: &str = "inotify queue";
const INSTANCES_CHECK: &str = "inotify instances";
const WITHOUT_INSTANCE_CHECK: &str = "watch instances";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueLimit {
    NotApplicable,
    Enough(u64),
    Low(u64),
}

pub(crate) fn queue_verdict(linux: bool, limit: Option<u64>) -> QueueLimit {
    match limit {
        Some(limit) if linux && limit < MIN_QUEUED_EVENTS => QueueLimit::Low(limit),
        Some(limit) if linux => QueueLimit::Enough(limit),
        _ => QueueLimit::NotApplicable,
    }
}

pub(crate) fn queue_check(linux: bool, limit: Option<u64>) -> Check {
    match queue_verdict(linux, limit) {
        QueueLimit::NotApplicable => Check::not_applicable(QUEUE_CHECK),
        QueueLimit::Enough(limit) => Check::ok(
            QUEUE_CHECK,
            format!("max_queued_events is {limit} for each workspace"),
        ),
        QueueLimit::Low(limit) => Check::warn(
            QUEUE_CHECK,
            format!(
                "max_queued_events is {limit}, below {MIN_QUEUED_EVENTS}; a workspace whose queue overflows is scanned again in full, so its changes show later"
            ),
            sysctl_fix(QUEUED_EVENTS_KEY, RECOMMENDED_QUEUED_EVENTS),
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstanceLimit {
    NotApplicable,
    Enough { limit: u64, needed: u64 },
    Low { limit: u64, needed: u64 },
}

/// The instances fold's daemon needs: one for each watched workspace, plus the margin for other programs. `watched` is `None` when the daemon does not answer, so only the margin counts.
pub(crate) fn instances_needed(watched: Option<u64>) -> u64 {
    watched.unwrap_or(0).saturating_add(INSTANCE_MARGIN)
}

pub(crate) fn instance_verdict(
    linux: bool,
    limit: Option<u64>,
    watched: Option<u64>,
) -> InstanceLimit {
    let needed = instances_needed(watched);
    match limit {
        Some(limit) if linux && limit < needed => InstanceLimit::Low { limit, needed },
        Some(limit) if linux => InstanceLimit::Enough { limit, needed },
        _ => InstanceLimit::NotApplicable,
    }
}

/// The value the fix names: the recommended number, or more when the workspaces need more.
pub(crate) fn recommended_instances(watched: Option<u64>) -> u64 {
    RECOMMENDED_INSTANCES.max(instances_needed(watched).next_power_of_two())
}

fn watched_words(watched: Option<u64>) -> String {
    match watched {
        Some(1) => "the daemon watches 1 workspace with 1 instance".to_owned(),
        Some(count) => format!("the daemon watches {count} workspaces with 1 instance each"),
        None => "the daemon does not answer, so doctor counts no watched workspaces".to_owned(),
    }
}

pub(crate) fn instances_check(linux: bool, limit: Option<u64>, watched: Option<u64>) -> Check {
    match instance_verdict(linux, limit, watched) {
        InstanceLimit::NotApplicable => Check::not_applicable(INSTANCES_CHECK),
        InstanceLimit::Enough { limit, needed } => Check::ok(
            INSTANCES_CHECK,
            format!(
                "max_user_instances is {limit}, at least the {needed} needed; {}",
                watched_words(watched)
            ),
        ),
        InstanceLimit::Low { limit, needed } => Check::warn(
            INSTANCES_CHECK,
            format!(
                "max_user_instances is {limit}, below {needed}: {} and {INSTANCE_MARGIN} stay free for other programs; {TIMER_SCAN}",
                watched_words(watched)
            ),
            sysctl_fix(INSTANCES_KEY, recommended_instances(watched)),
        ),
    }
}

/// The workspaces of the daemon's workspace list that it follows, by watch mode: `watching` or `degraded`. A workspace whose `watch` is null has no live state and holds no instance.
fn watch_modes(list: &Value) -> impl Iterator<Item = (&str, &str)> {
    list.get("workspaces")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| Some((entry.get("id")?.as_str()?, entry.get("watch")?.as_str()?)))
}

/// The number of workspaces the daemon follows in one project, each with one instance when it has its own.
pub(crate) fn watched_workspaces(list: &Value) -> u64 {
    let count = watch_modes(list).count();
    u64::try_from(count).unwrap_or(u64::MAX)
}

/// The workspaces of one project that the daemon scans on a timer now, such as `ws-9`.
pub(crate) fn degraded_workspaces(list: &Value) -> Vec<String> {
    watch_modes(list)
        .filter(|(_, mode)| *mode == "degraded")
        .map(|(id, _)| id.to_owned())
        .collect()
}

/// One stored `workspace.watch_degraded` event: the workspace, such as `ws-9`, its reason and its time, in the order the events were stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DegradedEvent {
    pub(crate) workspace: String,
    pub(crate) reason: Option<String>,
    pub(crate) at: Timestamp,
}

/// The degraded workspaces that have no instance: those whose newest degraded event since the daemon's start has reason `instance_limit`. A workspace degrades once for each start of the daemon, so an event from before `started_at` tells nothing of the watch now.
pub(crate) fn without_instance(
    degraded: &[String],
    events: &[DegradedEvent],
    started_at: Option<Timestamp>,
) -> Vec<String> {
    let mut newest: BTreeMap<&str, Option<&str>> = BTreeMap::new();
    for event in events
        .iter()
        .filter(|event| started_at.is_none_or(|start| event.at >= start))
    {
        newest.insert(&event.workspace, event.reason.as_deref());
    }
    let reason = DegradedReason::InstanceLimit.as_str();
    degraded
        .iter()
        .filter(|id| newest.get(id.as_str()).copied().flatten() == Some(reason))
        .cloned()
        .collect()
}

/// The project check that names the workspaces without an instance. `degraded` is `None` when the daemon gives no workspace list for the project.
pub(crate) fn without_instance_check(
    degraded: Option<&[String]>,
    events: &[DegradedEvent],
    started_at: Option<Timestamp>,
    recommended: u64,
) -> Check {
    let Some(degraded) = degraded else {
        return Check::ok(
            WITHOUT_INSTANCE_CHECK,
            "the daemon gives no workspace list, so doctor reads no watch state",
        );
    };
    let missing = without_instance(degraded, events, started_at);
    if missing.is_empty() {
        return Check::ok(
            WITHOUT_INSTANCE_CHECK,
            "no workspace lacks an inotify instance",
        );
    }
    Check::warn(
        WITHOUT_INSTANCE_CHECK,
        format!(
            "the system gave these workspaces no inotify instance, so they use the timer scan (instance_limit): {}; {TIMER_SCAN}",
            missing.join(", ")
        ),
        format!(
            "{}; the workspaces get an instance when the daemon starts again",
            sysctl_fix(INSTANCES_KEY, recommended)
        ),
    )
}
