#![cfg(test)]

use std::path::Path;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use fold_core::WorkerState;
use fold_core::roles::Role;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::auto_integrate::Running;
use crate::common::acceptance::{HUMAN, Harness, SLUG, WAIT};
use crate::common::roles::give_role;
use crate::worker_resume::{MAIN, REVIEWER, TASK_WORKER, until_runs, until_state};
use crate::workers_support::FakeClaude;

pub(crate) const LEAD: &str = "review-lead";
const AREA_LEAD: &str = "web-lead";
pub(crate) const SECOND_REVIEWER: &str = "reviewer-2";
const THIRD_REVIEWER: &str = "reviewer-3";

/// main's checks: one lane for the migrations, and an integration check that refuses two migrations on the same parent, as a second Alembic head would.
const CHECKS: &str = r#"
[[serial]]
name  = "migrations"
paths = ["alembic/versions/*.py"]

[[check]]
name  = "one-head"
run   = "test -z \"$(cat alembic/versions/*.py | grep '^down' | sort | uniq -d)\""
stage = "integration"
"#;

pub(crate) fn migration(revision: &str, down: &str) -> String {
    format!("revision = \"{revision}\"\ndown = \"{down}\"\n")
}

pub(crate) async fn status(harness: &Harness, task: &str) -> Value {
    harness.call(MAIN, "get_task", json!({"id": task})).await["status"].clone()
}

/// Area `web` with its lead, a task for each of `titles` and the task worker that proposes `P-<n>` for `T-<n>` with the files of `changes[n - 1]`, on a main whose `r1` holds [`CHECKS`] and the first migration. The review lead runs and is connected.
async fn proposed(fake: &FakeClaude, titles: &[&str], changes: &[Vec<(&str, String)>]) -> Harness {
    proposed_with(fake, CHECKS, titles, changes).await
}

/// [`proposed`] on a main whose `fold-checks.toml` is `checks`.
pub(crate) async fn proposed_with(
    fake: &FakeClaude,
    checks: &str,
    titles: &[&str],
    changes: &[Vec<(&str, String)>],
) -> Harness {
    let mut harness = Harness::start_with_files(
        fake.launcher.clone(),
        &[
            ("fold-checks.toml", checks),
            ("alembic/versions/0001.py", &migration("0001", "base")),
        ],
    )
    .await;
    harness.make_main(MAIN).await;
    give_role(&harness.handle.store, "review", Role::Reviewer, None).await;
    harness
        .call(MAIN, "create_label", json!({"name": "ui", "color": "sage"}))
        .await;
    harness
        .call(MAIN, "define_area", json!({"name": "web", "label": "ui"}))
        .await;
    let mut tasks = Vec::new();
    for title in titles {
        let created = harness
            .call(
                MAIN,
                "create_task",
                json!({"title": title, "labels": ["ui"]}),
            )
            .await;
        tasks.push(created["id"].clone());
    }
    harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    harness.connect(AREA_LEAD).await;
    harness
        .call(
            AREA_LEAD,
            "start_worker",
            json!({"role": "task_worker", "tasks": tasks}),
        )
        .await;
    until_state(&harness, TASK_WORKER, WorkerState::Idle).await;
    harness.connect(TASK_WORKER).await;
    for (index, (title, files)) in titles.iter().zip(changes).enumerate() {
        let work = harness
            .start_work(TASK_WORKER, &format!("T-{}", index + 1))
            .await;
        let files: Vec<(&str, &str)> = files
            .iter()
            .map(|(path, text)| (*path, text.as_str()))
            .collect();
        work.write(&files);
        assert_eq!(
            harness.propose(TASK_WORKER, &work, title).await,
            format!("P-{}", index + 1)
        );
    }
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    harness
        .call(MAIN, "start_worker", json!({"role": "review_lead"}))
        .await;
    until_state(&harness, LEAD, WorkerState::Idle).await;
    harness.connect(LEAD).await;
    harness
}

/// The review lead starts `reviewer` for `proposal`, which approves `v1`; fold then stops it with the review claim. The clock moves on a second after, so each approval has its own time.
pub(crate) async fn approve(harness: &mut Harness, proposal: &str, reviewer: &str) {
    let started = harness
        .call(
            LEAD,
            "start_worker",
            json!({"role": "reviewer", "proposals": [proposal]}),
        )
        .await;
    assert_eq!(started["worker"]["name"], reviewer, "{started}");
    until_state(harness, reviewer, WorkerState::Idle).await;
    harness.connect(reviewer).await;
    harness
        .call(
            reviewer,
            "submit_review",
            json!({"id": proposal, "version": 1, "verdict": "approve", "body": "Read and run."}),
        )
        .await;
    until_state(harness, reviewer, WorkerState::Stopped).await;
    harness
        .project
        .clock()
        .advance(Duration::from_secs(1))
        .unwrap();
}

/// The `lane` of `proposal` as `list_proposals` shows it.
async fn listed_lane(harness: &Harness, proposal: &str) -> Value {
    let listed = harness
        .call(LEAD, "list_proposals", json!({"state": "open"}))
        .await;
    listed["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == proposal)
        .map_or(Value::Null, |row| row["lane"].clone())
}

/// The proposal each revision after `r1` integrated, in order.
pub(crate) async fn integrated_in_order(harness: &Harness, count: u32) -> Vec<String> {
    eventually(WAIT, async || {
        let head = harness.head().await.number.number();
        if head > count { Ok(()) } else { Err(head) }
    })
    .await;
    let mut order = Vec::new();
    for number in 2..=count + 1 {
        let revision = harness
            .call(LEAD, "get_revision", json!({"r": format!("r{number}")}))
            .await;
        order.push(revision["proposal"].as_str().unwrap_or_default().to_owned());
    }
    order
}

/// Whether an agent of `agents` sent any message.
async fn messages_by(harness: &Harness, agents: &[&str]) -> Vec<Value> {
    harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "message.sent")
        .filter(|event| {
            let agent = event.actor.split('/').next().unwrap_or_default();
            agents.contains(&agent)
        })
        .map(|event| event.data)
        .collect()
}

/// Acceptance 50: three approved proposals each add a migration, so all three are members of the lane `migrations`. They integrate one at a time in the order of their first approval (`P-2`, `P-1`, `P-3`), not in number order. `P-1`'s integration check fails on the main that `P-2` made, so fold gives it to the reviewer that approved it, with the note that the lane moved; `P-3` waits behind it as position 2. The reviewer fixes the migration's parent in its checkout merged onto main and approves the fix, and fold integrates `P-1` and then `P-3`. No lead sends a message, and the author is never told or resumed.
#[tokio::test]
async fn acceptance_50_lane_members_integrate_in_approval_order_and_a_lane_failure_goes_to_its_reviewer()
 {
    let fake = FakeClaude::new("result");
    let mut harness = proposed(
        &fake,
        &[
            "Add the users table",
            "Add the posts table",
            "Add the tags table",
        ],
        &[
            vec![(
                "alembic/versions/0002_users.py",
                migration("0002_users", "0001"),
            )],
            vec![(
                "alembic/versions/0002_posts.py",
                migration("0002_posts", "0001"),
            )],
            vec![(
                "alembic/versions/0003_tags.py",
                migration("0003_tags", "0002_users"),
            )],
        ],
    )
    .await;
    assert_eq!(listed_lane(&harness, "P-1").await["position"], Value::Null);
    approve(&mut harness, "P-2", REVIEWER).await;
    approve(&mut harness, "P-1", SECOND_REVIEWER).await;
    approve(&mut harness, "P-3", THIRD_REVIEWER).await;
    let author_runs = fake.runs_of(TASK_WORKER).len();

    assert_eq!(
        listed_lane(&harness, "P-2").await,
        json!({"name": "migrations", "position": 1, "held": null})
    );
    assert_eq!(
        listed_lane(&harness, "P-1").await,
        json!({"name": "migrations", "position": 2, "held": "lane migrations, position 2"})
    );
    assert_eq!(listed_lane(&harness, "P-3").await["position"], 3);
    let refused = harness
        .refused(LEAD, "integrate_proposal", json!({"id": "P-3"}))
        .await;
    assert_eq!(refused["code"], "invalid_request", "{refused}");
    assert_eq!(refused["details"]["reason"], "lane_held", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|text| text.contains("lane migrations, position 3")),
        "{refused}"
    );

    let integrate = Running::start(&harness.state);
    let runs = until_runs(&fake, SECOND_REVIEWER, 2).await;
    let turn = &runs[1].stdin[1];
    for wanted in [
        "fold resumed you because P-1 v1 for T-1, which you approved, failed its integration checks",
        "on its merge with main r2",
        "Lane migrations moved",
        "You fix the failure, not the author",
        "create_review_checkout",
        "submit_review_fix",
    ] {
        assert!(turn.contains(wanted), "{wanted}: {turn}");
    }
    assert_eq!(status(&harness, "T-1").await, "review");
    let flagged = harness
        .call(SECOND_REVIEWER, "get_proposal", json!({"id": "P-1"}))
        .await;
    assert_eq!(
        flagged["lane"],
        json!({"name": "migrations", "position": 1, "held": null}),
        "{flagged}"
    );
    let (code, page) = harness
        .http(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/proposals/P-1"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{page}");
    assert_eq!(page["summary"]["conflicted"], true, "{page}");
    assert_eq!(
        page["lane"],
        json!({"name": "migrations", "position": 1, "held": null}),
        "{page}"
    );
    let failed = page["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["stage"] == "integration" && run["state"] == "failed")
        .map(|run| run["id"].clone())
        .unwrap_or_default();
    assert!(
        failed.as_str().is_some_and(|id| id.starts_with("cr-")),
        "{page}"
    );
    let reason = json!({"kind": "lane_checks_failed", "lane": "migrations", "run": failed, "fixer": SECOND_REVIEWER});
    assert_eq!(page["conflicted_reason"], reason, "{page}");
    assert_eq!(page["conflict"], Value::Null, "{page}");
    let (code, list) = harness
        .http(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/proposals?state=open"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{list}");
    let rows = list["proposals"].as_array().unwrap();
    let row = |id: &str| {
        rows.iter()
            .find(|row| row["id"] == id)
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(row("P-1")["conflicted_reason"], reason, "{list}");
    assert_eq!(row("P-1")["lane"]["position"], 1, "{list}");
    assert_eq!(
        row("P-3")["lane"],
        json!({"name": "migrations", "position": 2, "held": "lane migrations, position 2"}),
        "{list}"
    );
    assert_eq!(row("P-3")["conflicted_reason"], Value::Null, "{list}");
    let view = harness
        .call(LEAD, "get_proposal", json!({"id": "P-3"}))
        .await;
    assert_eq!(
        view["lane"],
        json!({"name": "migrations", "position": 2, "held": "lane migrations, position 2"}),
        "{view}"
    );
    let workers = harness
        .call(MAIN, "list_workers", json!({"include_stopped": true}))
        .await;
    let fixer = workers["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["name"] == SECOND_REVIEWER)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        fixer["lane_fix"],
        json!([{"proposal": "P-1", "name": "migrations", "position": 1, "held": null}]),
        "{fixer}"
    );
    until_state(&harness, SECOND_REVIEWER, WorkerState::Idle).await;

    let checkout = harness
        .call(
            SECOND_REVIEWER,
            "create_review_checkout",
            json!({"id": "P-1"}),
        )
        .await;
    let dir = Path::new(harness.root()).join(checkout["path"].as_str().unwrap());
    assert!(
        dir.join("alembic/versions/0002_posts.py").exists(),
        "the checkout holds the main that P-2 made"
    );
    std::fs::write(
        dir.join("alembic/versions/0002_users.py"),
        migration("0002_users", "0002_posts"),
    )
    .unwrap();
    let fixed = harness
        .call(
            SECOND_REVIEWER,
            "submit_review_fix",
            json!({
                "intent": "Make the change the task asks for.",
                "id": "P-1",
                "version": 1,
                "fixes": [{"path": "alembic/versions/0002_users.py", "reason": "put the migration on the head that P-2 made"}],
                "checks": "read the migration chain",
            }),
        )
        .await;
    assert_eq!(fixed["version"], "v2", "{fixed}");
    harness
        .call(
            SECOND_REVIEWER,
            "submit_review",
            json!({"id": "P-1", "version": 2, "verdict": "approve", "body": "One head again."}),
        )
        .await;

    assert_eq!(
        integrated_in_order(&harness, 3).await,
        ["P-2", "P-1", "P-3"]
    );
    integrate.stop().await;
    for task in ["T-1", "T-2", "T-3"] {
        assert_eq!(status(&harness, task).await, "done", "{task}");
    }
    assert_eq!(
        messages_by(&harness, &[LEAD, AREA_LEAD, MAIN]).await,
        Vec::<Value>::new()
    );
    assert_eq!(fake.runs_of(TASK_WORKER).len(), author_runs);
    assert!(
        harness
            .events()
            .await
            .iter()
            .all(|event| event.kind != "worker.resumed" || event.data["agent"] != TASK_WORKER)
    );
}

/// Acceptance 50: a proposal outside every lane integrates as today while the lane waits: the front member is approved but nobody integrates it, a human cannot integrate the unapproved member behind it, that member is held once approved, and the proposal that changes only `README.md` has no lane and integrates at once.
#[tokio::test]
async fn acceptance_50_a_proposal_outside_every_lane_integrates_as_today() {
    let fake = FakeClaude::new("result");
    let mut harness = proposed(
        &fake,
        &[
            "Add the users table",
            "Add the posts table",
            "Name the demo",
        ],
        &[
            vec![(
                "alembic/versions/0002_users.py",
                migration("0002_users", "0001"),
            )],
            vec![(
                "alembic/versions/0003_posts.py",
                migration("0003_posts", "0002_users"),
            )],
            vec![("README.md", "# demo: the blog\n".to_owned())],
        ],
    )
    .await;
    approve(&mut harness, "P-1", REVIEWER).await;
    // A human approves by integrating, but a member without an approval still waits behind the approved front.
    fold_daemon::services::projects::ensure_human(
        harness.project.registry(),
        harness.project.clock(),
        &fold_core::HumanName::parse(HUMAN).unwrap(),
    )
    .await
    .unwrap();
    let (code, refused) = harness
        .http(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/proposals/P-2/integrate"),
            Some(&json!({"expected_version": 1})),
            HUMAN,
        )
        .await;
    assert_ne!(code, StatusCode::OK, "{refused}");
    assert!(refused.to_string().contains("lane_held"), "{refused}");
    approve(&mut harness, "P-2", SECOND_REVIEWER).await;
    approve(&mut harness, "P-3", THIRD_REVIEWER).await;
    assert_eq!(listed_lane(&harness, "P-3").await, Value::Null);
    assert_eq!(
        listed_lane(&harness, "P-2").await["held"],
        "lane migrations, position 2"
    );

    let view = harness
        .call(LEAD, "get_proposal", json!({"id": "P-3"}))
        .await;
    assert!(view.get("lane").is_none(), "{view}");
    let integrated = harness
        .call(LEAD, "integrate_proposal", json!({"id": "P-3"}))
        .await;
    assert_eq!(integrated["result"], "queued", "{integrated}");
    assert_eq!(integrated_in_order(&harness, 1).await, ["P-3"]);
    eventually(WAIT, async || {
        let now = status(&harness, "T-3").await;
        if now == "done" { Ok(()) } else { Err(now) }
    })
    .await;
    let refused = harness
        .refused(LEAD, "integrate_proposal", json!({"id": "P-2"}))
        .await;
    assert_eq!(refused["details"]["reason"], "lane_held", "{refused}");
    assert_eq!(status(&harness, "T-2").await, "review");
}
