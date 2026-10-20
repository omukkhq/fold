#![cfg(test)]

use std::path::Path;

use axum::http::{Method, StatusCode};
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, SLUG, WAIT};
use crate::workers_support::FakeClaude;

const MAIN: &str = "frontend";

/// Area `web` over label `ui`, then a human starts its lead over HTTP; the answer is the started worker.
async fn start_lead(harness: &Harness) -> Value {
    harness.make_main(MAIN).await;
    harness
        .call(MAIN, "create_label", json!({"name": "ui", "color": "sage"}))
        .await;
    let (status, area) = harness
        .socket(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/areas"),
            Some(&json!({"name": "web", "label": "ui"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{area}");
    let (status, started) = harness
        .socket(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/workers"),
            Some(&json!({"role": "area_lead", "area": "web", "name": "ui-lead"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{started}");
    started
}

async fn listed_worker(harness: &Harness) -> Value {
    let (status, listed) = harness
        .socket(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/workers"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    listed["workers"][0].clone()
}

#[tokio::test]
async fn a_worker_shows_its_session_and_the_transcript_path_in_its_claude_folder() {
    let config = tempfile::TempDir::new().unwrap();
    let mut fake = FakeClaude::new("result");
    fake.launcher.env.push((
        "CLAUDE_CONFIG_DIR".to_owned(),
        config.path().to_string_lossy().into_owned(),
    ));
    let harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    let started = start_lead(&harness).await;
    let run = eventually(WAIT, async || {
        fake.runs_of("ui-lead").into_iter().next().ok_or(())
    })
    .await;
    let session = run.flag("--session-id").unwrap().to_owned();
    assert_eq!(started["session_id"], session.as_str());
    let root = Path::new(harness.root()).canonicalize().unwrap();
    let folder: String = root
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let expected = config
        .path()
        .join("projects")
        .join(folder)
        .join(format!("{session}.jsonl"));
    assert_eq!(started["transcript"], expected.to_string_lossy().as_ref());
    let listed = listed_worker(&harness).await;
    assert_eq!(listed["session_id"], session.as_str());
    assert_eq!(listed["transcript"], started["transcript"]);
}

#[tokio::test]
async fn a_worker_whose_claude_folder_is_unknown_has_no_transcript_path() {
    let fake = FakeClaude::new("result");
    let harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    let started = start_lead(&harness).await;
    assert!(
        started["session_id"]
            .as_str()
            .is_some_and(|id| id.len() == 36)
    );
    assert_eq!(started["transcript"], Value::Null);
    assert_eq!(listed_worker(&harness).await["transcript"], Value::Null);
}
