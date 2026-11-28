#![cfg(test)]

use std::path::Path;

use fold_core::WorkerState;
use serde_json::{Value, json};

use crate::worker_resume::{
    MAIN, REVIEWER, TASK_WORKER, proposed_by_a_task_worker, request_changes, until_runs,
    until_state,
};
use crate::workers_support::{FakeClaude, Run};

/// The `permissions.deny` rules of the settings file a run was given, as they are now; fold writes the file again at each start and resume of the agent.
fn denied(run: &Run) -> Value {
    let text = std::fs::read_to_string(run.flag("--settings").unwrap()).unwrap();
    let settings: Value = serde_json::from_str(&text).unwrap();
    settings["permissions"]["deny"].clone()
}

fn project_default(root: &str) {
    std::fs::write(
        Path::new(root).join(".fold/config.toml"),
        "deny_commands = [\"Bash(kubectl:*)\", \"Bash(psql:*)\"]\n",
    )
    .unwrap();
}

/// Acceptance 72: an area's `deny_commands` reach every worker of the area as `permissions.deny` after the project's list: its task worker on resume after the update, and the reviewer of its proposal. The lead of another area and the review lead get only the project's list, and the fold tools stay allowed.
#[tokio::test]
async fn acceptance_72_the_workers_and_reviewers_of_an_area_get_its_denied_commands() {
    let fake = FakeClaude::new("result");
    let (mut harness, _work) =
        proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;
    let first = until_runs(&fake, TASK_WORKER, 1).await;
    assert_eq!(denied(&first[0]), Value::Null, "no list writes no deny");

    project_default(harness.root());
    let updated = harness
        .call(
            MAIN,
            "update_area",
            json!({"name": "web", "deny_commands": ["Bash(docker exec:*)", "Bash(psql:*)"]}),
        )
        .await;
    assert_eq!(
        updated["area"]["deny_commands"],
        json!(["Bash(docker exec:*)", "Bash(psql:*)"])
    );
    let listed = harness.call(MAIN, "list_workers", json!({})).await;
    let web = listed["areas"]
        .as_array()
        .unwrap()
        .iter()
        .find(|area| area["name"] == "web")
        .unwrap();
    assert_eq!(
        web["deny_commands"],
        json!(["Bash(docker exec:*)", "Bash(psql:*)"])
    );
    let merged = json!(["Bash(kubectl:*)", "Bash(psql:*)", "Bash(docker exec:*)"]);

    harness
        .call(
            MAIN,
            "create_label",
            json!({"name": "server", "color": "sage"}),
        )
        .await;
    harness
        .call(
            MAIN,
            "define_area",
            json!({"name": "api", "label": "server"}),
        )
        .await;
    harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "api"}),
        )
        .await;
    let api_lead = until_runs(&fake, "api-lead", 1).await;
    assert_eq!(
        denied(&api_lead[0]),
        json!(["Bash(kubectl:*)", "Bash(psql:*)"])
    );

    harness
        .call(MAIN, "start_worker", json!({"role": "review_lead"}))
        .await;
    until_state(&harness, "review-lead", WorkerState::Idle).await;
    let review_lead = until_runs(&fake, "review-lead", 1).await;
    assert_eq!(
        denied(&review_lead[0]),
        json!(["Bash(kubectl:*)", "Bash(psql:*)"])
    );
    harness.connect("review-lead").await;
    harness
        .call(
            "review-lead",
            "start_worker",
            json!({"role": "reviewer", "proposals": ["P-1"]}),
        )
        .await;
    let reviewer = until_runs(&fake, REVIEWER, 1).await;
    assert_eq!(denied(&reviewer[0]), merged);
    let text = std::fs::read_to_string(reviewer[0].flag("--settings").unwrap()).unwrap();
    let settings: Value = serde_json::from_str(&text).unwrap();
    assert!(
        settings["permissions"]["allow"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule
                .as_str()
                .is_some_and(|rule| rule.starts_with("mcp__fold"))),
        "{settings}"
    );

    request_changes(&harness, "P-1", "Split the page.").await;
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert!(runs[1].has("--resume"), "the task worker resumes");
    assert_eq!(denied(&runs[1]), merged);
}

/// Acceptance 72: a changed list reaches the lead's next start; a cleared one leaves only the project's; a rule that would deny fold's own work is refused with `invalid_permission_rule`.
#[tokio::test]
async fn acceptance_72_a_changed_list_reaches_the_next_start_and_a_bad_rule_is_refused() {
    let fake = FakeClaude::new("result");
    let harness =
        crate::worker_resume::task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    project_default(harness.root());
    harness
        .call(
            MAIN,
            "update_area",
            json!({"name": "web", "deny_commands": ["Bash(ssh:*)"]}),
        )
        .await;
    harness
        .call(MAIN, "restart_worker", json!({"name": "web-lead"}))
        .await;
    let runs = until_runs(&fake, "web-lead", 2).await;
    assert_eq!(
        denied(&runs[1]),
        json!(["Bash(kubectl:*)", "Bash(psql:*)", "Bash(ssh:*)"])
    );

    harness
        .call(
            MAIN,
            "update_area",
            json!({"name": "web", "clear_deny_commands": true}),
        )
        .await;
    harness
        .call(MAIN, "restart_worker", json!({"name": "web-lead"}))
        .await;
    let runs = until_runs(&fake, "web-lead", 3).await;
    assert_eq!(denied(&runs[2]), json!(["Bash(kubectl:*)", "Bash(psql:*)"]));

    for rule in ["Bash(cd:*)", "Read(/srv/**)", "Bash(*)"] {
        let refused = harness
            .refused(
                MAIN,
                "update_area",
                json!({"name": "web", "deny_commands": [rule]}),
            )
            .await;
        assert_eq!(refused["code"], "invalid_permission_rule", "{rule}");
        assert_eq!(refused["details"]["rule"], rule);
    }
}
