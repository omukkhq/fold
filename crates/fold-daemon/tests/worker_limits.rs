#![cfg(test)]

use axum::http::{Method, StatusCode};
use fold_core::codes;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, SLUG};
use crate::workers_support::FakeClaude;

const MAIN: &str = "frontend";
const BEFORE: &str = "# the project\n[git]\nbranch = \"trunk\" # kept\n\n[workers]\ntask_coder = 4 # four at once\nbatch_reviewer = 3\n\n[integration]\nautomatic = true\n";
const AFTER: &str = "# the project\n[git]\nbranch = \"trunk\" # kept\n\n[workers]\ntask_coder = 10 # four at once\nbatch_reviewer = 3\n\n[integration]\nautomatic = true\n";

async fn lead(harness: &mut Harness) -> String {
    harness
        .call(MAIN, "create_label", json!({"name": "ui", "color": "sage"}))
        .await;
    harness
        .call(MAIN, "define_area", json!({"name": "web", "label": "ui"}))
        .await;
    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Draw the page", "labels": ["ui"]}),
        )
        .await;
    harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    harness.connect("web-lead").await;
    "web-lead".to_owned()
}

async fn limit_events(harness: &Harness) -> Vec<Value> {
    harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "workers.limits_changed")
        .map(|event| event.data)
        .collect()
}

#[tokio::test]
async fn the_main_agent_raises_the_coder_limit_and_the_next_start_follows_it() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    let config = std::path::Path::new(harness.root()).join(".fold/config.toml");
    std::fs::write(&config, BEFORE).unwrap();
    let lead = lead(&mut harness).await;
    let coder = json!({"role": "task_coder", "tasks": ["T-1"]});
    for _ in 0..4 {
        harness.call(&lead, "start_worker", coder.clone()).await;
    }
    let full = harness.refused(&lead, "start_worker", coder.clone()).await;
    assert_eq!(full["code"], codes::WORKER_LIMIT_REACHED);

    let set = harness
        .call(MAIN, "set_worker_limits", json!({"task_coder": 10}))
        .await;
    assert_eq!(set["limits"]["task_coder"], 10);
    assert_eq!(set["limits"]["batch_reviewer"], 3);
    assert_eq!(set["previous"]["task_coder"], 4);
    assert_eq!(std::fs::read_to_string(&config).unwrap(), AFTER);
    assert!(!config.with_extension("toml.new").exists());
    for _ in 0..6 {
        harness.call(&lead, "start_worker", coder.clone()).await;
    }
    let eleventh = harness.refused(&lead, "start_worker", coder).await;
    assert_eq!(eleventh["code"], codes::WORKER_LIMIT_REACHED);
    assert_eq!(
        eleventh["details"],
        json!({"role": "task_coder", "max": 10})
    );

    let events = limit_events(&harness).await;
    assert_eq!(
        events,
        [json!({
            "old": {"task_coder": 4, "batch_reviewer": 3, "max_per_area": null},
            "new": {"task_coder": 10, "batch_reviewer": 3, "max_per_area": null},
        })]
    );
    let listed = harness.call(MAIN, "list_workers", json!({})).await;
    assert_eq!(listed["limits"]["task_coder"], 10);

    let same = harness
        .call(MAIN, "set_worker_limits", json!({"task_coder": 10}))
        .await;
    assert_eq!(same["limits"], same["previous"]);
    assert_eq!(same["limits"]["task_coder"], 10);
    assert_eq!(std::fs::read_to_string(&config).unwrap(), AFTER);
    assert_eq!(limit_events(&harness).await.len(), 1);
}

#[tokio::test]
async fn a_lead_and_a_value_over_twenty_are_refused_and_a_human_sets_a_limit_over_http() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    let config = std::path::Path::new(harness.root()).join(".fold/config.toml");
    let lead = lead(&mut harness).await;
    let denied = harness
        .refused(&lead, "set_worker_limits", json!({"task_coder": 2}))
        .await;
    assert_eq!(denied["code"], codes::MAIN_AGENT_REQUIRED);
    harness
        .call(MAIN, "register_worker", json!({"name": "helper"}))
        .await;
    let subagent = harness
        .refused(
            MAIN,
            "set_worker_limits",
            json!({"task_coder": 2, "as": "frontend/helper"}),
        )
        .await;
    assert_eq!(subagent["code"], codes::MAIN_AGENT_REQUIRED);
    let over = harness
        .refused(MAIN, "set_worker_limits", json!({"batch_reviewer": 21}))
        .await;
    assert_eq!(over["code"], codes::INVALID_REQUEST);
    let nothing = harness.refused(MAIN, "set_worker_limits", json!({})).await;
    assert_eq!(nothing["code"], codes::INVALID_REQUEST);
    let both = harness
        .refused(
            MAIN,
            "set_worker_limits",
            json!({"max_per_area": 2, "clear_max_per_area": true}),
        )
        .await;
    assert_eq!(both["code"], codes::INVALID_REQUEST);
    assert!(limit_events(&harness).await.is_empty());

    let path = format!("/api/v1/projects/{SLUG}/worker-limits");
    let (status, limits) = harness
        .http(
            Method::PATCH,
            &path,
            Some(&json!({"max_per_area": 2, "batch_reviewer": 5})),
            "tanim",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{limits}");
    assert_eq!(
        limits,
        json!({"task_coder": 4, "batch_reviewer": 5, "max_per_area": 2})
    );
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(text.contains("[workers]"), "{text}");
    let (status, limits) = harness
        .http(
            Method::PATCH,
            &path,
            Some(&json!({"clear_max_per_area": true})),
            "tanim",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{limits}");
    assert_eq!(limits["max_per_area"], Value::Null);
    let (status, refused) = harness
        .http(
            Method::PATCH,
            &path,
            Some(&json!({"task_coder": 0})),
            "tanim",
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    let (status, listed) = harness
        .http(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/workers"),
            None,
            "tanim",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        listed["limits"],
        json!({"task_coder": 4, "batch_reviewer": 5, "max_per_area": null})
    );
    assert_eq!(limit_events(&harness).await.len(), 2);
}

#[tokio::test]
async fn a_limit_change_waits_for_a_config_edit_in_progress_and_keeps_it() {
    let fake = FakeClaude::new("result");
    let harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    let config = std::path::Path::new(harness.root()).join(".fold/config.toml");
    std::fs::write(&config, BEFORE).unwrap();

    let held = fold_vcs::atomic::EditLock::acquire(&config).unwrap();
    let edited = BEFORE.replace(
        "[integration]",
        "[remotes.origin]\nurl = \"/srv/a.git\"\nbranch = \"main\"\n\n[integration]",
    );
    let (set, ()) = tokio::join!(
        harness.call(MAIN, "set_worker_limits", json!({"task_coder": 10})),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            assert_eq!(std::fs::read_to_string(&config).unwrap(), BEFORE);
            fold_vcs::atomic::write_atomic(&config, edited.as_bytes()).unwrap();
            drop(held);
        }
    );
    assert_eq!(set["limits"]["task_coder"], 10);
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        edited.replace("task_coder = 4", "task_coder = 10")
    );
}
