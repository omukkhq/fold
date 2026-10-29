#![cfg(test)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::common::acceptance::{AGENTS, HUMAN, Harness, SLUG, WAIT, read_tree};
use crate::common::invariants::assert_store_consistent;
use crate::common::sse::Sse;
use axum::http::{Method, StatusCode};
use fold_core::events::types;
use fold_core::object::ObjectKind;
use fold_core::tree::{EntryKind, Tree, flatten};
use fold_core::{FoldError, WorkspaceId};
use fold_daemon::http::events::EventEnvelope;
use fold_daemon::jobs::{JobProgress, spawn_auto_checkpoints, spawn_lease_sweeper};
use fold_store::Store;
use fold_store::testing::eventually;
use fold_store::workspaces;
use fold_vcs::objects::ObjectStore;
use serde_json::json;
use sqlx::AssertSqlSafe;
use tokio::sync::watch;

/// How the first daemon ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Graceful,
    Dropped,
}

/// The state of step 1: three tasks with an edge, frontend and backend each holding a claim and a workspace with an edit, automatic checkpoints, an explicit checkpoint, a changeset, an open proposal and one unacknowledged message to each of frontend and backend.
struct Scene {
    harness: Harness,
    tasks: Vec<String>,
    /// frontend's workspace, which gets a new file while the daemon is down.
    edited: (WorkspaceId, PathBuf),
    /// backend's workspace, whose directory is deleted while the daemon is down.
    deleted: (WorkspaceId, PathBuf),
    /// The message to frontend, then the one to backend.
    messages: [String; 2],
}

impl Scene {
    async fn build() -> Self {
        let harness = Harness::start().await;
        fs::write(
            harness.project.paths().config(),
            "auto_checkpoint_quiet_seconds = 1\n",
        )
        .unwrap();
        let mut tasks = Vec::new();
        for (title, lead) in [
            ("Build the login screen", "frontend"),
            ("Add the login endpoint", "backend"),
            ("Ship the login", "frontend"),
        ] {
            let created = harness
                .call(
                    "review",
                    "create_task",
                    json!({"title": title, "labels": Harness::work_labels_of(lead)}),
                )
                .await;
            tasks.push(created["id"].as_str().unwrap().to_owned());
        }
        harness
            .call(
                "review",
                "add_dependency",
                json!({"task": tasks[2], "needs": tasks[0]}),
            )
            .await;
        let mut created = Vec::new();
        for (agent, task, file) in [
            ("frontend", &tasks[0], "src/screen.txt"),
            ("backend", &tasks[1], "src/endpoint.txt"),
        ] {
            harness.call(agent, "claim_task", json!({"id": task})).await;
            let workspace = harness
                .call(agent, "create_workspace", json!({"task": task}))
                .await;
            let id: WorkspaceId = workspace["workspace"].as_str().unwrap().parse().unwrap();
            let dir = Path::new(harness.root()).join(workspace["path"].as_str().unwrap());
            fs::create_dir_all(dir.join("src")).unwrap();
            fs::write(dir.join(file), format!("{agent} was here\n")).unwrap();
            created.push((id, dir));
        }
        for (id, _) in &created {
            live_files(&harness, *id, 1).await;
        }

        harness
            .project
            .clock()
            .advance(Duration::from_secs(2))
            .unwrap();
        let (stop, stopped) = watch::channel(false);
        let job = spawn_auto_checkpoints(harness.state.clone(), Duration::from_millis(10), stopped);
        for (agent, (id, _)) in [("frontend", &created[0]), ("backend", &created[1])] {
            eventually(WAIT, async || {
                let listed = harness
                    .call(agent, "list_checkpoints", json!({"ws": id.to_string()}))
                    .await;
                if listed["checkpoints"].as_array().unwrap().is_empty() {
                    Err(listed)
                } else {
                    Ok(())
                }
            })
            .await;
        }
        stop.send(true).unwrap();
        job.stopped(WAIT).await;

        let (frontend_ws, backend_ws) = (created[0].0.to_string(), created[1].0.to_string());
        harness
            .call(
                "frontend",
                "record_checkpoint",
                json!({"ws": frontend_ws, "label": "screen drafted"}),
            )
            .await;
        harness
            .call(
                "backend",
                "record_changes",
                json!({"ws": backend_ws, "message": "Add the login endpoint"}),
            )
            .await;
        let proposal = harness
            .call(
                "frontend",
                "create_proposal",
                json!({"intent": "Make the change the task asks for.", "task": tasks[0], "title": "Build the login screen"}),
            )
            .await;
        assert_eq!(proposal["id"], "P-1", "{proposal}");
        let messages = [
            harness
                .send("review", &["frontend"], "Is the screen done?")
                .await,
            harness
                .send("review", &["backend"], "Is the endpoint up?")
                .await,
        ];
        let deleted = created.pop().unwrap();
        let edited = created.pop().unwrap();
        Self {
            harness,
            tasks,
            edited,
            deleted,
            messages,
        }
    }
}

async fn head_tree(store: &Store) -> fold_core::object::ObjectHash {
    workspaces::head(store.read())
        .await
        .unwrap()
        .unwrap()
        .tree_hash
}

/// Every file of the head revision by path, with its bytes.
async fn head_files(harness: &Harness) -> BTreeMap<String, Vec<u8>> {
    let root = head_tree(&harness.handle.store).await;
    let objects = ObjectStore::open(&harness.project.paths().fold_dir()).unwrap();
    let load = |hash: &fold_core::object::ObjectHash| -> Result<Tree, FoldError> {
        let object = objects.read_expecting(hash, ObjectKind::Tree)?;
        Ok(Tree::decode(&object.payload)?)
    };
    flatten(&root, &load)
        .unwrap()
        .into_iter()
        .filter(|(_, entry)| entry.kind == EntryKind::File)
        .map(|(path, entry)| {
            let blob = objects
                .read_expecting(&entry.hash, ObjectKind::Blob)
                .unwrap();
            (String::from_utf8(path).unwrap(), blob.payload.to_vec())
        })
        .collect()
}

/// Waits until the live state of `workspace` lists `count` changed files, and returns their paths.
async fn live_files(harness: &Harness, workspace: WorkspaceId, count: usize) -> Vec<String> {
    let handle = &harness.handle;
    eventually(WAIT, async || {
        let files: Option<Vec<String>> = handle.live.get(workspace).map(|state| {
            state
                .view
                .files
                .iter()
                .map(|file| file.path.clone())
                .collect()
        });
        match files {
            Some(files) if files.len() == count => Ok(files),
            other => Err(other),
        }
    })
    .await
}

/// The columns a restart may change: a session open at the stop ends then or at the start, the deleted workspace moves to `missing`, and the live rescan refreshes the changed-file count of the edited one.
const RESTART_CHANGES: [(&str, &str); 3] = [
    ("sessions", "ended_at"),
    ("workspaces", "state"),
    ("workspaces", "changed_files"),
];

/// Every row of every table of the project database, by table, each row as the SQL quote of its columns, ordered by its columns from the first. SQLite's own tables, the migration table and the [`RESTART_CHANGES`] columns are left out.
async fn dump(store: &Store) -> BTreeMap<String, Vec<String>> {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table'
         AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations' ORDER BY name",
    )
    .fetch_all(store.read())
    .await
    .unwrap();
    let mut rows = BTreeMap::new();
    for table in tables {
        let columns: Vec<String> = sqlx::query_scalar(AssertSqlSafe(format!(
            "SELECT name FROM pragma_table_info('{table}')"
        )))
        .fetch_all(store.read())
        .await
        .unwrap();
        let columns: Vec<String> = columns
            .into_iter()
            .filter(|column| !RESTART_CHANGES.contains(&(table.as_str(), column.as_str())))
            .collect();
        let quoted: Vec<String> = columns
            .iter()
            .map(|column| format!("quote(\"{column}\")"))
            .collect();
        let order: Vec<String> = columns
            .iter()
            .map(|column| format!("\"{column}\""))
            .collect();
        let sql = format!(
            "SELECT json_array({}) FROM \"{table}\" ORDER BY {}",
            quoted.join(", "),
            order.join(", ")
        );
        let found: Vec<String> = sqlx::query_scalar(AssertSqlSafe(sql))
            .fetch_all(store.read())
            .await
            .unwrap();
        rows.insert(table, found);
    }
    rows
}

/// The `(seq, type)` of every stored event after `after`, in order.
async fn events_after(store: &Store, after: i64) -> Vec<(i64, String)> {
    sqlx::query_as("SELECT seq, type FROM events WHERE seq > ?1 ORDER BY seq")
        .bind(after)
        .fetch_all(store.read())
        .await
        .unwrap()
}

async fn last_seq(store: &Store) -> i64 {
    sqlx::query_scalar("SELECT COALESCE(MAX(seq), 0) FROM events")
        .fetch_one(store.read())
        .await
        .unwrap()
}

/// Reads `get_events` as `agent` after its cursor until a read returns nothing, and returns the seqs in order.
async fn read_events(harness: &Harness, agent: &str) -> Vec<i64> {
    let mut seqs = Vec::new();
    tokio::time::timeout(WAIT, async {
        loop {
            let page = harness
                .call(agent, "get_events", json!({"limit": 500}))
                .await;
            let events = page["events"].as_array().unwrap();
            if events.is_empty() {
                return;
            }
            seqs.extend(events.iter().map(|event| event["seq"].as_i64().unwrap()));
        }
    })
    .await
    .expect("the backlog drains within the wait");
    seqs
}

async fn progress_at_least(progress: &mut watch::Receiver<JobProgress>, checks: u64) {
    tokio::time::timeout(WAIT, progress.wait_for(|seen| seen.checks >= checks))
        .await
        .unwrap()
        .unwrap();
}

/// Acceptance 8, in the variant that ends the first daemon as `stop` says.
async fn restart_recovery(stop: Stop) {
    // Step 1: the state.
    let mut scene = Scene::build().await;

    // Step 2: a stream read to N, then more activity up to M. The reviewer's cursor stands at N.
    let n = last_seq(&scene.harness.handle.store).await;
    let mut sse = Sse::open(&scene.harness.state, Some("0")).await;
    sse.until("the event N", |frame| {
        (frame.id.as_deref() == Some(n.to_string().as_str())).then_some(())
    })
    .await;
    drop(sse);
    scene.harness.drain_events("review").await;
    let later = scene
        .harness
        .call(
            "review",
            "create_task",
            json!({"title": "Write the release notes"}),
        )
        .await;
    let later = later["id"].as_str().unwrap().to_owned();
    let (code, answer) = scene
        .harness
        .http(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/tasks/{later}/checklist-items"),
            Some(&json!({"text": "List the login changes"})),
            HUMAN,
        )
        .await;
    assert_eq!(code, StatusCode::CREATED, "{answer}");
    let mut statuses = Vec::new();
    for task in &scene.tasks {
        let detail = scene
            .harness
            .call("review", "get_task", json!({"id": task}))
            .await;
        statuses.push(detail["status"].clone());
    }
    assert_eq!(statuses[..2], [json!("review"), json!("in_progress")]);
    let store = scene.harness.handle.store.clone();
    let m = last_seq(&store).await;
    assert!(m > n);
    let missed = events_after(&store, n).await;
    let before = dump(&store).await;

    // Step 3: the daemon ends.
    match stop {
        Stop::Graceful => scene.harness.stop().await,
        Stop::Dropped => scene.harness.drop_daemon().await,
    }

    // Step 4: while it is down, one workspace directory goes and the other gets a new file.
    fs::remove_dir_all(&scene.deleted.1).unwrap();
    fs::write(scene.edited.1.join("src/after.txt"), "written while down\n").unwrap();
    let clock = scene.harness.project.clock().clone();
    clock.advance(Duration::from_secs(5)).unwrap();

    // Step 5: a new daemon on the same directory.
    scene.harness.start_again().await;
    let harness = &mut scene.harness;
    let store = harness.handle.store.clone();
    let after = dump(&store).await;
    for (table, rows) in &before {
        if table == "events" {
            assert_eq!(
                &after[table][..rows.len()],
                &rows[..],
                "recovery only appends events"
            );
        } else {
            assert_eq!(&after[table], rows, "{table}");
        }
    }
    let states: Vec<(i64, String)> =
        sqlx::query_as("SELECT number, state FROM workspaces ORDER BY number")
            .fetch_all(store.read())
            .await
            .unwrap();
    let mut expected_states = vec![
        (i64::from(scene.edited.0.number()), "active".to_owned()),
        (i64::from(scene.deleted.0.number()), "missing".to_owned()),
    ];
    expected_states.sort();
    assert_eq!(states, expected_states);
    assert_store_consistent(&store, harness.project.paths()).await;

    for agent in AGENTS {
        harness.connect(agent).await;
    }
    let deleted = harness
        .call(
            "review",
            "get_workspace",
            json!({"ws": scene.deleted.0.to_string()}),
        )
        .await;
    assert_eq!(deleted["state"], "missing", "{deleted}");
    let edited = harness
        .call(
            "review",
            "get_workspace",
            json!({"ws": scene.edited.0.to_string()}),
        )
        .await;
    assert_eq!(edited["state"], "active", "{edited}");
    for (task, status) in scene.tasks.iter().zip(&statuses) {
        let detail = harness
            .call("review", "get_task", json!({"id": task}))
            .await;
        assert_eq!(&detail["status"], status, "{detail}");
    }
    let proposal = harness
        .call("review", "get_proposal", json!({"id": "P-1"}))
        .await;
    assert_eq!(proposal["state"], "open", "{proposal}");
    for (agent, message) in ["frontend", "backend"].iter().zip(&scene.messages) {
        assert_eq!(
            harness.inbox(agent).await,
            std::slice::from_ref(message),
            "{agent}"
        );
    }
    let seqs = read_events(harness, "review").await;
    let newest = last_seq(&store).await;
    assert_eq!(
        seqs,
        ((n + 1)..=newest).collect::<Vec<_>>(),
        "no gap after the old cursor"
    );

    // Step 6: the stream resumes after N with every stored event once, then the snapshot.
    let mut files = live_files(harness, scene.edited.0, 2).await;
    files.sort();
    assert_eq!(files, ["src/after.txt", "src/screen.txt"]);
    let stored = events_after(&store, n).await;
    let mut sse = Sse::open(&harness.state, Some(&n.to_string())).await;
    let mut replayed = Vec::new();
    let snapshot = loop {
        let frame = sse.next().await;
        match frame.envelope {
            EventEnvelope::Event(event) => {
                assert_eq!(frame.id, Some(event.seq.to_string()));
                replayed.push(event);
            }
            EventEnvelope::Snapshot(snapshot) => break snapshot,
            other => panic!("a live frame before the snapshot: {other:?}"),
        }
    };
    let replayed_ids: Vec<(i64, String)> = replayed
        .iter()
        .map(|event| {
            (
                i64::try_from(event.seq).unwrap(),
                event.event_type.as_str().to_owned(),
            )
        })
        .collect();
    assert_eq!(replayed_ids, stored, "exactly the stored events after N");
    assert_eq!(&replayed_ids[..missed.len()], &missed[..], "N+1 to M first");
    let restart = &replayed[missed.len()..];
    let missing: Vec<WorkspaceId> = restart
        .iter()
        .filter(|event| event.event_type == types::WORKSPACE_MISSING)
        .filter_map(|event| event.workspace)
        .collect();
    assert_eq!(missing, [scene.deleted.0]);
    let mut ended: Vec<(String, Option<String>)> = restart
        .iter()
        .filter(|event| event.event_type == types::SESSION_ENDED)
        .map(|event| {
            (
                event.data["agent"].as_str().unwrap().to_owned(),
                event.data["reason"].as_str().map(str::to_owned),
            )
        })
        .collect();
    ended.sort();
    let reason = match stop {
        Stop::Graceful => None,
        Stop::Dropped => Some("daemon_restart".to_owned()),
    };
    let mut expected: Vec<(String, Option<String>)> = AGENTS
        .iter()
        .map(|agent| ((*agent).to_owned(), reason.clone()))
        .collect();
    expected.sort();
    assert_eq!(ended, expected);
    assert_eq!(snapshot.seq, u64::try_from(newest).unwrap());
    let shown: Vec<WorkspaceId> = snapshot
        .workspaces
        .iter()
        .map(|view| view.workspace)
        .collect();
    assert_eq!(
        shown,
        [scene.edited.0],
        "the missing workspace has no live state"
    );
    assert!(
        snapshot.workspaces[0]
            .files
            .iter()
            .any(|file| file.path == "src/after.txt"),
        "{snapshot:?}"
    );
    drop(sse);

    // Step 7: the project folder shows the head and the project is consistent.
    assert_eq!(
        read_tree(harness.project.paths().root()),
        head_files(harness).await
    );
    assert_store_consistent(&store, harness.project.paths()).await;

    // Step 8: 31 minutes later the lease sweeper marks each claim stale once.
    let marked = last_seq(&store).await;
    let (stop_jobs, stopped) = watch::channel(false);
    let sweeper = spawn_lease_sweeper(harness.state.clone(), Duration::from_millis(10), stopped);
    let mut progress = sweeper.progress();
    progress_at_least(&mut progress, 2).await;
    assert_eq!(progress.borrow().cycles, 0, "no lease ran out yet");
    clock.advance(Duration::from_secs(31 * 60)).unwrap();
    tokio::time::timeout(WAIT, progress.wait_for(|seen| seen.cycles >= 1))
        .await
        .unwrap()
        .unwrap();
    let checks = progress.borrow().checks;
    progress_at_least(&mut progress, checks + 3).await;
    stop_jobs.send(true).unwrap();
    sweeper.stopped(WAIT).await;
    let stale: Vec<Option<i64>> = sqlx::query_scalar(
        "SELECT task_id FROM events WHERE type = 'claim.stale' AND seq > ?1 ORDER BY task_id",
    )
    .bind(marked)
    .fetch_all(store.read())
    .await
    .unwrap();
    assert_eq!(stale, [Some(1), Some(2)], "one claim.stale for each claim");
}

/// Acceptance 8 after a graceful stop.
#[tokio::test]
async fn acceptance_8_restart_recovery_graceful() {
    restart_recovery(Stop::Graceful).await;
}

/// Acceptance 8 after a daemon dropped without a shutdown.
#[tokio::test]
async fn acceptance_8_restart_recovery_dropped() {
    restart_recovery(Stop::Dropped).await;
}
