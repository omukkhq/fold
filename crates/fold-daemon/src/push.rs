use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use fold_core::events::types;
use fold_core::push::{MAX_NOTICE_LEN, Notice, PushContext, is_pushed, notice_for, request_of};
use fold_core::requests::{Requester, deputy};
use fold_core::{
    AgentName, Clock, Event, FoldError, LiveEvent, MessageId, Tag, TaskId, TaskKind, TaskStatus,
    Timestamp, codes,
};
use fold_store::agents::AgentRow;
use fold_store::reviews;
use fold_store::tasks::{claims, requests};
use fold_store::{Store, StoreError, agents, events, tasks};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tokio::sync::Notify;
use tokio::sync::broadcast::error::RecvError;

use crate::jobs::ProjectTask;
use crate::services::events::{BusMessage, EventBus};
use crate::support::lock;

/// The newest sockets kept per agent. One agent rarely runs more sessions at once.
pub const MAX_TARGETS: usize = 4;

/// The most agents a session's inbox waits for before the project knows them; one more drops the oldest.
pub const MAX_PENDING: usize = 16;

/// Notices waiting for one socket; a full queue drops its oldest notice.
pub const QUEUE_LEN: usize = 16;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

pub const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// The longest inbox token accepted. A Claude Code token is 32 hex characters.
pub const MAX_TOKEN_LEN: usize = 512;

/// A session's inbox token. It has no `Display` and a redacted `Debug`, so it cannot reach a log line or a message by formatting.
#[derive(Clone)]
pub struct PushToken(Arc<str>);

impl PushToken {
    /// Refuses an empty token and one longer than [`MAX_TOKEN_LEN`] bytes with `invalid_push_target`.
    pub fn parse(token: &str) -> Result<PushToken, FoldError> {
        if token.is_empty() || token.len() > MAX_TOKEN_LEN {
            return Err(invalid_target(format!(
                "the token must be 1 to {MAX_TOKEN_LEN} bytes"
            )));
        }
        Ok(PushToken(token.into()))
    }
}

impl std::fmt::Debug for PushToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PushToken(<redacted>)")
    }
}

/// Refuses a relative socket path with `invalid_push_target`.
pub fn check_socket(socket: &Path) -> Result<(), FoldError> {
    if socket.is_absolute() {
        Ok(())
    } else {
        Err(invalid_target("the socket path must be absolute"))
    }
}

fn invalid_target(message: impl Into<String>) -> FoldError {
    FoldError::new(codes::INVALID_PUSH_TARGET, message).with_hint(
        "send the session's CLAUDE_CODE_MESSAGING_SOCKET and CLAUDE_CODE_MESSAGING_TOKEN",
    )
}

/// Opens a connection to a session's inbox socket. Production uses [`UnixStream::connect`]; a test can stand in a connection that never opens.
pub type Connect = Arc<
    dyn Fn(PathBuf) -> Pin<Box<dyn Future<Output = io::Result<UnixStream>> + Send>> + Send + Sync,
>;

fn unix_connect() -> Connect {
    Arc::new(|path| Box::pin(async move { UnixStream::connect(path).await }))
}

/// Hands one notice text to a supervised worker's stdin as a user turn; it returns false once the worker's process is gone.
pub type StdinSink = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// How an agent's notices reach it now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushReach {
    /// The agent's supervised worker process takes them on its stdin.
    Stdin,
    /// A session's inbox socket took the last notice posted to it, or has had none posted yet.
    Socket,
    /// The last post to every inbox socket of the agent failed.
    Failing,
    /// The agent has no inbox socket and no worker process, so it sees a message only when it reads its inbox.
    Unreachable,
}

impl PushReach {
    /// A notice queued now reaches the agent unless its post fails.
    pub fn reachable(self) -> bool {
        matches!(self, PushReach::Stdin | PushReach::Socket)
    }
}

/// The inbox sockets of one project's agents and the stdin of each worker process fold supervises. They are keyed by agent rather than by session because the session-start hook runs before the adapter connects. Each socket has its own queue and sender task, so a slow socket never holds up another one or the bus. A socket reported for an agent the project does not know yet waits in memory until a session's adapter registers that agent. With a folder from [`PushTargets::keep_files_in`], the newest socket and token of each registered agent are also kept there, so the daemon reaches the session again after it restarts.
#[derive(Clone)]
pub struct PushTargets {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for PushTargets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PushTargets").finish_non_exhaustive()
    }
}

impl Default for PushTargets {
    fn default() -> Self {
        Self::new()
    }
}

struct Inner {
    agents: Mutex<HashMap<AgentName, Vec<Target>>>,
    /// One per supervised worker. A session ending never clears it: the worker's adapter session can reconnect while its process lives, and only the supervisor removes it when the process ends.
    stdin: Mutex<HashMap<AgentName, Arc<Queue>>>,
    connect: Connect,
    /// The inbox reported for each agent the project does not know yet, by the session-start hook or the adapter's handshake.
    pending: Mutex<HashMap<AgentName, Pending>>,
    /// Unset keeps the targets in memory only.
    files: OnceLock<TargetFiles>,
    /// Unset records no push time.
    recorder: OnceLock<Recorder>,
}

/// Where a notice about a message that got through is recorded, as the recipients' `pushed_at`, and the bus that tells the web after the commit.
#[derive(Clone)]
struct Recorder {
    store: Store,
    clock: Arc<dyn Clock>,
    bus: EventBus,
}

struct Pending {
    socket: PathBuf,
    token: PushToken,
    at: Timestamp,
}

/// The daemon's own folder for one project's kept targets, outside every project folder, so no workspace, check or tree of the project reaches it by a relative path. One lock serialises every file operation in it, so a removal never takes a file that a newer session wrote in the meantime.
#[derive(Clone)]
struct TargetFiles {
    dir: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl TargetFiles {
    /// Runs `work` on the blocking pool while it holds the folder's lock.
    async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Path) -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        let files = self.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = lock(&files.lock);
            work(&files.dir)
        })
        .await
        .unwrap_or_else(|error| Err(io::Error::other(error)))
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        for target in lock(&self.agents).drain().flat_map(|(_, targets)| targets) {
            target.queue.close();
        }
        for (_, queue) in lock(&self.stdin).drain() {
            queue.close();
        }
    }
}

struct Target {
    socket: PathBuf,
    queue: Arc<Queue>,
    /// When the path was first registered. A restarted session has a new path, so this is when its session started.
    registered_at: Timestamp,
}

impl PushTargets {
    pub fn new() -> Self {
        Self::with_connector(unix_connect())
    }

    pub fn with_connector(connect: Connect) -> Self {
        Self {
            inner: Arc::new(Inner {
                agents: Mutex::new(HashMap::new()),
                stdin: Mutex::new(HashMap::new()),
                connect,
                pending: Mutex::new(HashMap::new()),
                files: OnceLock::new(),
                recorder: OnceLock::new(),
            }),
        }
    }

    /// Records in `store` when a notice about a message reaches the recipient's agent, from now on, and sends `message.pushed` on `bus` once the first time is committed. The first call decides.
    pub fn record_pushes_in(&self, store: Store, clock: Arc<dyn Clock>, bus: EventBus) {
        let _ = self.inner.recorder.set(Recorder { store, clock, bus });
    }

    /// Keeps each registered agent's newest socket and token in `dir`, the daemon's folder for this project, from now on. The first call decides; the project's open makes it before any session can reach the targets.
    pub fn keep_files_in(&self, dir: PathBuf) {
        let _ = self.inner.files.set(TargetFiles {
            dir,
            lock: Arc::new(Mutex::new(())),
        });
    }

    /// Holds the inbox of a session whose agent the project does not know yet, in place of an earlier one for the same name, until [`PushTargets::restore`] registers it. Nothing goes to disk.
    pub fn hold(&self, agent: &AgentName, socket: PathBuf, token: PushToken, at: Timestamp) {
        let mut pending = lock(&self.inner.pending);
        pending.insert(agent.clone(), Pending { socket, token, at });
        if pending.len() > MAX_PENDING
            && let Some(oldest) = pending
                .iter()
                .min_by_key(|(_, held)| held.at)
                .map(|(name, _)| name.clone())
        {
            pending.remove(&oldest);
        }
    }

    /// Writes the agent's socket and token to its file, in place of an earlier session's. A failure is logged with the agent alone and leaves the earlier file.
    pub async fn keep(&self, agent: &AgentName, socket: &Path, token: &PushToken) {
        let Some(files) = self.inner.files.get() else {
            return;
        };
        let (name, socket, token) = (agent.clone(), socket.to_path_buf(), token.clone());
        let written = files
            .run(move |dir| write_target(dir, &name, &socket, &token))
            .await;
        if written.is_err() {
            tracing::warn!(agent = %agent, "session push could not keep the session's inbox socket");
        }
    }

    /// Registers the agent's held inbox, else the socket kept in its file, as a target registered at `at`, and keeps a held one in the file. A kept socket that is gone or that nobody listens on has its file removed. A socket the agent has already counts from `at` when that is later, so the end of an earlier session leaves it. A file that cannot be read is left and logged with the agent alone. Returns whether a target was added.
    pub async fn restore(&self, agent: &AgentName, at: Timestamp) -> bool {
        let held = lock(&self.inner.pending).remove(agent);
        if let Some(held) = held {
            if self.advance(agent, &held.socket, at) {
                return false;
            }
            self.keep(agent, &held.socket, &held.token).await;
            self.register(agent, held.socket, held.token, at);
            return true;
        }
        let Some(files) = self.inner.files.get() else {
            return false;
        };
        let name = agent.clone();
        let read = files.run(move |dir| read_live_target(dir, &name)).await;
        let (socket, token) = match read {
            Ok(Some(target)) => target,
            Ok(None) => return false,
            Err(_) => {
                tracing::warn!(agent = %agent, "session push could not read the kept inbox socket");
                return false;
            }
        };
        if self.advance(agent, &socket, at) {
            return false;
        }
        self.register(agent, socket, token, at);
        true
    }

    /// Removes the temporary files that a write cut short left in the project's folder.
    pub async fn sweep(&self) {
        let Some(files) = self.inner.files.get() else {
            return;
        };
        if files.run(remove_temporary).await.is_err() {
            tracing::warn!("session push could not clear its temporary files");
        }
    }

    /// Moves the registration of the agent's `socket` forward to `at`, when it is later. Returns whether the agent has the socket.
    fn advance(&self, agent: &AgentName, socket: &Path, at: Timestamp) -> bool {
        let mut agents = lock(&self.inner.agents);
        let Some(target) = agents
            .get_mut(agent)
            .and_then(|targets| targets.iter_mut().find(|target| target.socket == socket))
        else {
            return false;
        };
        target.registered_at = target.registered_at.max(at);
        true
    }

    /// Removes the agent's file when it still names `socket`, whose session has gone.
    async fn drop_file(&self, agent: &AgentName, socket: &Path) {
        let Some(files) = self.inner.files.get() else {
            return;
        };
        let (name, socket) = (agent.clone(), socket.to_path_buf());
        if files
            .run(move |dir| remove_target(dir, &name, &socket))
            .await
            .is_err()
        {
            tracing::warn!(agent = %agent, "session push could not remove the kept inbox socket");
        }
    }

    /// How a notice to the agent would travel now.
    pub fn reach(&self, agent: &AgentName) -> PushReach {
        if lock(&self.inner.stdin).contains_key(agent) {
            return PushReach::Stdin;
        }
        match lock(&self.inner.agents).get(agent) {
            Some(targets) if targets.iter().any(|target| !target.queue.failing()) => {
                PushReach::Socket
            }
            Some(targets) if !targets.is_empty() => PushReach::Failing,
            _ => PushReach::Unreachable,
        }
    }

    /// Adds the socket as the agent's newest target and starts its sender. The same path again takes the new token and keeps its waiting notices and its registration time; a fifth socket drops the oldest.
    pub fn register(&self, agent: &AgentName, socket: PathBuf, token: PushToken, now: Timestamp) {
        let mut agents = lock(&self.inner.agents);
        let targets = agents.entry(agent.clone()).or_default();
        let (pending, registered_at) =
            match targets.iter().position(|target| target.socket == socket) {
                Some(index) => {
                    let old = targets.remove(index);
                    (old.queue.take(), old.registered_at)
                }
                None => (VecDeque::new(), now),
            };
        if targets.len() == MAX_TARGETS {
            targets.remove(0).queue.close();
        }
        let queue = Arc::new(Queue::with_pending(pending));
        targets.push(Target {
            socket: socket.clone(),
            queue: Arc::clone(&queue),
            registered_at,
        });
        drop(agents);
        let sender = Sender {
            agent: agent.clone(),
            socket,
            token,
            queue,
            targets: Arc::downgrade(&self.inner),
            connect: Arc::clone(&self.inner.connect),
        };
        tokio::spawn(sender.run());
    }

    /// Sends the agent's notices to its worker process through `sink`, in order, in place of an earlier stdin target of the agent.
    pub fn register_stdin(&self, agent: &AgentName, sink: StdinSink) {
        let queue = Arc::new(Queue::default());
        if let Some(old) = lock(&self.inner.stdin).insert(agent.clone(), Arc::clone(&queue)) {
            old.close();
        }
        let targets = Arc::downgrade(&self.inner);
        let agent = agent.clone();
        tokio::spawn(async move {
            while let Some(notice) = queue.next().await {
                if !sink(&notice.text) {
                    if let Some(inner) = targets.upgrade() {
                        PushTargets { inner }.clear_stdin_queue(&agent, &queue);
                    }
                    return;
                }
                record(&targets, &agent, &notice).await;
            }
        });
    }

    /// Drops the agent's stdin target, when its worker process has ended.
    pub fn clear_stdin(&self, agent: &AgentName) {
        if let Some(queue) = lock(&self.inner.stdin).remove(agent) {
            queue.close();
        }
    }

    fn clear_stdin_queue(&self, agent: &AgentName, queue: &Arc<Queue>) {
        let mut stdin = lock(&self.inner.stdin);
        if stdin
            .get(agent)
            .is_some_and(|current| Arc::ptr_eq(current, queue))
        {
            stdin.remove(agent);
        }
    }

    /// Whether the agent's notices go to a worker process's stdin.
    pub fn has_stdin(&self, agent: &AgentName) -> bool {
        lock(&self.inner.stdin).contains_key(agent)
    }

    /// Drops every socket target of the agent and the notices waiting for them.
    pub fn clear(&self, agent: &AgentName) {
        let removed = lock(&self.inner.agents).remove(agent);
        for target in removed.into_iter().flatten() {
            target.queue.close();
        }
    }

    /// Drops the agent's targets registered at or before `at`, with their kept files. A session's hook registers before its adapter connects, so passing the start of an ending session drops its own socket and keeps one a newer session registered since.
    pub async fn clear_registered_by(&self, agent: &AgentName, at: Timestamp) {
        let mut removed: Vec<Target> = Vec::new();
        {
            let mut agents = lock(&self.inner.agents);
            if let Some(targets) = agents.get_mut(agent) {
                let (gone, kept) = std::mem::take(targets)
                    .into_iter()
                    .partition(|target| target.registered_at <= at);
                *targets = kept;
                removed = gone;
                if targets.is_empty() {
                    agents.remove(agent);
                }
            }
        }
        for target in removed {
            target.queue.close();
            self.drop_file(agent, &target.socket).await;
        }
    }

    /// Drops every socket target and every held inbox and leaves the kept files, for a daemon that stops or starts while the sessions may live on.
    pub fn clear_all(&self) {
        lock(&self.inner.pending).clear();
        let removed: Vec<Target> = lock(&self.inner.agents)
            .drain()
            .flat_map(|(_, targets)| targets)
            .collect();
        for target in removed {
            target.queue.close();
        }
    }

    /// The agent's sockets, oldest first.
    pub fn sockets(&self, agent: &AgentName) -> Vec<PathBuf> {
        lock(&self.inner.agents)
            .get(agent)
            .map(|targets| targets.iter().map(|target| target.socket.clone()).collect())
            .unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        lock(&self.inner.agents).is_empty() && lock(&self.inner.stdin).is_empty()
    }

    /// Queues each notice on its supervised worker's stdin, else on every socket of its agent: a worker's own session-start hook can report an inbox socket too, and the notice must reach the worker once. `seq` is the event the notice is about and goes into the log line of a failed post. A notice that is not one short line of envelope-safe text is dropped here.
    pub fn deliver(&self, notices: &[Notice], seq: u64) {
        self.queue(notices, seq, None);
    }

    /// [`PushTargets::deliver`]; with `about`, the first post of each notice that gets through sets `pushed_at` on its agent's rows of that message.
    fn queue(&self, notices: &[Notice], seq: u64, about: Option<About>) {
        let agents = lock(&self.inner.agents);
        let stdin = lock(&self.inner.stdin);
        for notice in notices {
            if !postable(&notice.text) {
                tracing::warn!(agent = %notice.agent, seq, "session push refused a malformed notice");
                continue;
            }
            if let Some(queue) = stdin.get(&notice.agent) {
                queue.push(Queued {
                    text: Arc::from(notice.text.as_str()),
                    seq,
                    about,
                });
                continue;
            }
            let targets = agents.get(&notice.agent).map_or(&[][..], Vec::as_slice);
            if targets.is_empty() {
                tracing::info!(agent = %notice.agent, seq, "session push has no inbox socket for the agent");
            }
            for target in targets {
                target.queue.push(Queued {
                    text: Arc::from(notice.text.as_str()),
                    seq,
                    about,
                });
            }
        }
    }

    /// Removes the socket after its session has gone, unless the path was registered again since.
    fn forget(&self, agent: &AgentName, queue: &Arc<Queue>) {
        let mut agents = lock(&self.inner.agents);
        if let Some(targets) = agents.get_mut(agent) {
            targets.retain(|target| !Arc::ptr_eq(&target.queue, queue));
            if targets.is_empty() {
                agents.remove(agent);
            }
        }
    }
}

/// Sets `pushed_at` for a notice about a message that just got through and, when that set a first time, sends `message.pushed` after the commit; a failure is logged with the agent and the event's seq alone.
async fn record(targets: &Weak<Inner>, agent: &AgentName, notice: &Queued) {
    let Some(about) = notice.about else {
        return;
    };
    let Some(recorder) = targets
        .upgrade()
        .and_then(|inner| inner.recorder.get().cloned())
    else {
        return;
    };
    let now = recorder.clock.now();
    match crate::services::messages::record_pushed(&recorder.store, agent, about.message, now).await
    {
        Ok(0) => {}
        Ok(_) => match LiveEvent::new(
            types::MESSAGE_PUSHED,
            None,
            json!({
                "message": about.message.to_string(),
                "thread": about.thread.to_string(),
                "agent": agent.as_str(),
            }),
        ) {
            Ok(event) => recorder.bus.publish_live(event),
            Err(error) => {
                tracing::error!(agent = %agent, seq = notice.seq, %error, "session push could not build message.pushed");
            }
        },
        Err(error) => {
            tracing::warn!(agent = %agent, seq = notice.seq, code = error.log_code(), "session push could not record a delivered notice");
        }
    }
}

/// The message a `message.sent` event names, with its thread, read from the event's data.
fn sent_message(event: &Event) -> Option<About> {
    if event.event_type != types::MESSAGE_SENT {
        return None;
    }
    let id = |key: &str| {
        event
            .data
            .get(key)
            .and_then(serde_json::Value::as_str)
            .and_then(|text| text.parse::<MessageId>().ok())
    };
    Some(About {
        message: id("message")?,
        thread: id("thread")?,
    })
}

/// The session shows the sender only when the content rebuilds to the same bytes, so the text holds no newline and never names the envelope tag.
fn postable(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_NOTICE_LEN
        && !text.chars().any(char::is_control)
        && !text.contains("cross-session-message")
}

#[derive(Debug, Clone)]
struct Queued {
    text: Arc<str>,
    seq: u64,
    /// The sent message the notice is about, whose recipients' `pushed_at` its first successful post sets.
    about: Option<About>,
}

#[derive(Debug, Clone, Copy)]
struct About {
    message: MessageId,
    thread: MessageId,
}

#[derive(Default)]
struct Queue {
    state: Mutex<QueueState>,
    wake: Notify,
    /// The last post from this queue failed.
    failing: AtomicBool,
}

#[derive(Default)]
struct QueueState {
    pending: VecDeque<Queued>,
    closed: bool,
}

impl Queue {
    fn with_pending(pending: VecDeque<Queued>) -> Self {
        Self {
            state: Mutex::new(QueueState {
                pending,
                closed: false,
            }),
            wake: Notify::new(),
            failing: AtomicBool::new(false),
        }
    }

    fn failing(&self) -> bool {
        self.failing.load(Ordering::Relaxed)
    }

    /// Closes the queue and hands back the notices still waiting in it.
    fn take(&self) -> VecDeque<Queued> {
        let pending = {
            let mut state = lock(&self.state);
            state.closed = true;
            std::mem::take(&mut state.pending)
        };
        self.wake.notify_one();
        pending
    }

    fn push(&self, notice: Queued) {
        {
            let mut state = lock(&self.state);
            if state.closed {
                return;
            }
            if state.pending.len() == QUEUE_LEN {
                state.pending.pop_front();
            }
            state.pending.push_back(notice);
        }
        self.wake.notify_one();
    }

    fn close(&self) {
        {
            let mut state = lock(&self.state);
            state.closed = true;
            state.pending.clear();
        }
        self.wake.notify_one();
    }

    /// The next notice, or `None` once the target is dropped.
    async fn next(&self) -> Option<Queued> {
        loop {
            {
                let mut state = lock(&self.state);
                if state.closed {
                    return None;
                }
                if let Some(notice) = state.pending.pop_front() {
                    return Some(notice);
                }
            }
            self.wake.notified().await;
        }
    }
}

struct Sender {
    agent: AgentName,
    socket: PathBuf,
    token: PushToken,
    queue: Arc<Queue>,
    targets: Weak<Inner>,
    connect: Connect,
}

impl Sender {
    /// Posts one notice at a time until the target is dropped. Every failure drops only the notice and marks the target failing until a post succeeds, except a missing socket or one with no listener: the session has ended, so the socket and its kept file are dropped too.
    async fn run(self) {
        while let Some(notice) = self.queue.next().await {
            let Err(error) = self.post(&notice.text).await else {
                self.queue.failing.store(false, Ordering::Relaxed);
                record(&self.targets, &self.agent, &notice).await;
                continue;
            };
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) {
                tracing::warn!(agent = %self.agent, seq = notice.seq, "session push found the session gone and dropped its socket");
                if let Some(inner) = self.targets.upgrade() {
                    let targets = PushTargets { inner };
                    targets.forget(&self.agent, &self.queue);
                    targets.drop_file(&self.agent, &self.socket).await;
                }
                return;
            }
            self.queue.failing.store(true, Ordering::Relaxed);
            tracing::warn!(agent = %self.agent, seq = notice.seq, "session push dropped a notice");
        }
    }

    /// Sends the auth line and the notice line, shuts the write half and closes without reading: the session never replies.
    async fn post(&self, text: &str) -> io::Result<()> {
        let mut stream = tokio::time::timeout(CONNECT_TIMEOUT, (self.connect)(self.socket.clone()))
            .await
            .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
        let bytes = frames(&self.token, text)?;
        tokio::time::timeout(WRITE_TIMEOUT, async {
            stream.write_all(&bytes).await?;
            stream.shutdown().await
        })
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
    }
}

/// Struct fields keep the key order of the session's own frames, which a JSON map does not promise.
#[derive(Serialize)]
struct AuthLine<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    token: &'a str,
}

#[derive(Serialize)]
struct NoticeLine<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    priority: &'static str,
    message: NoticeMessage<'a>,
}

#[derive(Serialize)]
struct NoticeMessage<'a> {
    role: &'static str,
    content: &'a str,
}

/// The cross-session envelope with `from-name="fold"` alone: no `from`, because fold listens on no reply address, and no `from-mode`, so every session class applies its own default.
pub fn envelope(text: &str) -> String {
    format!("<cross-session-message from-name=\"fold\">\n{text}\n</cross-session-message>")
}

/// The auth line, sent on every platform although Linux does not check it, then the notice line; each is one JSON object and a newline.
fn frames(token: &PushToken, text: &str) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(&AuthLine {
        kind: "auth",
        token: &token.0,
    })
    .map_err(io::Error::other)?;
    bytes.push(b'\n');
    serde_json::to_writer(
        &mut bytes,
        &NoticeLine {
            kind: "user",
            priority: "next",
            message: NoticeMessage {
                role: "user",
                content: &envelope(text),
            },
        },
    )
    .map_err(io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// One agent's kept socket and token. Not `Debug`, as it holds the token.
#[derive(Serialize, Deserialize)]
struct StoredTarget {
    socket: PathBuf,
    token: String,
}

fn target_file(dir: &Path, agent: &AgentName) -> PathBuf {
    dir.join(format!("{agent}.json"))
}

/// Makes `dir` and its parent, the daemon's folder of every project's targets, where they are missing, and accepts each only as a real directory of this user that no one else can enter, narrowing a wider mode to 0700.
fn private_dir(dir: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let uid = rustix::process::getuid().as_raw();
    for folder in [dir.parent(), Some(dir)].into_iter().flatten() {
        let meta = std::fs::symlink_metadata(folder)?;
        if !meta.is_dir() || meta.uid() != uid {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        if meta.mode() & 0o077 != 0 {
            std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

/// Writes a temporary file of mode 0600 and renames it into place, so a reader never sees half a file and the token is never in a file anyone else can read.
fn write_target(dir: &Path, agent: &AgentName, socket: &Path, token: &PushToken) -> io::Result<()> {
    static TEMP: AtomicU64 = AtomicU64::new(0);
    private_dir(dir)?;
    let bytes = serde_json::to_vec(&StoredTarget {
        socket: socket.to_path_buf(),
        token: token.0.to_string(),
    })
    .map_err(io::Error::other)?;
    let temp = dir.join(format!(
        ".{agent}.{}.{}.tmp",
        std::process::id(),
        TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)
        .and_then(|mut file| {
            file.write_all(&bytes)?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&temp, target_file(dir, agent)))
        .and_then(|()| std::fs::File::open(dir)?.sync_all());
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// Reads the agent's file without following a symlink in its place.
fn read_target(dir: &Path, agent: &AgentName) -> io::Result<Option<StoredTarget>> {
    use rustix::fs::{Mode, OFlags};
    let opened = rustix::fs::open(
        target_file(dir, agent),
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    );
    let mut file = match opened {
        Ok(fd) => std::fs::File::from(fd),
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
}

/// The kept target while a session listens on its socket; the file of a socket that is gone or has no listener is removed. The check connects and closes without sending a line, which delivers nothing to the session.
fn read_live_target(dir: &Path, agent: &AgentName) -> io::Result<Option<(PathBuf, PushToken)>> {
    let Some(stored) = read_target(dir, agent)? else {
        return Ok(None);
    };
    check_socket(&stored.socket).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
    let token =
        PushToken::parse(&stored.token).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
    match std::fs::symlink_metadata(&stored.socket) {
        Ok(meta) if meta.file_type().is_socket() => {}
        Ok(_) => return Err(io::Error::from(io::ErrorKind::InvalidData)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            remove_target(dir, agent, &stored.socket)?;
            return Ok(None);
        }
        Err(error) => return Err(error),
    }
    match std::os::unix::net::UnixStream::connect(&stored.socket) {
        Ok(_) => Ok(Some((stored.socket, token))),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            remove_target(dir, agent, &stored.socket)?;
            Ok(None)
        }
        Err(_) => Ok(Some((stored.socket, token))),
    }
}

/// A newer session may have written its own socket since, so the file goes only while it names `socket`.
fn remove_target(dir: &Path, agent: &AgentName, socket: &Path) -> io::Result<()> {
    match read_target(dir, agent) {
        Ok(Some(stored)) if stored.socket == socket => {
            match std::fs::remove_file(target_file(dir, agent)) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            }
        }
        Ok(_) => Ok(()),
        Err(error) => Err(error),
    }
}

/// Removes the `.<agent>.<pid>.<n>.tmp` files of writes that stopped before their rename.
fn remove_temporary(dir: &Path) -> io::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') && name.ends_with(".tmp") {
            match std::fs::remove_file(entry.path()) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
    }
    Ok(())
}

/// The project's push delivery.
pub type PushTask = ProjectTask;

/// Starts the delivery task of one project: it reads each event from the bus, which carries an event only after its commit, and queues the notices the event sends. Push is a hint: an event committed while no socket was registered is never replayed. The only write is the recipients' `pushed_at` once a notice about a message gets through, followed by a live `message.pushed`.
pub fn run(store: Store, clock: Arc<dyn Clock>, bus: &EventBus, targets: PushTargets) -> PushTask {
    targets.record_pushes_in(store.clone(), clock, bus.clone());
    let receiver = bus.subscribe();
    ProjectTask::spawn(deliver_events(store, receiver, bus.closing(), targets))
}

/// It ends once the project's bus closes.
async fn deliver_events(
    store: Store,
    mut receiver: tokio::sync::broadcast::Receiver<BusMessage>,
    mut closing: tokio::sync::watch::Receiver<bool>,
    targets: PushTargets,
) {
    loop {
        let received = tokio::select! {
            biased;
            _ = closing.wait_for(|closed| *closed) => return,
            received = receiver.recv() => received,
        };
        let event = match received {
            Ok(BusMessage::Durable(event)) => event,
            Ok(BusMessage::Live(_)) => continue,
            Err(RecvError::Lagged(skipped)) => {
                tracing::warn!(skipped, "session push fell behind the bus");
                continue;
            }
            Err(RecvError::Closed) => return,
        };
        if !is_pushed(&event) || targets.is_empty() {
            continue;
        }
        match context(&store, &event).await {
            Ok(ctx) => targets.queue(&notice_for(&event, &ctx), event.seq, sent_message(&event)),
            Err(error) => tracing::warn!(
                seq = event.seq,
                code = error.log_code(),
                "session push could not load the event's context"
            ),
        }
    }
}

/// The agent of a worker row's `started_by`, an event actor such as `web-lead` or `web-lead/sub` for a subagent; `None` for a human and for fold.
fn starter_agent(started_by: &str) -> Option<AgentName> {
    let actor = fold_core::EventActor::parse(started_by).ok()?;
    AgentName::parse(actor.agent()?).ok()
}

/// Reads only what the event's rule needs.
async fn context(store: &Store, event: &Event) -> Result<PushContext, StoreError> {
    let mut ctx = PushContext::default();
    let kind = event.event_type;
    if kind == fold_core::events::types::MESSAGE_SENT {
        ctx.recipients = events::recipient_agents(store.read(), event.seq)
            .await?
            .into_iter()
            .collect();
        let message = event
            .data
            .get("message")
            .and_then(serde_json::Value::as_str)
            .and_then(|message| message.parse::<fold_core::MessageId>().ok());
        if event.actor.is_human()
            && let Some(message) = message
            && let Some(asker) = agents::asker_answered_by(store.read(), message).await?
        {
            ctx.recipients.remove(&asker);
        }
        return Ok(ctx);
    }
    if kind.as_str().starts_with("request.")
        && let Some(request) = request_of(event)
    {
        let mut conn = store.read().acquire().await?;
        if let Some(row) = requests::fetch(&mut conn, request).await? {
            ctx.requester = match row.requester {
                Requester::Agent(agent) => Some(agent),
                Requester::Human(_) => None,
            };
            if let Some(agent) = &ctx.requester
                && let Some(process) = agents::find_worker_process(&mut conn, agent).await?
                && !process.state.is_running()
                && let Some(deputy) = deputy(process.role, &process.started_by)
            {
                ctx.requester = match deputy {
                    Requester::Agent(agent) => Some(agent),
                    Requester::Human(_) => None,
                };
            }
            ctx.drafter = row.drafter.map(|drafter| drafter.worker.agent());
        }
        if kind == fold_core::events::types::REQUEST_CREATED {
            ctx.area_leads = agents::list_areas(store.read())
                .await?
                .into_iter()
                .filter_map(|area| area.lead)
                .collect();
            ctx.main_agents = tagged(store, Tag::is_main).await?;
        }
        return Ok(ctx);
    }
    if kind == fold_core::events::types::PROJECT_PAUSED
        || kind == fold_core::events::types::PROJECT_RESUMED
        || kind == fold_core::events::types::HUMAN_CHANGE_RECORDED
        || kind == fold_core::events::types::HUMAN_CHANGE_CONFLICTED
        || kind == fold_core::events::types::HUMAN_CHANGE_RESOLVED
        || kind == fold_core::events::types::HUMAN_CHANGE_WITHDRAWN
        || kind == fold_core::events::types::INTEGRATION_HELD
    {
        ctx.main_agents = tagged(store, Tag::is_main).await?;
        return Ok(ctx);
    }
    if kind == types::WORKER_FAILED || kind == types::WORKER_STOPPED {
        let agent = event
            .data
            .get("agent")
            .and_then(serde_json::Value::as_str)
            .and_then(|agent| AgentName::parse(agent).ok());
        let Some(agent) = agent else {
            return Ok(ctx);
        };
        let reason = event.data.get("reason").and_then(serde_json::Value::as_str);
        if kind == types::WORKER_FAILED && reason.is_some_and(fold_core::roles::needs_restart) {
            ctx.main_agents = tagged(store, Tag::is_main).await?;
            let mut conn = store.read().acquire().await?;
            ctx.starter = agents::find_worker_process(&mut conn, &agent)
                .await?
                .and_then(|row| starter_agent(&row.started_by));
        }
        ctx.unreviewed = unreviewed_of(store, &agent).await?;
        if !ctx.unreviewed.is_empty() {
            ctx.review_leads = review_leads(store).await?;
        }
        return Ok(ctx);
    }
    if kind == fold_core::events::types::WORKER_MODEL_UNREACHABLE {
        let agent = event
            .data
            .get("agent")
            .and_then(serde_json::Value::as_str)
            .and_then(|agent| AgentName::parse(agent).ok());
        if let Some(agent) = agent {
            ctx.main_agents = tagged(store, Tag::is_main).await?;
            let mut conn = store.read().acquire().await?;
            ctx.starter = agents::find_worker_process(&mut conn, &agent)
                .await?
                .and_then(|row| starter_agent(&row.started_by));
        }
        return Ok(ctx);
    }
    if kind == fold_core::events::types::TASK_REPORTED {
        if let Some(task) = event.task {
            route(store, task, None, &mut ctx).await?;
        }
        return Ok(ctx);
    }
    if kind == fold_core::events::types::TASK_SENT_BACK {
        if let Some(task) = event.task {
            ctx.implementation_claim = claims::unreleased_for_task(store.read(), task)
                .await?
                .map(|claim| claim.worker.agent());
        }
        return Ok(ctx);
    }
    if kind == fold_core::events::types::TASK_STATUS_CHANGED {
        if let Some(task) = event.task {
            ctx.implementation_claim = claims::unreleased_for_task(store.read(), task)
                .await?
                .map(|claim| claim.worker.agent());
            let to = event.data.get("to").and_then(serde_json::Value::as_str);
            if to == Some(TaskStatus::Todo.as_str()) {
                ctx.held_from_agents = tasks::get_by_number(store.read(), task)
                    .await?
                    .is_some_and(|row| row.kind == TaskKind::Human || row.hold.is_some());
                if ctx.implementation_claim.is_none() && !ctx.held_from_agents {
                    route(store, task, None, &mut ctx).await?;
                }
            }
        }
        return Ok(ctx);
    }
    if kind == fold_core::events::types::TASK_UPDATED {
        let added: Vec<&str> = event
            .data
            .get("labels_added")
            .and_then(serde_json::Value::as_array)
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect()
            })
            .unwrap_or_default();
        if let Some(task) = event.task
            && !added.is_empty()
        {
            ctx.held_from_agents = !open_to_agents(store, task).await?;
            if !ctx.held_from_agents {
                route(store, task, Some(&added), &mut ctx).await?;
            }
        }
        return Ok(ctx);
    }
    if kind == fold_core::events::types::TASK_HOLD_CLEARED {
        if let Some(task) = event.task {
            ctx.held_from_agents = !open_to_agents(store, task).await?;
            if !ctx.held_from_agents {
                route(store, task, None, &mut ctx).await?;
            }
        }
        return Ok(ctx);
    }
    if let Some(proposal) = event.proposal
        && let Some(row) = reviews::get_proposal(store.read(), proposal).await?
    {
        ctx.author = Some(row.author.agent());
        ctx.main_task = tasks::labels_for(store.read(), row.task)
            .await?
            .iter()
            .any(|label| label.name.is_main());
        ctx.review_claim = row.review_claim.map(|worker| worker.agent());
        ctx.retired = row.state != reviews::ProposalState::Open;
        if kind == fold_core::events::types::INTEGRATION_SUCCEEDED {
            ctx.reviewer = integrated_reviewer(store, event, proposal).await?;
        }
    }
    if kind != fold_core::events::types::INTEGRATION_CONFLICTED
        && kind != fold_core::events::types::INTEGRATION_SUCCEEDED
    {
        ctx.review_leads = review_leads(store).await?;
    }
    Ok(ctx)
}

/// An agent can take the task now: an unclaimed agent task in Todo that is neither on hold nor archived.
async fn open_to_agents(store: &Store, task: TaskId) -> Result<bool, StoreError> {
    if claims::unreleased_for_task(store.read(), task)
        .await?
        .is_some()
    {
        return Ok(false);
    }
    Ok(tasks::get_by_number(store.read(), task)
        .await?
        .is_some_and(|row| {
            row.status == TaskStatus::Todo
                && row.kind != TaskKind::Human
                && row.hold.is_none()
                && row.archived_at.is_none()
        }))
}

/// The areas whose label the task carries (only those among `only`, when given), their leads and the main agents, which take a ready or reported task when no lead does.
async fn route(
    store: &Store,
    task: TaskId,
    only: Option<&[&str]>,
    ctx: &mut PushContext,
) -> Result<(), StoreError> {
    let labels = tasks::labels_for(store.read(), task).await?;
    for area in agents::list_areas(store.read()).await? {
        if labels.iter().any(|label| label.name == area.label)
            && only.is_none_or(|only| only.contains(&area.label.as_str()))
        {
            ctx.area_leads.extend(area.lead);
            ctx.areas.insert(area.name);
        }
    }
    ctx.main_agents = tagged(store, Tag::is_main).await?;
    Ok(())
}

/// The integration released the review claim, so the holder is read from the released row; without one, the agent whose approval of the integrated version stands.
async fn integrated_reviewer(
    store: &Store,
    event: &Event,
    proposal: fold_core::ProposalId,
) -> Result<Option<AgentName>, StoreError> {
    if let Some(holder) = reviews::integrated_review_claim_holder(store.read(), proposal).await? {
        return Ok(Some(holder.agent()));
    }
    let version = event
        .data
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .and_then(|version| u32::try_from(version).ok());
    let Some(version) = version else {
        return Ok(None);
    };
    let reviews = reviews::review_facts(store.read(), proposal, version).await?;
    Ok(fold_core::reviews::standing_agent_approval(&reviews))
}

/// The proposals that a reviewer which ended leaves without a verdict and without a resume that fold owes it, so nobody would give them to a reviewer; none while the project is paused, since the resume of the pause runs the reviewer again.
async fn unreviewed_of(
    store: &Store,
    agent: &AgentName,
) -> Result<Vec<fold_core::ProposalId>, StoreError> {
    if agents::pause_state(store.read()).await?.is_paused() {
        return Ok(Vec::new());
    }
    let held = agents::unreviewed_claims_of_ended_reviewer(store.read(), agent).await?;
    if held.is_empty() {
        return Ok(held);
    }
    let owed: std::collections::BTreeSet<fold_core::ProposalId> =
        agents::reviews_owed_to_ended_reviewers(store.read(), None)
            .await?
            .into_iter()
            .filter(|review| review.agent == *agent)
            .map(|review| review.proposal)
            .collect();
    Ok(held
        .into_iter()
        .filter(|proposal| !owed.contains(proposal))
        .collect())
}

/// The agents that fold started as the review lead.
async fn review_leads(store: &Store) -> Result<std::collections::BTreeSet<AgentName>, StoreError> {
    let mut conn = store.read().acquire().await?;
    Ok(agents::list_worker_processes(&mut conn)
        .await?
        .into_iter()
        .filter(|process| process.role == fold_core::roles::Role::ReviewLead)
        .map(|process| process.agent)
        .collect())
}

/// The agents with a tag `is_tag` accepts.
async fn tagged(
    store: &Store,
    is_tag: fn(&Tag) -> bool,
) -> Result<std::collections::BTreeSet<AgentName>, StoreError> {
    let every = agents::list_agents(store.read()).await?;
    tagged_among(store, &every, is_tag).await
}

/// The agents of `every` with a tag `is_tag` accepts.
async fn tagged_among(
    store: &Store,
    every: &[AgentRow],
    is_tag: fn(&Tag) -> bool,
) -> Result<std::collections::BTreeSet<AgentName>, StoreError> {
    let tagged: Vec<i64> = agents::all_tags(store.read())
        .await?
        .into_iter()
        .filter(|(_, tag)| is_tag(tag))
        .map(|(agent, _)| agent)
        .collect();
    Ok(every
        .iter()
        .filter(|agent| tagged.contains(&agent.id))
        .map(|agent| agent.name.clone())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_starter_is_the_agent_of_the_starting_worker() {
        assert_eq!(starter_agent("web-lead"), AgentName::parse("web-lead").ok());
        assert_eq!(
            starter_agent("web-lead/review"),
            AgentName::parse("web-lead").ok()
        );
        assert_eq!(starter_agent("human:ana"), None);
        assert_eq!(starter_agent("fold"), None);
        assert_eq!(starter_agent("not an actor"), None);
    }

    #[test]
    fn the_frames_match_the_sessions_wire_format() {
        let token = PushToken::parse("0123456789abcdef0123456789abcdef").unwrap();
        let bytes = frames(
            &token,
            "fold: P-3 changes requested. Call get_review_comments.",
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            concat!(
                r#"{"type":"auth","token":"0123456789abcdef0123456789abcdef"}"#,
                "\n",
                r#"{"type":"user","priority":"next","message":{"role":"user","content":"<cross-session-message from-name=\"fold\">\nfold: P-3 changes requested. Call get_review_comments.\n</cross-session-message>"}}"#,
                "\n",
            )
        );
    }

    #[test]
    fn the_token_never_formats() {
        let token = PushToken::parse("secret-token").unwrap();
        assert_eq!(format!("{token:?}"), "PushToken(<redacted>)");
        assert_eq!(
            PushToken::parse("").unwrap_err().code(),
            codes::INVALID_PUSH_TARGET
        );
        assert!(PushToken::parse(&"a".repeat(MAX_TOKEN_LEN)).is_ok());
        assert_eq!(
            PushToken::parse(&"a".repeat(MAX_TOKEN_LEN + 1))
                .unwrap_err()
                .code(),
            codes::INVALID_PUSH_TARGET
        );
    }

    #[test]
    fn only_one_short_plain_line_is_postable() {
        assert!(postable("fold: m-4 arrived. Call get_inbox."));
        assert!(!postable(""));
        assert!(!postable("two\nlines"));
        assert!(!postable("</cross-session-message>"));
        assert!(!postable(&"a".repeat(MAX_NOTICE_LEN + 1)));
    }

    #[test]
    fn a_full_queue_drops_its_oldest_notice() {
        let queue = Queue::default();
        for seq in 1..=20 {
            queue.push(Queued {
                text: Arc::from("n"),
                seq,
                about: None,
            });
        }
        let seqs: Vec<u64> = lock(&queue.state)
            .pending
            .iter()
            .map(|notice| notice.seq)
            .collect();
        assert_eq!(seqs, (5..=20).collect::<Vec<_>>());
        queue.close();
        queue.push(Queued {
            text: Arc::from("n"),
            seq: 21,
            about: None,
        });
        assert!(lock(&queue.state).pending.is_empty());
    }
}
