#![cfg(test)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use fold_core::WorkspaceId;
use fold_vcs::ignore::IgnoreRules;
use fold_vcs::index::StatIndex;
use fold_vcs::paths::RepoPath;
use fold_vcs::scan::{DEFAULT_MAX_FILE_BYTES, ScanOptions, scan};
use fold_vcs::watch::{DegradedReason, INOTIFY_MASK, WatchConfig, WatchEvent, WatchHub};
use inotify::{EventMask, Inotify, WatchMask};
use notify::event::Flag;
use notify::{Event, EventKind};
use rustix::event::{PollFd, PollFlags, Timespec};

use crate::common::HOST;

const WAIT: Duration = Duration::from_secs(10);

/// The mask that notify 8.2.0 puts on every directory it watches on Linux.
const NOTIFY_MASK: WatchMask = WatchMask::ATTRIB
    .union(WatchMask::CREATE)
    .union(WatchMask::OPEN)
    .union(WatchMask::DELETE)
    .union(WatchMask::CLOSE_WRITE)
    .union(WatchMask::MODIFY)
    .union(WatchMask::MOVED_FROM)
    .union(WatchMask::MOVED_TO)
    .union(WatchMask::DELETE_SELF)
    .union(WatchMask::MOVE_SELF);

const KINDS: [(EventMask, &str); 14] = [
    (EventMask::Q_OVERFLOW, "overflow"),
    (EventMask::IGNORED, "ignored"),
    (EventMask::OPEN, "open"),
    (EventMask::ACCESS, "access"),
    (EventMask::CLOSE_NOWRITE, "close_nowrite"),
    (EventMask::CLOSE_WRITE, "close_write"),
    (EventMask::CREATE, "create"),
    (EventMask::DELETE, "delete"),
    (EventMask::MODIFY, "modify"),
    (EventMask::ATTRIB, "attrib"),
    (EventMask::MOVED_FROM, "moved_from"),
    (EventMask::MOVED_TO, "moved_to"),
    (EventMask::DELETE_SELF, "delete_self"),
    (EventMask::MOVE_SELF, "move_self"),
];

const MARKER: &str = "zz-marker";

fn ws(number: u32) -> WorkspaceId {
    WorkspaceId::from_number(number).unwrap()
}

fn config(degraded_interval: Duration) -> WatchConfig {
    WatchConfig {
        debounce: Duration::from_millis(100),
        max_wait: Duration::from_millis(1_000),
        degraded_interval,
        ..WatchConfig::default()
    }
}

fn rules(root: &Path) -> Arc<IgnoreRules> {
    Arc::new(IgnoreRules::new(root))
}

fn path(text: &str) -> RepoPath {
    RepoPath::parse(text.as_bytes()).unwrap()
}

/// A workspace of `dirs` folders with `files` files each, the size of a real project checkout.
fn large_workspace(parent: &Path, name: &str, dirs: usize, files: usize) -> PathBuf {
    let root = parent.join(name);
    for dir in 0..dirs {
        let folder = root.join(format!("src/m{dir:03}"));
        fs::create_dir_all(&folder).unwrap();
        for file in 0..files {
            fs::write(
                folder.join(format!("f{file:03}.rs")),
                format!("// {dir} {file}\n"),
            )
            .unwrap();
        }
    }
    root
}

fn dirs_of(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![root.to_path_buf()];
    let mut at = 0;
    while at < out.len() {
        let dir = out[at].clone();
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            if entry.file_type().unwrap().is_dir() {
                out.push(entry.path());
            }
        }
        at += 1;
    }
    out
}

/// What the test suite of a project does to a workspace: it reads every file, again and again.
fn read_every_file(root: &Path, rounds: usize) {
    for _ in 0..rounds {
        for dir in dirs_of(root) {
            for entry in fs::read_dir(&dir).unwrap().flatten() {
                if entry.file_type().unwrap().is_file() {
                    fs::read(entry.path()).unwrap();
                }
            }
        }
    }
}

fn full_scan(root: &Path) {
    let rules = IgnoreRules::new(root);
    let mut index = StatIndex::empty(HOST);
    let options = ScanOptions {
        rules: &rules,
        max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        caps: HOST,
        recorded: None,
    };
    let result = scan(root, &mut index, &options).unwrap();
    assert!(result.entries.len() > 1_000);
}

/// The queued events of a set of inotify instances, counted by workspace and kind.
type Counts = BTreeMap<String, BTreeMap<&'static str, u64>>;

fn kind_of(mask: EventMask) -> &'static str {
    KINDS
        .iter()
        .find(|(bit, _)| mask.contains(*bit))
        .map_or("other", |(_, name)| name)
}

/// Each instance names the workspaces of its watches; with one shared instance, the workspace of an event comes from its watch. The instances are read continuously, as a watcher does, until the workload has finished and every queue is empty. A queue that overflowed counts one `overflow` and loses the rest.
fn count_until_done(
    instances: &mut [(Inotify, HashMap<i32, String>)],
    running: &thread::JoinHandle<()>,
) -> Counts {
    let mut counts: Counts = BTreeMap::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        assert!(Instant::now() < deadline, "the workload did not finish");
        let done = running.is_finished();
        let mut any = false;
        {
            let mut fds: Vec<PollFd<'_>> = instances
                .iter()
                .map(|(inotify, _)| PollFd::new(inotify, PollFlags::IN))
                .collect();
            let timeout = Timespec {
                tv_sec: 0,
                tv_nsec: 100_000_000,
            };
            rustix::event::poll(&mut fds, Some(&timeout)).unwrap();
        }
        for (inotify, names) in instances.iter_mut() {
            loop {
                let events = match inotify.read_events(&mut buffer) {
                    Ok(events) => events,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) => panic!("{error}"),
                };
                let mut read = false;
                for event in events {
                    read = true;
                    let workspace = names
                        .get(&event.wd.get_watch_descriptor_id())
                        .cloned()
                        .unwrap_or_else(|| "queue".to_owned());
                    *counts
                        .entry(workspace)
                        .or_default()
                        .entry(kind_of(event.mask))
                        .or_default() += 1;
                }
                if !read {
                    break;
                }
                any = true;
            }
        }
        if done && !any {
            return counts;
        }
    }
}

fn total(counts: &Counts, kind: &str) -> u64 {
    counts
        .values()
        .map(|kinds| kinds.get(kind).copied().unwrap_or(0))
        .sum()
}

fn report(label: &str, counts: &Counts) {
    let all: u64 = counts.values().flat_map(|kinds| kinds.values()).sum();
    eprintln!("{label}: {all} queued events");
    for (workspace, kinds) in counts {
        eprintln!("  {workspace}: {kinds:?}");
    }
}

/// A full scan of one large workspace while a test suite reads every file of several workspaces. It runs twice under raw instances owned by the test: once as notify watched, one shared instance with notify's mask, and once as fold watches, one instance for each workspace with fold's mask.
#[test]
fn reproduction_counts_queued_events_by_kind_and_workspace() {
    let parent = tempfile::tempdir().unwrap();
    let roots: Vec<PathBuf> = (1..=4)
        .map(|n| large_workspace(parent.path(), &format!("ws-{n}"), 100, 30))
        .collect();

    let watch = |shared: bool, mask: WatchMask| -> Vec<(Inotify, HashMap<i32, String>)> {
        let mut instances: Vec<(Inotify, HashMap<i32, String>)> = Vec::new();
        for (n, root) in roots.iter().enumerate() {
            if instances.is_empty() || !shared {
                instances.push((Inotify::init().unwrap(), HashMap::new()));
            }
            let (inotify, names) = instances.last_mut().unwrap();
            for dir in dirs_of(root) {
                let wd = inotify.watches().add(&dir, mask).unwrap();
                names.insert(wd.get_watch_descriptor_id(), format!("ws-{}", n + 1));
            }
        }
        instances
    };
    let workload = |roots: Vec<PathBuf>| {
        thread::spawn(move || {
            full_scan(&roots[0]);
            for root in &roots {
                read_every_file(root, 3);
            }
            for root in &roots {
                fs::write(root.join(MARKER), b"").unwrap();
            }
        })
    };

    let mut before = watch(true, NOTIFY_MASK);
    let running = workload(roots.clone());
    let notify_counts = count_until_done(&mut before, &running);
    running.join().unwrap();
    drop(before);
    for root in &roots {
        fs::remove_file(root.join(MARKER)).unwrap();
    }

    let mut after = watch(false, INOTIFY_MASK);
    assert_eq!(after.len(), roots.len());
    let running = workload(roots.clone());
    let fold_counts = count_until_done(&mut after, &running);
    running.join().unwrap();

    report("notify: one shared instance, IN_OPEN", &notify_counts);
    report("fold: one instance per workspace", &fold_counts);

    // Every open of a file or folder took a slot of the shared queue: the scan alone opens each of the 3,000 files and 101 folders of its workspace.
    // A host with the default queue of 16,384 may lose most of them to an overflow of the shared queue.
    assert!(total(&notify_counts, "open") > 4 * 3 * 3_000 || total(&notify_counts, "overflow") > 0);
    for kind in ["open", "access", "close_nowrite", "overflow"] {
        assert_eq!(total(&fold_counts, kind), 0, "{kind}");
    }
    // Only the markers themselves are left: their create and close-write in each workspace.
    for n in 1..=4 {
        let kinds = &fold_counts[&format!("ws-{n}")];
        assert_eq!(kinds.values().sum::<u64>(), 2, "{kinds:?}");
    }
}

fn native(config: WatchConfig) -> (WatchHub, Receiver<WatchEvent>) {
    let (tx, rx) = mpsc::channel();
    let hub = WatchHub::start(config, move |event| {
        let _ = tx.send(event);
    });
    (hub, rx)
}

fn changed_until(rx: &Receiver<WatchEvent>, id: WorkspaceId, wanted: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(WatchEvent::Changed { workspace, paths }) if workspace == id => {
                if paths.contains(&path(wanted)) {
                    return;
                }
            }
            Ok(other) => panic!("unexpected event {other:?}"),
            Err(error) => panic!("waited for {wanted}: {error}"),
        }
    }
}

#[test]
fn a_full_scan_of_a_large_workspace_queues_nothing_that_scans_again() {
    let parent = tempfile::tempdir().unwrap();
    let root = large_workspace(parent.path(), "ws", 100, 30);
    let (hub, rx) = native(config(Duration::from_secs(60)));
    hub.add_workspace(ws(1), &root, rules(&root), Arc::default());
    assert_eq!(hub.watched_dirs(ws(1)).len(), 102);
    fs::write(root.join("settled"), b"").unwrap();
    changed_until(&rx, ws(1), "settled");

    full_scan(&root);
    read_every_file(&root, 3);
    full_scan(&root);
    // A marker written after the scans arrives alone: the scans queued no event that names a path or asks for a rescan.
    fs::write(root.join(MARKER), b"").unwrap();
    assert_eq!(
        rx.recv_timeout(WAIT).unwrap(),
        WatchEvent::Changed {
            workspace: ws(1),
            paths: vec![path(MARKER)],
        }
    );
    assert_eq!(
        rx.recv_timeout(Duration::from_millis(300)),
        Err(RecvTimeoutError::Timeout)
    );
}

/// The watches of every inotify instance this process holds, as the kernel reports them: one set of watched inodes for each instance, with the event bits of each watch.
fn live_instances() -> BTreeSet<BTreeSet<(u64, u32)>> {
    let mut instances = BTreeSet::new();
    for entry in fs::read_dir("/proc/self/fd").unwrap().flatten() {
        let Ok(target) = fs::read_link(entry.path()) else {
            continue;
        };
        if target != Path::new("anon_inode:inotify") {
            continue;
        }
        let Ok(info) = fs::read_to_string(Path::new("/proc/self/fdinfo").join(entry.file_name()))
        else {
            continue;
        };
        let watches: BTreeSet<(u64, u32)> = info
            .lines()
            .filter(|line| line.starts_with("inotify "))
            .map(|line| {
                let field = |key: &str| {
                    line.split_whitespace()
                        .find_map(|part| part.strip_prefix(key))
                        .unwrap()
                };
                let ino = u64::from_str_radix(field("ino:"), 16).unwrap();
                let mask = u32::from_str_radix(field("mask:"), 16).unwrap();
                (ino, mask & WatchMask::ALL_EVENTS.bits())
            })
            .collect();
        if !watches.is_empty() {
            instances.insert(watches);
        }
    }
    instances
}

fn watched_inodes(hub: &WatchHub, id: WorkspaceId) -> BTreeSet<(u64, u32)> {
    hub.watched_dirs(id)
        .iter()
        .map(|dir| (fs::metadata(dir).unwrap().ino(), INOTIFY_MASK.bits()))
        .collect()
}

#[test]
fn each_workspace_has_its_own_instance_with_folds_mask() {
    let parent = tempfile::tempdir().unwrap();
    let roots: Vec<PathBuf> = (1..=3)
        .map(|n| large_workspace(parent.path(), &format!("ws-{n}"), 3, 2))
        .collect();
    let (hub, _rx) = native(config(Duration::from_secs(60)));
    for (n, root) in roots.iter().enumerate() {
        hub.add_workspace(ws(n as u32 + 1), root, rules(root), Arc::default());
    }
    let wanted: BTreeSet<BTreeSet<(u64, u32)>> =
        (1..=3).map(|n| watched_inodes(&hub, ws(n))).collect();
    assert_eq!(wanted.len(), 3);
    assert_eq!(live_instances(), wanted);

    // A removed workspace closes its instance; the others keep theirs.
    let gone = watched_inodes(&hub, ws(2));
    hub.remove_workspace(ws(2));
    let deadline = Instant::now() + WAIT;
    while live_instances().contains(&gone) {
        assert!(Instant::now() < deadline, "the instance stayed open");
        thread::yield_now();
    }
    assert_eq!(live_instances().len(), 2);
}

#[test]
fn a_lost_queue_rescans_only_its_own_workspace() {
    let parent = tempfile::tempdir().unwrap();
    let one = large_workspace(parent.path(), "one", 2, 2);
    let two = large_workspace(parent.path(), "two", 2, 2);
    let (hub, rx) = native(config(Duration::from_secs(60)));
    hub.add_workspace(ws(1), &one, rules(&one), Arc::default());
    hub.add_workspace(ws(2), &two, rules(&two), Arc::default());
    let notifier = hub.notifier();

    // An overflow names no path; it belongs to the queue it came from.
    notifier.send_for(
        ws(1),
        Ok(Event::new(EventKind::Other).set_flag(Flag::Rescan)),
    );
    assert_eq!(
        rx.recv_timeout(WAIT).unwrap(),
        WatchEvent::RescanRequested { workspace: ws(1) }
    );
    // A path is read only against the root of the queue's workspace.
    notifier.send_for(
        ws(2),
        Ok(Event::new(EventKind::Create(notify::event::CreateKind::File)).add_path(one.join("x"))),
    );
    fs::write(two.join("b"), b"b").unwrap();
    changed_until(&rx, ws(2), "b");
    fs::write(one.join("a"), b"a").unwrap();
    changed_until(&rx, ws(1), "a");
    assert_eq!(
        rx.recv_timeout(Duration::from_millis(300)),
        Err(RecvTimeoutError::Timeout)
    );
}

#[test]
fn a_refused_instance_puts_only_that_workspace_on_timer_scans() {
    let parent = tempfile::tempdir().unwrap();
    let one = large_workspace(parent.path(), "one", 2, 2);
    let two = large_workspace(parent.path(), "two", 2, 2);
    let interval = Duration::from_millis(300);
    let (tx, rx) = mpsc::channel();
    let hub = WatchHub::start_with_instances(
        config(interval),
        |id| {
            if id == ws(2) {
                Err(io::Error::from_raw_os_error(libc::EMFILE))
            } else {
                Inotify::init()
            }
        },
        move |event| {
            let _ = tx.send(event);
        },
    );
    hub.add_workspace(ws(1), &one, rules(&one), Arc::default());
    hub.add_workspace(ws(2), &two, rules(&two), Arc::default());
    assert_eq!(
        rx.recv_timeout(WAIT).unwrap(),
        WatchEvent::Degraded {
            workspace: ws(2),
            reason: DegradedReason::InstanceLimit,
        }
    );
    assert_eq!(DegradedReason::InstanceLimit.as_str(), "instance_limit");
    assert!(hub.watched_dirs(ws(2)).is_empty());
    assert_eq!(hub.watched_dirs(ws(1)).len(), 4);

    // The first tick shows the timer runs; any tick already queued is read here, so the next one comes after the write.
    assert_eq!(
        rx.recv_timeout(WAIT).unwrap(),
        WatchEvent::RescanRequested { workspace: ws(2) }
    );

    // The other workspace keeps its watch; the refused one sees its change through the next timer scan.
    fs::write(one.join("a"), b"a").unwrap();
    let written = Instant::now();
    fs::write(two.join("b"), b"b").unwrap();
    let mut changed = false;
    let mut rescanned = None;
    while !changed || rescanned.is_none() {
        match rx.recv_timeout(WAIT).unwrap() {
            WatchEvent::Changed { workspace, paths } => {
                assert_eq!(workspace, ws(1));
                assert_eq!(paths, vec![path("a")]);
                changed = true;
            }
            WatchEvent::RescanRequested { workspace } => {
                assert_eq!(workspace, ws(2));
                rescanned.get_or_insert(written.elapsed());
            }
            other => panic!("unexpected event {other:?}"),
        }
    }
    // A loaded machine may run the tick late, but never later than a few intervals.
    assert!(rescanned.unwrap() <= interval * 2 + Duration::from_secs(3));
}

/// Ties the measured notify mask to the watcher itself: the kernel reports the event bits of each watch of a live `notify` watcher.
#[test]
fn the_notify_watcher_watches_with_the_measured_mask() {
    use notify::{RecursiveMode, Watcher};
    let dir = tempfile::tempdir().unwrap();
    let mut watcher =
        notify::RecommendedWatcher::new(|_: notify::Result<Event>| {}, notify::Config::default())
            .unwrap();
    watcher
        .watch(dir.path(), RecursiveMode::NonRecursive)
        .unwrap();
    let ino = fs::metadata(dir.path()).unwrap().ino();
    assert_eq!(
        live_instances(),
        BTreeSet::from([BTreeSet::from([(ino, NOTIFY_MASK.bits())])])
    );
}
