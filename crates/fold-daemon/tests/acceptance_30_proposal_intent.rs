#![cfg(test)]

use std::path::Path;
use std::process::Command;

use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, WAIT};

const FIRST: &str =
    "The login route checks the password against the stored hash and opens a session.";
const SECOND: &str = "The login route checks the password against the stored hash and opens a session.\n\nA GET of the route is refused with 405.";

/// The intent of v1: one part for the route.
fn first() -> Value {
    json!({
        "summary": FIRST,
        "parts": [{"name": "Login route", "does": "POST /login checks the password and opens a session.", "files": ["src/login.txt"]}],
        "contracts": {"routes": ["POST /login"]},
    })
}

/// The intent of v2: the route refuses GET and a second part holds its test.
fn second() -> Value {
    json!({
        "summary": SECOND,
        "parts": [
            {"name": "Login route", "does": "POST /login checks the password and opens a session; GET answers 405.", "files": ["src/login.txt"]},
            {"name": "Tests", "does": "A GET of /login gets 405.", "files": ["tests/login.txt"]},
        ],
        "contracts": {"routes": ["POST /login", "GET /login answers 405"]},
        "not_changed": ["Sign-up."],
    })
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?} failed");
    String::from_utf8(output.stdout).unwrap()
}

/// How many revisions after `r1` have their Git commit.
async fn committed(h: &Harness) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM revision_commits c JOIN revisions r ON r.id = c.revision_id WHERE r.number > 1",
    )
    .fetch_one(h.handle.store.read())
    .await
    .unwrap()
}

/// Acceptance 30: backend sends its login endpoint with an intent, which fold refuses empty and refuses again while a changed file belongs to no part. The reviewer reads the intent part by part, sees that the diff does less than it says and asks for changes; backend sends v2 with its own intent, a second part for its test, and v1 keeps its own. The reviewer approves and integrates v2, and the Git commit of the integration has the title as its subject and v2's summary and parts as its body. No event carries the intent.
#[tokio::test]
async fn acceptance_30_each_version_states_its_intent_and_the_commit_body_carries_it() {
    let h = Harness::start().await;
    let task = h.create_task("backend", "Add the login endpoint").await;
    let work = h.start_work("backend", &task).await;
    work.write(&[("src/login.txt", "POST /login\n")]);
    h.call(
        "backend",
        "record_changes",
        json!({"ws": work.workspace, "message": "login"}),
    )
    .await;
    let refused = h
        .refused(
            "backend",
            "create_proposal",
            json!({"task": task, "title": "Add the login endpoint", "intent": {"summary": " ", "parts": first()["parts"]}}),
        )
        .await;
    assert_eq!(refused["code"], "invalid_intent");
    assert_eq!(refused["details"]["field"], "intent.summary");
    let uncovered = h
        .refused(
            "backend",
            "create_proposal",
            json!({"task": task, "title": "Add the login endpoint", "intent": {"summary": FIRST, "parts": [{"name": "Login route", "does": "Adds the route.", "files": []}]}}),
        )
        .await;
    assert_eq!(uncovered["details"]["reason"], "part_files_uncovered");
    assert_eq!(uncovered["details"]["uncovered"], json!(["src/login.txt"]));
    let created = h
        .call(
            "backend",
            "create_proposal",
            json!({"task": task, "title": "Add the login endpoint", "intent": first(), "evidence": "check-login: 1 passed"}),
        )
        .await;
    let proposal = created["id"].as_str().unwrap().to_owned();
    h.give_review("review", &proposal).await;
    let read = h
        .call(
            "review",
            "get_proposal",
            json!({"id": proposal, "detail": "full"}),
        )
        .await;
    assert_eq!(read["intent"], first());
    h.call(
        "review",
        "submit_review",
        json!({"id": proposal, "version": 1, "verdict": "request_changes", "body": "The task asks for GET to be refused; the diff does not do it."}),
    )
    .await;

    work.write(&[
        ("src/login.txt", "POST /login\nGET /login -> 405\n"),
        ("tests/login.txt", "GET /login is 405\n"),
    ]);
    h.call(
        "backend",
        "record_changes",
        json!({"ws": work.workspace, "message": "refuse GET"}),
    )
    .await;
    let updated = h
        .call(
            "backend",
            "update_proposal",
            json!({"id": proposal, "intent": second()}),
        )
        .await;
    assert_eq!(updated["version"], "v2", "{updated}");
    let current = h
        .call(
            "review",
            "get_proposal",
            json!({"id": proposal, "detail": "full"}),
        )
        .await;
    assert_eq!(current["intent"], second());
    let short = h
        .call("review", "get_proposal", json!({"id": proposal}))
        .await;
    assert_eq!(
        short["intent"],
        json!({"summary": SECOND, "parts": [{"name": "Login route", "files": 1}, {"name": "Tests", "files": 1}]})
    );
    let earlier = h
        .call(
            "review",
            "get_proposal",
            json!({"id": proposal, "version": 1, "detail": "full"}),
        )
        .await;
    assert_eq!(earlier["intent"], first(), "v1 keeps its own intent");

    h.approve(&proposal).await;
    let integrated = h
        .call(
            "review",
            "integrate_proposal",
            json!({"id": proposal, "expected_version": 2}),
        )
        .await;
    assert_eq!(integrated["result"], "succeeded", "{integrated}");
    eventually(WAIT, async || match committed(&h).await {
        1 => Ok(()),
        count => Err(count),
    })
    .await;
    let git_dir = h.project.paths().git_dir();
    let body = git(&git_dir, &["log", "-1", "--format=%B", "refs/fold/head"]);
    assert!(
        body.starts_with(&format!(
            "Add the login endpoint\n\n{SECOND}\n\nLogin route: POST /login checks the password and opens a session; GET answers 405.\nTests: A GET of /login gets 405.\n\nFold-Proposal"
        )),
        "{body}"
    );
    assert!(
        body.contains(&format!(
            "Fold-Proposal: {proposal}\nFold-Version: 2\nFold-Task: {task}\n"
        )),
        "{body}"
    );

    for event in h.events().await {
        let data = event.data.to_string();
        assert!(
            !data.contains("stored hash") && !data.contains("Sign-up"),
            "{} carries the intent",
            event.kind
        );
    }
}
