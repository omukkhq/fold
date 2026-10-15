use std::sync::Arc;
use std::time::Duration;

use fold_core::events::types;
use fold_core::{AgentName, EventFilter, Presence, Timestamp};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::events::PresenceChange;
use fold_daemon::jobs::{JobProgress, spawn_presence};
use fold_daemon::projects::Projects;
use fold_daemon::services::agents::{AgentService, SessionContext};
use fold_daemon::services::events::{BusMessage, EventBus};
use fold_store::agents::WorkerRow;
use fold_store::events;
use fold_store::projects::ProjectRow;
use fold_store::testing::TestProject;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::TryRecvError;
use tokio::sync::watch;

const WINDOW: i64 = 120_000;
const WAIT: Duration = Duration::from_secs(10);

#[cfg(test)]
struct Fixture {
    project: TestProject,
    agents: AgentService,
    bus: EventBus,
}

#[cfg(test)]
impl Fixture {
    async fn new() -> Self {
        let project = TestProject::new().await;
        let bus = EventBus::new();
        let agents = AgentService::new(
            project.store().clone(),
            bus.clone(),
            Arc::new(project.clock().clone()),
        );
        Self {
            project,
            agents,
            bus,
        }
    }

    async fn register(&self, name: &str) -> (SessionContext, WorkerRow) {
        register(&self.agents, name).await
    }

    fn advance(&self, millis: i64) {
        let now = self.project.now().as_millis();
        self.project
            .clock()
            .set(Timestamp::from_millis(now + millis).unwrap());
    }
}

#[cfg(test)]
async fn register(agents: &AgentService, name: &str) -> (SessionContext, WorkerRow) {
    let mut ctx = agents.start_session(1).await.unwrap();
    agents
        .register_agent(&mut ctx, &AgentName::parse(name).unwrap(), &[])
        .await
        .unwrap();
    let worker = agents.resolve_worker(&ctx, None).await.unwrap();
    (ctx, worker)
}

/// The live presence frames on the bus so far; stored events are skipped.
#[cfg(test)]
fn frames(receiver: &mut Receiver<BusMessage>) -> Vec<PresenceChange> {
    let mut frames = Vec::new();
    loop {
        match receiver.try_recv() {
            Ok(BusMessage::Live(live)) => {
                assert_eq!(live.event_type, types::AGENT_PRESENCE_CHANGED);
                assert_eq!(live.workspace, None);
                frames.push(serde_json::from_value(live.data.clone()).unwrap());
            }
            Ok(BusMessage::Durable(_)) => {}
            Err(TryRecvError::Empty) => return frames,
            Err(error) => panic!("bus failed: {error}"),
        }
    }
}

#[cfg(test)]
fn change(agent: &str, presence: Presence) -> PresenceChange {
    PresenceChange {
        agent: agent.to_owned(),
        presence,
    }
}

#[cfg(test)]
async fn stored_presence_events(project: &TestProject) -> usize {
    events::read_after(project.store().read(), 0, &EventFilter::default(), 1000)
        .await
        .unwrap()
        .iter()
        .filter(|event| event.event_type == types::AGENT_PRESENCE_CHANGED)
        .count()
}

#[tokio::test]
async fn a_call_of_an_idle_agent_sends_one_active_frame_and_stores_nothing() {
    let fixture = Fixture::new().await;
    let mut bus = fixture.bus.subscribe();
    let (ctx, worker) = fixture.register("frontend").await;
    fixture
        .agents
        .record_call(&ctx, &worker, "list_tasks", None)
        .await
        .unwrap();
    assert_eq!(
        frames(&mut bus),
        [change("frontend", Presence::Active)],
        "the first call the service sees announces the agent"
    );

    fixture.advance(WINDOW - 1);
    fixture
        .agents
        .record_call(&ctx, &worker, "list_tasks", None)
        .await
        .unwrap();
    assert_eq!(frames(&mut bus), [], "an active agent stays active");

    fixture.advance(WINDOW);
    fixture
        .agents
        .record_call(&ctx, &worker, "claim_task", None)
        .await
        .unwrap();
    assert_eq!(frames(&mut bus), [change("frontend", Presence::Active)]);
    fixture
        .agents
        .record_call(&ctx, &worker, "claim_task", None)
        .await
        .unwrap();
    assert_eq!(frames(&mut bus), []);
    assert_eq!(stored_presence_events(&fixture.project).await, 0);
}

#[tokio::test]
async fn the_sweep_sends_one_frame_when_the_window_passes() {
    let fixture = Fixture::new().await;
    let (ctx, worker) = fixture.register("frontend").await;
    fixture.register("backend").await;
    fixture
        .agents
        .record_call(&ctx, &worker, "list_tasks", None)
        .await
        .unwrap();
    let mut bus = fixture.bus.subscribe();
    assert_eq!(fixture.agents.sweep_presence().await.unwrap(), 0);

    fixture.advance(WINDOW - 1);
    assert_eq!(fixture.agents.sweep_presence().await.unwrap(), 0);
    assert_eq!(frames(&mut bus), []);

    fixture.advance(1);
    assert_eq!(fixture.agents.sweep_presence().await.unwrap(), 2);
    let mut sent = frames(&mut bus);
    sent.sort_by(|a, b| a.agent.cmp(&b.agent));
    assert_eq!(
        sent,
        [
            change("backend", Presence::Idle),
            change("frontend", Presence::Idle)
        ],
        "a connected agent that never called goes idle too"
    );
    assert_eq!(fixture.agents.sweep_presence().await.unwrap(), 0);
    assert_eq!(frames(&mut bus), []);
    assert_eq!(stored_presence_events(&fixture.project).await, 0);
}

#[tokio::test]
async fn an_active_agent_whose_last_session_ends_is_announced_offline() {
    let fixture = Fixture::new().await;
    let (ctx, _) = fixture.register("frontend").await;
    assert_eq!(fixture.agents.sweep_presence().await.unwrap(), 0);
    let mut bus = fixture.bus.subscribe();
    fixture.agents.end_session(&ctx).await.unwrap();
    assert_eq!(fixture.agents.sweep_presence().await.unwrap(), 1);
    assert_eq!(frames(&mut bus), [change("frontend", Presence::Offline)]);
    fixture.advance(WINDOW);
    assert_eq!(fixture.agents.sweep_presence().await.unwrap(), 0);
}

#[tokio::test]
async fn a_call_that_lands_after_an_idle_frame_announces_active_again() {
    let fixture = Fixture::new().await;
    let (ctx, worker) = fixture.register("frontend").await;
    let called = fixture.project.now();
    let mut bus = fixture.bus.subscribe();
    assert_eq!(fixture.agents.sweep_presence().await.unwrap(), 0);
    fixture.advance(WINDOW);
    assert_eq!(fixture.agents.sweep_presence().await.unwrap(), 1);
    assert_eq!(frames(&mut bus), [change("frontend", Presence::Idle)]);

    // The call read the time before the sweep did, so it still finds the agent active.
    fixture
        .project
        .clock()
        .set(Timestamp::from_millis(called.as_millis() + WINDOW - 1).unwrap());
    fixture
        .agents
        .record_call(&ctx, &worker, "list_tasks", None)
        .await
        .unwrap();
    assert_eq!(frames(&mut bus), [change("frontend", Presence::Active)]);
}

#[cfg(test)]
async fn wait_for(
    progress: &mut watch::Receiver<JobProgress>,
    done: impl Fn(&JobProgress) -> bool,
) -> JobProgress {
    tokio::time::timeout(WAIT, progress.wait_for(|seen| done(seen)))
        .await
        .unwrap()
        .map(|seen| *seen)
        .unwrap()
}

#[tokio::test]
async fn the_presence_job_sends_the_idle_frame_from_the_clock() {
    let project = TestProject::new().await;
    let clock = project.clock().clone();
    let state: AppState = Arc::new(DaemonState {
        registry: project.registry().clone(),
        projects: Projects::default(),
        clock: Arc::new(clock.clone()),
        assets: AssetSource::Embedded,
        listen: "127.0.0.1".parse().unwrap(),
    });
    let row = ProjectRow {
        slug: "demo".to_owned(),
        name: "Demo".to_owned(),
        root: project.paths().root().to_string_lossy().into_owned(),
    };
    let handle = state
        .projects
        .open(&row, Arc::new(clock.clone()))
        .await
        .unwrap();
    let agents = handle.agents();
    let (ctx, worker) = register(&agents, "frontend").await;
    let mut bus = handle.bus.subscribe();
    agents
        .record_call(&ctx, &worker, "list_tasks", None)
        .await
        .unwrap();
    assert_eq!(frames(&mut bus), [change("frontend", Presence::Active)]);

    let (stop, stopped) = watch::channel(false);
    let job = spawn_presence(state, Duration::from_millis(5), stopped);
    let mut progress = job.progress();
    let seen = wait_for(&mut progress, |seen| seen.checks >= 3).await;
    assert_eq!(seen.cycles, 0, "nothing changed");
    assert_eq!(frames(&mut bus), []);

    clock.advance(Duration::from_millis(WINDOW as u64)).unwrap();
    let frame = tokio::time::timeout(WAIT, async {
        loop {
            if let BusMessage::Live(live) = bus.recv().await.unwrap() {
                return serde_json::from_value::<PresenceChange>(live.data.clone()).unwrap();
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(frame, change("frontend", Presence::Idle));
    let checks = wait_for(&mut progress, |seen| seen.cycles == 1)
        .await
        .checks;
    wait_for(&mut progress, |seen| seen.checks >= checks + 2).await;
    assert_eq!(frames(&mut bus), [], "one frame per crossing");
    assert_eq!(stored_presence_events(&project).await, 0);

    stop.send(true).unwrap();
    job.stopped(WAIT).await;
}
