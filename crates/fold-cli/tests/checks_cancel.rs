#![cfg(test)]

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use fold_store::testing::eventually;
use rmcp::model::CallToolRequestParams;
use serde_json::{Value, json};

use crate::common::{Client, DaemonGuard, Host, adapter, daemons};

/// Much longer than a cancel takes, and much shorter than the check, so only a cancel ends a run in it.
const WAIT: Duration = Duration::from_secs(60);

/// A check that runs far longer than the test.
const SLOW_CHECKS: &str = "[[check]]\nname = \"slow\"\nrun = \"sleep 600\"\n";

/// `tool` with `arguments` through the main agent's adapter, as its parsed JSON result.
async fn call(client: &Client, tool: &str, arguments: Value) -> Value {
    let Value::Object(arguments) = arguments else {
        panic!("tool arguments are an object");
    };
    let result = client
        .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments))
        .await
        .unwrap();
    assert_ne!(result.is_error, Some(true), "{tool}: {result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

/// `fld <args>` as the human ana in `root`.
async fn fld(host: &Host, root: &Path, args: &[&str]) -> Output {
    host.fold(args, root)
        .env("FOLD_HUMAN", "ana")
        .output()
        .await
        .unwrap()
}

/// A task of the main agent with a workspace whose checks run [`SLOW_CHECKS`]; returns the task id.
async fn task_with_slow_checks(client: &Client, root: &Path, title: &str) -> String {
    let created = call(client, "create_task", json!({"title": title})).await;
    let task = created["id"].as_str().unwrap().to_owned();
    call(client, "claim_task", json!({"id": task})).await;
    let workspace = call(client, "create_workspace", json!({"task": task})).await;
    let dir: PathBuf = root.join(workspace["path"].as_str().unwrap());
    std::fs::write(dir.join("fold-checks.toml"), SLOW_CHECKS).unwrap();
    task
}

/// Starts a run of `task`'s checks and returns its id.
async fn start_run(client: &Client, task: &str) -> String {
    let started = call(client, "run_checks", json!({"task": task})).await;
    assert_eq!(started["checks"], "running", "{started}");
    started["run"].as_str().unwrap().to_owned()
}

/// Waits until `run` has ended, then checks that a cancel ended it.
async fn assert_cancelled(client: &Client, run: &str) {
    eventually(WAIT, async || {
        let read = call(client, "get_check_run", json!({"run": run})).await;
        if read["state"] == "running" {
            Err(read)
        } else {
            Ok(())
        }
    })
    .await;
    let read = call(client, "get_check_run", json!({"run": run})).await;
    assert_eq!(read["state"], "failed", "{read}");
    assert_eq!(read["reason"], "cancelled", "{read}");
}

/// `fld checks cancel` with a run id, `--task` and `--all` each ends the running runs it names and prints their ids; an unknown run id prints the error code and exits non-zero.
#[tokio::test(flavor = "multi_thread")]
async fn fld_checks_cancel_ends_the_runs_it_names_and_prints_their_ids() {
    let host = Host::new().await;
    let root = host.dir.path().join("site");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("README.md"), "# site\n").unwrap();
    let init = fld(&host, &root, &["init"]).await;
    assert!(init.status.success(), "{init:?}");
    let mut command = host.fold(&["mcp"], &root);
    command
        .env("FOLD_AGENT", "main")
        .env("FOLD_AGENT_TAGS", "main");
    let (client, mut child) = adapter(command).await;
    let guard = DaemonGuard(daemons(&host.default_socket()));

    let first = task_with_slow_checks(&client, &root, "First").await;
    let second = task_with_slow_checks(&client, &root, "Second").await;

    let run = start_run(&client, &first).await;
    let output = fld(&host, &root, &["checks", "cancel", &run]).await;
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{run} cancelled\n")
    );
    assert_cancelled(&client, &run).await;

    let own = start_run(&client, &first).await;
    let other = start_run(&client, &second).await;
    let output = fld(&host, &root, &["checks", "cancel", "--task", &first]).await;
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{own} cancelled\n")
    );
    assert_cancelled(&client, &own).await;
    let still = call(&client, "get_check_run", json!({"run": other})).await;
    assert_eq!(still["state"], "running", "--task leaves other tasks' runs");

    let again = start_run(&client, &first).await;
    let output = fld(&host, &root, &["checks", "cancel", "--all"]).await;
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{other} cancelled\n{again} cancelled\n")
    );
    assert_cancelled(&client, &other).await;
    assert_cancelled(&client, &again).await;

    let output = fld(&host, &root, &["checks", "cancel", "--all"]).await;
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "no check run was running\n"
    );

    let output = fld(&host, &root, &["checks", "cancel", "cr-999"]).await;
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("check_run_not_found: cr-999 does not exist (call get_proposal"),
        "{stderr}"
    );

    client.cancel().await.unwrap();
    child.wait().await.unwrap();
    guard.stop().await;
}
