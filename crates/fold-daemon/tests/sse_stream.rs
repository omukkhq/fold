use std::sync::Arc;
use std::time::Duration;

use crate::daemon_support::caught_up;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use fold_core::events::types;
use fold_core::object::ObjectKind;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    Actor, AgentActor, AgentName, CheckpointId, EventActor, EventType, LiveEvent, NewEvent,
    Presence, RevisionId, TaskDescription, TaskId, TaskTitle, Timestamp, WorkerName, WorkspaceId,
};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::events::{
    ChangeKind, ChangedFile, CheckpointSummary, EventEnvelope, LiveWorkspace, PresenceChange,
};
use fold_daemon::http::router;
use fold_daemon::projects::{ProjectHandle, Projects};
use fold_daemon::services::events::{EventBuffer, EventBus};
use fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME;
use fold_daemon::services::tasks::CreateTask;
use fold_daemon::services::workspaces::WorkspaceService;
use fold_store::StoreError;
use fold_store::projects::ProjectRow;
use fold_store::testing::TestProject;
use fold_store::workspaces::{create_initial_revision, initial_revision};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const PATH: &str = "/api/v1/projects/demo/events";
/// How long one frame may take to arrive.
const WAIT: Duration = Duration::from_secs(20);

#[cfg(test)]
struct Fixture {
    project: TestProject,
    state: AppState,
    handle: Arc<ProjectHandle>,
    /// The newest `seq` once the project is open: the events the commit job stored for `r1`. The events a test appends follow it.
    base: u64,
}

#[cfg(test)]
impl Fixture {
    async fn new() -> Self {
        Self::listening("127.0.0.1").await
    }

    /// The project `demo` with `r1` holding `README.md`, open in a daemon that listens on `listen`.
    async fn listening(listen: &str) -> Self {
        let project = TestProject::new().await;
        let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
        let mut tree = FlatTree::new();
        tree.insert(
            b"README.md".to_vec(),
            FlatEntry {
                kind: EntryKind::File,
                hash: objects.write(ObjectKind::Blob, b"# demo\n").unwrap(),
            },
        );
        let tree = write_initial_tree(&objects, tree).unwrap();
        // The project folder holds r1's file, so the folder has no change of its own to report.
        std::fs::write(project.paths().root().join("README.md"), b"# demo\n").unwrap();
        let revision = initial_revision(tree, project.now()).unwrap();
        let stored = write_revision(&objects, &revision).unwrap();
        create_initial_revision(project.store(), &stored, None)
            .await
            .unwrap();
        let clock = Arc::new(project.clock().clone());
        let state: AppState = Arc::new(DaemonState {
            registry: project.registry().clone(),
            projects: Projects::default(),
            clock: clock.clone(),
            assets: AssetSource::Embedded,
            listen: format!("{listen}:0").parse().unwrap(),
            session_lifetime: DEFAULT_SESSION_LIFETIME,
        });
        let row = ProjectRow {
            slug: "demo".to_owned(),
            name: "Demo".to_owned(),
            root: project.paths().root().to_string_lossy().into_owned(),
        };
        let handle = state.projects.open(&row, clock).await.unwrap();
        caught_up(&handle).await;
        let base = fold_store::events::head_seq(handle.store.read())
            .await
            .unwrap();
        Self {
            project,
            state,
            handle,
            base,
        }
    }

    /// The `seq` of the `n`th event the test stored, counting from 1.
    fn at(&self, n: u64) -> u64 {
        self.base + n
    }

    /// [`Fixture::at`] as a `Last-Event-ID` header value.
    fn id(&self, n: u64) -> String {
        self.at(n).to_string()
    }

    fn bus(&self) -> &EventBus {
        &self.handle.bus
    }

    /// Appends `count` events in one write and publishes them after the commit, as a service does. Returns their `seq`s.
    async fn append(&self, count: usize) -> Vec<u64> {
        let buffer = self.commit(count).await;
        let seqs = buffer.events().iter().map(|event| event.seq).collect();
        self.bus().publish(buffer);
        seqs
    }

    /// Appends `count` events in one write without publishing them.
    async fn commit(&self, count: usize) -> EventBuffer {
        let mut buffer = EventBuffer::new();
        let now = self.project.now();
        self.handle
            .store
            .write(async |tx| {
                for _ in 0..count {
                    buffer
                        .append(tx, &new_event(types::TASK_UPDATED), now)
                        .await?;
                }
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
        buffer
    }

    /// Opens the stream and returns its frames; panics unless the daemon answered 200 with an event stream.
    async fn open(&self, last_event_id: Option<&str>) -> Frames {
        let response = crate::common::signed_in_router(&self.state, "ana")
            .await
            .oneshot(request(PATH, last_event_id))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"].to_str().unwrap(),
            "text/event-stream"
        );
        Frames {
            body: response.into_body(),
            buffer: String::new(),
        }
    }

    /// The status and error code of a request that is refused.
    async fn refused(&self, path: &str, last_event_id: Option<&str>) -> (StatusCode, String) {
        self.refused_by(
            crate::common::signed_in_router(&self.state, "ana").await,
            path,
            last_event_id,
        )
        .await
    }

    /// [`Fixture::refused`] through `router`.
    async fn refused_by(
        &self,
        router: Router,
        path: &str,
        last_event_id: Option<&str>,
    ) -> (StatusCode, String) {
        let response = router.oneshot(request(path, last_event_id)).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        (status, body["error"]["code"].as_str().unwrap().to_owned())
    }

    fn publish_workspace(&self, number: u32, summary: &str) {
        let view = live_workspace(number, summary);
        let event = LiveEvent::new(
            types::WORKSPACE_CHANGED,
            Some(view.workspace),
            serde_json::to_value(&view).unwrap(),
        )
        .unwrap();
        self.bus().publish_live(event);
    }

    fn publish_presence(&self, agent: &str, presence: &str) {
        let event = LiveEvent::new(
            types::AGENT_PRESENCE_CHANGED,
            None,
            json!({"agent": agent, "presence": presence}),
        )
        .unwrap();
        self.bus().publish_live(event);
    }
}

#[cfg(test)]
fn request(path: &str, last_event_id: Option<&str>) -> Request<Body> {
    let mut request = Request::builder()
        .uri(path)
        .header("accept", "text/event-stream");
    if let Some(id) = last_event_id {
        request = request.header("last-event-id", id);
    }
    request.body(Body::empty()).unwrap()
}

#[cfg(test)]
fn new_event(event_type: EventType) -> NewEvent {
    NewEvent::new(event_type, EventActor::parse("frontend").unwrap())
        .unwrap()
        .task(TaskId::from_number(1).unwrap())
}

#[cfg(test)]
fn live_workspace(number: u32, summary: &str) -> LiveWorkspace {
    LiveWorkspace {
        workspace: WorkspaceId::from_number(number).unwrap(),
        task: Some(TaskId::from_number(number).unwrap()),
        worker: "frontend".to_owned(),
        base: RevisionId::from_number(1).unwrap(),
        files: vec![ChangedFile {
            path: "src/lib.rs".to_owned(),
            previous_path: None,
            change: ChangeKind::Modified,
            added_lines: 1,
            removed_lines: 0,
            binary: false,
        }],
        summary: summary.to_owned(),
        last_activity_at: Timestamp::from_millis(1_000).unwrap(),
        checkpoints: vec![CheckpointSummary {
            id: CheckpointId::from_number(1).unwrap(),
            label: None,
            automatic: true,
            created_at: Timestamp::from_millis(900).unwrap(),
        }],
        proposal: None,
        stalled: false,
        edits_after_version: None,
        check_run: None,
    }
}

/// One SSE frame: its `event` and `id` fields and its `data` read as the shared envelope.
#[cfg(test)]
#[derive(Debug)]
struct Frame {
    event: Option<String>,
    id: Option<String>,
    envelope: EventEnvelope,
}

#[cfg(test)]
impl Frame {
    /// The `seq` of a stored event frame; panics for any other frame.
    fn seq(&self) -> u64 {
        match &self.envelope {
            EventEnvelope::Event(event) => {
                assert_eq!(self.id, Some(event.seq.to_string()));
                assert_eq!(self.event.as_deref(), Some(event.event_type.as_str()));
                event.seq
            }
            other => panic!("expected a stored event, got {other:?}"),
        }
    }

    /// The `seq` and the workspaces of a snapshot frame; panics for any other frame.
    fn snapshot(&self) -> (u64, Vec<LiveWorkspace>) {
        match &self.envelope {
            EventEnvelope::Snapshot(snapshot) => {
                assert_eq!(self.event.as_deref(), Some("snapshot"));
                assert_eq!(self.id, None);
                (snapshot.seq, snapshot.workspaces.clone())
            }
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }

    fn workspace(&self) -> LiveWorkspace {
        match &self.envelope {
            EventEnvelope::WorkspaceChanged(view) => {
                assert_eq!(self.event.as_deref(), Some("workspace.changed"));
                assert_eq!(
                    self.id, None,
                    "a live frame never moves the client's last id"
                );
                view.clone()
            }
            other => panic!("expected workspace.changed, got {other:?}"),
        }
    }

    fn presence(&self) -> PresenceChange {
        match &self.envelope {
            EventEnvelope::AgentPresenceChanged(change) => {
                assert_eq!(self.event.as_deref(), Some("agent.presence_changed"));
                assert_eq!(
                    self.id, None,
                    "a live frame never moves the client's last id"
                );
                change.clone()
            }
            other => panic!("expected agent.presence_changed, got {other:?}"),
        }
    }
}

#[cfg(test)]
/// Reads the body of one open stream frame by frame. Every wait has a timeout.
struct Frames {
    body: Body,
    buffer: String,
}

#[cfg(test)]
impl Frames {
    /// The next frame, or `None` once the daemon ended the stream. Keep-alive comments do not extend the wait.
    async fn next(&mut self) -> Option<Frame> {
        tokio::time::timeout(WAIT, self.read())
            .await
            .expect("no frame arrived in time")
    }

    async fn read(&mut self) -> Option<Frame> {
        loop {
            while let Some(end) = self.buffer.find("\n\n") {
                let block: String = self.buffer.drain(..end + 2).collect();
                if let Some(frame) = parse(&block) {
                    return Some(frame);
                }
            }
            match self.body.frame().await {
                None => return None,
                Some(chunk) => {
                    if let Ok(data) = chunk.unwrap().into_data() {
                        self.buffer.push_str(std::str::from_utf8(&data).unwrap());
                    }
                }
            }
        }
    }

    async fn frame(&mut self) -> Frame {
        self.next().await.expect("the stream ended")
    }

    async fn seqs(&mut self, count: usize) -> Vec<u64> {
        let mut seqs = Vec::with_capacity(count);
        for _ in 0..count {
            seqs.push(self.frame().await.seq());
        }
        seqs
    }
}

/// A frame of `event`, `id` and `data` lines; `None` for a block of comments only, such as a keep-alive.
#[cfg(test)]
fn parse(block: &str) -> Option<Frame> {
    let mut event = None;
    let mut id = None;
    let mut data = Vec::new();
    for line in block.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            event = Some(value.trim_start().to_owned());
        } else if let Some(value) = line.strip_prefix("id:") {
            id = Some(value.trim_start().to_owned());
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.trim_start().to_owned());
        }
    }
    if data.is_empty() {
        return None;
    }
    let envelope = serde_json::from_str(&data.join("\n"))
        .unwrap_or_else(|error| panic!("a frame is not an EventEnvelope: {error}"));
    Some(Frame {
        event,
        id,
        envelope,
    })
}

#[tokio::test]
async fn a_reconnect_replays_the_missed_events_then_the_snapshot_then_new_events() {
    let fx = Fixture::new().await;
    assert_eq!(
        fx.append(10).await,
        (1..=10).map(|n| fx.at(n)).collect::<Vec<u64>>()
    );
    let mut frames = fx.open(Some(&fx.id(4))).await;
    assert_eq!(
        frames.seqs(6).await,
        (5..=10).map(|n| fx.at(n)).collect::<Vec<u64>>()
    );
    assert_eq!(frames.frame().await.snapshot(), (fx.at(10), Vec::new()));
    assert_eq!(fx.append(1).await, vec![fx.at(11)]);
    assert_eq!(frames.frame().await.seq(), fx.at(11));
}

#[tokio::test]
async fn a_first_connection_starts_at_the_snapshot_without_old_events() {
    let fx = Fixture::new().await;
    fx.append(3).await;
    let mut frames = fx.open(None).await;
    assert_eq!(frames.frame().await.snapshot(), (fx.at(3), Vec::new()));
    fx.append(1).await;
    assert_eq!(frames.frame().await.seq(), fx.at(4));
}

#[tokio::test]
async fn an_event_committed_after_the_subscription_is_sent_exactly_once() {
    let fx = Fixture::new().await;
    fx.append(10).await;
    // The answer is back, so the stream has subscribed; nothing of the replay is read yet.
    let mut frames = fx.open(Some(&fx.id(4))).await;
    fx.append(1).await;
    assert_eq!(
        frames.seqs(7).await,
        (5..=11).map(|n| fx.at(n)).collect::<Vec<u64>>()
    );
    assert_eq!(frames.frame().await.snapshot().0, fx.at(11));
    fx.append(1).await;
    assert_eq!(
        frames.frame().await.seq(),
        fx.at(12),
        "the 11th is not sent a second time"
    );

    let mut fresh = fx.open(None).await;
    fx.append(1).await;
    assert_eq!(fresh.frame().await.snapshot().0, fx.at(12));
    assert_eq!(fresh.frame().await.seq(), fx.at(13));
    fx.append(1).await;
    assert_eq!(fresh.frame().await.seq(), fx.at(14));
}

#[tokio::test]
async fn an_event_committed_before_the_subscription_and_published_after_it_is_sent_once() {
    let fx = Fixture::new().await;
    fx.append(4).await;
    let late = fx.commit(1).await;
    let mut frames = fx.open(Some(&fx.id(4))).await;
    fx.bus().publish(late);
    assert_eq!(frames.seqs(1).await, vec![fx.at(5)]);
    assert_eq!(frames.frame().await.snapshot().0, fx.at(5));
    fx.append(1).await;
    assert_eq!(
        frames.frame().await.seq(),
        fx.at(6),
        "the 5th is not sent a second time"
    );

    let late = fx.commit(1).await;
    let mut fresh = fx.open(None).await;
    fx.bus().publish(late);
    assert_eq!(fresh.frame().await.snapshot().0, fx.at(7));
    fx.append(1).await;
    assert_eq!(
        fresh.frame().await.seq(),
        fx.at(8),
        "the 7th is older than the snapshot"
    );
}

#[tokio::test]
async fn events_published_out_of_commit_order_are_sent_in_seq_order() {
    let fx = Fixture::new().await;
    let mut frames = fx.open(None).await;
    assert_eq!(frames.frame().await.snapshot().0, fx.at(0));
    let first = fx.commit(1).await;
    let second = fx.commit(1).await;
    fx.bus().publish(second);
    fx.bus().publish(first);
    fx.append(1).await;
    assert_eq!(
        frames.seqs(3).await,
        (1..=3).map(|n| fx.at(n)).collect::<Vec<u64>>()
    );
}

#[tokio::test]
async fn live_frames_have_no_id_and_collapse_to_the_newest_per_workspace_and_agent() {
    let fx = Fixture::new().await;
    let mut frames = fx.open(None).await;
    frames.frame().await.snapshot();
    // All of these reach the bus before the client reads again.
    fx.publish_workspace(1, "v1");
    fx.publish_workspace(1, "v2");
    fx.publish_workspace(2, "w1");
    fx.publish_workspace(1, "v3");
    fx.publish_presence("frontend", "active");
    fx.publish_workspace(2, "w2");
    fx.publish_presence("frontend", "idle");
    fx.append(1).await;
    assert_eq!(frames.frame().await.workspace(), live_workspace(1, "v3"));
    assert_eq!(frames.frame().await.workspace(), live_workspace(2, "w2"));
    assert_eq!(
        frames.frame().await.presence(),
        PresenceChange {
            agent: "frontend".to_owned(),
            presence: Presence::Idle,
        }
    );
    assert_eq!(frames.frame().await.seq(), fx.at(1));
}

#[tokio::test]
async fn a_newer_live_state_replaces_one_that_is_queued_but_unsent() {
    let fx = Fixture::new().await;
    let mut frames = fx.open(None).await;
    frames.frame().await.snapshot();
    fx.append(1).await;
    fx.publish_workspace(1, "v1");
    assert_eq!(frames.frame().await.seq(), fx.at(1));
    fx.publish_workspace(1, "v2");
    fx.publish_workspace(1, "v3");
    assert_eq!(frames.frame().await.workspace(), live_workspace(1, "v3"));
    fx.append(1).await;
    assert_eq!(frames.frame().await.seq(), fx.at(2));
}

#[tokio::test]
async fn a_workspace_state_queued_before_the_snapshot_is_left_to_the_snapshot() {
    let fx = Fixture::new().await;
    fx.append(2).await;
    let mut frames = fx.open(Some(&fx.id(1))).await;
    // The live state holds no ws-1, as after the workspace ended: only its old frame is still queued.
    fx.publish_workspace(1, "ended");
    fx.publish_presence("frontend", "idle");
    assert_eq!(frames.seqs(1).await, vec![fx.at(2)]);
    assert_eq!(frames.frame().await.snapshot(), (fx.at(2), Vec::new()));
    assert_eq!(
        frames.frame().await.presence(),
        PresenceChange {
            agent: "frontend".to_owned(),
            presence: Presence::Idle,
        }
    );
    fx.append(1).await;
    assert_eq!(frames.frame().await.seq(), fx.at(3));
}

#[tokio::test]
async fn the_snapshot_and_the_live_frames_carry_the_real_workspace_state() {
    let fx = Fixture::new().await;
    let creator = agent("frontend");
    crate::common::roles::lead_work_areas(&fx.handle.store).await;
    fx.handle
        .tasks
        .create_task(
            &creator,
            CreateTask {
                title: TaskTitle::parse("task").unwrap(),
                description: TaskDescription::new("body"),
                labels: crate::common::roles::work_labels(),
            },
        )
        .await
        .unwrap();
    let agents = fx.handle.agents();
    let mut ctx = agents.start_session(1).await.unwrap();
    agents
        .register_agent(&mut ctx, &AgentName::parse("frontend").unwrap(), &[])
        .await
        .unwrap();
    let view = WorkspaceService::for_project(&fx.handle)
        .create_workspace(&creator, TaskId::from_number(1).unwrap())
        .await
        .unwrap()
        .workspace;
    let expected = fx.handle.live.get(view.id).unwrap().view.clone();

    let mut frames = fx.open(None).await;
    let (seq, workspaces) = frames.frame().await.snapshot();
    assert!(seq > 0);
    assert_eq!(workspaces, vec![expected]);

    let dir = fx.project.paths().root().join(&view.path);
    std::fs::write(dir.join("notes.md"), "one\ntwo\n").unwrap();
    let state = fx.handle.live.rescan(view.id).await.unwrap();
    assert!(state.view.files.iter().any(|file| file.path == "notes.md"));
    let changed = loop {
        let frame = frames.frame().await;
        if matches!(frame.envelope, EventEnvelope::WorkspaceChanged(_)) {
            let changed = frame.workspace();
            if changed == state.view {
                break changed;
            }
        }
    };
    assert_eq!(changed.workspace, view.id);
}

#[tokio::test]
async fn a_lagging_client_is_closed_and_its_reconnect_receives_the_missed_events() {
    let fx = Fixture::new().await;
    fx.append(2).await;
    let mut frames = fx.open(None).await;
    assert_eq!(frames.frame().await.snapshot().0, fx.at(2));
    // More than the bus holds, while the client reads nothing.
    let missed = fx.append(1_100).await;
    assert!(frames.next().await.is_none(), "the lagging stream ends");

    let mut again = fx.open(Some(&fx.id(2))).await;
    assert_eq!(again.seqs(missed.len()).await, missed);
    assert_eq!(again.frame().await.snapshot().0, fx.at(1_102));
}

#[tokio::test]
async fn a_bad_last_event_id_or_an_unknown_project_is_refused() {
    let fx = Fixture::new().await;
    fx.append(10).await;
    let future = fx.id(11);
    for id in [
        "abc",
        "-1",
        "",
        "4.5",
        "+4",
        &future,
        "18446744073709551616",
    ] {
        assert_eq!(
            fx.refused(PATH, Some(id)).await,
            (StatusCode::BAD_REQUEST, "invalid_last_event_id".to_owned()),
            "Last-Event-ID {id:?}"
        );
    }
    let mut frames = fx.open(Some(&fx.id(10))).await;
    assert_eq!(frames.frame().await.snapshot().0, fx.at(10));
    assert_eq!(
        fx.refused("/api/v1/projects/nope/events", None).await,
        (StatusCode::NOT_FOUND, "project_not_found".to_owned())
    );
}

#[tokio::test]
async fn on_loopback_and_on_every_address_the_stream_needs_the_session_cookie() {
    for listen in ["127.0.0.1", "0.0.0.0"] {
        let fx = Fixture::listening(listen).await;
        assert_eq!(
            fx.refused_by(router(fx.state.clone()), PATH, None).await,
            (StatusCode::UNAUTHORIZED, "auth_required".to_owned()),
            "{listen}"
        );
    }
}

#[tokio::test]
async fn a_quiet_stream_sends_a_keep_alive_comment_every_15_seconds() {
    let fx = Fixture::new().await;
    let mut frames = fx.open(None).await;
    frames.frame().await.snapshot();
    assert!(frames.buffer.is_empty());
    tokio::time::pause();
    let started = tokio::time::Instant::now();
    let chunk = tokio::time::timeout(WAIT, frames.body.frame())
        .await
        .expect("no keep-alive arrived in time")
        .unwrap()
        .unwrap();
    let text = String::from_utf8(chunk.into_data().unwrap().to_vec()).unwrap();
    assert!(text.starts_with(':') && text.ends_with("\n\n"), "{text:?}");
    // The period started when the snapshot was sent, a moment before the clock was paused.
    let elapsed = started.elapsed();
    assert!(
        elapsed > Duration::from_secs(14) && elapsed < Duration::from_secs(16),
        "{elapsed:?}"
    );
}

#[tokio::test]
async fn the_stream_ends_at_shutdown_and_dropping_it_drops_its_subscription() {
    let fx = Fixture::new().await;
    let before = fx.bus().subscribers();
    let mut first = fx.open(None).await;
    let second = fx.open(None).await;
    assert_eq!(fx.bus().subscribers(), before + 2);
    drop(second);
    assert_eq!(fx.bus().subscribers(), before + 1);
    first.frame().await.snapshot();
    fx.bus().close();
    assert!(first.next().await.is_none());
}

#[test]
fn the_route_streams_the_shared_event_envelope() {
    let doc = serde_json::to_value(fold_daemon::http::openapi()).unwrap();
    let get = &doc["paths"]["/api/v1/projects/{slug}/events"]["get"];
    assert_eq!(get["operationId"], "stream_events");
    assert_eq!(
        get["responses"]["200"]["content"]["text/event-stream"]["schema"]["$ref"],
        "#/components/schemas/EventEnvelope"
    );
    for status in ["400", "401", "404"] {
        assert_eq!(
            get["responses"][status]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/ErrorEnvelope",
            "status {status}"
        );
    }
    let header = get["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|parameter| parameter["in"] == "header")
        .unwrap();
    assert_eq!(header["name"], "Last-Event-ID");
}

#[cfg(test)]
fn agent(name: &str) -> Actor {
    let worker = WorkerName::parse(name).unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: Vec::new(),
    })
}
