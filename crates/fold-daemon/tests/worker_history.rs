#![cfg(test)]

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use fold_core::roles::{Role, WorkerState};
use fold_core::{AgentName, AreaName, TaskId, WorkerName, WorkspaceId};
use fold_daemon::live::ActivityState;
use fold_daemon::services::agents::needs_you;
use fold_daemon::services::tasks::sweep_leases;
use fold_daemon::services::workspaces::check_stalls;
use fold_store::StoreError;
use fold_store::agents::{self, NewWorkerProcess, ProcessEnd};
use serde_json::json;

use crate::common::acceptance::Harness;
use crate::common::roles::give_claim;
use crate::worker_resume::{MAIN, TASK_WORKER, task_worker_for};
use crate::workers_support::FakeClaude;

/// About the worker history of a project that ran for weeks.
const HISTORY: usize = 1_200;
/// The default `stall_minutes`.
const STALL: Duration = Duration::from_secs(5 * 60);
/// The default `lease_minutes`.
const LEASE: Duration = Duration::from_secs(30 * 60);
/// The ended task worker that still holds the claim on `T-2`.
const STOPPED_HOLDER: &str = "old-worker-0";
/// The ended task worker whose process could not reach fold.
const FAILED: &str = "old-worker-1";

/// Records `HISTORY` ended task workers of area `web`, each with its agent and main worker, as fold leaves them once they stop: [`FAILED`] failed with `mcp_unavailable`, every other one stopped.
async fn record_history(harness: &Harness) {
    let now = harness.project.now();
    harness
        .handle
        .store
        .write(async |tx| {
            let area = AreaName::parse("web").unwrap();
            let area_id = agents::find_area(tx.conn(), &area).await?.unwrap().id;
            for n in 0..HISTORY {
                let name = AgentName::parse(&format!("old-worker-{n}")).unwrap();
                let agent_id = agents::insert_agent(tx, &name, now).await?;
                agents::insert_worker(tx, agent_id, &WorkerName::main(&name), now).await?;
                let session_id = format!("00000000-0000-4000-8000-{n:012}");
                let id = agents::insert_worker_process(
                    tx,
                    &NewWorkerProcess {
                        agent_id,
                        role: Role::TaskWorker,
                        area_id: Some(area_id),
                        model: None,
                        session_id: &session_id,
                        started_by: "web-lead",
                        now,
                    },
                )
                .await?;
                let end = if name.as_str() == FAILED {
                    ProcessEnd::Failed("mcp_unavailable")
                } else {
                    ProcessEnd::Stopped
                };
                agents::record_exit(tx, id, end, Some(0), now).await?;
            }
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
}

fn advance(harness: &Harness, by: Duration) {
    harness.project.clock().advance(by).unwrap();
}

/// What each hot path decides on the project as one comparable value: the activity the stall rule gives the running task worker's workspace, whether that workspace holds its watch, the agents each `claim.stale` tells by task, and the Needs you items by kind and agent.
#[derive(Debug, PartialEq, Eq)]
struct Answers {
    activity: Option<(ActivityState, Vec<String>)>,
    watched: bool,
    told: Vec<(String, serde_json::Value)>,
    needs_you: BTreeSet<(String, Option<String>)>,
}

/// A running task worker on `T-1` among [`HISTORY`] ended ones, one of which holds `T-2`, past one stall window and one lease.
async fn project_with_history() -> (Harness, WorkspaceId) {
    let fake = FakeClaude::new("result");
    let harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Fix the footer", "labels": ["ui"]}),
        )
        .await;
    record_history(&harness).await;
    give_claim(
        &harness.handle.store,
        STOPPED_HOLDER,
        TaskId::from_number(2).unwrap(),
        harness.project.now(),
    )
    .await;
    (harness, work.workspace.parse().unwrap())
}

async fn answers(harness: &Harness, workspace: WorkspaceId) -> Answers {
    advance(harness, STALL);
    check_stalls(&harness.handle, harness.project.now())
        .await
        .unwrap();
    let activity = harness
        .handle
        .live
        .get(workspace)
        .and_then(|state| state.view.activity.clone())
        .map(|seen| (seen.state, seen.told));
    harness.handle.live.follow_owners().await;
    let watched = harness.handle.live.has_watch(workspace).await;
    advance(harness, LEASE);
    sweep_leases(&harness.handle, harness.project.now())
        .await
        .unwrap();
    let told = harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "claim.stale")
        .map(|event| {
            let task = event.task.map(|task| task.to_string()).unwrap_or_default();
            (task, event.data["told"].clone())
        })
        .collect();
    let needs_you = needs_you(&harness.handle.store)
        .await
        .unwrap()
        .into_iter()
        .map(|item| {
            (
                format!("{:?}", item.kind),
                item.agent.map(|agent| agent.to_string()),
            )
        })
        .collect();
    Answers {
        activity,
        watched,
        told,
        needs_you,
    }
}

/// The stall rule, the watch owners, the lease sweep and Needs you give the same answers with a long worker history as the rules say without one: the running task worker is silent and its lead hears of it, its workspace keeps its watch, a stopped holder's stale claim still goes to the agent that started it, and of the ended workers only the one that failed to reach fold waits for a person.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_hot_paths_give_the_same_answers_with_many_stopped_workers() {
    let (harness, workspace) = project_with_history().await;
    let got = answers(&harness, workspace).await;
    let lead = json!(["web-lead"]);
    assert_eq!(
        got,
        Answers {
            activity: Some((ActivityState::Silent, vec!["web-lead".to_owned()])),
            watched: true,
            told: vec![("T-1".to_owned(), lead.clone()), ("T-2".to_owned(), lead)],
            needs_you: BTreeSet::from([("WorkerUnreachable".to_owned(), Some(FAILED.to_owned()))]),
        }
    );
}

/// The time of each hot path on [`HISTORY`] worker rows, written to stderr for the evidence of a change; nothing asserts it, so a slow machine never fails it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_hot_paths_on_many_worker_rows_are_timed() {
    const RUNS: u32 = 20;
    let (harness, workspace) = project_with_history().await;
    let now = || harness.project.now();
    let started = Instant::now();
    for _ in 0..RUNS {
        check_stalls(&harness.handle, now()).await.unwrap();
    }
    let stalls = started.elapsed() / RUNS;
    let started = Instant::now();
    for _ in 0..RUNS {
        sweep_leases(&harness.handle, now()).await.unwrap();
    }
    let leases = started.elapsed() / RUNS;
    let started = Instant::now();
    for _ in 0..RUNS {
        needs_you(&harness.handle.store).await.unwrap();
    }
    let needs = started.elapsed() / RUNS;
    let started = Instant::now();
    for _ in 0..RUNS {
        harness.handle.live.follow_owners().await;
    }
    let owners = started.elapsed() / RUNS;
    assert!(harness.handle.live.has_watch(workspace).await);
    let mut conn = harness.handle.store.read().acquire().await.unwrap();
    let rows = agents::list_worker_processes(&mut conn)
        .await
        .unwrap()
        .len();
    eprintln!(
        "worker rows {rows}: stalls {stalls:?}, leases {leases:?}, needs you {needs:?}, active owners {owners:?}",
    );
}
