use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use fold_core::agent_changes::{ToolWindow, agents_at};
use fold_core::events::types;
use fold_core::folder::unrecorded_changes;
use fold_core::tree::FlatTree;
use fold_core::{Actor, AgentName, FoldError, NewEvent, Timestamp, codes, policy};
use fold_store::workspaces::{
    AgentChangeEnd, clear_agent_change, insert_agent_change, open_agent_changes,
};
use fold_store::{Store, StoreError};
use fold_vcs::paths::{RepoPath, changed_time};
use serde_json::json;

use super::ProjectError;
use crate::projects::ProjectHandle;
use crate::services::agents::starter_of;
use crate::services::events::{EventBuffer, EventBus};
use crate::support::lock;

/// The most paths an event of a worker's change lists; the rest are counted.
const LISTED_PATHS: usize = 50;

/// How long an ended tool call still places a change: a change is found at the next follow of the project folder, which the end of a call starts within a second, or after an integration that may come later.
const KEPT: Duration = Duration::from_secs(10 * 60);

/// The most ended tool calls kept per project, so a busy project keeps a bounded list.
const MAX_ENDED: usize = 4096;

/// Now by the system clock. File times come from that clock, never from the project's [`fold_core::Clock`], which tests set by hand, so the tool calls that a file time is placed in are timed on it too.
pub(crate) fn file_clock_now() -> Timestamp {
    Timestamp::from_system_time(SystemTime::now()).unwrap_or(Timestamp::EPOCH)
}

/// The tool calls of the project's supervised workers that can write files: those that run and those that ended in the last [`KEPT`]. They live in memory only, so a restarted daemon places no change in a call made before it started.
#[derive(Debug, Default)]
pub struct ToolWindows {
    calls: Mutex<Calls>,
    /// A call ended since the Git job last looked, so the job follows the project folder at its next tick.
    ended: AtomicBool,
}

#[derive(Debug, Default)]
struct Calls {
    /// By agent and Claude Code's id of the call.
    running: BTreeMap<(AgentName, String), Timestamp>,
    ended: VecDeque<ToolWindow>,
}

impl ToolWindows {
    pub fn open(&self, agent: &AgentName, call: &str, at: Timestamp) {
        lock(&self.calls)
            .running
            .entry((agent.clone(), call.to_owned()))
            .or_insert(at);
    }

    /// Ends the call `call` of `agent`, when it runs.
    pub fn close(&self, agent: &AgentName, call: &str, at: Timestamp) {
        let mut calls = lock(&self.calls);
        if let Some(start) = calls.running.remove(&(agent.clone(), call.to_owned())) {
            calls.push_ended(ToolWindow {
                agent: agent.clone(),
                start,
                end: Some(at),
            });
            self.ended.store(true, Ordering::SeqCst);
        }
    }

    /// Ends every running call of `agent`, whose process ended.
    pub fn close_all(&self, agent: &AgentName, at: Timestamp) {
        let mut calls = lock(&self.calls);
        let open: Vec<(AgentName, String)> = calls
            .running
            .keys()
            .filter(|(owner, _)| owner == agent)
            .cloned()
            .collect();
        for key in open {
            if let Some(start) = calls.running.remove(&key) {
                calls.push_ended(ToolWindow {
                    agent: agent.clone(),
                    start,
                    end: Some(at),
                });
                self.ended.store(true, Ordering::SeqCst);
            }
        }
    }

    /// The calls that run and those that ended since `now` less [`KEPT`].
    pub fn windows(&self, now: Timestamp) -> Vec<ToolWindow> {
        let mut calls = lock(&self.calls);
        let oldest = now.checked_sub(KEPT).unwrap_or(Timestamp::EPOCH);
        calls
            .ended
            .retain(|window| window.end.is_none_or(|end| end >= oldest));
        calls
            .running
            .iter()
            .map(|((agent, _), start)| ToolWindow {
                agent: agent.clone(),
                start: *start,
                end: None,
            })
            .chain(calls.ended.iter().cloned())
            .collect()
    }

    /// Whether a call ended since the last look, which this look clears.
    pub fn take_ended(&self) -> bool {
        self.ended.swap(false, Ordering::SeqCst)
    }
}

impl Calls {
    fn push_ended(&mut self, window: ToolWindow) {
        if self.ended.len() == MAX_ENDED {
            self.ended.pop_front();
        }
        self.ended.push_back(window);
    }
}

/// A path where the project folder differs from its checkout record, with the time that places the change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeenChange {
    pub path: Vec<u8>,
    /// See [`changed_time`]; `None` when no time could be read.
    pub changed_at: Option<Timestamp>,
}

/// The paths where `folder`, as read under `root`, differs from `record`, with the time of each. Blocking.
pub(crate) fn seen_changes(root: &Path, record: &FlatTree, folder: &FlatTree) -> Vec<SeenChange> {
    unrecorded_changes(record, folder)
        .into_iter()
        .map(|change| {
            let changed_at = RepoPath::parse(&change.path)
                .ok()
                .and_then(|path| changed_time(root, &path))
                .and_then(|time| Timestamp::from_system_time(time).ok());
            SeenChange {
                path: change.path,
                changed_at,
            }
        })
        .collect()
}

fn names(paths: &BTreeSet<Vec<u8>>) -> Vec<String> {
    paths
        .iter()
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect()
}

/// The `folder.agent_change_cleared` event of `paths`, ended as `how`.
fn cleared_event(
    actor: &Actor,
    how: AgentChangeEnd,
    paths: &BTreeSet<Vec<u8>>,
) -> Result<NewEvent, FoldError> {
    let listed = names(paths);
    NewEvent::new(types::FOLDER_AGENT_CHANGE_CLEARED, actor.event_actor())
        .and_then(|event| {
            event.data(json!({
                "how": how.as_str(),
                "paths": listed.iter().take(LISTED_PATHS).collect::<Vec<_>>(),
                "path_count": listed.len(),
            }))
        })
        .map_err(|error| FoldError::new(codes::INTERNAL_ERROR, error.to_string()))
}

/// Notes, after a follow of the project folder read `seen`, the paths that changed during a tool call of a supervised worker as that worker's, with `folder.agent_changed`, and ends as `settled`, with `folder.agent_change_cleared`, those whose path the folder holds as its checkout record again. One transaction holds both. The caller holds the folder lock. A failure is logged and changes nothing, so the follow and the commits it leads to go on: the change then shows as the human's, as it would without the note.
pub(crate) async fn note_agent_changes(
    store: &Store,
    bus: &EventBus,
    windows: &ToolWindows,
    seen: &[SeenChange],
    now: Timestamp,
) {
    if let Err(error) = note(store, bus, windows, seen, now).await {
        tracing::warn!(
            code = error.code().unwrap_or(codes::INTERNAL_ERROR),
            "the changes of workers in the project folder were not noted"
        );
    }
}

async fn note(
    store: &Store,
    bus: &EventBus,
    windows: &ToolWindows,
    seen: &[SeenChange],
    now: Timestamp,
) -> Result<(), ProjectError> {
    let calls = windows.windows(file_clock_now());
    let open = open_agent_changes(store.read()).await?;
    if calls.is_empty() && open.is_empty() {
        return Ok(());
    }
    let present: BTreeSet<&[u8]> = seen.iter().map(|change| change.path.as_slice()).collect();
    let settled: BTreeSet<Vec<u8>> = open
        .iter()
        .filter(|row| !present.contains(row.path.as_slice()))
        .map(|row| row.path.clone())
        .collect();
    let known: BTreeSet<(&[u8], &AgentName)> = open
        .iter()
        .map(|row| (row.path.as_slice(), &row.agent))
        .collect();
    let mut found: Vec<(&[u8], AgentName, Timestamp)> = Vec::new();
    for change in seen {
        let Some(at) = change.changed_at else {
            continue;
        };
        for agent in agents_at(&calls, at) {
            if !known.contains(&(change.path.as_slice(), &agent)) {
                found.push((change.path.as_slice(), agent, at));
            }
        }
    }
    if settled.is_empty() && found.is_empty() {
        return Ok(());
    }
    let settled_event = (!settled.is_empty())
        .then(|| cleared_event(&Actor::System, AgentChangeEnd::Settled, &settled))
        .transpose()?;
    let buffer = store
        .write(async |tx| -> Result<EventBuffer, ProjectError> {
            let mut buffer = EventBuffer::new();
            for path in &settled {
                clear_agent_change(tx, path, AgentChangeEnd::Settled, now).await?;
            }
            if let Some(event) = &settled_event {
                buffer.append(tx, event, now).await?;
            }
            let mut agents = BTreeSet::new();
            let mut paths = BTreeSet::new();
            for (path, agent, at) in &found {
                if insert_agent_change(tx, path, agent, *at, now).await? {
                    agents.insert(agent.as_str().to_owned());
                    paths.insert(path.to_vec());
                }
            }
            if !paths.is_empty() {
                let listed = names(&paths);
                let event = NewEvent::new(types::FOLDER_AGENT_CHANGED, Actor::System.event_actor())
                    .and_then(|event| {
                        event.data(json!({
                            "agents": agents,
                            "paths": listed.iter().take(LISTED_PATHS).collect::<Vec<_>>(),
                            "path_count": listed.len(),
                        }))
                    })
                    .map_err(|error| FoldError::new(codes::INTERNAL_ERROR, error.to_string()))?;
                buffer.append(tx, &event, now).await?;
                tracing::info!(
                    paths = listed.len(),
                    agents = agents.len(),
                    "the project folder changed during a worker's tool call"
                );
            }
            Ok(buffer)
        })
        .await?;
    bus.publish(buffer);
    Ok(())
}

/// A path of the project folder that changed during the tool calls of `agents`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentChange {
    pub path: String,
    /// In name order.
    pub agents: Vec<AgentName>,
    /// When fold first saw it.
    pub since: Timestamp,
}

/// The open changes of workers in the project folder, in path order.
pub async fn agent_changes(store: &Store) -> Result<Vec<AgentChange>, StoreError> {
    let mut found: Vec<AgentChange> = Vec::new();
    for row in open_agent_changes(store.read()).await? {
        let path = String::from_utf8_lossy(&row.path).into_owned();
        match found.last_mut() {
            Some(last) if last.path == path => {
                last.agents.push(row.agent);
                last.since = last.since.min(row.seen_at);
            }
            _ => found.push(AgentChange {
                path,
                agents: vec![row.agent],
                since: row.seen_at,
            }),
        }
    }
    Ok(found)
}

/// Who `actor` is for the rights to settle a worker's change, read from its worker process.
pub(crate) async fn starter(
    handle: &ProjectHandle,
    actor: &Actor,
) -> Result<policy::Starter, ProjectError> {
    let supervised = match actor {
        Actor::Agent(agent) => {
            let mut conn = handle
                .store
                .read()
                .acquire()
                .await
                .map_err(StoreError::from)?;
            fold_store::agents::find_worker_process(&mut conn, &agent.agent)
                .await?
                .map(|row| (row.role, row.area))
        }
        Actor::Human { .. } | Actor::System => None,
    };
    Ok(policy::Starter::of(actor, supervised))
}

/// [`agent_changes`] for an agent that may settle them (`policy::can_settle_agent_changes`): the main agent reads them for the human, and no other agent learns which of its files fold holds back.
pub async fn list_agent_changes(
    handle: &ProjectHandle,
    actor: &Actor,
) -> Result<Vec<AgentChange>, ProjectError> {
    policy::can_settle_agent_changes(&starter(handle, actor).await?).map_err(FoldError::from)?;
    Ok(agent_changes(&handle.store).await?)
}

/// There is no open change of a worker to settle.
pub(crate) fn no_agent_change() -> FoldError {
    FoldError::new(
        codes::AGENT_CHANGE_NOT_FOUND,
        "fold found no change a worker made in the project folder",
    )
    .with_hint("run `fld status` to see the project folder's changes")
}

/// Ends the open changes of workers in the project folder as `how`, with `folder.agent_change_cleared`, in one transaction that first checks that `actor` may settle them (`policy::can_settle_agent_changes`). With `paths` only those end. Returns the paths that ended, in path order; refused with `agent_change_not_found` when none was open and `required` is set.
pub(crate) async fn end_agent_changes(
    handle: &ProjectHandle,
    actor: &Actor,
    how: AgentChangeEnd,
    paths: Option<&BTreeSet<Vec<u8>>>,
    required: bool,
) -> Result<Vec<String>, ProjectError> {
    let now = handle.clock.now();
    let (buffer, ended) = handle
        .store
        .write(
            async |tx| -> Result<(EventBuffer, Vec<String>), ProjectError> {
                let starter = starter_of(tx, actor).await?;
                policy::can_settle_agent_changes(&starter).map_err(FoldError::from)?;
                let open: BTreeSet<Vec<u8>> = open_agent_changes(tx.conn())
                    .await?
                    .into_iter()
                    .map(|row| row.path)
                    .filter(|path| paths.is_none_or(|paths| paths.contains(path)))
                    .collect();
                let mut buffer = EventBuffer::new();
                if open.is_empty() {
                    if required {
                        return Err(no_agent_change().into());
                    }
                    return Ok((buffer, Vec::new()));
                }
                for path in &open {
                    clear_agent_change(tx, path, how, now).await?;
                }
                buffer
                    .append(tx, &cleared_event(actor, how, &open)?, now)
                    .await?;
                Ok((buffer, names(&open)))
            },
        )
        .await?;
    handle.bus.publish(buffer);
    if !ended.is_empty() {
        tracing::info!(
            paths = ended.len(),
            how = how.as_str(),
            "changes a worker made in the project folder are settled"
        );
    }
    Ok(ended)
}

/// Lets the changes that workers made in the project folder count as the human's own unrecorded changes, which `fld record` records or `fld stash` puts aside as any other. For a human, or the main agent when the human asks.
pub async fn ignore_agent_changes(
    handle: &ProjectHandle,
    actor: &Actor,
) -> Result<Vec<String>, ProjectError> {
    end_agent_changes(handle, actor, AgentChangeEnd::Ignored, None, true).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis).unwrap()
    }

    #[test]
    fn a_call_counts_while_it_runs_and_for_a_while_after_it_ended() {
        let windows = ToolWindows::default();
        let agent = AgentName::parse("reviewer-35").unwrap();
        windows.open(&agent, "toolu_1", at(1_000));
        windows.open(&agent, "toolu_1", at(5_000));
        assert!(!windows.take_ended());
        assert_eq!(windows.windows(at(2_000))[0].end, None);
        assert_eq!(windows.windows(at(2_000))[0].start, at(1_000));
        windows.close(&agent, "toolu_unknown", at(3_000));
        assert!(!windows.take_ended());
        windows.close(&agent, "toolu_1", at(3_000));
        assert!(windows.take_ended());
        assert!(!windows.take_ended());
        assert_eq!(
            windows.windows(at(4_000)),
            [ToolWindow {
                agent: agent.clone(),
                start: at(1_000),
                end: Some(at(3_000)),
            }]
        );
        let later = at(3_000).checked_add(KEPT).unwrap();
        assert_eq!(windows.windows(later).len(), 1);
        assert!(
            windows
                .windows(later.checked_add(Duration::from_millis(1)).unwrap())
                .is_empty()
        );
        windows.open(&agent, "toolu_2", at(10_000));
        windows.open(&agent, "toolu_3", at(10_000));
        windows.close_all(&agent, at(11_000));
        assert!(windows.take_ended());
        assert!(
            windows
                .windows(at(12_000))
                .iter()
                .all(|window| window.end == Some(at(11_000)))
        );
    }
}
