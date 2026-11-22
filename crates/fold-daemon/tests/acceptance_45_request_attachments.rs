#![cfg(test)]

use axum::http::StatusCode;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, body};

/// Acceptance 45, an agent reads a human's attachments: a human creates a request in the web app with a text file, a screenshot and a PDF, and the agent `backend` lists them with `get_request`, reads the text inline and the screenshot as image content with `get_request_attachment`, gets only the name, size and type of the PDF, and a typed error for a position the request does not have.
#[tokio::test]
async fn acceptance_45_an_agent_reads_the_text_and_the_image_a_human_attached() {
    let h = Harness::start().await;
    let log = "panic at board.rs:12 — the search box is empty\n";
    let screenshot: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00board";
    let pdf: &[u8] = b"%PDF-1.7\n\x00\x01";
    let (status, created) = h
        .create_request_form(
            &[
                (
                    "body",
                    None,
                    None,
                    b"The board search fails; see the files.",
                ),
                ("files", Some("run.log"), Some("text/plain"), log.as_bytes()),
                ("files", Some("board.png"), Some("image/png"), screenshot),
                ("files", Some("spec.pdf"), Some("application/pdf"), pdf),
            ],
            Some(HUMAN),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    let read = h
        .call("backend", "get_request", json!({"id": "RQ-1"}))
        .await;
    assert_eq!(
        read["attachments"],
        json!([
            {"position": 1, "name": "run.log", "media_type": "text/plain", "size": log.len()},
            {"position": 2, "name": "board.png", "media_type": "image/png", "size": screenshot.len()},
            {"position": 3, "name": "spec.pdf", "media_type": "application/pdf", "size": pdf.len()},
        ])
    );

    let text = h
        .call(
            "backend",
            "get_request_attachment",
            json!({"id": "RQ-1", "n": 1}),
        )
        .await;
    assert_eq!(
        text,
        json!({
            "request": "RQ-1", "n": 1, "name": "run.log", "media_type": "text/plain",
            "size": log.len(), "content": "text", "offset": 0, "next_offset": null, "text": log,
        })
    );

    let image = h
        .call_raw(
            "backend",
            "get_request_attachment",
            json!({"id": "RQ-1", "n": 2}),
        )
        .await;
    assert_eq!(image.is_error, Some(false), "{image:?}");
    let named = body(&image);
    assert_eq!(named["content"], "image");
    assert_eq!(named["name"], "board.png");
    let block = image.content[1].as_image().expect("an image block");
    assert_eq!(block.mime_type, "image/png");
    assert_eq!(STANDARD.decode(&block.data).unwrap(), screenshot);
    // The footer, when the agent has news, comes after the image.
    for later in &image.content[2..] {
        assert!(later.as_text().unwrap().text.starts_with("fold:"));
    }

    let other = h
        .call(
            "backend",
            "get_request_attachment",
            json!({"id": "RQ-1", "n": 3}),
        )
        .await;
    assert_eq!(other["content"], "none");
    assert_eq!(other["size"], pdf.len());
    assert_eq!(other["media_type"], "application/pdf");
    assert!(other.get("text").is_none());

    let missing = h
        .call_raw(
            "backend",
            "get_request_attachment",
            json!({"id": "RQ-1", "n": 9}),
        )
        .await;
    assert_eq!(missing.is_error, Some(true));
    let error: Value = body(&missing)["error"].clone();
    assert_eq!(error["code"], "not_found");
    assert!(error["hint"].as_str().unwrap().contains("get_request"));

    let unknown = h
        .call_raw(
            "backend",
            "get_request_attachment",
            json!({"id": "RQ-7", "n": 1}),
        )
        .await;
    assert_eq!(body(&unknown)["error"]["code"], "request_not_found");
}
