use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::ManualClock;
use fold_daemon::daemon::AppState;
use fold_daemon::http::router;
use fold_daemon::services::agents::Launcher;
use fold_daemon::{Daemon, DaemonConfig, RunningDaemon};
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tokio::process::Command;
use tower::ServiceExt;

const BASE: &str = "/api/v1/projects/demo";

/// A private host with a daemon in this process, so the board can be read through the router without a port.
#[cfg(test)]
struct Host {
    dir: TempDir,
    daemon: RunningDaemon,
}

#[cfg(test)]
impl Host {
    async fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let runtime = dir.path().join("run");
        std::fs::create_dir_all(&runtime).unwrap();
        let socket = runtime.join("fold.sock");
        let env =
            |name: &str, path: PathBuf| (name.to_owned(), path.to_string_lossy().into_owned());
        // The workers run the stub of the fld under test, and their `fld mcp` reaches this daemon only.
        let launcher = Launcher {
            claude: dir.path().join("no-claude"),
            fold: Some(PathBuf::from(env!("CARGO_BIN_EXE_fld"))),
            env: vec![
                env("FOLD_SOCKET", socket.clone()),
                env("HOME", dir.path().to_owned()),
                env("XDG_DATA_HOME", dir.path().join("data")),
                env("XDG_CONFIG_HOME", dir.path().join("config")),
                env("XDG_RUNTIME_DIR", runtime.clone()),
            ],
        };
        let daemon = Daemon::start_with(
            DaemonConfig {
                listen: "127.0.0.1:0".parse().unwrap(),
                socket,
                data_dir: dir.path().join("data").join("fold"),
                runtime_dir: runtime,
                web_dir: None,
                session_lifetime: fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME,
            },
            Arc::new(ManualClock::new()),
            launcher,
        )
        .await
        .unwrap();
        Self { dir, daemon }
    }

    fn project(&self) -> std::path::PathBuf {
        self.dir.path().join("demo")
    }

    async fn seed(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_fld"))
            .args(["dev", "seed", "--dir"])
            .arg(self.project())
            .arg("--socket")
            .arg(self.daemon.socket_path())
            .current_dir(self.dir.path())
            .env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_DATA_HOME", self.dir.path().join("data"))
            .env("XDG_CONFIG_HOME", self.dir.path().join("config"))
            .env("FOLD_HUMAN", "dev")
            // Coverage runs name the profile file through this variable, which `env_clear` drops.
            .envs(
                std::env::var_os("LLVM_PROFILE_FILE").map(|profile| ("LLVM_PROFILE_FILE", profile)),
            )
            .output()
            .await
            .unwrap()
    }

    async fn get(&self, path: &str) -> Value {
        let state: AppState = self.daemon.state().clone();
        let request = Request::builder()
            .method(Method::GET)
            .uri(format!("{BASE}{path}"))
            .header("cookie", "fold_human=dev")
            .body(Body::empty())
            .unwrap();
        let response = router(state).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "GET {path}");
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }
}

#[cfg(test)]
fn cards(board: &Value) -> Vec<&Value> {
    board["columns"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|column| column["cards"].as_array().unwrap())
        .collect()
}

/// The board without each card's `activity` and `stalled` attention. The workspace watcher reports the seed's last file writes on its own schedule after the seed has ended, so those fields can change between two reads while no command runs.
#[cfg(test)]
fn stored_board(mut board: Value) -> Value {
    for column in board["columns"].as_array_mut().unwrap() {
        for card in column["cards"].as_array_mut().unwrap() {
            let card = card.as_object_mut().unwrap();
            card.remove("activity");
            if let Some(Value::Array(items)) = card.get_mut("attention") {
                items.retain(|item| item["kind"] != "stalled");
            }
        }
    }
    board
}

#[cfg(test)]
fn has_attention(card: &Value, kind: &str) -> bool {
    card["attention"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["kind"] == kind)
}

#[cfg(test)]
fn text(output: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[tokio::test]
async fn the_seed_fills_every_column_and_every_state_of_the_review_screens() {
    let host = Host::new().await;
    let output = host.seed().await;
    let (stdout, stderr) = text(&output);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("seeded project demo"), "{stdout}");
    assert!(stdout.contains("board: http://127.0.0.1:7420/p/demo/board"));

    let board = host.get("/board").await;
    let columns = board["columns"].as_array().unwrap();
    let statuses: Vec<&str> = columns
        .iter()
        .map(|column| column["status"].as_str().unwrap())
        .collect();
    assert_eq!(
        statuses,
        ["backlog", "todo", "in_progress", "review", "done"]
    );
    let totals: Vec<u64> = columns
        .iter()
        .map(|column| column["total"].as_u64().unwrap())
        .collect();
    assert_eq!(
        totals,
        [2, 2, 4, 2, 3],
        "the seed builds the same board every run"
    );

    let cards = cards(&board);
    assert!(
        cards.iter().any(|card| card["blocked"] == true),
        "a card with the blocked indicator"
    );
    let done = columns
        .iter()
        .find(|column| column["status"] == "done")
        .unwrap()["cards"]
        .as_array()
        .unwrap();
    assert!(
        done.iter()
            .any(|card| card["title"] == "Ship the user dashboard"),
        "the container task is Done by its children"
    );
    let question = cards
        .iter()
        .find(|card| has_attention(card, "question"))
        .expect("a task with the question item");
    assert_eq!(question["title"], "Limit requests per client");
    let marked = cards
        .iter()
        .find(|card| has_attention(card, "human_review"))
        .expect("a task awaiting a human review");
    assert_eq!(marked["human_review"]["reviewer"], "dev");
    assert_eq!(marked["status"], "review");

    let proposals = host.get("/proposals").await;
    let proposals = proposals["proposals"].as_array().unwrap();
    assert!(
        proposals
            .iter()
            .any(|proposal| proposal["conflicted"] == true)
    );
    assert!(
        proposals
            .iter()
            .any(|proposal| proposal["current_version"] == 2),
        "a proposal with a second version"
    );
    let awaiting = proposals
        .iter()
        .find(|proposal| proposal["task"] == marked["task"])
        .unwrap();
    assert_eq!(awaiting["state"], "open");

    let mut ranges = 0;
    let mut sides = Vec::new();
    for proposal in proposals {
        let id = proposal["id"].as_str().unwrap();
        let comments = host.get(&format!("/proposals/{id}/comments")).await;
        for comment in comments["comments"].as_array().unwrap() {
            if let (Some(start), Some(end)) =
                (comment["start_line"].as_u64(), comment["end_line"].as_u64())
            {
                ranges += usize::from(start < end);
                sides.push(comment["side"].as_str().unwrap().to_owned());
            }
        }
    }
    assert_eq!(ranges, 1, "one comment on a range of lines");
    assert!(sides.iter().any(|side| side == "old"));
    assert!(sides.iter().any(|side| side == "new"));

    // Every agent but the main agent is a worker that fold started for its lead, with its role and the stub's model.
    let workers = host.get("/workers").await;
    let workers = workers["workers"].as_array().unwrap();
    let started = |role: &str, by: &str| {
        workers
            .iter()
            .filter(|worker| worker["role"] == role && worker["started_by"] == by)
            .count()
    };
    assert_eq!(started("review_lead", "main"), 1);
    assert_eq!(started("area_lead", "main"), 2);
    assert_eq!(started("task_worker", "api-lead"), 4);
    assert_eq!(started("task_worker", "web-lead"), 4);
    assert_eq!(started("reviewer", "review-lead"), 6);
    assert_eq!(workers.len(), 17);
    assert!(
        workers
            .iter()
            .all(|worker| worker["model"] == "fold-dev-stub" && worker["state"] != "failed")
    );
    let task_workers: Vec<&str> = workers
        .iter()
        .filter(|worker| worker["role"] == "task_worker")
        .map(|worker| worker["name"].as_str().unwrap())
        .collect();
    assert!(
        proposals
            .iter()
            .all(|proposal| task_workers.contains(&proposal["author"].as_str().unwrap())),
        "each proposal comes from the task worker its lead started"
    );
    let routes = cards
        .iter()
        .find(|card| card["title"] == "Add the user routes")
        .unwrap();
    assert_eq!(routes["status"], "in_progress");
}

#[tokio::test]
async fn a_second_run_fails_with_seed_exists_and_changes_nothing() {
    let host = Host::new().await;
    assert!(host.seed().await.status.success());
    let board = stored_board(host.get("/board").await);
    let proposals = host.get("/proposals").await;

    let output = host.seed().await;
    let (stdout, stderr) = text(&output);
    assert!(!output.status.success());
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("seed_exists"), "{stderr}");
    assert!(stderr.contains("make clean-keep-cargo"), "{stderr}");
    assert_eq!(stored_board(host.get("/board").await), board);
    assert_eq!(host.get("/proposals").await, proposals);
    assert!(Path::new(&host.project()).exists());
}
