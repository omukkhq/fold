#![cfg(test)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use fold_core::WorkspaceId;
use fold_vcs::ignore::IgnoreRules;
use fold_vcs::paths::RepoPath;
use fold_vcs::watch::{
    DegradedReason, WatchBackend, WatchConfig, WatchEvent, WatchHub, WatchRefused,
};
use notify::event::{AccessKind, CreateKind, Flag, RemoveKind};
use notify::{Event, EventKind};

const WAIT: Duration = Duration::from_secs(10);

fn config(debounce_ms: u64, max_wait_ms: u64) -> WatchConfig {
    WatchConfig {
        debounce: Duration::from_millis(debounce_ms),
        max_wait: Duration::from_millis(max_wait_ms),
        degraded_interval: Duration::from_secs(60),
    }
}

fn ws(number: u32) -> WorkspaceId {
    WorkspaceId::from_number(number).unwrap()
}

fn rules(root: &Path) -> Arc<IgnoreRules> {
    Arc::new(IgnoreRules::new(root))
}

fn path(text: &str) -> RepoPath {
    RepoPath::parse(text.as_bytes()).unwrap()
}

fn paths(texts: &[&str]) -> Vec<RepoPath> {
    texts.iter().map(|text| path(text)).collect()
}

fn native(config: WatchConfig) -> (WatchHub, Receiver<WatchEvent>) {
    let (tx, rx) = mpsc::channel();
    let hub = WatchHub::start(config, move |event| {
        let _ = tx.send(event);
    });
    (hub, rx)
}

fn with_backend(backend: Fake, config: WatchConfig) -> (WatchHub, Receiver<WatchEvent>) {
    let (tx, rx) = mpsc::channel();
    let hub = WatchHub::start_with(backend, config, move |event| {
        let _ = tx.send(event);
    });
    (hub, rx)
}

fn next(rx: &Receiver<WatchEvent>) -> WatchEvent {
    rx.recv_timeout(WAIT).unwrap()
}

fn quiet(rx: &Receiver<WatchEvent>, window: Duration) {
    assert_eq!(rx.recv_timeout(window), Err(RecvTimeoutError::Timeout));
}

/// Collects `Changed` batches of one workspace until every wanted path has been named.
fn changed_until(
    rx: &Receiver<WatchEvent>,
    id: WorkspaceId,
    wanted: &[&str],
) -> BTreeSet<RepoPath> {
    let wanted: BTreeSet<RepoPath> = wanted.iter().map(|text| path(text)).collect();
    let mut seen = BTreeSet::new();
    let deadline = Instant::now() + WAIT;
    while !wanted.is_subset(&seen) {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(WatchEvent::Changed { workspace, paths }) if workspace == id => seen.extend(paths),
            Ok(other) => panic!("unexpected event {other:?}"),
            Err(error) => panic!("waited for {wanted:?}, saw {seen:?}: {error}"),
        }
    }
    seen
}

/// Writes a marker file and waits for its batch. FSEvents may still report changes made just before its stream started, and they arrive before the marker's; inotify reports none of them.
fn settle(rx: &Receiver<WatchEvent>, id: WorkspaceId, root: &Path) {
    fs::write(root.join("settled"), b"").unwrap();
    changed_until(rx, id, &["settled"]);
}

fn workspace_dir(parent: &tempfile::TempDir, name: &str) -> PathBuf {
    let root = parent.path().join(name);
    fs::create_dir(&root).unwrap();
    root
}

#[derive(Default)]
struct FakeState {
    watched: BTreeSet<PathBuf>,
    limit: Option<usize>,
    unsupported: bool,
    backslash: bool,
    subtrees: bool,
}

#[derive(Clone, Default)]
struct Fake(Arc<Mutex<FakeState>>);

impl Fake {
    fn limited(limit: usize) -> Self {
        let fake = Self::default();
        fake.0.lock().unwrap().limit = Some(limit);
        fake
    }

    fn unsupported() -> Self {
        let fake = Self::default();
        fake.0.lock().unwrap().unsupported = true;
        fake
    }

    fn backslash() -> Self {
        let fake = Self::default();
        fake.0.lock().unwrap().backslash = true;
        fake
    }

    fn subtrees() -> Self {
        let fake = Self::default();
        fake.0.lock().unwrap().subtrees = true;
        fake
    }

    fn watched(&self) -> usize {
        self.0.lock().unwrap().watched.len()
    }

    fn watched_paths(&self) -> Vec<PathBuf> {
        self.0.lock().unwrap().watched.iter().cloned().collect()
    }
}

impl WatchBackend for Fake {
    fn watch_dir(&mut self, dir: &Path) -> Result<(), WatchRefused> {
        let mut state = self.0.lock().unwrap();
        if state.unsupported {
            return Err(WatchRefused::Unsupported);
        }
        if state
            .limit
            .is_some_and(|limit| state.watched.len() >= limit)
        {
            return Err(WatchRefused::LimitReached);
        }
        state.watched.insert(dir.to_path_buf());
        Ok(())
    }

    fn unwatch_dir(&mut self, dir: &Path) {
        self.0.lock().unwrap().watched.remove(dir);
    }

    fn backslash_separates(&self) -> bool {
        self.0.lock().unwrap().backslash
    }

    fn watches_subtrees(&self) -> bool {
        self.0.lock().unwrap().subtrees
    }
}

#[test]
fn a_backend_that_watches_subtrees_gets_only_the_roots() {
    let parent = tempfile::tempdir().unwrap();
    let root = workspace_dir(&parent, "one");
    let other = workspace_dir(&parent, "two");
    fs::create_dir_all(root.join("src/deep")).unwrap();
    fs::create_dir(root.join("gone")).unwrap();
    let fake = Fake::subtrees();
    let (hub, rx) = with_backend(fake.clone(), config(50, 1_000));
    hub.add_workspace(ws(1), &root, rules(&root), Arc::default());
    hub.add_workspace(ws(2), &other, rules(&other), Arc::default());
    assert_eq!(fake.watched_paths(), vec![root.clone(), other.clone()]);
    assert_eq!(
        hub.watched_dirs(ws(1)),
        vec![
            root.clone(),
            root.join("gone"),
            root.join("src"),
            root.join("src/deep")
        ]
    );

    // A change deep in the tree arrives on the root's watch alone.
    let notifier = hub.notifier();
    fs::write(root.join("src/deep/a"), b"a").unwrap();
    notifier.send(Ok(
        Event::new(EventKind::Create(CreateKind::File)).add_path(root.join("src/deep/a"))
    ));
    assert_eq!(
        changed_until(&rx, ws(1), &["src/deep/a"]),
        BTreeSet::from([path("src/deep/a")])
    );

    // A removed directory leaves the hub's record; the root keeps its watch.
    fs::remove_dir(root.join("gone")).unwrap();
    notifier.send(Ok(
        Event::new(EventKind::Remove(RemoveKind::Folder)).add_path(root.join("gone"))
    ));
    changed_until(&rx, ws(1), &["gone"]);
    assert_eq!(
        hub.watched_dirs(ws(1)),
        vec![root.clone(), root.join("src"), root.join("src/deep")]
    );
    assert_eq!(fake.watched_paths(), vec![root.clone(), other.clone()]);

    hub.remove_workspace(ws(1));
    assert_eq!(fake.watched_paths(), vec![other]);
    hub.shutdown();
}

#[test]
fn one_write_produces_one_changed() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, rx) = native(config(100, 2_000));
    hub.add_workspace(ws(1), dir.path(), rules(dir.path()), Arc::default());
    fs::write(dir.path().join("a.txt"), b"one").unwrap();
    assert_eq!(
        next(&rx),
        WatchEvent::Changed {
            workspace: ws(1),
            paths: paths(&["a.txt"]),
        }
    );
    quiet(&rx, Duration::from_millis(400));
    hub.shutdown();
}

#[test]
fn a_burst_inside_the_debounce_window_is_one_batch() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, rx) = native(config(500, 30_000));
    hub.add_workspace(ws(1), dir.path(), rules(dir.path()), Arc::default());
    let names: Vec<String> = (0..100).map(|n| format!("f{n:03}.txt")).collect();
    for name in &names {
        fs::write(dir.path().join(name), b"x").unwrap();
    }
    let WatchEvent::Changed { workspace, paths } = next(&rx) else {
        panic!("expected one batch");
    };
    assert_eq!(workspace, ws(1));
    let expected: Vec<RepoPath> = names.iter().map(|name| path(name)).collect();
    assert_eq!(paths, expected);
    quiet(&rx, Duration::from_millis(1_000));
}

#[test]
fn continuous_writes_still_flush_after_max_wait() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, rx) = native(config(200, 500));
    hub.add_workspace(ws(1), dir.path(), rules(dir.path()), Arc::default());
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let stop = Arc::clone(&stop);
        let file = dir.path().join("busy.txt");
        thread::spawn(move || {
            let mut round = 0u64;
            while !stop.load(Ordering::SeqCst) {
                fs::write(&file, round.to_le_bytes()).unwrap();
                round += 1;
                // Paces the writes well under the debounce so the window never closes on its own.
                thread::sleep(Duration::from_millis(20));
            }
        })
    };
    let event = rx.recv_timeout(WAIT);
    let still_writing = !writer.is_finished();
    stop.store(true, Ordering::SeqCst);
    writer.join().unwrap();
    assert!(still_writing);
    assert_eq!(
        event.unwrap(),
        WatchEvent::Changed {
            workspace: ws(1),
            paths: paths(&["busy.txt"]),
        }
    );
}

#[test]
fn workspaces_get_only_their_own_paths() {
    let parent = tempfile::tempdir().unwrap();
    let one = workspace_dir(&parent, "one");
    let two = workspace_dir(&parent, "two");
    let (hub, rx) = native(config(100, 2_000));
    hub.add_workspace(ws(1), &one, rules(&one), Arc::default());
    hub.add_workspace(ws(2), &two, rules(&two), Arc::default());
    fs::write(one.join("a"), b"a").unwrap();
    fs::write(two.join("b"), b"b").unwrap();
    let mut batches = BTreeMap::new();
    while batches.len() < 2 {
        let WatchEvent::Changed { workspace, paths } = next(&rx) else {
            panic!("expected batches");
        };
        batches
            .entry(workspace)
            .or_insert_with(Vec::new)
            .extend(paths);
    }
    assert_eq!(batches[&ws(1)], paths(&["a"]));
    assert_eq!(batches[&ws(2)], paths(&["b"]));
}

#[test]
fn ignored_directories_cost_no_watch_and_no_event() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join(".gitignore"), b"build/\n").unwrap();
    fs::create_dir_all(root.join("build/deep")).unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::create_dir(root.join(".git")).unwrap();
    let (hub, rx) = native(config(100, 2_000));
    hub.add_workspace(ws(1), root, rules(root), Arc::default());
    assert_eq!(
        hub.watched_dirs(ws(1)),
        vec![root.to_path_buf(), root.join("src")]
    );
    settle(&rx, ws(1), root);
    fs::write(root.join("build/out.o"), b"o").unwrap();
    fs::write(root.join("build/deep/out.o"), b"o").unwrap();
    fs::write(root.join(".git/HEAD"), b"h").unwrap();
    fs::write(root.join("src/lib.rs"), b"l").unwrap();
    assert_eq!(
        next(&rx),
        WatchEvent::Changed {
            workspace: ws(1),
            paths: paths(&["src/lib.rs"]),
        }
    );
    quiet(&rx, Duration::from_millis(300));
}

#[test]
fn folds_own_staging_and_git_metadata_send_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (hub, rx) = native(config(100, 2_000));
    hub.add_workspace(ws(1), root, rules(root), Arc::default());
    fs::create_dir_all(root.join(".fold/tmp")).unwrap();
    for n in 0..5 {
        let staged = root.join(format!(".fold/tmp/stage-{n}"));
        fs::write(&staged, b"s").unwrap();
        fs::rename(&staged, root.join(format!(".fold/tmp/done-{n}"))).unwrap();
    }
    fs::create_dir_all(root.join("src/.fold")).unwrap();
    fs::write(root.join("src/.fold/state"), b"s").unwrap();
    fs::create_dir(root.join(".git")).unwrap();
    fs::write(root.join(".git/index"), b"i").unwrap();
    fs::write(root.join("tracked"), b"t").unwrap();
    let seen = changed_until(&rx, ws(1), &["src", "tracked"]);
    assert_eq!(seen, paths(&["src", "tracked"]).into_iter().collect());
    assert_eq!(
        hub.watched_dirs(ws(1)),
        vec![root.to_path_buf(), root.join("src")]
    );
    fs::write(root.join(".fold/tmp/late"), b"l").unwrap();
    fs::write(root.join("src/.fold/state"), b"t").unwrap();
    quiet(&rx, Duration::from_millis(300));
}

#[test]
fn a_rescan_brings_the_watches_in_line_with_the_tree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("old")).unwrap();
    let fake = Fake::default();
    let (hub, rx) = with_backend(fake.clone(), config(50, 1_000));
    hub.add_workspace(ws(1), root, rules(root), Arc::default());
    assert_eq!(
        hub.watched_dirs(ws(1)),
        vec![root.to_path_buf(), root.join("old")]
    );
    // Made while notifications were lost, so the hub hears of neither change.
    fs::remove_dir(root.join("old")).unwrap();
    fs::create_dir_all(root.join("new/deep")).unwrap();
    let notifier = hub.notifier();
    notifier.send(Ok(Event::new(EventKind::Other).set_flag(Flag::Rescan)));
    assert_eq!(rescan_of(next(&rx)), ws(1));
    assert_eq!(
        hub.watched_dirs(ws(1)),
        vec![root.to_path_buf(), root.join("new"), root.join("new/deep")]
    );
    assert_eq!(fake.watched(), 3);
    notifier.send(Ok(
        Event::new(EventKind::Create(CreateKind::File)).add_path(root.join("new/deep/file"))
    ));
    assert_eq!(
        next(&rx),
        WatchEvent::Changed {
            workspace: ws(1),
            paths: paths(&["new/deep/file"]),
        }
    );
}

#[test]
fn a_changed_ignore_file_moves_the_watches() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("gen/inner")).unwrap();
    let (hub, rx) = native(config(100, 2_000));
    hub.add_workspace(ws(1), root, rules(root), Arc::default());
    assert_eq!(hub.watched_dirs(ws(1)).len(), 3);
    fs::write(root.join(".foldignore"), b"gen/\n").unwrap();
    changed_until(&rx, ws(1), &[".foldignore"]);
    assert_eq!(hub.watched_dirs(ws(1)), vec![root.to_path_buf()]);
    fs::write(root.join(".foldignore"), b"other/\n").unwrap();
    changed_until(&rx, ws(1), &[".foldignore"]);
    assert_eq!(
        hub.watched_dirs(ws(1)),
        vec![root.to_path_buf(), root.join("gen"), root.join("gen/inner")]
    );
}

#[test]
fn a_new_directory_is_scanned_and_a_removed_one_loses_its_watches() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (hub, rx) = native(config(100, 2_000));
    hub.add_workspace(ws(1), root, rules(root), Arc::default());
    fs::create_dir_all(root.join("new/deep")).unwrap();
    fs::write(root.join("new/a"), b"a").unwrap();
    fs::write(root.join("new/deep/b"), b"b").unwrap();
    let seen = changed_until(&rx, ws(1), &["new/a", "new/deep/b"]);
    assert!(seen.contains(&path("new")));
    assert_eq!(
        hub.watched_dirs(ws(1)),
        vec![root.to_path_buf(), root.join("new"), root.join("new/deep")]
    );
    fs::remove_dir_all(root.join("new")).unwrap();
    changed_until(&rx, ws(1), &["new"]);
    assert_eq!(hub.watched_dirs(ws(1)), vec![root.to_path_buf()]);
}

#[test]
fn a_renamed_directory_is_watched_under_its_new_name() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("a/sub")).unwrap();
    let (hub, rx) = native(config(100, 2_000));
    hub.add_workspace(ws(1), root, rules(root), Arc::default());
    fs::rename(root.join("a"), root.join("b")).unwrap();
    fs::create_dir(root.join("a")).unwrap();
    changed_until(&rx, ws(1), &["a", "b", "b/sub"]);
    assert_eq!(
        hub.watched_dirs(ws(1)),
        vec![
            root.to_path_buf(),
            root.join("a"),
            root.join("b"),
            root.join("b/sub")
        ]
    );
    fs::write(root.join("a/x"), b"x").unwrap();
    changed_until(&rx, ws(1), &["a/x"]);
    fs::write(root.join("b/sub/y"), b"y").unwrap();
    changed_until(&rx, ws(1), &["b/sub/y"]);
}

#[test]
fn the_watch_limit_degrades_to_timer_scans_once() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("d1")).unwrap();
    fs::create_dir(root.join("d2")).unwrap();
    let fake = Fake::limited(3);
    let interval = Duration::from_millis(150);
    let (hub, rx) = with_backend(
        fake.clone(),
        WatchConfig {
            degraded_interval: interval,
            ..config(50, 1_000)
        },
    );
    hub.add_workspace(ws(1), root, rules(root), Arc::default());
    assert_eq!(fake.watched(), 3);
    fs::create_dir(root.join("d3")).unwrap();
    fs::create_dir(root.join("d4")).unwrap();
    let sent = Instant::now();
    let notifier = hub.notifier();
    for name in ["d3", "d4"] {
        notifier.send(Ok(
            Event::new(EventKind::Create(CreateKind::Folder)).add_path(root.join(name))
        ));
    }
    assert_eq!(
        next(&rx),
        WatchEvent::Degraded {
            workspace: ws(1),
            reason: DegradedReason::WatchLimit,
        }
    );
    assert_eq!(DegradedReason::WatchLimit.as_str(), "watch_limit");
    let mut changed = BTreeSet::new();
    let mut ticks = Vec::new();
    while ticks.len() < 2 {
        match next(&rx) {
            WatchEvent::Changed { paths, .. } => changed.extend(paths),
            WatchEvent::RescanRequested { workspace } => {
                assert_eq!(workspace, ws(1));
                ticks.push(sent.elapsed());
            }
            other => panic!("unexpected event {other:?}"),
        }
    }
    assert_eq!(changed, paths(&["d3", "d4"]).into_iter().collect());
    assert!(ticks[0] >= interval);
    assert!(ticks[1] >= interval * 2);
    assert_eq!(fake.watched(), 3);
    assert_eq!(hub.watched_dirs(ws(1)).len(), 3);
}

#[test]
fn an_unsupported_backend_puts_every_workspace_on_timer_scans() {
    let parent = tempfile::tempdir().unwrap();
    let one = workspace_dir(&parent, "one");
    let two = workspace_dir(&parent, "two");
    let interval = Duration::from_millis(100);
    let (hub, rx) = with_backend(
        Fake::unsupported(),
        WatchConfig {
            degraded_interval: interval,
            ..config(50, 1_000)
        },
    );
    let added = Instant::now();
    hub.add_workspace(ws(1), &one, rules(&one), Arc::default());
    hub.add_workspace(ws(2), &two, rules(&two), Arc::default());
    let mut degraded = BTreeSet::new();
    let mut ticks: BTreeMap<WorkspaceId, usize> = BTreeMap::new();
    while ticks.values().filter(|&&count| count >= 2).count() < 2 {
        match next(&rx) {
            WatchEvent::Degraded { workspace, reason } => {
                assert_eq!(reason, DegradedReason::NoNativeWatcher);
                assert_eq!(reason.as_str(), "no_native_watcher");
                assert!(degraded.insert(workspace), "degraded twice");
            }
            WatchEvent::RescanRequested { workspace } => {
                assert!(degraded.contains(&workspace));
                assert!(added.elapsed() >= interval);
                *ticks.entry(workspace).or_default() += 1;
            }
            other => panic!("unexpected event {other:?}"),
        }
    }
    assert!(hub.watched_dirs(ws(1)).is_empty());
    fs::remove_dir(&one).unwrap();
    loop {
        match next(&rx) {
            WatchEvent::RootMissing { workspace } => {
                assert_eq!(workspace, ws(1));
                break;
            }
            WatchEvent::RescanRequested { .. } => {}
            other => panic!("unexpected event {other:?}"),
        }
    }
    for _ in 0..3 {
        assert_eq!(next(&rx), WatchEvent::RescanRequested { workspace: ws(2) });
    }
}

#[test]
fn reported_paths_are_filtered_and_slash_joined() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let outside = tempfile::tempdir().unwrap();
    let (hub, rx) = with_backend(Fake::backslash(), config(50, 1_000));
    hub.add_workspace(ws(1), root, rules(root), Arc::default());
    let notifier = hub.notifier();
    let file = |at: PathBuf| Event::new(EventKind::Create(CreateKind::File)).add_path(at);
    notifier.send(Ok(file(outside.path().join("x"))));
    notifier.send(Ok(
        Event::new(EventKind::Access(AccessKind::Any)).add_path(root.join("read"))
    ));
    notifier.send(Ok(Event::new(EventKind::Other).add_path(root.join("other"))));
    notifier.send(Ok(file(root.join(".git\\config"))));
    notifier.send(Ok(file(root.join("sub\\file.txt"))));
    assert_eq!(
        next(&rx),
        WatchEvent::Changed {
            workspace: ws(1),
            paths: paths(&["sub/file.txt"]),
        }
    );
}

#[cfg(unix)]
#[test]
fn a_backslash_is_a_name_byte_where_the_platform_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (hub, rx) = native(config(100, 2_000));
    hub.add_workspace(ws(1), root, rules(root), Arc::default());
    fs::write(root.join("a\\b"), b"x").unwrap();
    assert_eq!(
        next(&rx),
        WatchEvent::Changed {
            workspace: ws(1),
            paths: paths(&["a\\b"]),
        }
    );
}

#[test]
fn overflow_requests_a_rescan_of_the_affected_workspaces() {
    let parent = tempfile::tempdir().unwrap();
    let one = workspace_dir(&parent, "one");
    let two = workspace_dir(&parent, "two");
    let (hub, rx) = with_backend(Fake::default(), config(100, 1_000));
    hub.add_workspace(ws(1), &one, rules(&one), Arc::default());
    hub.add_workspace(ws(2), &two, rules(&two), Arc::default());
    let notifier = hub.notifier();
    let overflow = || Event::new(EventKind::Other).set_flag(Flag::Rescan);

    notifier.send(Ok(overflow()));
    let mut rescanned = vec![rescan_of(next(&rx)), rescan_of(next(&rx))];
    rescanned.sort();
    assert_eq!(rescanned, vec![ws(1), ws(2)]);

    notifier.send(Ok(overflow().add_path(one.join("deep/file"))));
    assert_eq!(rescan_of(next(&rx)), ws(1));
    notifier.send(Err(
        notify::Error::generic("queue lost").add_path(two.clone())
    ));
    assert_eq!(rescan_of(next(&rx)), ws(2));

    // A pending batch is covered by the rescan, so it never leaves; the later batch of the other workspace shows that.
    let created = |at: PathBuf| Event::new(EventKind::Create(CreateKind::File)).add_path(at);
    notifier.send(Ok(created(one.join("a"))));
    notifier.send(Ok(overflow().add_path(one.join("a"))));
    assert_eq!(rescan_of(next(&rx)), ws(1));
    notifier.send(Ok(created(two.join("b"))));
    assert_eq!(
        next(&rx),
        WatchEvent::Changed {
            workspace: ws(2),
            paths: paths(&["b"]),
        }
    );
}

fn rescan_of(event: WatchEvent) -> WorkspaceId {
    match event {
        WatchEvent::RescanRequested { workspace } => workspace,
        other => panic!("expected a rescan, got {other:?}"),
    }
}

#[test]
fn a_removed_root_is_reported_once() {
    let parent = tempfile::tempdir().unwrap();
    let root = workspace_dir(&parent, "ws");
    fs::create_dir_all(root.join("src/deep")).unwrap();
    fs::write(root.join("src/deep/file"), b"f").unwrap();
    let (hub, rx) = native(config(100, 2_000));
    hub.add_workspace(ws(1), &root, rules(&root), Arc::default());
    fs::remove_dir_all(&root).unwrap();
    assert_eq!(next(&rx), WatchEvent::RootMissing { workspace: ws(1) });
    quiet(&rx, Duration::from_millis(400));
    assert!(hub.watched_dirs(ws(1)).is_empty());

    let gone = parent.path().join("never");
    hub.add_workspace(ws(2), &gone, rules(&gone), Arc::default());
    assert_eq!(next(&rx), WatchEvent::RootMissing { workspace: ws(2) });
}

#[test]
fn a_removed_workspace_sends_nothing_more() {
    let parent = tempfile::tempdir().unwrap();
    let one = workspace_dir(&parent, "one");
    let two = workspace_dir(&parent, "two");
    let (hub, rx) = native(config(100, 2_000));
    hub.add_workspace(ws(1), &one, rules(&one), Arc::default());
    hub.add_workspace(ws(2), &two, rules(&two), Arc::default());
    fs::write(one.join("early"), b"e").unwrap();
    hub.remove_workspace(ws(1));
    assert!(hub.watched_dirs(ws(1)).is_empty());
    fs::write(one.join("late"), b"l").unwrap();
    fs::write(two.join("marker"), b"m").unwrap();
    assert_eq!(
        next(&rx),
        WatchEvent::Changed {
            workspace: ws(2),
            paths: paths(&["marker"]),
        }
    );
    quiet(&rx, Duration::from_millis(300));
}

#[test]
fn adding_an_id_again_replaces_the_workspace() {
    let parent = tempfile::tempdir().unwrap();
    let one = workspace_dir(&parent, "one");
    let two = workspace_dir(&parent, "two");
    let (hub, rx) = native(config(100, 2_000));
    hub.add_workspace(ws(1), &one, rules(&one), Arc::default());
    hub.add_workspace(ws(1), &two, rules(&two), Arc::default());
    assert_eq!(hub.watched_dirs(ws(1)), vec![two.clone()]);
    fs::write(one.join("old"), b"o").unwrap();
    fs::write(two.join("new"), b"n").unwrap();
    assert_eq!(
        next(&rx),
        WatchEvent::Changed {
            workspace: ws(1),
            paths: paths(&["new"]),
        }
    );
}

#[test]
fn shutdown_joins_the_thread_and_drops_the_sink() {
    let dir = tempfile::tempdir().unwrap();
    let (hub, rx) = native(config(100, 2_000));
    hub.add_workspace(ws(1), dir.path(), rules(dir.path()), Arc::default());
    let notifier = hub.notifier();
    hub.shutdown();
    assert_eq!(rx.recv_timeout(WAIT), Err(RecvTimeoutError::Disconnected));
    notifier.send(Ok(Event::new(EventKind::Remove(RemoveKind::Any))));

    let (dropped, rx) = native(config(100, 2_000));
    drop(dropped);
    assert_eq!(rx.recv_timeout(WAIT), Err(RecvTimeoutError::Disconnected));
}

#[test]
fn a_tracked_file_below_an_ignored_directory_is_watched_and_the_rest_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join(".gitignore"), b"build/\n").unwrap();
    fs::create_dir_all(root.join("build/keep")).unwrap();
    fs::create_dir_all(root.join("build/cache")).unwrap();
    fs::write(root.join("build/keep/version.txt"), b"1").unwrap();
    let tracked: fold_core::tree::FlatTree = [(
        b"build/keep/version.txt".to_vec(),
        fold_core::tree::FlatEntry {
            kind: fold_core::tree::EntryKind::File,
            hash: fold_core::object::hash_object(fold_core::object::ObjectKind::Blob, b"1"),
        },
    )]
    .into();
    let (hub, rx) = native(config(100, 2_000));
    hub.add_workspace(
        ws(1),
        root,
        rules(root),
        Arc::new(fold_vcs::ignore::TrackedSet::new(tracked)),
    );
    assert_eq!(
        hub.watched_dirs(ws(1)),
        vec![
            root.to_path_buf(),
            root.join("build"),
            root.join("build/keep")
        ]
    );
    settle(&rx, ws(1), root);
    fs::write(root.join("build/cache/out.o"), b"o").unwrap();
    fs::write(root.join("build/keep/other.o"), b"o").unwrap();
    fs::write(root.join("build/keep/version.txt"), b"2").unwrap();
    assert_eq!(
        next(&rx),
        WatchEvent::Changed {
            workspace: ws(1),
            paths: paths(&["build/keep/version.txt"]),
        }
    );
    quiet(&rx, Duration::from_millis(300));
}
