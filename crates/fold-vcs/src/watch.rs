use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, Metadata};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use fold_core::WorkspaceId;
use fold_core::tree::FlatTree;
use notify::EventKind;
#[cfg(not(target_os = "linux"))]
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::ignore::{Cover, FOLDIGNORE, GITIGNORE, IgnoreRules, TrackedSet, tracked_names};
use crate::paths::{RepoPath, entries_at_or_below, is_reserved_name, os_bytes, os_path};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchConfig {
    /// Quiet time after the last notification before a batch leaves.
    pub debounce: Duration,
    /// Longest time a batch is held after its first notification, however busy the workspace stays.
    pub max_wait: Duration,
    /// Time between two `RescanRequested` events of a workspace in timer mode.
    pub degraded_interval: Duration,
    /// Lost notifications of one workspace within `overflow_window` that put it in timer mode; 0 never does.
    pub overflow_limit: u32,
    pub overflow_window: Duration,
    /// Time a workspace stays in timer mode after `overflow_limit` losses before the hub watches it again.
    pub overflow_pause: Duration,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            debounce: Duration::from_millis(500),
            max_wait: Duration::from_secs(2),
            degraded_interval: Duration::from_secs(10),
            overflow_limit: 3,
            overflow_window: Duration::from_secs(60),
            overflow_pause: Duration::from_secs(600),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WatchRefused {
    /// The platform's watch limit is used up; the workspace falls back to timer scans.
    LimitReached,
    /// The backend cannot watch at all; every workspace falls back to timer scans.
    Unsupported,
    /// This one directory cannot be watched, because it is gone or unreadable. The hub keeps no watch for it and tries again when an event names it.
    Unavailable,
    /// The system refused a new notification instance for the workspace; only that workspace falls back to timer scans.
    InstanceLimit,
}

/// The platform's native notifications. A backend delivers what it observes through the [`Notifier`] of its hub.
pub trait WatchBackend: Send {
    /// One non-recursive watch on `dir`, or one on its whole tree when [`WatchBackend::watches_subtrees`] is true.
    fn watch_dir(&mut self, dir: &Path) -> Result<(), WatchRefused>;
    /// Removing a watch the backend already dropped is not an error.
    fn unwatch_dir(&mut self, dir: &Path);
    /// The watch of `dir` for the workspace `workspace`. The hub calls only this, so a backend that keeps each workspace apart, such as one notification queue per workspace, learns whose directory it is.
    fn watch_in(&mut self, workspace: WorkspaceId, dir: &Path) -> Result<(), WatchRefused> {
        let _ = workspace;
        self.watch_dir(dir)
    }
    fn unwatch_in(&mut self, workspace: WorkspaceId, dir: &Path) {
        let _ = workspace;
        self.unwatch_dir(dir);
    }
    /// True when one watch on a workspace root reports every change below it. The hub then hands the backend only the root and keeps its own record of the directories below, which still decides what is new, moved or ignored.
    fn watches_subtrees(&self) -> bool {
        false
    }
    /// True when the reported paths use `\` as a separator. On other platforms `\` is an ordinary file name byte and must stay one.
    fn backslash_separates(&self) -> bool {
        cfg!(windows)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DegradedReason {
    WatchLimit,
    NoNativeWatcher,
    InstanceLimit,
    /// The workspace lost notifications again and again; the hub watches it again after `overflow_pause`.
    Overflow,
}

impl DegradedReason {
    /// The key in a `workspace.watch_degraded` event's data that names the reason.
    pub const DATA_KEY: &'static str = "reason";

    pub fn as_str(self) -> &'static str {
        match self {
            Self::WatchLimit => "watch_limit",
            Self::NoNativeWatcher => "no_native_watcher",
            Self::InstanceLimit => "instance_limit",
            Self::Overflow => "overflow",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// Sorted and deduplicated. A directory path stands for everything below it.
    Changed {
        workspace: WorkspaceId,
        paths: Vec<RepoPath>,
    },
    /// Notifications were lost; only a scan of the whole workspace is accurate.
    RescanRequested {
        workspace: WorkspaceId,
    },
    Degraded {
        workspace: WorkspaceId,
        reason: DegradedReason,
    },
    RootMissing {
        workspace: WorkspaceId,
    },
    /// The workspace has its watches again after the timer mode of [`DegradedReason::Overflow`].
    Restored {
        workspace: WorkspaceId,
    },
}

enum Msg {
    Raw(notify::Result<notify::Event>),
    /// A notification from a queue that belongs to one workspace, which owns it whatever its paths say.
    Attributed {
        workspace: WorkspaceId,
        event: notify::Result<notify::Event>,
    },
    Add {
        id: WorkspaceId,
        root: PathBuf,
        rules: Arc<IgnoreRules>,
        tracked: Arc<TrackedSet>,
        done: Sender<()>,
    },
    Remove {
        id: WorkspaceId,
        done: Sender<()>,
    },
    Retrack {
        id: WorkspaceId,
        old: Arc<FlatTree>,
        done: Sender<()>,
    },
    Watched {
        id: WorkspaceId,
        reply: Sender<Vec<PathBuf>>,
    },
    Scanned(Scanned),
    Shutdown,
}

/// Where a backend delivers raw notifications, in notify's event type whatever the backend.
#[derive(Clone)]
pub struct Notifier(Sender<Msg>);

impl Notifier {
    pub fn send(&self, event: notify::Result<notify::Event>) {
        // The hub is gone after shutdown; a late notification has nowhere to go.
        let _ = self.0.send(Msg::Raw(event));
    }

    /// A notification of the workspace `workspace` alone: its paths are read against that workspace's root only, and a lost notification rescans only that workspace.
    pub fn send_for(&self, workspace: WorkspaceId, event: notify::Result<notify::Event>) {
        let _ = self.0.send(Msg::Attributed { workspace, event });
    }
}

impl notify::EventHandler for Notifier {
    fn handle_event(&mut self, event: notify::Result<notify::Event>) {
        self.send(event);
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) struct NativeBackend {
    watcher: RecommendedWatcher,
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
impl NativeBackend {
    pub(crate) fn new(notifier: Notifier) -> notify::Result<Self> {
        Ok(Self {
            watcher: RecommendedWatcher::new(notifier, notify::Config::default())?,
        })
    }
}

/// FSEvents watches a whole tree with one stream, and notify restarts its stream, dropping what happens meanwhile, on every path it adds. So each workspace root gets a recursive stream of its own, and adding or removing a workspace never touches another workspace's stream.
#[cfg(target_os = "macos")]
pub(crate) struct NativeBackend {
    notifier: Notifier,
    streams: BTreeMap<PathBuf, RecommendedWatcher>,
}

#[cfg(target_os = "macos")]
impl NativeBackend {
    pub(crate) fn new(notifier: Notifier) -> notify::Result<Self> {
        Ok(Self {
            notifier,
            streams: BTreeMap::new(),
        })
    }
}

#[cfg(target_os = "macos")]
impl WatchBackend for NativeBackend {
    fn watch_dir(&mut self, dir: &Path) -> Result<(), WatchRefused> {
        let mut stream = RecommendedWatcher::new(self.notifier.clone(), notify::Config::default())
            .map_err(|error| refusal(&error))?;
        stream
            .watch(dir, RecursiveMode::Recursive)
            .map_err(|error| refusal(&error))?;
        self.streams.insert(dir.to_path_buf(), stream);
        Ok(())
    }

    fn unwatch_dir(&mut self, dir: &Path) {
        self.streams.remove(dir);
    }

    fn watches_subtrees(&self) -> bool {
        true
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
impl WatchBackend for NativeBackend {
    fn watch_dir(&mut self, dir: &Path) -> Result<(), WatchRefused> {
        self.watcher
            .watch(dir, RecursiveMode::NonRecursive)
            .map_err(|error| refusal(&error))
    }

    fn unwatch_dir(&mut self, dir: &Path) {
        // notify drops the watches of a deleted directory by itself, so the watch may already be gone.
        let _ = self.watcher.unwatch(dir);
    }
}

#[cfg(target_os = "linux")]
pub use linux::INOTIFY_MASK;
#[cfg(target_os = "linux")]
pub(crate) use linux::NativeBackend;

/// One inotify instance for each workspace, so each workspace has its own kernel queue of `fs.inotify.max_queued_events`: a busy workspace cannot fill the queue of another, and an `IN_Q_OVERFLOW` names exactly the workspace whose queue overflowed. One reader thread polls every instance and hands each notification to the hub as that workspace's own.
#[cfg(target_os = "linux")]
mod linux {
    use std::collections::{BTreeMap, HashMap};
    use std::ffi::OsStr;
    use std::io;
    use std::os::fd::OwnedFd;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
    use std::thread::{self, JoinHandle};

    use fold_core::WorkspaceId;
    use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};
    use notify::event::{
        CreateKind, DataChange, Flag, MetadataKind, ModifyKind, RemoveKind, RenameMode,
    };
    use notify::{Event, EventKind};
    use rustix::event::{EventfdFlags, PollFd, PollFlags};

    use super::{Notifier, WatchBackend, WatchRefused};

    /// Every change that can alter a file or directory, and nothing a read causes: no `IN_OPEN`, `IN_ACCESS` or `IN_CLOSE_NOWRITE`, so scans, builds and test runs that only read take no queue slot.
    pub const INOTIFY_MASK: WatchMask = WatchMask::CREATE
        .union(WatchMask::DELETE)
        .union(WatchMask::CLOSE_WRITE)
        .union(WatchMask::MODIFY)
        .union(WatchMask::MOVED_FROM)
        .union(WatchMask::MOVED_TO)
        .union(WatchMask::ATTRIB)
        .union(WatchMask::DELETE_SELF)
        .union(WatchMask::MOVE_SELF);

    pub(crate) type InstanceFactory = Box<dyn FnMut(WorkspaceId) -> io::Result<Inotify> + Send>;

    const READ_BUFFER: usize = 64 * 1024;

    struct Instance {
        /// Tells a closed instance from a new one of the same workspace, so the reader never reads a queue the hub has closed.
        serial: u64,
        inotify: Inotify,
        /// The reader polls this duplicate of the instance's descriptor, so a poll never waits on a number that the hub closed and the system handed out again.
        polled: Arc<OwnedFd>,
        dirs: HashMap<i32, PathBuf>,
        /// Two paths share a descriptor when the kernel hands out the watch of a renamed directory again under its new name.
        wds: HashMap<PathBuf, WatchDescriptor>,
    }

    impl Instance {
        /// The kernel dropped the watch `wd`, because its directory was deleted or the watch was removed.
        fn forget(&mut self, wd: i32) {
            if let Some(path) = self.dirs.remove(&wd)
                && self
                    .wds
                    .get(&path)
                    .is_some_and(|known| known.get_watch_descriptor_id() == wd)
            {
                self.wds.remove(&path);
            }
        }
    }

    #[derive(Default)]
    struct Shared {
        instances: BTreeMap<WorkspaceId, Instance>,
        serial: u64,
        stop: bool,
    }

    pub(crate) struct NativeBackend {
        shared: Arc<Mutex<Shared>>,
        wake: Arc<OwnedFd>,
        factory: InstanceFactory,
        reader: Option<JoinHandle<()>>,
    }

    fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
        shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wake(fd: &OwnedFd) {
        // A counter that cannot grow any more wakes the reader all the same.
        let _ = rustix::io::write(fd, &1u64.to_ne_bytes());
    }

    impl NativeBackend {
        pub(crate) fn new(notifier: Notifier) -> io::Result<Self> {
            Self::with_factory(notifier, Box::new(|_| Inotify::init()))
        }

        pub(crate) fn with_factory(
            notifier: Notifier,
            factory: InstanceFactory,
        ) -> io::Result<Self> {
            let wake = Arc::new(rustix::event::eventfd(
                0,
                EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK,
            )?);
            let shared = Arc::new(Mutex::new(Shared::default()));
            let reader = {
                let shared = Arc::clone(&shared);
                let wake = Arc::clone(&wake);
                thread::Builder::new()
                    .name("fold-inotify".to_owned())
                    .spawn(move || read_loop(&shared, &wake, &notifier))?
            };
            Ok(Self {
                shared,
                wake,
                factory,
                reader: Some(reader),
            })
        }
    }

    impl Drop for NativeBackend {
        fn drop(&mut self) {
            lock(&self.shared).stop = true;
            wake(&self.wake);
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
        }
    }

    impl WatchBackend for NativeBackend {
        /// Every watch belongs to the instance of a workspace, so a watch that names none is refused.
        fn watch_dir(&mut self, _dir: &Path) -> Result<(), WatchRefused> {
            Err(WatchRefused::Unsupported)
        }

        fn unwatch_dir(&mut self, _dir: &Path) {}

        fn watch_in(&mut self, workspace: WorkspaceId, dir: &Path) -> Result<(), WatchRefused> {
            let mut guard = lock(&self.shared);
            let shared = &mut *guard;
            if !shared.instances.contains_key(&workspace) {
                let inotify =
                    (self.factory)(workspace).map_err(|error| instance_refusal(&error))?;
                let polled = rustix::io::fcntl_dupfd_cloexec(&inotify, 0)
                    .map_err(|errno| instance_refusal(&errno.into()))?;
                shared.serial += 1;
                shared.instances.insert(
                    workspace,
                    Instance {
                        serial: shared.serial,
                        inotify,
                        polled: Arc::new(polled),
                        dirs: HashMap::new(),
                        wds: HashMap::new(),
                    },
                );
                wake(&self.wake);
            }
            let Some(instance) = shared.instances.get_mut(&workspace) else {
                return Err(WatchRefused::Unavailable);
            };
            // The lock stays held until the descriptor is known, so the reader never meets an event of a watch it cannot name.
            match instance.inotify.watches().add(dir, INOTIFY_MASK) {
                Ok(wd) => {
                    instance
                        .dirs
                        .insert(wd.get_watch_descriptor_id(), dir.to_path_buf());
                    instance.wds.insert(dir.to_path_buf(), wd);
                    Ok(())
                }
                Err(error) => {
                    if instance.wds.is_empty() {
                        shared.instances.remove(&workspace);
                        wake(&self.wake);
                    }
                    Err(watch_refusal(&error))
                }
            }
        }

        fn unwatch_in(&mut self, workspace: WorkspaceId, dir: &Path) {
            let mut guard = lock(&self.shared);
            let shared = &mut *guard;
            let Some(instance) = shared.instances.get_mut(&workspace) else {
                return;
            };
            if let Some(wd) = instance.wds.remove(dir) {
                let id = wd.get_watch_descriptor_id();
                // A descriptor that names another path now watches a renamed directory that is still wanted.
                if instance.dirs.get(&id).is_some_and(|known| known == dir) {
                    instance.dirs.remove(&id);
                    // The kernel drops the watch of a deleted directory by itself, so it may already be gone.
                    let _ = instance.inotify.watches().remove(wd);
                }
            }
            if instance.wds.is_empty() {
                shared.instances.remove(&workspace);
                wake(&self.wake);
            }
        }
    }

    pub(crate) fn instance_refusal(error: &io::Error) -> WatchRefused {
        match error.raw_os_error() {
            Some(libc::EMFILE | libc::ENFILE) => WatchRefused::InstanceLimit,
            _ => WatchRefused::Unsupported,
        }
    }

    pub(crate) fn watch_refusal(error: &io::Error) -> WatchRefused {
        match error.raw_os_error() {
            Some(libc::ENOSPC) => WatchRefused::LimitReached,
            _ => WatchRefused::Unavailable,
        }
    }

    struct Polled {
        workspace: WorkspaceId,
        serial: u64,
        fd: Arc<OwnedFd>,
    }

    fn read_loop(shared: &Mutex<Shared>, wake_fd: &OwnedFd, notifier: &Notifier) {
        let mut buffer = vec![0u8; READ_BUFFER];
        let mut polled: Vec<Polled> = Vec::new();
        loop {
            {
                let shared = lock(shared);
                if shared.stop {
                    return;
                }
                let same = polled.len() == shared.instances.len()
                    && polled
                        .iter()
                        .zip(shared.instances.iter())
                        .all(|(old, (id, instance))| {
                            old.workspace == *id && old.serial == instance.serial
                        });
                if !same {
                    polled = shared
                        .instances
                        .iter()
                        .map(|(id, instance)| Polled {
                            workspace: *id,
                            serial: instance.serial,
                            fd: Arc::clone(&instance.polled),
                        })
                        .collect();
                }
            }
            let mut fds: Vec<PollFd<'_>> = Vec::with_capacity(polled.len() + 1);
            fds.push(PollFd::new(wake_fd, PollFlags::IN));
            fds.extend(
                polled
                    .iter()
                    .map(|entry| PollFd::new(&*entry.fd, PollFlags::IN)),
            );
            match rustix::event::poll(&mut fds, None) {
                Ok(_) => {}
                Err(rustix::io::Errno::INTR) => continue,
                Err(error) => {
                    tracing::warn!(%error, "inotify reader stopped");
                    return;
                }
            }
            if !fds[0].revents().is_empty() {
                let mut count = [0u8; 8];
                let _ = rustix::io::read(wake_fd, &mut count);
            }
            let ready: Vec<(WorkspaceId, u64)> = fds[1..]
                .iter()
                .zip(&polled)
                .filter(|(fd, _)| !fd.revents().is_empty())
                .map(|(_, entry)| (entry.workspace, entry.serial))
                .collect();
            drop(fds);
            let mut out = Vec::new();
            {
                let mut shared = lock(shared);
                for (id, serial) in ready {
                    if let Some(instance) = shared
                        .instances
                        .get_mut(&id)
                        .filter(|instance| instance.serial == serial)
                    {
                        drain(id, instance, &mut buffer, &mut out);
                    }
                }
            }
            for (id, event) in out {
                notifier.send_for(id, Ok(event));
            }
        }
    }

    /// Reads the instance's queue until it is empty.
    fn drain(
        id: WorkspaceId,
        instance: &mut Instance,
        buffer: &mut [u8],
        out: &mut Vec<(WorkspaceId, Event)>,
    ) {
        loop {
            let events = match instance.inotify.read_events(buffer) {
                Ok(events) => events,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return,
                Err(error) => {
                    tracing::warn!(workspace = %id, %error, "inotify queue could not be read");
                    return;
                }
            };
            let mut read = false;
            for event in events {
                read = true;
                let wd = event.wd.get_watch_descriptor_id();
                if event.mask.contains(EventMask::IGNORED) {
                    instance.forget(wd);
                } else if let Some(event) = translate(&instance.dirs, wd, event.mask, event.name) {
                    out.push((id, event));
                }
            }
            if !read {
                return;
            }
        }
    }

    /// The notification in notify's terms, or `None` for one the hub has no use for. A self event, which has no name, names the watched directory itself.
    pub(crate) fn translate(
        dirs: &HashMap<i32, PathBuf>,
        wd: i32,
        mask: EventMask,
        name: Option<&OsStr>,
    ) -> Option<Event> {
        if mask.contains(EventMask::Q_OVERFLOW) {
            return Some(Event::new(EventKind::Other).set_flag(Flag::Rescan));
        }
        let folder = mask.contains(EventMask::ISDIR);
        let kind = if mask.contains(EventMask::CREATE) {
            EventKind::Create(if folder {
                CreateKind::Folder
            } else {
                CreateKind::File
            })
        } else if mask.contains(EventMask::DELETE) {
            EventKind::Remove(if folder {
                RemoveKind::Folder
            } else {
                RemoveKind::File
            })
        } else if mask.intersects(EventMask::DELETE_SELF | EventMask::UNMOUNT) {
            EventKind::Remove(RemoveKind::Any)
        } else if mask.contains(EventMask::MOVED_FROM) {
            EventKind::Modify(ModifyKind::Name(RenameMode::From))
        } else if mask.contains(EventMask::MOVED_TO) {
            EventKind::Modify(ModifyKind::Name(RenameMode::To))
        } else if mask.contains(EventMask::MOVE_SELF) {
            EventKind::Modify(ModifyKind::Name(RenameMode::Any))
        } else if mask.intersects(EventMask::MODIFY | EventMask::CLOSE_WRITE) {
            EventKind::Modify(ModifyKind::Data(DataChange::Any))
        } else if mask.contains(EventMask::ATTRIB) {
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any))
        } else {
            return None;
        };
        let dir = dirs.get(&wd)?;
        let path = name.map_or_else(|| dir.clone(), |name| dir.join(name));
        Some(Event::new(kind).add_path(path))
    }

    #[cfg(test)]
    mod tests {
        use std::fs::File;
        use std::os::unix::fs::FileExt;

        use super::*;

        fn instance(dir: &Path) -> Instance {
            let inotify = Inotify::init().unwrap();
            let polled = Arc::new(rustix::io::fcntl_dupfd_cloexec(&inotify, 0).unwrap());
            let wd = inotify.watches().add(dir, INOTIFY_MASK).unwrap();
            Instance {
                serial: 0,
                inotify,
                polled,
                dirs: [(wd.get_watch_descriptor_id(), dir.to_path_buf())].into(),
                wds: [(dir.to_path_buf(), wd)].into(),
            }
        }

        /// A real kernel overflow: one workspace's queue gets more events than `fs.inotify.max_queued_events` while nobody reads it, and the other workspace's queue holds only its own event.
        #[test]
        fn an_overflow_names_only_the_workspace_whose_queue_overflowed() {
            let max: usize = std::fs::read_to_string("/proc/sys/fs/inotify/max_queued_events")
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            let parent = tempfile::tempdir().unwrap();
            let busy_root = parent.path().join("busy");
            let calm_root = parent.path().join("calm");
            std::fs::create_dir(&busy_root).unwrap();
            std::fs::create_dir(&calm_root).unwrap();
            let busy_id = WorkspaceId::from_number(1).unwrap();
            let calm_id = WorkspaceId::from_number(2).unwrap();
            let mut busy = instance(&busy_root);
            let mut calm = instance(&calm_root);

            // The kernel merges an event into the one before it when both are the same, so the writes take turns between two files.
            let files = [
                File::create(busy_root.join("a")).unwrap(),
                File::create(busy_root.join("b")).unwrap(),
            ];
            for n in 0..=max {
                files[n % 2].write_all_at(b"x", n as u64 / 2).unwrap();
            }
            std::fs::write(calm_root.join("c"), b"c").unwrap();

            let mut buffer = vec![0u8; READ_BUFFER];
            let mut out = Vec::new();
            drain(busy_id, &mut busy, &mut buffer, &mut out);
            let lost: Vec<WorkspaceId> = out
                .iter()
                .filter(|(_, event)| event.need_rescan())
                .map(|(id, _)| *id)
                .collect();
            assert_eq!(lost, vec![busy_id]);

            out.clear();
            drain(calm_id, &mut calm, &mut buffer, &mut out);
            assert!(!out.is_empty());
            assert!(
                out.iter()
                    .all(|(id, event)| *id == calm_id && !event.need_rescan())
            );
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn refusal(error: &notify::Error) -> WatchRefused {
    match error.kind {
        notify::ErrorKind::MaxFilesWatch => WatchRefused::LimitReached,
        _ => WatchRefused::Unavailable,
    }
}

/// Stands in when no native watcher can be created, so every workspace takes the timer path.
struct NoBackend;

impl WatchBackend for NoBackend {
    fn watch_dir(&mut self, _dir: &Path) -> Result<(), WatchRefused> {
        Err(WatchRefused::Unsupported)
    }

    fn unwatch_dir(&mut self, _dir: &Path) {}
}

/// One per project: one backend and one thread that owns every watch, batch and timer, so the order of commands and notifications is the order they were sent. Only the whole-workspace walk after lost notifications runs on a scan thread of its own, so the hub keeps reading while it runs. Events leave through the sink on that thread, which keeps this crate free of an async runtime. The sink must return quickly, because every debounce deadline waits for it, and must never call the hub, whose commands wait for that same thread.
pub struct WatchHub {
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
}

impl WatchHub {
    pub fn start(config: WatchConfig, sink: impl Fn(WatchEvent) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let backend: Box<dyn WatchBackend> = match NativeBackend::new(Notifier(tx.clone())) {
            Ok(native) => Box::new(native),
            Err(_) => Box::new(NoBackend),
        };
        Self::spawn(tx, rx, backend, config, Box::new(sink))
    }

    /// A hub over fold's inotify backend whose instances come from `factory`, one call for each workspace that gets its first watch. A factory error of `EMFILE` or `ENFILE` puts only that workspace in timer mode with [`DegradedReason::InstanceLimit`].
    #[cfg(target_os = "linux")]
    pub fn start_with_instances(
        config: WatchConfig,
        factory: impl FnMut(WorkspaceId) -> std::io::Result<inotify::Inotify> + Send + 'static,
        sink: impl Fn(WatchEvent) + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let backend: Box<dyn WatchBackend> =
            match NativeBackend::with_factory(Notifier(tx.clone()), Box::new(factory)) {
                Ok(native) => Box::new(native),
                Err(_) => Box::new(NoBackend),
            };
        Self::spawn(tx, rx, backend, config, Box::new(sink))
    }

    /// A hub over any backend. The backend's notifications go to [`WatchHub::notifier`].
    pub fn start_with(
        backend: impl WatchBackend + 'static,
        config: WatchConfig,
        sink: impl Fn(WatchEvent) + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        Self::spawn(tx, rx, Box::new(backend), config, Box::new(sink))
    }

    fn spawn(
        tx: Sender<Msg>,
        rx: Receiver<Msg>,
        backend: Box<dyn WatchBackend>,
        config: WatchConfig,
        sink: Box<dyn Fn(WatchEvent) + Send>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (scans, jobs) = mpsc::channel();
        let scanner = {
            let done = tx.clone();
            let stop = Arc::clone(&stop);
            thread::Builder::new()
                .name("fold-watch-scan".to_owned())
                .spawn(move || scan_loop(&jobs, &done, &stop))
                .ok()
        };
        Self::spawn_parts(tx, rx, backend, config, sink, scans, scanner, stop)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the parts of a hub, which tests assemble with a scanner of their own"
    )]
    fn spawn_parts(
        tx: Sender<Msg>,
        rx: Receiver<Msg>,
        backend: Box<dyn WatchBackend>,
        config: WatchConfig,
        sink: Box<dyn Fn(WatchEvent) + Send>,
        scans: Sender<ScanJob>,
        scanner: Option<JoinHandle<()>>,
        stop: Arc<AtomicBool>,
    ) -> Self {
        let hub = Hub {
            backslash: backend.backslash_separates(),
            backend,
            config,
            sink,
            workspaces: BTreeMap::new(),
            outbox: Vec::new(),
            scans,
            scanner,
            stop,
            generation: 0,
        };
        let thread = thread::spawn(move || hub.run(&rx));
        Self {
            tx,
            thread: Some(thread),
        }
    }

    pub fn notifier(&self) -> Notifier {
        Notifier(self.tx.clone())
    }

    /// Returns once every tracked directory has its watch, so no change made after the call is missed. Adding an id again replaces the earlier workspace. `tracked` holds the paths that count although an ignore rule matches them.
    pub fn add_workspace(
        &self,
        id: WorkspaceId,
        root: &Path,
        rules: Arc<IgnoreRules>,
        tracked: Arc<TrackedSet>,
    ) {
        let (done, wait) = mpsc::channel();
        let sent = self.tx.send(Msg::Add {
            id,
            root: root.to_path_buf(),
            rules,
            tracked,
            done,
        });
        if sent.is_ok() {
            let _ = wait.recv();
        }
    }

    /// Picks the watches of the workspace again after its tracked set moved from `old` to what it holds now, so an edit below an ignored directory that now holds a tracked path is seen at once, and a directory that holds none any more loses its watch. Returns once the watches are in place.
    pub fn retrack(&self, id: WorkspaceId, old: Arc<FlatTree>) {
        let (done, wait) = mpsc::channel();
        if self.tx.send(Msg::Retrack { id, old, done }).is_ok() {
            let _ = wait.recv();
        }
    }

    /// Returns once the watches are gone and the unsent batch is dropped; no event for the workspace follows.
    pub fn remove_workspace(&self, id: WorkspaceId) {
        let (done, wait) = mpsc::channel();
        if self.tx.send(Msg::Remove { id, done }).is_ok() {
            let _ = wait.recv();
        }
    }

    /// The directories of the workspace that hold a watch, sorted.
    pub fn watched_dirs(&self, id: WorkspaceId) -> Vec<PathBuf> {
        let (reply, wait) = mpsc::channel();
        if self.tx.send(Msg::Watched { id, reply }).is_err() {
            return Vec::new();
        }
        wait.recv().unwrap_or_default()
    }

    /// Stops the thread and waits for it, which drops the sink.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = self.tx.send(Msg::Shutdown);
            let _ = thread.join();
        }
    }
}

impl Drop for WatchHub {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Hub {
    backend: Box<dyn WatchBackend>,
    backslash: bool,
    config: WatchConfig,
    sink: Box<dyn Fn(WatchEvent) + Send>,
    workspaces: BTreeMap<WorkspaceId, Workspace>,
    outbox: Vec<WatchEvent>,
    /// Whole-workspace walks go to the scan thread, so the hub keeps reading notifications while one runs.
    scans: Sender<ScanJob>,
    scanner: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    /// Tells a workspace from an earlier one added under the same id, so a late scan result never lands on it.
    generation: u64,
}

impl Hub {
    /// The hub reads the clock only in this loop; every decision below takes the instant read here as an input. A pending rescan starts once the hub has read everything sent so far, which holds the rest of the overflowed queue, or at the latest `debounce` after the loss.
    fn run(mut self, rx: &Receiver<Msg>) {
        loop {
            let now = Instant::now();
            self.fire(now);
            self.dispatch(now, false);
            self.send_out();
            let next = match rx.try_recv() {
                Ok(msg) => msg,
                Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {
                    self.dispatch(Instant::now(), true);
                    self.send_out();
                    match self.next_deadline() {
                        Some(deadline) => match rx
                            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                        {
                            Ok(msg) => msg,
                            Err(RecvTimeoutError::Timeout) => continue,
                            Err(RecvTimeoutError::Disconnected) => break,
                        },
                        None => match rx.recv() {
                            Ok(msg) => msg,
                            Err(_) => break,
                        },
                    }
                }
            };
            let now = Instant::now();
            match next {
                Msg::Raw(raw) => self.raw(raw, None, now),
                Msg::Attributed { workspace, event } => self.raw(event, Some(workspace), now),
                Msg::Add {
                    id,
                    root,
                    rules,
                    tracked,
                    done,
                } => {
                    self.add(id, root, rules, tracked, now);
                    self.send_out();
                    let _ = done.send(());
                }
                Msg::Remove { id, done } => {
                    if let Some(mut workspace) = self.workspaces.remove(&id) {
                        workspace.unwatch_below(self.backend.as_mut(), b"");
                    }
                    let _ = done.send(());
                }
                Msg::Retrack { id, old, done } => {
                    if let Some(workspace) = self
                        .workspaces
                        .get_mut(&id)
                        .filter(|workspace| !workspace.missing && workspace.root_exists())
                    {
                        let mut ctx = Ctx {
                            backend: self.backend.as_mut(),
                            config: &self.config,
                            out: &mut self.outbox,
                            now,
                        };
                        workspace.retrack(&mut ctx, &old);
                    }
                    self.send_out();
                    let _ = done.send(());
                }
                Msg::Watched { id, reply } => {
                    let dirs = self
                        .workspaces
                        .get(&id)
                        .map(|workspace| {
                            let mut dirs: Vec<PathBuf> = workspace
                                .watched
                                .values()
                                .map(|(dir, _, _)| dir.clone())
                                .collect();
                            dirs.sort();
                            dirs
                        })
                        .unwrap_or_default();
                    let _ = reply.send(dirs);
                }
                Msg::Scanned(scanned) => self.scanned(scanned, now),
                Msg::Shutdown => break,
            }
        }
        self.stop.store(true, Ordering::Relaxed);
        let Self { scans, scanner, .. } = self;
        drop(scans);
        if let Some(scanner) = scanner {
            let _ = scanner.join();
        }
    }

    fn send_out(&mut self) {
        for event in self.outbox.drain(..) {
            (self.sink)(event);
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.workspaces
            .values()
            .flat_map(|workspace| {
                [
                    workspace.batch.flush_at(&self.config),
                    workspace.next_tick,
                    workspace.retry_at,
                    workspace
                        .pending
                        .filter(|_| !workspace.scanning)
                        .and_then(|since| since.checked_add(self.config.debounce)),
                ]
            })
            .flatten()
            .min()
    }

    fn fire(&mut self, now: Instant) {
        for workspace in self.workspaces.values_mut() {
            if let Some(paths) = workspace.batch.take_due(now, &self.config) {
                tracing::debug!(workspace = %workspace.id, paths = paths.len(), "workspace changed");
                self.outbox.push(WatchEvent::Changed {
                    workspace: workspace.id,
                    paths,
                });
            }
            if let Some(tick) = workspace.next_tick.filter(|tick| *tick <= now) {
                if workspace.root_exists() {
                    workspace.next_tick = next_tick(tick, now, self.config.degraded_interval);
                    self.outbox.push(WatchEvent::RescanRequested {
                        workspace: workspace.id,
                    });
                } else {
                    workspace.root_missing(self.backend.as_mut(), &mut self.outbox);
                }
            }
            if workspace.retry_at.is_some_and(|retry| retry <= now) {
                workspace.retry_at = None;
                if workspace.degraded == Some(DegradedReason::Overflow) && !workspace.missing {
                    workspace.degraded = None;
                    workspace.next_tick = None;
                    workspace.overflows = Overflows::default();
                    workspace.restoring = true;
                    workspace.pending = Some(now);
                    tracing::info!(workspace = %workspace.id, "watching the workspace again after overflows");
                }
            }
        }
    }

    /// Sends each pending rescan to the scan thread: with `all` every one, else those waiting `debounce` or longer. A workspace has at most one scan running; a loss meanwhile waits as its next pending rescan.
    fn dispatch(&mut self, now: Instant, all: bool) {
        let debounce = self.config.debounce;
        let due: Vec<WorkspaceId> = self
            .workspaces
            .values()
            .filter(|workspace| !workspace.scanning)
            .filter(|workspace| {
                workspace.pending.is_some_and(|since| {
                    all || since.checked_add(debounce).is_some_and(|at| at <= now)
                })
            })
            .map(|workspace| workspace.id)
            .collect();
        for id in due {
            self.start_scan(id, now);
        }
    }

    fn start_scan(&mut self, id: WorkspaceId, now: Instant) {
        let Some(workspace) = self.workspaces.get_mut(&id) else {
            return;
        };
        workspace.pending = None;
        if workspace.missing {
            return;
        }
        if !workspace.root_exists() {
            workspace.root_missing(self.backend.as_mut(), &mut self.outbox);
            return;
        }
        // The whole-workspace scan covers whatever the batch held.
        workspace.batch.clear();
        if workspace.degraded.is_some() {
            self.outbox
                .push(WatchEvent::RescanRequested { workspace: id });
            return;
        }
        let job = ScanJob {
            id,
            generation: workspace.generation,
            since: workspace.stamp,
            root: workspace.root.clone(),
            rules: Arc::clone(&workspace.rules),
            tracked: Arc::clone(&workspace.tracked),
        };
        match self.scans.send(job) {
            Ok(()) => workspace.scanning = true,
            Err(mpsc::SendError(job)) => {
                // Without a scan thread the walk runs here, as it must still run somewhere.
                let scanned = job.run(&self.stop);
                workspace.scanning = true;
                self.scanned(scanned, now);
            }
        }
    }

    /// The result of a whole-workspace walk: the watches follow the tree as it was walked, except where a notification read during the walk already moved them.
    fn scanned(&mut self, scanned: Scanned, now: Instant) {
        let Some(workspace) = self
            .workspaces
            .get_mut(&scanned.id)
            .filter(|workspace| workspace.generation == scanned.generation)
        else {
            return;
        };
        workspace.scanning = false;
        if workspace.missing {
            return;
        }
        if !workspace.root_exists() {
            workspace.root_missing(self.backend.as_mut(), &mut self.outbox);
            return;
        }
        workspace.batch.clear();
        let mut ctx = Ctx {
            backend: self.backend.as_mut(),
            config: &self.config,
            out: &mut self.outbox,
            now,
        };
        workspace.apply(&mut ctx, scanned.walk, Some(scanned.since));
        if std::mem::take(&mut workspace.restoring) && workspace.degraded.is_none() {
            self.outbox.push(WatchEvent::Restored {
                workspace: scanned.id,
            });
        }
        tracing::info!(workspace = %scanned.id, "workspace rescanned after lost notifications");
        self.outbox.push(WatchEvent::RescanRequested {
            workspace: scanned.id,
        });
    }

    fn add(
        &mut self,
        id: WorkspaceId,
        root: PathBuf,
        rules: Arc<IgnoreRules>,
        tracked: Arc<TrackedSet>,
        now: Instant,
    ) {
        if let Some(mut old) = self.workspaces.remove(&id) {
            old.unwatch_below(self.backend.as_mut(), b"");
        }
        rules.reload();
        let real_root = fs::canonicalize(&root).ok().filter(|real| *real != root);
        self.generation += 1;
        let mut workspace = Workspace {
            generation: self.generation,
            stamp: 0,
            pending: None,
            scanning: false,
            overflows: Overflows::default(),
            retry_at: None,
            restoring: false,
            id,
            root,
            real_root,
            rules,
            tracked,
            watched: BTreeMap::new(),
            batch: Batch::default(),
            degraded: None,
            next_tick: None,
            missing: false,
        };
        if workspace.root_exists() {
            let mut ctx = Ctx {
                backend: self.backend.as_mut(),
                config: &self.config,
                out: &mut self.outbox,
                now,
            };
            workspace.sync_tree(&mut ctx, None, false);
        } else {
            workspace.root_missing(self.backend.as_mut(), &mut self.outbox);
        }
        self.workspaces.insert(id, workspace);
    }

    /// `owner` is the workspace whose own queue delivered the notification, `None` for a queue that several workspaces share.
    fn raw(
        &mut self,
        raw: notify::Result<notify::Event>,
        owner: Option<WorkspaceId>,
        now: Instant,
    ) {
        let event = match raw {
            Ok(event) if !event.need_rescan() => event,
            Ok(event) => return self.rescan(self.lost_in(owner, &event.paths), now),
            Err(error) => return self.rescan(self.lost_in(owner, &error.paths), now),
        };
        if matches!(event.kind, EventKind::Access(_) | EventKind::Other) {
            return;
        }
        for path in &event.paths {
            self.touched(path, owner, now);
        }
    }

    /// The workspaces whose notifications were lost: the owner of the queue, else every workspace that owns one of the paths, or every workspace when the platform named none.
    fn lost_in(&self, owner: Option<WorkspaceId>, paths: &[PathBuf]) -> Vec<WorkspaceId> {
        if let Some(owner) = owner {
            return vec![owner];
        }
        if paths.is_empty() {
            return self.workspaces.keys().copied().collect();
        }
        let owners: BTreeSet<WorkspaceId> =
            paths.iter().filter_map(|path| self.owner(path)).collect();
        owners.into_iter().collect()
    }

    /// The lost notifications may have named new or removed directories, so the workspace gets a rescan that brings the watches in line with the tree as well. Losses before that rescan starts join it. A workspace that loses notifications `overflow_limit` times within `overflow_window` drops its watches and goes to timer mode for `overflow_pause`.
    fn rescan(&mut self, ids: Vec<WorkspaceId>, now: Instant) {
        for id in ids {
            let Some(workspace) = self.workspaces.get_mut(&id) else {
                continue;
            };
            if workspace.missing {
                continue;
            }
            if !workspace.root_exists() {
                workspace.root_missing(self.backend.as_mut(), &mut self.outbox);
                continue;
            }
            if workspace.degraded.is_some() {
                workspace.batch.clear();
                self.outbox
                    .push(WatchEvent::RescanRequested { workspace: id });
                continue;
            }
            if workspace.overflows.record(now, &self.config) {
                let mut ctx = Ctx {
                    backend: self.backend.as_mut(),
                    config: &self.config,
                    out: &mut self.outbox,
                    now,
                };
                workspace.degrade(&mut ctx, DegradedReason::Overflow);
                workspace.unwatch_below(ctx.backend, b"");
                workspace.retry_at = now.checked_add(self.config.overflow_pause);
                workspace.pending = None;
                workspace.batch.clear();
                self.outbox
                    .push(WatchEvent::RescanRequested { workspace: id });
                continue;
            }
            if workspace.pending.is_none() {
                workspace.pending = Some(now);
                tracing::info!(workspace = %id, "notifications lost, rescan pending");
            }
        }
    }

    /// The workspace whose root holds the path; the deepest root wins.
    fn owner(&self, path: &Path) -> Option<WorkspaceId> {
        self.workspaces
            .values()
            .filter(|workspace| workspace.relative_os(path).is_some())
            .max_by_key(|workspace| workspace.root.components().count())
            .map(|workspace| workspace.id)
    }

    fn touched(&mut self, path: &Path, owner: Option<WorkspaceId>, now: Instant) {
        let Some(id) = owner.or_else(|| self.owner(path)) else {
            return;
        };
        let backslash = self.backslash;
        let Some(workspace) = self.workspaces.get_mut(&id) else {
            return;
        };
        if workspace.missing {
            return;
        }
        let Some(relative) = workspace.relative(path, backslash) else {
            return;
        };
        let mut ctx = Ctx {
            backend: self.backend.as_mut(),
            config: &self.config,
            out: &mut self.outbox,
            now,
        };
        match relative {
            None => {
                if !workspace.root_exists() {
                    workspace.root_missing(ctx.backend, ctx.out);
                }
            }
            Some(path) => workspace.touched(&mut ctx, path),
        }
    }
}

struct Ctx<'a> {
    backend: &'a mut dyn WatchBackend,
    config: &'a WatchConfig,
    out: &'a mut Vec<WatchEvent>,
    now: Instant,
}

/// The paths of one workspace waiting to leave as one `Changed`, with the instants of the first and the last notification. The instants come from the hub's monotonic clock and measure intervals only; none leaves this module, so they are not `Timestamp`s.
#[derive(Debug, Default)]
struct Batch {
    pending: BTreeSet<RepoPath>,
    window: Option<(Instant, Instant)>,
}

impl Batch {
    fn record(&mut self, path: RepoPath, now: Instant) {
        let first = self.window.map_or(now, |(first, _)| first);
        self.window = Some((first, now));
        self.pending.insert(path);
    }

    /// `debounce` after the last notification or `max_wait` after the first, whichever comes first. A duration too long to add to an instant never ends.
    fn flush_at(&self, config: &WatchConfig) -> Option<Instant> {
        let (first, last) = self.window?;
        let quiet = last.checked_add(config.debounce);
        let longest = first.checked_add(config.max_wait);
        quiet.into_iter().chain(longest).min()
    }

    /// The sorted, deduplicated paths once the batch is due at `now`.
    fn take_due(&mut self, now: Instant, config: &WatchConfig) -> Option<Vec<RepoPath>> {
        if self.flush_at(config)? > now {
            return None;
        }
        self.window = None;
        Some(std::mem::take(&mut self.pending).into_iter().collect())
    }

    fn clear(&mut self) {
        self.pending.clear();
        self.window = None;
    }
}

/// The timer-mode tick after the one due at `tick`. A hub that fell behind by more than an interval skips the missed ticks instead of sending them in a burst.
fn next_tick(tick: Instant, now: Instant, interval: Duration) -> Option<Instant> {
    match tick.checked_add(interval) {
        Some(next) if next > now => Some(next),
        _ => now.checked_add(interval),
    }
}

/// A directory's device and inode, so a directory replaced under the same name gets a fresh watch.
type DirIdentity = (u64, u64);

fn dir_identity(meta: &Metadata) -> DirIdentity {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

struct Workspace {
    id: WorkspaceId,
    generation: u64,
    /// Counts the watches this workspace has added; a scan result never drops a watch added after its walk began.
    stamp: u64,
    /// Since when a rescan waits to start.
    pending: Option<Instant>,
    scanning: bool,
    overflows: Overflows,
    /// When a workspace in timer mode for [`DegradedReason::Overflow`] gets its watches again.
    retry_at: Option<Instant>,
    /// The rescan that runs is the one that brings the watches back, so its end sends `Restored`.
    restoring: bool,
    root: PathBuf,
    /// The canonical root when it differs; some platforms report canonical paths whatever path was watched.
    real_root: Option<PathBuf>,
    rules: Arc<IgnoreRules>,
    tracked: Arc<TrackedSet>,
    /// Keyed by the workspace-relative directory bytes, empty for the root. The path is the exact one given to the backend, which keys its watches by it; the number is the workspace's `stamp` when the watch was added.
    watched: BTreeMap<Vec<u8>, (PathBuf, DirIdentity, u64)>,
    batch: Batch,
    degraded: Option<DegradedReason>,
    next_tick: Option<Instant>,
    missing: bool,
}

impl Workspace {
    fn root_exists(&self) -> bool {
        fs::metadata(&self.root).is_ok_and(|meta| meta.is_dir())
    }

    fn relative_os<'p>(&self, path: &'p Path) -> Option<&'p Path> {
        path.strip_prefix(&self.root)
            .ok()
            .or_else(|| path.strip_prefix(self.real_root.as_ref()?).ok())
    }

    /// `Some(None)` for the root itself; `None` for a path outside the root or one no `RepoPath` can name.
    fn relative(&self, path: &Path, backslash: bool) -> Option<Option<RepoPath>> {
        let mut names: Vec<Vec<u8>> = Vec::new();
        for component in self.relative_os(path)?.components() {
            let Component::Normal(name) = component else {
                return None;
            };
            let name = os_bytes(name)?;
            if backslash {
                names.extend(
                    name.split(|&byte| byte == b'\\')
                        .filter(|part| !part.is_empty())
                        .map(<[u8]>::to_vec),
                );
            } else {
                names.push(name);
            }
        }
        if names.is_empty() {
            return Some(None);
        }
        RepoPath::parse(&names.join(&b'/')).ok().map(Some)
    }

    fn full(&self, path: &RepoPath) -> PathBuf {
        self.root.join(os_path(path))
    }

    fn touched(&mut self, ctx: &mut Ctx<'_>, path: RepoPath) {
        if is_ignore_file(&path) {
            self.rules.reload();
            let folder = parent(&path);
            self.sync_tree(ctx, folder, false);
        }
        let full = self.full(&path);
        let identity = fs::symlink_metadata(&full)
            .ok()
            .filter(Metadata::is_dir)
            .map(|meta| dir_identity(&meta));
        let known = self
            .watched
            .get(path.as_bytes())
            .map(|(_, known, _)| *known);
        let tracked = self.tracked.get();
        let ignored =
            self.rules
                .cover(Some(&tracked), &path, identity.is_some() || known.is_some())
                == Cover::Ignored;
        if let Some(identity) = identity.filter(|_| !ignored) {
            // The directory path stands for everything below it; only a directory new to the hub is walked.
            if known != Some(identity) {
                self.sync_tree(ctx, Some(path.clone()), true);
            }
        } else {
            self.unwatch_below(ctx.backend, path.as_bytes());
        }
        if !ignored {
            self.batch.record(path, ctx.now);
        }
    }

    /// Brings the watches in line with the tracked set after it moved from `old`. A tracked path decides only whether a directory an ignore rule covers counts, so for each path that came or went, the topmost directory above it that a rule covers is looked at again: it keeps its watch and walks its tracked names while a tracked path is below it, and loses every watch below it otherwise.
    fn retrack(&mut self, ctx: &mut Ctx<'_>, old: &FlatTree) {
        let tracked = self.tracked.get();
        let came = tracked.keys().filter(|path| !old.contains_key(*path));
        let went = old.keys().filter(|path| !tracked.contains_key(*path));
        let mut tops: BTreeSet<RepoPath> = BTreeSet::new();
        for path in came.chain(went) {
            let Ok(path) = RepoPath::parse(path) else {
                continue;
            };
            let mut dir: Option<RepoPath> = None;
            for name in path.parents_and_name().0 {
                let Ok(next) = RepoPath::child(dir.as_ref(), name) else {
                    break;
                };
                if self.rules.is_ignored(&next, true) {
                    tops.insert(next);
                    break;
                }
                dir = Some(next);
            }
        }
        for top in tops {
            if Cover::of(true, Some(&tracked), &top, true) == Cover::Ignored {
                self.unwatch_below(ctx.backend, top.as_bytes());
            } else {
                self.sync_tree(ctx, Some(top), false);
            }
        }
    }

    /// Watches `dir` and every tracked directory below it that has no live watch, and drops the watches of directories below it that are gone or now ignored. With `report`, every tracked path found goes into the batch, so files made before the watch existed are not missed. A workspace in timer mode adds no watch, so without `report` there is nothing to walk for.
    fn sync_tree(&mut self, ctx: &mut Ctx<'_>, dir: Option<RepoPath>, report: bool) {
        if self.degraded.is_some() && !report {
            return;
        }
        let walk = walk(
            &self.root,
            &self.rules,
            &self.tracked.get(),
            dir,
            report,
            &AtomicBool::new(false),
        );
        self.apply(ctx, walk, None);
    }

    /// Brings the watches at and below the walk's start in line with what it found. With `since`, a watch added after the stamp `since`, while the walk ran on the scan thread, comes from a newer notification and stays as it is.
    fn apply(&mut self, ctx: &mut Ctx<'_>, walk: Walk, since: Option<u64>) {
        for path in walk.found {
            self.batch.record(path, ctx.now);
        }
        let newer = |stamp: u64| since.is_some_and(|since| stamp > since);
        let wanted: BTreeSet<&[u8]> = walk.dirs.iter().map(|(key, _, _)| key.as_slice()).collect();
        let stale: Vec<Vec<u8>> = entries_at_or_below(&self.watched, &walk.start)
            .filter(|(key, (_, _, stamp))| {
                !wanted.contains(key.as_slice())
                    && !newer(*stamp)
                    && !walk.unread.iter().any(|dir| strictly_below(key, dir))
            })
            .map(|(key, _)| key.clone())
            .collect();
        let subtrees = ctx.backend.watches_subtrees();
        for key in stale {
            if let Some((full, _, _)) = self.watched.remove(&key)
                && (key.is_empty() || !subtrees)
            {
                ctx.backend.unwatch_in(self.id, &full);
            }
        }
        for (key, full, identity) in walk.dirs {
            if self
                .watched
                .get(&key)
                .is_some_and(|(_, _, stamp)| newer(*stamp))
            {
                continue;
            }
            self.watch(ctx, key, full, identity);
        }
    }

    fn watch(&mut self, ctx: &mut Ctx<'_>, key: Vec<u8>, full: PathBuf, identity: DirIdentity) {
        match self.watched.get(&key) {
            Some((_, known, _)) if *known == identity => return,
            Some(_) => self.unwatch_below(ctx.backend, &key),
            None => {}
        }
        if self.degraded.is_some() {
            return;
        }
        if !key.is_empty() && ctx.backend.watches_subtrees() {
            self.stamp += 1;
            self.watched.insert(key, (full, identity, self.stamp));
            return;
        }
        match ctx.backend.watch_in(self.id, &full) {
            Ok(()) => {
                self.stamp += 1;
                self.watched.insert(key, (full, identity, self.stamp));
            }
            Err(WatchRefused::LimitReached) => self.degrade(ctx, DegradedReason::WatchLimit),
            Err(WatchRefused::Unsupported) => self.degrade(ctx, DegradedReason::NoNativeWatcher),
            Err(WatchRefused::InstanceLimit) => self.degrade(ctx, DegradedReason::InstanceLimit),
            Err(WatchRefused::Unavailable) => {
                tracing::debug!(workspace = %self.id, "a directory could not be watched");
            }
        }
    }

    /// Timer mode: no new watch is tried and a rescan is requested every `degraded_interval`. The watches that exist stay, unless the hub drops them for [`DegradedReason::Overflow`].
    fn degrade(&mut self, ctx: &mut Ctx<'_>, reason: DegradedReason) {
        if self.degraded.is_some() {
            return;
        }
        self.degraded = Some(reason);
        self.next_tick = ctx.now.checked_add(ctx.config.degraded_interval);
        tracing::warn!(
            workspace = %self.id,
            reason = reason.as_str(),
            watches = self.watched.len(),
            "file watching degraded to timer scans"
        );
        ctx.out.push(WatchEvent::Degraded {
            workspace: self.id,
            reason,
        });
    }

    /// Drops the watch on the directory `key` and on every directory below it; an empty key drops them all.
    fn unwatch_below(&mut self, backend: &mut dyn WatchBackend, key: &[u8]) {
        let below: Vec<Vec<u8>> = entries_at_or_below(&self.watched, key)
            .map(|(dir, _)| dir.clone())
            .collect();
        let subtrees = backend.watches_subtrees();
        for dir in below {
            if let Some((full, _, _)) = self.watched.remove(&dir)
                && (dir.is_empty() || !subtrees)
            {
                backend.unwatch_in(self.id, &full);
            }
        }
    }

    fn root_missing(&mut self, backend: &mut dyn WatchBackend, out: &mut Vec<WatchEvent>) {
        if self.missing {
            return;
        }
        self.missing = true;
        self.batch.clear();
        self.next_tick = None;
        self.unwatch_below(backend, b"");
        tracing::info!(workspace = %self.id, "workspace root missing");
        out.push(WatchEvent::RootMissing { workspace: self.id });
    }
}

/// The directories a walk found, in the order it met them, so a parent gets its watch before its children.
#[derive(Debug, Default)]
struct Walk {
    /// The key of the directory the walk started at, empty for the root.
    start: Vec<u8>,
    dirs: Vec<(Vec<u8>, PathBuf, DirIdentity)>,
    /// Directories that could not be listed; the watches below them stay as they are.
    unread: Vec<Vec<u8>>,
    /// Every tracked path found, when the walk reports them.
    found: Vec<RepoPath>,
}

/// True when the directory `key` lies below the directory `dir`, not at it.
fn strictly_below(key: &[u8], dir: &[u8]) -> bool {
    if dir.is_empty() {
        return !key.is_empty();
    }
    key.len() > dir.len() && key.starts_with(dir) && key[dir.len()] == b'/'
}

/// The tracked directories at and below `dir`. A directory an ignore rule covers counts only as far as tracked paths are below it, and only their names are looked at there. Reads the files only, so it runs on any thread; `stop` ends it early.
fn walk(
    root: &Path,
    rules: &IgnoreRules,
    tracked: &FlatTree,
    dir: Option<RepoPath>,
    report: bool,
    stop: &AtomicBool,
) -> Walk {
    let full_of = |path: &RepoPath| root.join(os_path(path));
    let mut out = Walk {
        start: dir
            .as_ref()
            .map_or_else(Vec::new, |dir| dir.as_bytes().to_vec()),
        ..Walk::default()
    };
    let covered = dir.as_ref().is_some_and(|dir| rules.is_ignored(dir, true));
    let mut stack = vec![(dir, covered)];
    while let Some((dir, covered)) = stack.pop() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let key: Vec<u8> = dir
            .as_ref()
            .map_or_else(Vec::new, |dir| dir.as_bytes().to_vec());
        let full = match &dir {
            Some(dir) => full_of(dir),
            None => root.to_path_buf(),
        };
        let meta = match fs::symlink_metadata(&full) {
            Ok(meta) if meta.is_dir() => meta,
            _ => continue,
        };
        out.dirs
            .push((key.clone(), full.clone(), dir_identity(&meta)));
        let children: Vec<(RepoPath, bool, Cover)> = if covered {
            tracked_names(Some(tracked), dir.as_ref())
                .into_iter()
                .filter_map(|name| {
                    let child = RepoPath::child(dir.as_ref(), &name).ok()?;
                    let is_dir = fs::symlink_metadata(full_of(&child)).ok()?.is_dir();
                    let cover = Cover::of(true, Some(tracked), &child, is_dir);
                    Some((child, is_dir, cover))
                })
                .collect()
        } else {
            let Ok(reader) = fs::read_dir(&full) else {
                out.unread.push(key);
                continue;
            };
            let rules = rules.dir_rules(dir.as_ref());
            reader
                .flatten()
                .filter_map(|entry| {
                    let name = os_bytes(&entry.file_name())?;
                    if is_reserved_name(&name) {
                        return None;
                    }
                    let child = RepoPath::child(dir.as_ref(), &name).ok()?;
                    let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
                    let cover = rules.cover(Some(tracked), &child, is_dir);
                    Some((child, is_dir, cover))
                })
                .collect()
        };
        for (child, is_dir, cover) in children {
            if cover == Cover::Ignored {
                continue;
            }
            if report {
                out.found.push(child.clone());
            }
            if is_dir {
                stack.push((Some(child), cover.is_covered()));
            }
        }
    }
    out
}

/// One whole-workspace walk for the scan thread, with what it needs to run without the hub.
struct ScanJob {
    id: WorkspaceId,
    generation: u64,
    since: u64,
    root: PathBuf,
    rules: Arc<IgnoreRules>,
    tracked: Arc<TrackedSet>,
}

impl ScanJob {
    fn run(self, stop: &AtomicBool) -> Scanned {
        // Lost notifications may have named a changed ignore file.
        self.rules.reload();
        let walk = walk(
            &self.root,
            &self.rules,
            &self.tracked.get(),
            None,
            false,
            stop,
        );
        Scanned {
            id: self.id,
            generation: self.generation,
            since: self.since,
            walk,
        }
    }
}

struct Scanned {
    id: WorkspaceId,
    generation: u64,
    since: u64,
    walk: Walk,
}

/// Runs each job and hands its result back to the hub until the hub stops.
fn scan_loop(jobs: &Receiver<ScanJob>, done: &Sender<Msg>, stop: &AtomicBool) {
    while let Ok(job) = jobs.recv() {
        if stop.load(Ordering::Relaxed) || done.send(Msg::Scanned(job.run(stop))).is_err() {
            return;
        }
    }
}

/// The recent losses of notifications of one workspace. The instants come from the hub's monotonic clock and only measure the window.
#[derive(Debug, Default)]
struct Overflows {
    recent: Vec<Instant>,
}

impl Overflows {
    /// Records a loss at `now`. True when it is the `overflow_limit`-th within `overflow_window`; the record then starts empty again.
    fn record(&mut self, now: Instant, config: &WatchConfig) -> bool {
        if config.overflow_limit == 0 {
            return false;
        }
        self.recent
            .retain(|at| now.saturating_duration_since(*at) < config.overflow_window);
        self.recent.push(now);
        if self.recent.len() >= config.overflow_limit as usize {
            self.recent.clear();
            return true;
        }
        false
    }
}

fn is_ignore_file(path: &RepoPath) -> bool {
    path.components()
        .last()
        .is_some_and(|name| name == GITIGNORE.as_bytes() || name == FOLDIGNORE.as_bytes())
}

fn parent(path: &RepoPath) -> Option<RepoPath> {
    let bytes = path.as_bytes();
    let at = bytes.iter().rposition(|&byte| byte == b'/')?;
    RepoPath::parse(&bytes[..at]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn refusals_map_from_notify_errors() {
        let limit = notify::Error::new(notify::ErrorKind::MaxFilesWatch);
        assert_eq!(refusal(&limit), WatchRefused::LimitReached);
        assert_eq!(
            refusal(&notify::Error::path_not_found()),
            WatchRefused::Unavailable
        );
        let io = notify::Error::io(std::io::Error::other("x"));
        assert_eq!(refusal(&io), WatchRefused::Unavailable);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn native_backend_refuses_a_missing_directory() {
        let (tx, _rx) = mpsc::channel();
        let mut native = NativeBackend::new(Notifier(tx)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            native.watch_dir(&dir.path().join("absent")),
            Err(WatchRefused::Unavailable)
        );
        native.watch_dir(dir.path()).unwrap();
        native.unwatch_dir(dir.path());
        native.unwatch_dir(dir.path());
        assert_eq!(native.backslash_separates(), cfg!(windows));
    }

    #[test]
    fn reasons_have_stable_names() {
        assert_eq!(DegradedReason::WatchLimit.as_str(), "watch_limit");
        assert_eq!(
            DegradedReason::NoNativeWatcher.as_str(),
            "no_native_watcher"
        );
        assert_eq!(DegradedReason::InstanceLimit.as_str(), "instance_limit");
        assert_eq!(WatchConfig::default().debounce, Duration::from_millis(500));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn inotify_refusals_map_from_errnos() {
        use std::io::Error;
        for errno in [libc::EMFILE, libc::ENFILE] {
            assert_eq!(
                linux::instance_refusal(&Error::from_raw_os_error(errno)),
                WatchRefused::InstanceLimit
            );
        }
        assert_eq!(
            linux::instance_refusal(&Error::from_raw_os_error(libc::ENOSYS)),
            WatchRefused::Unsupported
        );
        assert_eq!(
            linux::watch_refusal(&Error::from_raw_os_error(libc::ENOSPC)),
            WatchRefused::LimitReached
        );
        assert_eq!(
            linux::watch_refusal(&Error::from_raw_os_error(libc::ENOENT)),
            WatchRefused::Unavailable
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn inotify_backend_watches_only_per_workspace() {
        let (tx, _rx) = mpsc::channel();
        let mut native = NativeBackend::new(Notifier(tx)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let id = WorkspaceId::from_number(1).unwrap();
        assert_eq!(native.watch_dir(dir.path()), Err(WatchRefused::Unsupported));
        native.unwatch_dir(dir.path());
        assert_eq!(
            native.watch_in(id, &dir.path().join("absent")),
            Err(WatchRefused::Unavailable)
        );
        native.watch_in(id, dir.path()).unwrap();
        native.unwatch_in(id, dir.path());
        native.unwatch_in(id, dir.path());
        assert!(!native.backslash_separates());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn inotify_events_translate_to_the_hubs_terms() {
        use inotify::EventMask;
        use notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};
        use std::collections::HashMap;
        use std::ffi::OsStr;

        let dirs: HashMap<i32, PathBuf> = [(1, PathBuf::from("/w/src"))].into();
        let name = Some(OsStr::new("a"));
        let kind = |mask: EventMask, name: Option<&OsStr>| {
            linux::translate(&dirs, 1, mask, name).map(|event| (event.kind, event.paths))
        };
        let at = |text: &str| vec![PathBuf::from(text)];

        // An overflow has no watch and no path; it is the queue's own, so it stands for the queue's workspace.
        let lost = linux::translate(&dirs, -1, EventMask::Q_OVERFLOW, None).unwrap();
        assert!(lost.need_rescan());
        assert!(lost.paths.is_empty());

        assert_eq!(
            kind(EventMask::CREATE | EventMask::ISDIR, name),
            Some((EventKind::Create(CreateKind::Folder), at("/w/src/a")))
        );
        assert_eq!(
            kind(EventMask::CREATE, name).map(|(kind, _)| kind),
            Some(EventKind::Create(CreateKind::File))
        );
        assert_eq!(
            kind(EventMask::DELETE | EventMask::ISDIR, name).map(|(kind, _)| kind),
            Some(EventKind::Remove(RemoveKind::Folder))
        );
        assert_eq!(
            kind(EventMask::DELETE, name).map(|(kind, _)| kind),
            Some(EventKind::Remove(RemoveKind::File))
        );
        assert_eq!(
            kind(EventMask::DELETE_SELF, None),
            Some((EventKind::Remove(RemoveKind::Any), at("/w/src")))
        );
        assert_eq!(
            kind(EventMask::MOVED_FROM, name).map(|(kind, _)| kind),
            Some(EventKind::Modify(ModifyKind::Name(RenameMode::From)))
        );
        assert_eq!(
            kind(EventMask::MOVED_TO, name).map(|(kind, _)| kind),
            Some(EventKind::Modify(ModifyKind::Name(RenameMode::To)))
        );
        assert_eq!(
            kind(EventMask::MOVE_SELF, None).map(|(kind, _)| kind),
            Some(EventKind::Modify(ModifyKind::Name(RenameMode::Any)))
        );
        for mask in [EventMask::MODIFY, EventMask::CLOSE_WRITE, EventMask::ATTRIB] {
            assert!(matches!(
                kind(mask, name),
                Some((EventKind::Modify(_), paths)) if paths == at("/w/src/a")
            ));
        }
        for mask in [EventMask::OPEN, EventMask::ACCESS, EventMask::CLOSE_NOWRITE] {
            assert_eq!(kind(mask, name), None);
        }
        // A watch the backend no longer knows names nothing.
        assert!(linux::translate(&dirs, 9, EventMask::CREATE, name).is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_inotify_mask_holds_no_read_events() {
        use inotify::WatchMask;
        assert!(
            !INOTIFY_MASK
                .intersects(WatchMask::OPEN | WatchMask::ACCESS | WatchMask::CLOSE_NOWRITE)
        );
        assert!(INOTIFY_MASK.contains(
            WatchMask::CREATE
                | WatchMask::DELETE
                | WatchMask::CLOSE_WRITE
                | WatchMask::MODIFY
                | WatchMask::MOVED_FROM
                | WatchMask::MOVED_TO
                | WatchMask::ATTRIB
                | WatchMask::DELETE_SELF
                | WatchMask::MOVE_SELF
        ));
    }

    fn config() -> WatchConfig {
        WatchConfig {
            debounce: Duration::from_millis(100),
            max_wait: Duration::from_millis(1_000),
            degraded_interval: Duration::from_millis(500),
            ..WatchConfig::default()
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn repo(text: &str) -> RepoPath {
        RepoPath::parse(text.as_bytes()).unwrap()
    }

    #[test]
    fn a_batch_leaves_after_quiet_time_or_max_wait() {
        let config = config();
        let start = Instant::now();
        let mut batch = Batch::default();
        assert_eq!(batch.flush_at(&config), None);
        assert_eq!(batch.take_due(start, &config), None);

        batch.record(repo("b"), start);
        batch.record(repo("a"), start + ms(50));
        batch.record(repo("b"), start + ms(60));
        assert_eq!(batch.flush_at(&config), Some(start + ms(160)));
        assert_eq!(batch.take_due(start + ms(159), &config), None);
        assert_eq!(
            batch.take_due(start + ms(160), &config),
            Some(vec![repo("a"), repo("b")])
        );
        assert_eq!(batch.flush_at(&config), None);

        let later = start + ms(5_000);
        for step in 0..20 {
            batch.record(repo("busy"), later + ms(step * 90));
        }
        assert_eq!(batch.flush_at(&config), Some(later + ms(1_000)));
        batch.clear();
        assert_eq!(batch.take_due(later + ms(10_000), &config), None);
    }

    #[test]
    fn a_duration_past_the_end_of_time_never_ends() {
        let start = Instant::now();
        let endless = WatchConfig {
            debounce: Duration::MAX,
            max_wait: Duration::MAX,
            degraded_interval: Duration::MAX,
            ..WatchConfig::default()
        };
        let mut batch = Batch::default();
        batch.record(repo("a"), start);
        assert_eq!(batch.flush_at(&endless), None);
        assert_eq!(batch.take_due(start + ms(1), &endless), None);
        let quiet_only = WatchConfig {
            max_wait: Duration::MAX,
            ..config()
        };
        assert_eq!(batch.flush_at(&quiet_only), Some(start + ms(100)));
        assert_eq!(next_tick(start, start, Duration::MAX), None);
    }

    #[test]
    fn ticks_keep_their_rhythm_and_skip_what_was_missed() {
        let start = Instant::now();
        let interval = ms(500);
        assert_eq!(
            next_tick(start, start + ms(10), interval),
            Some(start + ms(500))
        );
        assert_eq!(
            next_tick(start, start + ms(500), interval),
            Some(start + ms(1_000))
        );
        assert_eq!(
            next_tick(start, start + ms(2_300), interval),
            Some(start + ms(2_800))
        );
    }

    /// Takes every batch due by `until` (all of them when `None`) and checks each against the notifications it holds.
    fn drain(
        batch: &mut Batch,
        open: &mut Vec<(Instant, RepoPath)>,
        config: &WatchConfig,
        until: Option<Instant>,
    ) -> Result<(), proptest::test_runner::TestCaseError> {
        while let Some(at) = batch
            .flush_at(config)
            .filter(|at| until.is_none_or(|until| *at <= until))
        {
            let first = open.first().map(|(at, _)| *at).unwrap();
            let last = open.last().map(|(at, _)| *at).unwrap();
            proptest::prop_assert_eq!(at, (last + config.debounce).min(first + config.max_wait));
            proptest::prop_assert_eq!(batch.take_due(at - ms(1), config), None);
            let paths = batch.take_due(at, config).unwrap();
            let wanted: BTreeSet<RepoPath> = open.drain(..).map(|(_, path)| path).collect();
            proptest::prop_assert_eq!(paths, wanted.into_iter().collect::<Vec<_>>());
        }
        Ok(())
    }

    proptest::proptest! {
        /// Driven the way the hub drives it, every recorded path leaves in the first batch due after it was recorded, each batch is sorted and deduplicated, and a batch leaves exactly `debounce` after its last notification or `max_wait` after its first, whichever comes first.
        #[test]
        fn batches_follow_debounce_and_max_wait(
            debounce in 1u64..200,
            max_wait in 1u64..1_000,
            steps in proptest::collection::vec((0u64..300, 0usize..8), 1..60),
        ) {
            let config = WatchConfig {
                debounce: ms(debounce),
                max_wait: ms(max_wait),
                degraded_interval: ms(1),
                ..WatchConfig::default()
            };
            let names: Vec<RepoPath> = (0..8).map(|n| repo(&format!("f{n}"))).collect();
            let mut now = Instant::now();
            let mut batch = Batch::default();
            let mut open = Vec::new();
            for (gap, name) in steps {
                now += ms(gap);
                drain(&mut batch, &mut open, &config, Some(now))?;
                batch.record(names[name].clone(), now);
                open.push((now, names[name].clone()));
            }
            drain(&mut batch, &mut open, &config, None)?;
            proptest::prop_assert!(open.is_empty());
        }
    }

    #[test]
    fn three_losses_within_the_window_trip_the_limit() {
        let config = WatchConfig::default();
        let start = Instant::now();
        let mut overflows = Overflows::default();
        assert!(!overflows.record(start, &config));
        assert!(!overflows.record(start + ms(1_000), &config));
        assert!(overflows.record(start + ms(59_999), &config));
        // The record starts empty again after it tripped.
        assert!(!overflows.record(start + ms(60_000), &config));

        // Losses spread wider than the window never add up.
        let mut spread = Overflows::default();
        for step in 0..10 {
            assert!(!spread.record(start + ms(step * 30_000), &config));
        }

        let never = WatchConfig {
            overflow_limit: 0,
            ..config
        };
        let mut off = Overflows::default();
        for _ in 0..10 {
            assert!(!off.record(start, &never));
        }
        assert_eq!(DegradedReason::Overflow.as_str(), "overflow");
        assert_eq!(config.overflow_limit, 3);
        assert_eq!(config.overflow_window, Duration::from_secs(60));
        assert_eq!(config.overflow_pause, Duration::from_secs(600));
    }

    #[test]
    fn a_walk_is_below_only_strictly() {
        assert!(strictly_below(b"a", b""));
        assert!(!strictly_below(b"", b""));
        assert!(strictly_below(b"a/b", b"a"));
        assert!(!strictly_below(b"a", b"a"));
        assert!(!strictly_below(b"ab", b"a"));
    }

    /// Accepts every watch.
    struct Accept;

    impl WatchBackend for Accept {
        fn watch_dir(&mut self, _dir: &Path) -> Result<(), WatchRefused> {
            Ok(())
        }

        fn unwatch_dir(&mut self, _dir: &Path) {}
    }

    const WAIT: Duration = Duration::from_secs(10);

    fn ws(number: u32) -> WorkspaceId {
        WorkspaceId::from_number(number).unwrap()
    }

    fn overflow() -> notify::Result<notify::Event> {
        Ok(notify::Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan))
    }

    fn add(tx: &Sender<Msg>, id: WorkspaceId, root: &Path) {
        let (done, _) = mpsc::channel();
        tx.send(Msg::Add {
            id,
            root: root.to_path_buf(),
            rules: Arc::new(IgnoreRules::new(root)),
            tracked: Arc::default(),
            done,
        })
        .unwrap();
    }

    /// A hub whose whole-workspace scans wait in `jobs` until the test runs them, over the messages already sent to `tx`.
    fn held_hub(
        tx: Sender<Msg>,
        rx: Receiver<Msg>,
    ) -> (WatchHub, Receiver<ScanJob>, Receiver<WatchEvent>) {
        let (scans, jobs) = mpsc::channel();
        let (sink, events) = mpsc::channel();
        let hub = WatchHub::spawn_parts(
            tx,
            rx,
            Box::new(Accept),
            WatchConfig {
                overflow_limit: 0,
                ..config()
            },
            Box::new(move |event| {
                let _ = sink.send(event);
            }),
            scans,
            None,
            Arc::new(AtomicBool::new(false)),
        );
        (hub, jobs, events)
    }

    fn finish(hub: &WatchHub, job: ScanJob) {
        hub.tx
            .send(Msg::Scanned(job.run(&AtomicBool::new(false))))
            .unwrap();
    }

    #[test]
    fn an_overflow_rescans_only_its_workspace_once() {
        let parent = tempfile::tempdir().unwrap();
        let one = parent.path().join("one");
        let two = parent.path().join("two");
        fs::create_dir_all(one.join("old")).unwrap();
        fs::create_dir(&two).unwrap();
        let (tx, rx) = mpsc::channel();
        add(&tx, ws(1), &one);
        add(&tx, ws(2), &two);
        // Both losses are read before the rescan starts, so they join it.
        tx.send(Msg::Attributed {
            workspace: ws(1),
            event: overflow(),
        })
        .unwrap();
        tx.send(Msg::Attributed {
            workspace: ws(1),
            event: overflow(),
        })
        .unwrap();
        let (hub, jobs, events) = held_hub(tx, rx);

        let job = jobs.recv_timeout(WAIT).unwrap();
        assert_eq!(job.id, ws(1));
        fs::remove_dir(one.join("old")).unwrap();
        fs::create_dir(one.join("new")).unwrap();
        finish(&hub, job);
        assert_eq!(
            events.recv_timeout(WAIT).unwrap(),
            WatchEvent::RescanRequested { workspace: ws(1) }
        );
        assert_eq!(hub.watched_dirs(ws(1)), vec![one.clone(), one.join("new")]);
        assert_eq!(hub.watched_dirs(ws(2)), vec![two.clone()]);
        assert!(jobs.try_recv().is_err());

        // Losses while a rescan runs wait as one more rescan, which starts when the first ends.
        hub.notifier().send_for(ws(1), overflow());
        let running = jobs.recv_timeout(WAIT).unwrap();
        hub.notifier().send_for(ws(1), overflow());
        hub.notifier().send_for(ws(1), overflow());
        assert_eq!(hub.watched_dirs(ws(1)).len(), 2);
        assert!(jobs.try_recv().is_err());
        finish(&hub, running);
        assert_eq!(
            events.recv_timeout(WAIT).unwrap(),
            WatchEvent::RescanRequested { workspace: ws(1) }
        );
        let again = jobs.recv_timeout(WAIT).unwrap();
        assert_eq!(again.id, ws(1));
        finish(&hub, again);
        assert_eq!(
            events.recv_timeout(WAIT).unwrap(),
            WatchEvent::RescanRequested { workspace: ws(1) }
        );
        assert_eq!(hub.watched_dirs(ws(1)).len(), 2);
        assert!(jobs.try_recv().is_err());
    }

    #[test]
    fn the_hub_reads_other_workspaces_while_a_rescan_runs() {
        let parent = tempfile::tempdir().unwrap();
        let one = parent.path().join("one");
        let two = parent.path().join("two");
        fs::create_dir(&one).unwrap();
        fs::create_dir(&two).unwrap();
        let (tx, rx) = mpsc::channel();
        add(&tx, ws(1), &one);
        add(&tx, ws(2), &two);
        let (hub, jobs, events) = held_hub(tx, rx);
        let notifier = hub.notifier();
        notifier.send_for(ws(1), overflow());
        // The walk is done, but the hub has not heard of it yet.
        let stale = jobs
            .recv_timeout(WAIT)
            .unwrap()
            .run(&AtomicBool::new(false));

        // The scan of the first workspace has not ended, and the second still reports its changes.
        let created = notify::Event::new(EventKind::Create(notify::event::CreateKind::File))
            .add_path(two.join("b"));
        notifier.send_for(ws(2), Ok(created));
        assert_eq!(
            events.recv_timeout(WAIT).unwrap(),
            WatchEvent::Changed {
                workspace: ws(2),
                paths: vec![repo("b")],
            }
        );
        // A directory made during the scan gets its watch from its own notification, and the older scan result keeps it.
        fs::create_dir(one.join("late")).unwrap();
        let made = notify::Event::new(EventKind::Create(notify::event::CreateKind::Folder))
            .add_path(one.join("late"));
        notifier.send_for(ws(1), Ok(made));
        assert_eq!(hub.watched_dirs(ws(1)), vec![one.clone(), one.join("late")]);

        hub.tx.send(Msg::Scanned(stale)).unwrap();
        assert_eq!(
            events.recv_timeout(WAIT).unwrap(),
            WatchEvent::RescanRequested { workspace: ws(1) }
        );
        assert_eq!(hub.watched_dirs(ws(1)), vec![one.clone(), one.join("late")]);
    }

    #[test]
    fn a_scan_result_for_a_removed_workspace_does_nothing() {
        let parent = tempfile::tempdir().unwrap();
        let one = parent.path().join("one");
        let other = parent.path().join("other");
        fs::create_dir_all(one.join("sub")).unwrap();
        fs::create_dir(&other).unwrap();
        let (tx, rx) = mpsc::channel();
        add(&tx, ws(1), &one);
        let (hub, jobs, events) = held_hub(tx, rx);

        // Removed during the walk: the late result finds no workspace.
        hub.notifier().send_for(ws(1), overflow());
        let gone = jobs.recv_timeout(WAIT).unwrap();
        hub.remove_workspace(ws(1));
        finish(&hub, gone);
        assert!(hub.watched_dirs(ws(1)).is_empty());

        // Removed and added again under the same id: the late result of the earlier workspace leaves the new one alone.
        hub.add_workspace(
            ws(1),
            &other,
            Arc::new(IgnoreRules::new(&other)),
            Arc::default(),
        );
        hub.notifier().send_for(ws(1), overflow());
        let stale = jobs.recv_timeout(WAIT).unwrap();
        hub.remove_workspace(ws(1));
        hub.add_workspace(
            ws(1),
            &one,
            Arc::new(IgnoreRules::new(&one)),
            Arc::default(),
        );
        finish(&hub, stale);
        assert_eq!(hub.watched_dirs(ws(1)), vec![one.clone(), one.join("sub")]);
        assert!(events.try_recv().is_err());
        assert!(jobs.try_recv().is_err());
    }

    #[test]
    fn ignore_files_and_parents() {
        let nested = RepoPath::parse(b"a/b/.gitignore").unwrap();
        assert!(is_ignore_file(&nested));
        assert_eq!(parent(&nested).unwrap().as_bytes(), b"a/b");
        let top = RepoPath::parse(b".foldignore").unwrap();
        assert!(is_ignore_file(&top));
        assert!(parent(&top).is_none());
        assert!(!is_ignore_file(&RepoPath::parse(b"a/x.gitignore").unwrap()));
    }
}
