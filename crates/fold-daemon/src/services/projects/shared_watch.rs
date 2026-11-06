use std::collections::{BTreeMap, BTreeSet};

use fold_core::agent_changes::agents_at;
use fold_core::events::types;
use fold_core::{AgentName, EventActor, NewEvent, Timestamp, codes};
use fold_vcs::paths::RepoPath;
use fold_vcs::shared::{FileStamp, file_hash, read_shared, shared_files};
use serde_json::json;

use super::ProjectError;
use super::agent_changes::file_clock_now;
use super::remote::RemoteContext;
use crate::services::events::EventBuffer;

/// The most shared files fold watches in one project; the rest go unwatched.
const MAX_WATCHED: usize = 2000;

/// A file up to this size gets its hash when fold first notes it, so a later rewrite with the same bytes is no change; a larger one is hashed only once its stamp changed.
const HASHED_AT_ONCE: u64 = 1 << 20;

/// The most paths a `shared.changed` event lists.
const LISTED_PATHS: usize = 50;

#[derive(Debug, Clone)]
struct Seen {
    stamp: FileStamp,
    hash: Option<[u8; 32]>,
}

/// What fold last noted of the shared files: each file's size, modification time and, where known, content hash, with the shared paths of that look. A file of a newly shared path counts as changed only when its time is later than the list's last change. It lives in memory, so the first look after a daemon start only notes.
#[derive(Debug, Default)]
pub(crate) struct SharedWatch {
    seen: Option<(BTreeSet<String>, BTreeMap<String, Seen>)>,
}

impl SharedWatch {
    /// Reads the shared files again and records each change whose time falls in a supervised worker's tool call as that worker's, with `shared.changed`, so Needs you names it. A change no call covers is a person's or the main agent's, and only moves what fold noted. A failure is logged and changes nothing.
    pub(crate) async fn look(&mut self, context: &RemoteContext) {
        let paths = context.paths.clone();
        let previous = self.seen.take();
        let looked = tokio::task::spawn_blocking(move || {
            let root = paths.root();
            let shared = read_shared(&paths.fold_dir()).unwrap_or_default();
            let listed_at = std::fs::metadata(paths.fold_dir().join(fold_vcs::shared::SHARED))
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| Timestamp::from_system_time(time).ok());
            let roots: BTreeSet<String> = shared.iter().map(RepoPath::to_string).collect();
            let files = shared_files(root, &shared, MAX_WATCHED);
            let mut seen = BTreeMap::new();
            let mut changed: Vec<(String, Timestamp)> = Vec::new();
            let watched = |path: &str, earlier: &BTreeSet<String>| {
                earlier
                    .iter()
                    .any(|share| path == share || path.starts_with(&format!("{share}/")))
            };
            for (path, stamp) in files {
                let old = previous.as_ref().and_then(|(_, seen)| seen.get(&path));
                let entry = match old {
                    Some(old) if old.stamp == stamp => old.clone(),
                    Some(old) => {
                        let hash = file_hash(&root.join(&path));
                        if hash.is_none() || old.hash.is_none() || hash != old.hash {
                            changed.push((path.clone(), stamp_time(stamp)));
                        }
                        Seen { stamp, hash }
                    }
                    // A file fold has not noted yet, below a path shared earlier or one shared since the last look, changed when its time is later than the list's last change.
                    None => {
                        let at = stamp_time(stamp);
                        if previous.is_some() && listed_at.is_some_and(|listed| at > listed) {
                            changed.push((path.clone(), at));
                        }
                        let hash = (stamp.size <= HASHED_AT_ONCE)
                            .then(|| file_hash(&root.join(&path)))
                            .flatten();
                        Seen { stamp, hash }
                    }
                };
                seen.insert(path, entry);
            }
            if let Some((earlier, before)) = &previous {
                let now = file_clock_now();
                for path in before.keys() {
                    if !seen.contains_key(path) && watched(path, earlier) && watched(path, &roots) {
                        changed.push((path.clone(), now));
                    }
                }
            }
            changed.sort();
            ((roots, seen), changed)
        })
        .await;
        let Ok((seen, changed)) = looked else {
            tracing::warn!("could not read the shared files");
            return;
        };
        self.seen = Some(seen);
        if changed.is_empty() {
            return;
        }
        let windows = context.tool_windows.windows(file_clock_now());
        let mut by_agent: BTreeMap<AgentName, Vec<String>> = BTreeMap::new();
        for (path, at) in changed {
            for agent in agents_at(&windows, at) {
                by_agent.entry(agent).or_default().push(path.clone());
            }
        }
        if by_agent.is_empty() {
            return;
        }
        let now = context.clock.now();
        let mut events = Vec::new();
        for (agent, paths) in &by_agent {
            let listed: Vec<&String> = paths.iter().take(LISTED_PATHS).collect();
            match NewEvent::new(types::SHARED_CHANGED, EventActor::fold()).and_then(|event| {
                event.data(
                    json!({"agent": agent.as_str(), "paths": listed, "path_count": paths.len()}),
                )
            }) {
                Ok(event) => events.push((agent, paths, event)),
                Err(_) => return,
            }
        }
        let written = context
            .store
            .write(async |tx| -> Result<EventBuffer, ProjectError> {
                let mut buffer = EventBuffer::new();
                for (agent, paths, event) in &events {
                    if fold_store::workspaces::insert_shared_change(tx, agent, paths, now)
                        .await?
                        .is_some()
                    {
                        buffer.append(tx, event, now).await?;
                    }
                }
                Ok(buffer)
            })
            .await;
        match written {
            Ok(buffer) => {
                tracing::info!(
                    agents = by_agent.len(),
                    "shared files changed during a worker's tool call"
                );
                context.bus.publish(buffer);
            }
            Err(error) => {
                tracing::warn!(
                    code = error.code().unwrap_or(codes::INTERNAL_ERROR),
                    "could not record a change of shared files"
                );
            }
        }
    }
}

/// The time that places a change: the file's modification time, else now.
fn stamp_time(stamp: FileStamp) -> Timestamp {
    stamp
        .modified
        .and_then(|time| Timestamp::from_system_time(time).ok())
        .unwrap_or_else(file_clock_now)
}
