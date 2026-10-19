use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use fold_core::push::{MAX_NOTICE_LEN, Notice, PushContext, is_pushed, notice_for, request_of};
use fold_core::requests::{Requester, deputy};
use fold_core::{AgentName, Event, FoldError, Tag, TaskId, TaskKind, TaskStatus, Timestamp, codes};
use fold_store::agents::AgentRow;
use fold_store::reviews;
use fold_store::tasks::{claims, requests};
use fold_store::{Store, StoreError, agents, events, tasks};
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tokio::sync::Notify;
use tokio::sync::broadcast::error::RecvError;

use crate::jobs::ProjectTask;
use crate::services::events::{BusMessage, EventBus};
use crate::support::lock;

/// The newest sockets kept per agent. One agent rarely runs more sessions at once.
pub const MAX_TARGETS: usize = 4;

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

/// The inbox sockets of one project's agents, in memory only, and the stdin of each worker process fold supervises. They are keyed by agent rather than by session because the session-start hook runs before the adapter connects. Each socket has its own queue and sender task, so a slow socket never holds up another one or the bus.
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
            }),
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

    /// Drops the agent's targets registered at or before `at`. A session's hook registers before its adapter connects, so passing the start of an ending session drops its own socket and keeps one a newer session registered since.
    pub fn clear_registered_by(&self, agent: &AgentName, at: Timestamp) {
        let mut removed = Vec::new();
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
        }
    }

    pub fn clear_all(&self) {
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
                });
                continue;
            }
            for target in agents.get(&notice.agent).into_iter().flatten() {
                target.queue.push(Queued {
                    text: Arc::from(notice.text.as_str()),
                    seq,
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
}

#[derive(Default)]
struct Queue {
    state: Mutex<QueueState>,
    wake: Notify,
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
        }
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
    /// Posts one notice at a time until the target is dropped. Every failure drops only the notice, except a missing socket or one with no listener: the session has ended, so the socket is dropped too.
    async fn run(self) {
        while let Some(notice) = self.queue.next().await {
            let Err(error) = self.post(&notice.text).await else {
                continue;
            };
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) {
                tracing::info!(agent = %self.agent, seq = notice.seq, "session push found the session gone and dropped its socket");
                if let Some(inner) = self.targets.upgrade() {
                    PushTargets { inner }.forget(&self.agent, &self.queue);
                }
                return;
            }
            tracing::info!(agent = %self.agent, seq = notice.seq, "session push dropped a notice");
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

/// The project's push delivery.
pub type PushTask = ProjectTask;

/// Starts the delivery task of one project: it reads each event from the bus, which carries an event only after its commit, and queues the notices the event sends. Push is a hint: it writes nothing, and an event committed while no socket was registered is never replayed.
pub fn run(store: Store, bus: &EventBus, targets: PushTargets) -> PushTask {
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
            Ok(ctx) => targets.deliver(&notice_for(&event, &ctx), event.seq),
            Err(error) => tracing::warn!(
                seq = event.seq,
                code = error.log_code(),
                "session push could not load the event's context"
            ),
        }
    }
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
        });
        assert!(lock(&queue.state).pending.is_empty());
    }
}
