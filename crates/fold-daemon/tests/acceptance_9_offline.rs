#![cfg(test)]

use std::collections::BTreeMap;
use std::path::Path;

use crate::common::acceptance::{Harness, Work, assert_in_order, read_tree};
use crate::common::invariants::assert_consistent;
use crate::common::sse::Sse;
use fold_core::{GitConfig, ProjectConfig};
use fold_daemon::http::events::EventEnvelope;
use serde_json::{Value, json};
use tempfile::TempDir;

/// The ignored test that the outer test runs inside the namespace.
const INNER: &str = "acceptance_9_offline::acceptance_9_offline_inner";
/// Set by the outer test on the child, so the inner test proves the isolation only where it is promised.
const NAMESPACE_VAR: &str = "FOLD_TEST_OFFLINE_NAMESPACE";
/// The line the inner test prints once the isolation is proven; the outer test looks for it.
const PROOF: &str = "offline proof:";
const PUBLIC_ADDRESS: &str = "1.1.1.1:443";

/// The folder `fld init` makes the project: `r1` holds exactly these files.
const SOURCE: [(&str, &str); 3] = [
    ("README.md", "# demo\n"),
    ("src/app.txt", "start the app\n"),
    ("docs/notes.txt", "notes for the team\n"),
];
const ENDPOINT: (&str, &str) = ("src/login.txt", "POST /login\n");
const SCREEN_README: &str = "# demo: the login screen\n";
const ENDPOINT_README: &str = "# demo: the login endpoint\n";
const REMOTE_EVENTS: [&str; 4] = [
    "remote.pushed",
    "remote.push_failed",
    "remote.pulled",
    "remote.pull_conflicted",
];

/// Acceptance 9: the whole flow runs in a new user and network namespace, whose only interface is a loopback that is down, so a connect to any address fails at once. The child is this test binary running [`INNER`].
#[cfg(target_os = "linux")]
#[test]
fn acceptance_9_offline() {
    use std::io::ErrorKind;
    use std::process::Command;

    let exe = std::env::current_exe().unwrap();
    let output = Command::new("unshare")
        .args(["--user", "--map-root-user", "--net"])
        .arg(&exe)
        .args(["--exact", INNER, "--ignored", "--nocapture"])
        .env(NAMESPACE_VAR, "1")
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) if error.kind() == ErrorKind::NotFound => panic!(
            "acceptance 9 runs fold with no network through `unshare`, which is missing: install the util-linux package ({error})"
        ),
        Err(error) => panic!("unshare did not start: {error}"),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let report = format!("--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}");
    if !output.status.success() {
        if stdout.contains("running 1 test") {
            panic!("the offline flow failed inside the namespace\n{report}");
        }
        panic!(
            "unshare could not create a user and network namespace: it needs `unshare` from the util-linux package and unprivileged user namespaces (sysctl kernel.unprivileged_userns_clone=1, and kernel.apparmor_restrict_unprivileged_userns=0 where AppArmor restricts them)\n{report}"
        );
    }
    assert!(
        stdout.contains("test result: ok. 1 passed"),
        "the child did not run {INNER}\n{report}"
    );
    assert!(
        stdout.contains(PROOF),
        "the child did not prove that it has no network\n{report}"
    );
}

/// Other platforms have no namespace to run in; the flow needs no network, so it runs directly.
#[cfg(not(target_os = "linux"))]
#[tokio::test]
async fn acceptance_9_offline() {
    flow().await;
}

/// The flow of acceptance 9. Run alone it still passes, which shows that the flow needs no network; inside the outer test's namespace it first proves that there is none.
#[tokio::test]
#[ignore = "acceptance_9_offline runs it inside a namespace with no network"]
async fn acceptance_9_offline_inner() {
    if std::env::var_os(NAMESPACE_VAR).is_some() {
        prove_isolation();
    }
    flow().await;
}

/// A name lookup is no proof: through nss-resolve it goes over a local Unix socket, which the namespace does not cut. A connect to a literal address and the interface list depend on the network namespace alone.
fn prove_isolation() {
    use std::io::ErrorKind;
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;

    let interfaces: Vec<String> = std::fs::read_to_string("/proc/net/dev")
        .unwrap()
        .lines()
        .skip(2)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, _)| name.trim().to_owned())
        .collect();
    assert_eq!(
        interfaces,
        ["lo"],
        "the namespace has a network interface other than loopback"
    );
    let address: SocketAddr = PUBLIC_ADDRESS.parse().unwrap();
    let connect = TcpStream::connect_timeout(&address, Duration::from_secs(2))
        .expect_err("a connect to a public address succeeded inside the namespace");
    assert!(
        matches!(
            connect.kind(),
            ErrorKind::NetworkUnreachable | ErrorKind::HostUnreachable
        ),
        "a connect to {address} failed with {connect}, which is not an unreachable network"
    );
    println!("{PROOF} the only interface is lo; connect to {address} failed ({connect})");
}

async fn status(harness: &Harness, task: &str) -> Value {
    harness
        .call("review", "get_task", json!({"id": task}))
        .await["status"]
        .clone()
}

/// `(path, change)` of each entry of a `files` list.
fn changes(body: &Value) -> Vec<(String, String)> {
    body["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            (
                file["path"].as_str().unwrap().to_owned(),
                file["change"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// `worker` of `agent` claims `task` and creates its workspace.
async fn start_work_as(harness: &Harness, agent: &str, worker: &str, task: &str) -> Work {
    let claimed = harness
        .call(agent, "claim_task", json!({"id": task, "as": worker}))
        .await;
    assert_eq!(claimed["holder"], worker);
    let created = harness
        .call(
            agent,
            "create_workspace",
            json!({"task": task, "as": worker}),
        )
        .await;
    Work {
        task: task.to_owned(),
        workspace: created["workspace"].as_str().unwrap().to_owned(),
        dir: Path::new(harness.root()).join(created["path"].as_str().unwrap()),
    }
}

/// `worker` of `agent` records the files of `work` and opens a proposal; returns its id.
async fn propose_as(
    harness: &Harness,
    agent: &str,
    worker: &str,
    work: &Work,
    title: &str,
) -> String {
    harness
        .call(
            agent,
            "record_changes",
            json!({"ws": work.workspace, "message": title, "as": worker}),
        )
        .await;
    let created = harness
        .call(
            agent,
            "create_proposal",
            json!({"task": work.task, "title": title, "evidence": "tests pass", "as": worker}),
        )
        .await;
    assert_eq!(created["status"], "review", "{created}");
    created["id"].as_str().unwrap().to_owned()
}

/// `(seq, type)` of every stored event after `after`.
async fn stored_after(harness: &Harness, after: u64) -> BTreeMap<u64, String> {
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT seq, type FROM events WHERE seq > ? ORDER BY seq")
            .bind(i64::try_from(after).unwrap())
            .fetch_all(harness.handle.store.read())
            .await
            .unwrap();
    rows.into_iter()
        .map(|(seq, kind)| (u64::try_from(seq).unwrap(), kind))
        .collect()
}

async fn flow() {
    let sources = TempDir::new().unwrap();
    let source = sources.path().join("app");
    for (path, content) in SOURCE {
        let target = source.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, content).unwrap();
    }
    let (harness, initialized) = Harness::start_initialized(&source).await;

    // No remote and no push on integrate, so no integration starts a `git` process.
    let config = std::fs::read_to_string(harness.project.paths().config()).unwrap();
    assert_eq!(
        ProjectConfig::from_toml(&config).unwrap().git,
        GitConfig::default()
    );

    let mut sse = Sse::open(&harness.state, None).await;
    let opened = sse.next().await;
    let EventEnvelope::Snapshot(snapshot) = &opened.envelope else {
        panic!("the snapshot comes first: {opened:?}");
    };
    let watched_from = snapshot.seq;

    // 1. History: init made r1 from the folder's files and the ignore files it wrote beside them.
    assert_eq!(initialized.revision.to_string(), "r1");
    assert_eq!(initialized.imported.files, 5);
    let repository = harness
        .call("review", "get_repository_state", json!({}))
        .await;
    assert_eq!(repository["head"], "r1");
    assert_eq!(repository["recent"].as_array().unwrap().len(), 1);
    let r1 = harness
        .call("review", "get_revision", json!({"r": "r1"}))
        .await;
    let mut listed: Vec<String> = changes(&r1).into_iter().map(|(path, _)| path).collect();
    listed.sort();
    let mut expected: Vec<String> = SOURCE.iter().map(|(path, _)| (*path).to_owned()).collect();
    expected.extend([".foldignore".to_owned(), ".gitignore".to_owned()]);
    expected.sort();
    assert_eq!(listed, expected, "{r1}");
    let source_tree: BTreeMap<String, Vec<u8>> = SOURCE
        .iter()
        .map(|(path, content)| ((*path).to_owned(), content.as_bytes().to_vec()))
        .collect();
    // The project folder is main, ignore files included.
    let mut folder = read_tree(harness.project.paths().root());
    assert!(folder.remove(".foldignore").is_some());
    assert!(folder.remove(".gitignore").is_some());
    assert_eq!(folder, source_tree);

    // 2. Task changes: the screen needs the endpoint, so it waits in Backlog.
    let endpoint = harness
        .create_task("backend", "Add the login endpoint")
        .await;
    let screen = harness
        .create_task("frontend", "Build the login screen")
        .await;
    harness
        .call(
            "frontend",
            "add_dependency",
            json!({"task": screen, "needs": endpoint}),
        )
        .await;
    assert_eq!(status(&harness, &endpoint).await, "todo");
    assert_eq!(status(&harness, &screen).await, "backlog");

    // 3. Workspaces: claim, workspace, edit, checkpoint, changeset.
    let work = harness.start_work("backend", &endpoint).await;
    work.write(&[ENDPOINT]);
    let working = harness
        .call(
            "backend",
            "get_workspace_changes",
            json!({"ws": work.workspace}),
        )
        .await;
    assert_eq!(
        changes(&working),
        [(ENDPOINT.0.to_owned(), "added".to_owned())]
    );
    let checkpoint = harness
        .call(
            "backend",
            "record_checkpoint",
            json!({"ws": work.workspace, "label": "endpoint drafted"}),
        )
        .await;
    assert!(
        checkpoint["checkpoint"]
            .as_str()
            .unwrap()
            .starts_with("ck-"),
        "{checkpoint}"
    );
    let recorded = harness
        .call(
            "backend",
            "record_changes",
            json!({"ws": work.workspace, "message": "Add the login endpoint"}),
        )
        .await;
    assert!(
        recorded["changeset"].as_str().unwrap().starts_with("C-"),
        "{recorded}"
    );
    assert_eq!(status(&harness, &endpoint).await, "in_progress");

    // 4. Messages: a question, a reply in its thread, both acknowledged.
    let question = harness
        .send("frontend", &["backend"], "Is the login endpoint recorded?")
        .await;
    assert_eq!(harness.inbox("backend").await, [question.as_str()]);
    let answer = harness
        .send_with(
            "backend",
            json!({"to": ["frontend"], "body": "Yes, as a changeset.", "reply_to": question}),
        )
        .await;
    harness
        .call("backend", "acknowledge_message", json!({"id": question}))
        .await;
    assert_eq!(harness.inbox("frontend").await, [answer.as_str()]);
    harness
        .call("frontend", "acknowledge_message", json!({"id": answer}))
        .await;
    assert!(harness.inbox("backend").await.is_empty());
    assert!(harness.inbox("frontend").await.is_empty());

    // 5. Proposal and integration: review, the review lead, approves and integrates.
    crate::common::roles::give_role(
        &harness.handle.store,
        "review",
        fold_core::roles::Role::ReviewLead,
        None,
    )
    .await;
    let created = harness
        .call(
            "backend",
            "create_proposal",
            json!({"task": endpoint, "title": "Add the login endpoint", "evidence": "tests pass"}),
        )
        .await;
    assert_eq!(created["status"], "review", "{created}");
    let proposal = created["id"].as_str().unwrap().to_owned();
    harness
        .call("review", "claim_review", json!({"id": proposal}))
        .await;
    harness.approve(&proposal).await;
    let integrated = harness
        .call(
            "review",
            "integrate_proposal",
            json!({"id": proposal, "expected_version": 1}),
        )
        .await;
    assert_eq!(
        integrated,
        json!({"id": proposal, "result": "succeeded", "revision": "r2"})
    );
    assert_eq!(harness.head().await.number.to_string(), "r2");
    assert_eq!(status(&harness, &endpoint).await, "done");
    assert_eq!(harness.completions(&endpoint).await, 1);
    assert_eq!(status(&harness, &screen).await, "todo");
    let compared = harness
        .call(
            "review",
            "compare_revisions",
            json!({"from": "r1", "to": "r2"}),
        )
        .await;
    assert_eq!(
        changes(&compared),
        [(ENDPOINT.0.to_owned(), "added".to_owned())]
    );

    // 6. The merge path: a second pair of workers changes the same line, the second integration is refused with a conflict report.
    for (agent, name) in [("frontend", "ui"), ("backend", "api")] {
        let registered = harness
            .call(agent, "register_worker", json!({"name": name}))
            .await;
        assert_eq!(registered["worker"], format!("{agent}/{name}"));
    }
    let readme = harness
        .create_task("review", "Name the login endpoint in the readme")
        .await;
    let first = start_work_as(&harness, "frontend", "frontend/ui", &screen).await;
    let second = start_work_as(&harness, "backend", "backend/api", &readme).await;
    first.write(&[("README.md", SCREEN_README)]);
    second.write(&[("README.md", ENDPOINT_README)]);
    let first_proposal = propose_as(
        &harness,
        "frontend",
        "frontend/ui",
        &first,
        "Name the login screen",
    )
    .await;
    let second_proposal = propose_as(
        &harness,
        "backend",
        "backend/api",
        &second,
        "Name the login endpoint",
    )
    .await;
    harness.approve(&first_proposal).await;
    harness.approve(&second_proposal).await;
    let integrated = harness
        .call(
            "review",
            "integrate_proposal",
            json!({"id": first_proposal}),
        )
        .await;
    assert_eq!(integrated["revision"], "r3");
    let error = harness
        .refused(
            "review",
            "integrate_proposal",
            json!({"id": second_proposal}),
        )
        .await;
    assert_eq!(error["code"], "integration_conflicted", "{error}");
    assert_eq!(
        error["details"],
        json!({"proposal": second_proposal, "files": [{"path": "README.md", "kind": "text"}]})
    );
    assert_eq!(harness.head().await.number.to_string(), "r3");
    let view = harness
        .call("backend", "get_proposal", json!({"id": second_proposal}))
        .await;
    assert_eq!(view["state"], "open");
    assert_eq!(view["conflict"]["main_revision"], "r3", "{view}");
    assert_eq!(view["conflict"]["base_revision"], "r2", "{view}");
    assert_eq!(status(&harness, &screen).await, "done");
    assert_eq!(status(&harness, &readme).await, "in_progress");
    assert_eq!(harness.completions(&readme).await, 0);

    // 7. The open stream delivered every event the flow stored, in order.
    let stored = stored_after(&harness, watched_from).await;
    let last = *stored.keys().last().unwrap();
    let mut delivered = BTreeMap::new();
    sse.until("the last stored event", |frame| match &frame.envelope {
        EventEnvelope::Event(event) => {
            delivered.insert(event.seq, event.event_type.as_str().to_owned());
            (event.seq >= last).then_some(())
        }
        _ => None,
    })
    .await;
    assert_eq!(delivered, stored);

    let events = harness.events().await;
    assert_in_order(
        &events,
        &[
            "task.created",
            "dependency.added",
            "checkpoint.recorded",
            "changeset.recorded",
            "message.sent",
            "message.acknowledged",
            "proposal.opened",
            "integration.succeeded",
            "main.advanced",
            "integration.succeeded",
            "integration.conflicted",
        ],
    );
    let remote: Vec<&str> = events
        .iter()
        .map(|event| event.kind.as_str())
        .filter(|kind| REMOTE_EVENTS.contains(kind))
        .collect();
    assert!(remote.is_empty(), "{remote:?}");

    assert_consistent(&harness.project).await;
}
