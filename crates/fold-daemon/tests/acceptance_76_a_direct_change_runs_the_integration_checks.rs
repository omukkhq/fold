#![cfg(test)]

use std::path::{Path, PathBuf};

use fold_store::testing::eventually;
use rmcp::model::{CallToolRequest, CallToolRequestParams, ClientRequest};
use rmcp::service::PeerRequestOptions;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, WAIT};

const MAIN: &str = "main";

/// The gate files of main's integration check `e2e`: it fails while `fail` exists and waits while `hold` exists.
struct Gate {
    _dir: tempfile::TempDir,
    fail: PathBuf,
    hold: PathBuf,
}

/// Puts the check `e2e` on main through a direct change, whose own run passes because neither gate file exists yet.
async fn gated(h: &Harness) -> Gate {
    let dir = tempfile::tempdir().unwrap();
    let fail = dir.path().join("fail");
    let hold = dir.path().join("hold");
    let checks = format!(
        "[[check]]\nname = \"e2e\"\nrun = \"while [ -e '{}' ]; do sleep 0.05; done; if [ -e '{}' ]; then echo broken; exit 1; fi\"\nstage = \"integration\"\n",
        hold.display(),
        fail.display()
    );
    let submitted = direct_change(h, &[("fold-checks.toml", &checks)], "Add the e2e check").await;
    assert_eq!(submitted["result"], "succeeded", "{submitted}");
    Gate {
        _dir: dir,
        fail,
        hold,
    }
}

/// The main agent writes `files` in its main workspace, records them and gives the changeset.
async fn recorded(h: &Harness, files: &[(&str, &str)], message: &str) -> Value {
    let opened = h.call(MAIN, "open_main_workspace", json!({})).await;
    let dir = Path::new(h.root()).join(opened["path"].as_str().unwrap());
    for (path, text) in files {
        let at = dir.join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, text).unwrap();
    }
    h.call(
        MAIN,
        "record_changes",
        json!({"ws": opened["workspace"], "message": message}),
    )
    .await
}

async fn submit(h: &Harness, changeset: &Value, message: &str) -> Value {
    h.call(
        MAIN,
        "submit_main_change",
        json!({"changeset": changeset, "message": message}),
    )
    .await
}

async fn direct_change(h: &Harness, files: &[(&str, &str)], message: &str) -> Value {
    let changes = recorded(h, files, message).await;
    submit(h, &changes["changeset"], message).await
}

/// Waits until a `checks.started` of a direct change's run names `changeset`.
async fn direct_run_started(h: &Harness, changeset: &str) {
    eventually(WAIT, async || {
        let started = h.events().await.into_iter().any(|event| {
            event.kind == "checks.started"
                && event.data["purpose"] == "integrate_direct"
                && event.data["changeset"] == changeset
        });
        if started {
            Ok(())
        } else {
            Err("the direct change's run has not started")
        }
    })
    .await;
}

/// Submits `changeset` with a cancellable call and cancels it once the direct change's run started, so its turn is held.
async fn cancelled_submit(h: &Harness, changeset: &str) {
    let Value::Object(arguments) = json!({"changeset": changeset, "message": "Add the notes"})
    else {
        unreachable!()
    };
    let request = ClientRequest::CallToolRequest(CallToolRequest::new(
        CallToolRequestParams::new("submit_main_change").with_arguments(arguments),
    ));
    let call = h
        .client(MAIN)
        .peer()
        .send_cancellable_request(request, PeerRequestOptions::no_options())
        .await
        .unwrap();
    direct_run_started(h, changeset).await;
    call.cancel(Some("the main agent stopped waiting".into()))
        .await
        .unwrap();
}

/// Acceptance 76: a direct change runs main's integration checks on its merge before it integrates. A failed check integrates nothing: `submit_main_change` answers `checks_failed` with the run, `get_check_run` reads it with the changeset, fold's message to the main agent names the changeset, and the main workspace stays open. The fixed change, recorded again, integrates once its checks pass.
#[tokio::test]
async fn acceptance_76_a_failed_check_keeps_the_direct_change_off_main_until_it_is_fixed() {
    let mut h = Harness::start().await;
    h.connect_tagged(MAIN, &["main"]).await;
    let gate = gated(&h).await;
    let before = h.head().await.number;

    std::fs::write(&gate.fail, "").unwrap();
    let changes = recorded(&h, &[("docs/notes.md", "notes\n")], "Add the notes").await;
    let changeset = changes["changeset"].as_str().unwrap().to_owned();
    let failed = submit(&h, &changes["changeset"], "Add the notes").await;
    assert_eq!(failed["result"], "checks_failed", "{failed}");
    assert_eq!(failed["changeset"], changeset.as_str());
    assert_eq!(h.head().await.number, before, "main did not move");

    let run = h
        .call(MAIN, "get_check_run", json!({"run": failed["run"]}))
        .await;
    assert_eq!(run["state"], "failed", "{run}");
    assert_eq!(run["stage"], "integration", "{run}");
    assert_eq!(run["purpose"], "integrate_direct", "{run}");
    assert_eq!(run["changeset"], changeset.as_str(), "{run}");
    assert_eq!(run["failed"], json!(["e2e"]), "{run}");
    assert_eq!(run["results"][0]["log_tail"], "broken", "{run}");
    let run_id = failed["run"].as_str().unwrap();
    let told = h.fold_messages(MAIN).await;
    assert!(
        told.iter().any(|body| body.contains(&changeset)
            && body.contains(run_id)
            && body.contains("e2e failed (exit 1)")),
        "{told:?}"
    );

    let open = h.call(MAIN, "open_main_workspace", json!({})).await;
    assert_eq!(
        open["reused"], true,
        "the main workspace stays open: {open}"
    );
    std::fs::remove_file(&gate.fail).unwrap();
    let fixed = recorded(&h, &[("docs/notes.md", "fixed notes\n")], "Add the notes").await;
    let passed = submit(&h, &fixed["changeset"], "Add the notes").await;
    assert_eq!(passed["result"], "succeeded", "{passed}");
    assert_ne!(h.head().await.number, before);
}

/// Acceptance 76: while a direct change runs its integration checks it holds the integration mutex, so a proposal queued meanwhile starts nothing and integrates only after the direct change.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_76_no_integration_starts_while_a_direct_change_runs_its_checks() {
    let mut h = Harness::start().await;
    h.connect_tagged(MAIN, &["main"]).await;
    let gate = gated(&h).await;
    let task = h.create_task("backend", "Add the login endpoint").await;
    let work = h.start_work("backend", &task).await;
    work.write(&[("src/login.txt", "GET /login\n")]);
    let proposal = h.propose("backend", &work, "Add the login endpoint").await;
    h.give_review("review", &proposal).await;
    h.approve(&proposal).await;

    std::fs::write(&gate.hold, "").unwrap();
    let changes = recorded(&h, &[("docs/notes.md", "notes\n")], "Add the notes").await;
    let changeset = changes["changeset"].as_str().unwrap().to_owned();
    let before = h.head().await.number;
    let submitting = submit(&h, &changes["changeset"], "Add the notes");
    let meanwhile = async {
        direct_run_started(&h, &changeset).await;
        let queued = h
            .call(
                "review",
                "integrate_proposal",
                json!({"id": proposal, "expected_version": 1}),
            )
            .await;
        assert_eq!(queued["result"], "queued", "{queued}");
        let events = h.events().await;
        assert!(
            !events.iter().any(|event| event.kind == "checks.started"
                && event.proposal.as_deref() == Some(proposal.as_str())),
            "the proposal's run started while the direct change held the mutex"
        );
        assert_eq!(h.head().await.number, before);
        std::fs::remove_file(&gate.hold).unwrap();
    };
    let (submitted, ()) = tokio::join!(submitting, meanwhile);
    assert_eq!(submitted["result"], "succeeded", "{submitted}");
    let order = eventually(WAIT, async || {
        let order: Vec<String> = h
            .events()
            .await
            .into_iter()
            .filter(|event| event.kind == "integration.succeeded")
            .map(|event| {
                event
                    .proposal
                    .unwrap_or_else(|| event.data["changeset"].as_str().unwrap().to_owned())
            })
            .collect();
        if order.len() == 3 {
            Ok(order)
        } else {
            Err(order)
        }
    })
    .await;
    assert_eq!(&order[1..], [changeset.as_str(), proposal.as_str()]);
}

/// Acceptance 76: a `submit_main_change` call that the main agent cancels while its checks run does not stop the integration: the run ends, main moves, and fold's message tells the main agent the result.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_76_a_cancelled_call_still_integrates_and_tells_the_main_agent() {
    let mut h = Harness::start().await;
    h.connect_tagged(MAIN, &["main"]).await;
    let gate = gated(&h).await;
    std::fs::write(&gate.hold, "").unwrap();
    let changes = recorded(&h, &[("docs/notes.md", "notes\n")], "Add the notes").await;
    let changeset = changes["changeset"].as_str().unwrap().to_owned();
    cancelled_submit(&h, &changeset).await;
    std::fs::remove_file(&gate.hold).unwrap();
    let revision = eventually(WAIT, async || {
        let succeeded = h.events().await.into_iter().find(|event| {
            event.kind == "integration.succeeded" && event.data["changeset"] == changeset.as_str()
        });
        succeeded.ok_or("the direct change has not integrated")
    })
    .await;
    assert_eq!(revision.data["kind"], "direct");
    let wanted = format!("The direct change {changeset} integrated as");
    eventually(WAIT, async || {
        let told = h.fold_messages(MAIN).await;
        if told.iter().any(|body| body.starts_with(&wanted)) {
            Ok(())
        } else {
            Err(told)
        }
    })
    .await;
    let runs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM check_runs WHERE state = 'running'")
        .fetch_one(h.handle.store.read())
        .await
        .unwrap();
    assert_eq!(runs, 0, "the direct change's run ended");
}
