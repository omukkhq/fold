use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, Metadata};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use fold_core::WorkspaceId;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::ignore::{FOLDIGNORE, GITIGNORE, IgnoreRules};
use crate::paths::{RepoPath, is_reserved_name, os_bytes, os_path};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchConfig {
    /// Quiet time after the last notification before a batch leaves.
    pub debounce: Duration,
    /// Longest time a batch is held after its first notification, however busy the workspace stays.
    pub max_wait: Duration,
    /// Time between two `RescanRequested` events of a workspace in timer mode.
    pub degraded_interval: Duration,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            debounce: Duration::from_millis(500),
            max_wait: Duration::from_secs(2),
            degraded_interval: Duration::from_secs(10),
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
}

/// The platform's native notifications. A backend delivers what it observes through the [`Notifier`] of its hub.
pub trait WatchBackend: Send {
    /// One non-recursive watch on `dir`.
    fn watch_dir(&mut self, dir: &Path) -> Result<(), WatchRefused>;
    /// Removing a watch the backend already dropped is not an error.
    fn unwatch_dir(&mut self, dir: &Path);
    /// True when the reported paths use `\` as a separator. On other platforms `\` is an ordinary file name byte and must stay one.
    fn backslash_separates(&self) -> bool {
        cfg!(windows)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DegradedReason {
    WatchLimit,
    NoNativeWatcher,
}

impl DegradedReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WatchLimit => "watch_limit",
            Self::NoNativeWatcher => "no_native_watcher",
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
}

enum Msg {
    Raw(notify::Result<notify::Event>),
    Add {
        id: WorkspaceId,
        root: PathBuf,
        rules: Arc<IgnoreRules>,
        done: Sender<()>,
    },
    Remove {
        id: WorkspaceId,
        done: Sender<()>,
    },
    Watched {
        id: WorkspaceId,
        reply: Sender<Vec<PathBuf>>,
    },
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
}

impl notify::EventHandler for Notifier {
    fn handle_event(&mut self, event: notify::Result<notify::Event>) {
        self.send(event);
    }
}

pub(crate) struct NativeBackend {
    watcher: RecommendedWatcher,
}

impl NativeBackend {
    pub(crate) fn new(notifier: Notifier) -> notify::Result<Self> {
        Ok(Self {
            watcher: RecommendedWatcher::new(notifier, notify::Config::default())?,
        })
    }
}

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

/// One per project: one backend and one thread that owns every watch, batch and timer, so the order of commands and notifications is the order they were sent. Events leave through the sink on that thread, which keeps this crate free of an async runtime. The sink must return quickly, because every debounce deadline waits for it, and must never call the hub, whose commands wait for that same thread.
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
        let hub = Hub {
            backslash: backend.backslash_separates(),
            backend,
            config,
            sink,
            workspaces: BTreeMap::new(),
            outbox: Vec::new(),
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

    /// Returns once every tracked directory has its watch, so no change made after the call is missed. Adding an id again replaces the earlier workspace.
    pub fn add_workspace(&self, id: WorkspaceId, root: &Path, rules: Arc<IgnoreRules>) {
        let (done, wait) = mpsc::channel();
        let sent = self.tx.send(Msg::Add {
            id,
            root: root.to_path_buf(),
            rules,
            done,
        });
        if sent.is_ok() {
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
}

impl Hub {
    /// The hub reads the clock only in this loop; every decision below takes the instant read here as an input.
    fn run(mut self, rx: &Receiver<Msg>) {
        loop {
            self.fire(Instant::now());
            self.send_out();
            let next = match self.next_deadline() {
                Some(deadline) => {
                    match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                        Ok(msg) => msg,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                None => match rx.recv() {
                    Ok(msg) => msg,
                    Err(_) => break,
                },
            };
            let now = Instant::now();
            match next {
                Msg::Raw(raw) => self.raw(raw, now),
                Msg::Add {
                    id,
                    root,
                    rules,
                    done,
                } => {
                    self.add(id, root, rules, now);
                    self.send_out();
                    let _ = done.send(());
                }
                Msg::Remove { id, done } => {
                    if let Some(mut workspace) = self.workspaces.remove(&id) {
                        workspace.unwatch_below(self.backend.as_mut(), b"");
                    }
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
                                .map(|(dir, _)| dir.clone())
                                .collect();
                            dirs.sort();
                            dirs
                        })
                        .unwrap_or_default();
                    let _ = reply.send(dirs);
                }
                Msg::Shutdown => break,
            }
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
            .flat_map(|workspace| [workspace.batch.flush_at(&self.config), workspace.next_tick])
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
        }
    }

    fn add(&mut self, id: WorkspaceId, root: PathBuf, rules: Arc<IgnoreRules>, now: Instant) {
        if let Some(mut old) = self.workspaces.remove(&id) {
            old.unwatch_below(self.backend.as_mut(), b"");
        }
        rules.reload();
        let real_root = fs::canonicalize(&root).ok().filter(|real| *real != root);
        let mut workspace = Workspace {
            id,
            root,
            real_root,
            rules,
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

    fn raw(&mut self, raw: notify::Result<notify::Event>, now: Instant) {
        let event = match raw {
            Ok(event) if !event.need_rescan() => event,
            Ok(event) => return self.rescan(&event.paths, now),
            Err(error) => return self.rescan(&error.paths, now),
        };
        if matches!(event.kind, EventKind::Access(_) | EventKind::Other) {
            return;
        }
        for path in &event.paths {
            self.touched(path, now);
        }
    }

    /// Every workspace that owns one of the paths, or every workspace when the platform named none. The lost notifications may have named new or removed directories, so the watches are brought in line with the tree as well.
    fn rescan(&mut self, paths: &[PathBuf], now: Instant) {
        let ids: Vec<WorkspaceId> = if paths.is_empty() {
            self.workspaces.keys().copied().collect()
        } else {
            let owners: BTreeSet<WorkspaceId> =
                paths.iter().filter_map(|path| self.owner(path)).collect();
            owners.into_iter().collect()
        };
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
            // The whole-workspace scan covers whatever the batch held.
            workspace.batch.clear();
            let mut ctx = Ctx {
                backend: self.backend.as_mut(),
                config: &self.config,
                out: &mut self.outbox,
                now,
            };
            workspace.sync_tree(&mut ctx, None, false);
            tracing::info!(workspace = %id, "notifications lost, rescan requested");
            self.outbox
                .push(WatchEvent::RescanRequested { workspace: id });
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

    fn touched(&mut self, path: &Path, now: Instant) {
        let Some(id) = self.owner(path) else {
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

#[cfg(unix)]
fn dir_identity(meta: &Metadata) -> DirIdentity {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

#[cfg(not(unix))]
fn dir_identity(_meta: &Metadata) -> DirIdentity {
    (0, 0)
}

struct Workspace {
    id: WorkspaceId,
    root: PathBuf,
    /// The canonical root when it differs; some platforms report canonical paths whatever path was watched.
    real_root: Option<PathBuf>,
    rules: Arc<IgnoreRules>,
    /// Keyed by the workspace-relative directory bytes, empty for the root. The path is the exact one given to the backend, which keys its watches by it.
    watched: BTreeMap<Vec<u8>, (PathBuf, DirIdentity)>,
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

    fn full(&self, path: &RepoPath) -> Option<PathBuf> {
        os_path(path).ok().map(|relative| self.root.join(relative))
    }

    fn touched(&mut self, ctx: &mut Ctx<'_>, path: RepoPath) {
        if is_ignore_file(&path) {
            self.rules.reload();
            let folder = parent(&path);
            self.sync_tree(ctx, folder, false);
        }
        let Some(full) = self.full(&path) else {
            return;
        };
        let identity = fs::symlink_metadata(&full)
            .ok()
            .filter(Metadata::is_dir)
            .map(|meta| dir_identity(&meta));
        let known = self.watched.get(path.as_bytes()).map(|(_, known)| *known);
        let ignored = self
            .rules
            .is_ignored(&path, identity.is_some() || known.is_some());
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

    /// Watches `dir` and every tracked directory below it that has no live watch, and drops the watches of directories below it that are gone or now ignored. With `report`, every tracked path found goes into the batch, so files made before the watch existed are not missed. A workspace in timer mode adds no watch, so without `report` the walk stops there.
    fn sync_tree(&mut self, ctx: &mut Ctx<'_>, dir: Option<RepoPath>, report: bool) {
        let mut stack = vec![dir];
        while let Some(dir) = stack.pop() {
            if self.degraded.is_some() && !report {
                return;
            }
            let key: Vec<u8> = dir
                .as_ref()
                .map_or_else(Vec::new, |dir| dir.as_bytes().to_vec());
            let full = match &dir {
                Some(dir) => match self.full(dir) {
                    Some(full) => full,
                    None => continue,
                },
                None => self.root.clone(),
            };
            let meta = match fs::symlink_metadata(&full) {
                Ok(meta) if meta.is_dir() => meta,
                _ => {
                    self.unwatch_below(ctx.backend, &key);
                    continue;
                }
            };
            self.watch(ctx, key.clone(), full.clone(), dir_identity(&meta));
            let Ok(reader) = fs::read_dir(&full) else {
                continue;
            };
            let mut listed: BTreeSet<Vec<u8>> = BTreeSet::new();
            for entry in reader.flatten() {
                let Some(name) = os_bytes(&entry.file_name()) else {
                    continue;
                };
                if is_reserved_name(&name) {
                    continue;
                }
                let Ok(child) = RepoPath::child(dir.as_ref(), &name) else {
                    continue;
                };
                let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
                if self.rules.is_ignored_below(&child, is_dir) {
                    if is_dir {
                        self.unwatch_below(ctx.backend, child.as_bytes());
                    }
                    continue;
                }
                if report {
                    self.batch.record(child.clone(), ctx.now);
                }
                if is_dir {
                    listed.insert(child.as_bytes().to_vec());
                    stack.push(Some(child));
                }
            }
            // A watched directory the listing no longer shows as a tracked directory went while no notification said so.
            for child in self.children(&key) {
                if !listed.contains(&child) {
                    self.unwatch_below(ctx.backend, &child);
                }
            }
        }
    }

    /// The watched directories directly inside the directory `key`.
    fn children(&self, key: &[u8]) -> Vec<Vec<u8>> {
        let mut prefix = key.to_vec();
        if !prefix.is_empty() {
            prefix.push(b'/');
        }
        self.watched
            .range(prefix.clone()..)
            .map(|(dir, _)| dir)
            .take_while(|dir| dir.starts_with(&prefix))
            .filter(|dir| dir.len() > prefix.len() && !dir[prefix.len()..].contains(&b'/'))
            .cloned()
            .collect()
    }

    fn watch(&mut self, ctx: &mut Ctx<'_>, key: Vec<u8>, full: PathBuf, identity: DirIdentity) {
        match self.watched.get(&key) {
            Some((_, known)) if *known == identity => return,
            Some(_) => self.unwatch_below(ctx.backend, &key),
            None => {}
        }
        if self.degraded.is_some() {
            return;
        }
        match ctx.backend.watch_dir(&full) {
            Ok(()) => {
                self.watched.insert(key, (full, identity));
            }
            Err(WatchRefused::LimitReached) => self.degrade(ctx, DegradedReason::WatchLimit),
            Err(WatchRefused::Unsupported) => self.degrade(ctx, DegradedReason::NoNativeWatcher),
            Err(WatchRefused::Unavailable) => {
                tracing::debug!(workspace = %self.id, "a directory could not be watched");
            }
        }
    }

    /// Timer mode: the watches that exist stay, no new one is tried, and a rescan is requested every `degraded_interval`.
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
        let below: Vec<Vec<u8>> = self
            .watched
            .range(key.to_vec()..)
            .map(|(dir, _)| dir)
            .take_while(|dir| dir.starts_with(key))
            .filter(|dir| {
                key.is_empty() || dir.len() == key.len() || dir.get(key.len()) == Some(&b'/')
            })
            .cloned()
            .collect();
        for dir in below {
            if let Some((full, _)) = self.watched.remove(&dir) {
                backend.unwatch_dir(&full);
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
        assert_eq!(WatchConfig::default().debounce, Duration::from_millis(500));
    }

    fn config() -> WatchConfig {
        WatchConfig {
            debounce: Duration::from_millis(100),
            max_wait: Duration::from_millis(1_000),
            degraded_interval: Duration::from_millis(500),
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
