#![cfg(test)]

use crate::common::{DaemonGuard, Host, daemons};

#[tokio::test]
async fn fld_cost_prints_a_new_projects_empty_totals_and_refuses_an_unknown_task() {
    let host = Host::new().await;
    let root = host.dir.path().join("site");
    std::fs::create_dir_all(&root).unwrap();
    let init = host
        .fold(&["init"], &root)
        .env("FOLD_HUMAN", "ana")
        .output()
        .await
        .unwrap();
    assert!(init.status.success(), "{init:?}");

    let cost = host
        .fold(&["cost"], &root)
        .env("FOLD_HUMAN", "ana")
        .output()
        .await
        .unwrap();
    let guard = DaemonGuard(daemons(&host.default_socket()));
    assert!(cost.status.success(), "{cost:?}");
    let text = String::from_utf8(cost.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[..5],
        [
            "COST  $0.00 all time · $0.00 today · $0.00 last 7 days",
            "0 workers · 0 turns",
            "TOKENS  0 · 0 cache read · 0 cache write · 0 output · 0 input",
            "",
            "BY ROLE"
        ],
        "{text}"
    );
    assert!(text.contains("\nBY AREA\n  no turns\n"), "{text}");
    assert!(text.contains("  no task has a turn yet\n"), "{text}");
    assert!(
        text.contains("The main agent runs in your own Claude Code session"),
        "{text}"
    );

    let json = host
        .fold(&["cost", "--since", "7d", "--json"], &root)
        .env("FOLD_HUMAN", "ana")
        .output()
        .await
        .unwrap();
    assert!(json.status.success(), "{json:?}");
    let body: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(body["total"]["cost_micro_usd"], 0);
    assert_eq!(body["total"]["tokens"]["total"], 0);
    assert!(body["since"].is_i64(), "{body}");

    let missing = host
        .fold(&["cost", "--task", "T-9"], &root)
        .env("FOLD_HUMAN", "ana")
        .output()
        .await
        .unwrap();
    assert!(!missing.status.success());
    let error = String::from_utf8_lossy(&missing.stderr);
    assert!(error.contains("task_not_found"), "{error}");
    guard.stop().await;
}
