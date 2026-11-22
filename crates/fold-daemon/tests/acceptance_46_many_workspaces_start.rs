#![cfg(test)]

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use crate::common::acceptance::{Harness, WAIT};
use fold_core::WorkspaceId;
use fold_daemon::live::{DEFAULT_WATCH_GAP, DEFAULT_WATCH_GROUP, WatchStart};
use fold_store::testing::eventually;
use fold_vcs::watch::{Notifier, WatchEvent, WatchHub};
use notify::event::{EventKind, Flag};

/// The native watcher of the project, with the events it sends and its notifier kept for the test.
#[derive(Clone, Default)]
struct Recorder {
    events: Arc<Mutex<Vec<WatchEvent>>>,
    notifier: Arc<OnceLock<Notifier>>,
}

impl Recorder {
    fn start(&self) -> WatchStart {
        let recorder = self.clone();
        WatchStart::custom(move |config, sink| {
            let events = Arc::clone(&recorder.events);
            let hub = WatchHub::start(config, move |event: WatchEvent| {
                events.lock().unwrap().push(event.clone());
                sink(event);
            });
            let _ = recorder.notifier.set(hub.notifier());
            hub
        })
    }

    /// How many rescans the watcher asked for, for each workspace.
    fn rescans(&self, workspace: WorkspaceId) -> usize {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| matches!(event, WatchEvent::RescanRequested { workspace: id } if *id == workspace))
            .count()
    }

    fn changed(&self, workspace: WorkspaceId) -> bool {
        self.events.lock().unwrap().iter().any(
            |event| matches!(event, WatchEvent::Changed { workspace: id, .. } if *id == workspace),
        )
    }

    /// Lost notifications of `workspace` alone, as its own inotify queue reports an overflow.
    fn overflow(&self, workspace: WorkspaceId) {
        let lost = notify::Event::new(EventKind::Other).set_flag(Flag::Rescan);
        self.notifier.get().unwrap().send_for(workspace, Ok(lost));
    }
}

/// Acceptance 46, many workspaces at a daemon start: `frontend` and `backend` each work in four workspaces when the daemon stops. The daemon opens the project again and gives the workspaces their watch and first scan four at a time, two seconds of the project's clock apart, not all at once. Then every workspace reports an edit through its watch with no rescan tick, the watcher asks for no rescan by itself, one lost queue of one workspace rescans that workspace once and no other, and two losses before the rescan runs still give one rescan.
#[tokio::test]
async fn acceptance_46_workspaces_get_their_watch_in_groups_and_an_overflow_rescans_one_workspace_once()
 {
    let mut h = Harness::start().await;
    let mut work: Vec<(WorkspaceId, PathBuf)> = Vec::new();
    for agent in ["frontend", "backend"] {
        for n in 1..=4 {
            let task = h.create_task(agent, &format!("Part {n} of {agent}")).await;
            let started = h.start_work(agent, &task).await;
            work.push((started.workspace.parse().unwrap(), started.dir));
        }
    }
    assert_eq!(work.len(), 2 * DEFAULT_WATCH_GROUP);

    // The daemon stops and starts again with no session connected, so no worker acts yet.
    h.stop().await;
    let recorder = Recorder::default();
    h.watch = recorder.start();
    h.start_again().await;
    let live = h.handle.live.clone();
    assert_eq!(live.waiting(), work.len());
    for (workspace, _) in &work {
        assert!(
            live.get(*workspace).is_none(),
            "no first scan before its group"
        );
    }

    live.tick();
    assert_eq!(
        live.waiting(),
        DEFAULT_WATCH_GROUP,
        "one group at the first tick"
    );
    live.tick();
    assert_eq!(
        live.waiting(),
        DEFAULT_WATCH_GROUP,
        "the next group waits for the gap"
    );
    h.project.clock().advance(DEFAULT_WATCH_GAP).unwrap();
    live.tick();
    assert_eq!(live.waiting(), 0);
    eventually(WAIT, async || {
        let scanned = work
            .iter()
            .filter(|(workspace, _)| live.get(*workspace).is_some())
            .count();
        if scanned == work.len() {
            Ok(())
        } else {
            Err(scanned)
        }
    })
    .await;

    // The clock stays far from `rescan_seconds` and nothing ticks, so only a watch reports these edits.
    for (_, dir) in &work {
        fs::write(dir.join("notes.md"), "watched\n").unwrap();
    }
    for (workspace, _) in &work {
        eventually(WAIT, async || match live.get(*workspace) {
            Some(state) if state.view.files.iter().any(|file| file.path == "notes.md") => Ok(()),
            other => Err(other.map(|state| state.view.files.len())),
        })
        .await;
        assert!(recorder.changed(*workspace));
        assert_eq!(recorder.rescans(*workspace), 0, "no rescan loop");
    }

    let (lost, _) = work[0];
    recorder.overflow(lost);
    eventually(WAIT, async || match recorder.rescans(lost) {
        1 => Ok(()),
        other => Err(other),
    })
    .await;

    let (twice, twice_dir) = &work[1];
    recorder.overflow(*twice);
    recorder.overflow(*twice);
    eventually(WAIT, async || match recorder.rescans(*twice) {
        1 => Ok(()),
        other => Err(other),
    })
    .await;
    // An edit after the losses arrives through the watch, after any rescan the losses could still cause.
    fs::write(twice_dir.join("after.md"), "still watched\n").unwrap();
    eventually(WAIT, async || match live.get(*twice) {
        Some(state) if state.view.files.iter().any(|file| file.path == "after.md") => Ok(()),
        other => Err(other.map(|state| state.view.files.len())),
    })
    .await;
    assert_eq!(
        recorder.rescans(*twice),
        1,
        "two losses before the rescan give one rescan"
    );
    assert_eq!(recorder.rescans(lost), 1);
    for (workspace, _) in &work[2..] {
        assert_eq!(
            recorder.rescans(*workspace),
            0,
            "a loss rescans only its own workspace"
        );
    }
}
