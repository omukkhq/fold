#![cfg(test)]

use crate::common::Host;

/// `fld subscribe` refuses a plain http override off this machine before it opens any connection.
#[tokio::test]
async fn subscribe_reports_a_refused_send_with_a_hint() {
    let host = Host::new().await;
    let output = host
        .fold(&["subscribe", "ana@example.com"], host.dir.path())
        .env("FOLD_NEWS_URL", "http://example.com/x")
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("FOLD_NEWS_URL"), "{error}");
    assert!(error.contains("hint:"), "{error}");
}

#[tokio::test]
async fn subscribe_without_an_address_off_a_terminal_names_the_next_step() {
    let host = Host::new().await;
    let output = host
        .fold(&["subscribe"], host.dir.path())
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("fld subscribe ana@example.com"), "{error}");
}
