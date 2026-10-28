use std::time::{Duration, Instant};

use fold_core::projects::notify::{Answer, SEND_TIMEOUT, TargetKind, signature_header};
use fold_core::{FoldError, Timestamp};

use super::settings::Target;

/// The headers of a generic send: Standard Webhooks' id, timestamp and signature.
fn signed_headers(
    target: &Target,
    webhook_id: &str,
    body: &[u8],
    now: Timestamp,
) -> Result<Vec<(&'static str, String)>, FoldError> {
    let timestamp = now.as_millis().div_euclid(1000);
    let signature = signature_header(&target.signing_secrets(now), webhook_id, timestamp, body)?;
    Ok(vec![
        ("webhook-id", webhook_id.to_owned()),
        ("webhook-timestamp", timestamp.to_string()),
        ("webhook-signature", signature),
    ])
}

/// The URL a send posts to: a Discord webhook answers with the message only when it waits for it, so its URL gets `wait=true`.
fn post_url(target: &Target) -> String {
    let url = target.url.expose();
    match target.kind {
        TargetKind::Discord if url.contains('?') => format!("{url}&wait=true"),
        TargetKind::Discord => format!("{url}?wait=true"),
        TargetKind::Webhook | TargetKind::Slack => url.to_owned(),
    }
}

/// A `Retry-After` of whole seconds; an HTTP date is read as no wait of its own.
fn retry_after(response: &ureq::http::Response<ureq::Body>) -> Option<Duration> {
    response
        .headers()
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// Posts `body` to `target`, blocking until the receiver answers or [`SEND_TIMEOUT`] passes, with the time it took. A redirect is never followed, so the body and its signature go only to the URL a person gave. The answer carries the status alone: no response body or header text of the receiver reaches fold's records.
pub fn post(
    target: &Target,
    webhook_id: &str,
    body: &[u8],
    now: Timestamp,
) -> Result<(Answer, Duration), FoldError> {
    let headers = match target.kind {
        TargetKind::Webhook => signed_headers(target, webhook_id, body, now)?,
        TargetKind::Slack | TargetKind::Discord => Vec::new(),
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(SEND_TIMEOUT))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into();
    let mut request = agent
        .post(post_url(target))
        .header("content-type", "application/json")
        .header("user-agent", concat!("fold/", env!("CARGO_PKG_VERSION")));
    for (name, value) in &headers {
        request = request.header(*name, value);
    }
    let started = Instant::now();
    let answer = match request.send(body) {
        Ok(response) => Answer::Status {
            code: response.status().as_u16(),
            retry_after: retry_after(&response),
        },
        Err(ureq::Error::StatusCode(code)) => Answer::Status {
            code,
            retry_after: None,
        },
        Err(ureq::Error::Timeout(_)) => Answer::Timeout,
        Err(ureq::Error::HostNotFound) => Answer::HostNotFound,
        Err(ureq::Error::Tls(_) | ureq::Error::Rustls(_) | ureq::Error::Pem(_)) => Answer::Tls,
        Err(_) => Answer::Unreachable,
    };
    Ok((answer, started.elapsed()))
}

/// The answer as a person reads it in an error: `the receiver answered HTTP 503`.
pub fn answer_text(answer: Answer) -> String {
    match answer {
        Answer::Status { code, .. } => format!("the receiver answered HTTP {code}"),
        Answer::Timeout => "the receiver did not answer in time".to_owned(),
        Answer::HostNotFound => "the receiver's host name was not found".to_owned(),
        Answer::Tls => "the TLS connection to the receiver failed".to_owned(),
        Answer::Unreachable => "the receiver could not be reached".to_owned(),
    }
}
