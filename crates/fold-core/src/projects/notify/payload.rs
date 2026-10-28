use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Map, Value, json};
use sha2::Sha256;

use super::{Notice, NoticeProject, NoticeType, Part};
use crate::error::{FoldError, codes};
use crate::time::Timestamp;

/// The most notice lines one Slack or Discord message shows; an "and N more" line with a link to the board follows.
pub const CHAT_LINES: usize = 10;

/// Discord refuses a message longer than 2000 characters; the lines stop below it.
const DISCORD_CHARS: usize = 1900;

const SECRET_PREFIX: &str = "whsec_";

fn internal(message: &str) -> FoldError {
    FoldError::new(codes::INTERNAL_ERROR, message)
}

/// A Standard Webhooks secret from `bytes` of the OS random source: `whsec_` and their base64. The receiver keys its check with the same text.
pub fn new_secret(bytes: &[u8]) -> String {
    format!("{SECRET_PREFIX}{}", STANDARD.encode(bytes))
}

/// The key of a `whsec_` secret.
fn secret_key(secret: &str) -> Result<Vec<u8>, FoldError> {
    let encoded = secret.strip_prefix(SECRET_PREFIX).unwrap_or(secret);
    STANDARD
        .decode(encoded)
        .map_err(|_| internal("the signing secret is not base64"))
}

/// `v1,<base64 of HMAC-SHA256 over "<id>.<timestamp>.<body>">`, the Standard Webhooks signature of one secret.
pub fn sign(secret: &str, id: &str, timestamp: i64, body: &[u8]) -> Result<String, FoldError> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&secret_key(secret)?)
        .map_err(|_| internal("the signing key was refused"))?;
    mac.update(id.as_bytes());
    mac.update(b".");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    Ok(format!(
        "v1,{}",
        STANDARD.encode(mac.finalize().into_bytes())
    ))
}

/// The `webhook-signature` header: one signature per secret, space-separated, so a receiver that still holds the previous secret during a rotation checks the post too.
pub fn signature_header(
    secrets: &[&str],
    id: &str,
    timestamp: i64,
    body: &[u8],
) -> Result<String, FoldError> {
    let signatures = secrets
        .iter()
        .map(|secret| sign(secret, id, timestamp, body))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(signatures.join(" "))
}

fn to_bytes(value: &Value) -> Result<Vec<u8>, FoldError> {
    serde_json::to_vec(value).map_err(|_| internal("the notice body did not serialise"))
}

/// The JSON body of a generic send: `fold.notices` with the project and its notices, oldest first, each with its own `type`, `seq` and `timestamp`.
pub fn generic_body(
    project: &NoticeProject,
    notices: &[Notice],
    now: Timestamp,
) -> Result<Vec<u8>, FoldError> {
    let notices: Vec<Value> = notices
        .iter()
        .map(|notice| {
            let mut entry = Map::new();
            entry.insert("type".to_owned(), json!(notice.kind));
            entry.insert("seq".to_owned(), json!(notice.seq));
            entry.insert("timestamp".to_owned(), json!(notice.at.to_string()));
            for (key, value) in &notice.data {
                entry.insert(key.clone(), value.clone());
            }
            Value::Object(entry)
        })
        .collect();
    to_bytes(&json!({
        "type": "fold.notices",
        "timestamp": now.to_string(),
        "data": {"project": project, "notices": notices},
    }))
}

#[derive(Clone, Copy)]
enum Chat {
    Slack,
    Discord,
}

impl Chat {
    fn escape(self, text: &str) -> String {
        match self {
            Chat::Slack => text
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;"),
            Chat::Discord => {
                let mut escaped = String::with_capacity(text.len());
                for ch in text.chars() {
                    if matches!(
                        ch,
                        '\\' | '*'
                            | '_'
                            | '~'
                            | '`'
                            | '|'
                            | '['
                            | ']'
                            | '('
                            | ')'
                            | '<'
                            | '>'
                            | '#'
                            | '@'
                    ) {
                        escaped.push('\\');
                    }
                    escaped.push(ch);
                }
                escaped
            }
        }
    }

    fn link(self, label: &str, url: Option<&str>) -> String {
        let label = self.escape(label);
        match (self, url) {
            (_, None) => label,
            (Chat::Slack, Some(url)) => format!("<{url}|{}>", label.replace('|', "¦")),
            (Chat::Discord, Some(url)) => format!("[{label}](<{url}>)"),
        }
    }

    fn line(self, project: &NoticeProject, notice: &Notice) -> String {
        let mut line = self.link(&project.name, project.url.as_deref());
        line.push_str(" · ");
        for part in &notice.line {
            match part {
                Part::Text { text } => line.push_str(&self.escape(text)),
                Part::Link { label, url } => line.push_str(&self.link(label, url.as_deref())),
            }
        }
        line
    }

    fn text(self, project: &NoticeProject, notices: &[Notice]) -> String {
        let mut lines: Vec<String> = Vec::new();
        let mut length = 0;
        for notice in notices.iter().take(CHAT_LINES) {
            let line = self.line(project, notice);
            if matches!(self, Chat::Discord) && length + line.chars().count() + 1 > DISCORD_CHARS {
                break;
            }
            length += line.chars().count() + 1;
            lines.push(line);
        }
        let more = notices.len() - lines.len();
        if more > 0 {
            let words = format!("and {more} more");
            lines.push(self.link(&words, project.url.as_deref()));
        }
        lines.join("\n")
    }
}

/// A Slack chat line, as the tests and the log read it.
pub fn chat_line(project: &NoticeProject, notice: &Notice) -> String {
    Chat::Slack.line(project, notice)
}

/// The body of a Slack incoming webhook: one line per notice in Slack's mrkdwn, at most [`CHAT_LINES`], then "and N more".
pub fn slack_body(project: &NoticeProject, notices: &[Notice]) -> Result<Vec<u8>, FoldError> {
    to_bytes(&json!({"text": Chat::Slack.text(project, notices)}))
}

/// The body of a Discord webhook: the same lines in Discord's markdown, with every mention turned off.
pub fn discord_body(project: &NoticeProject, notices: &[Notice]) -> Result<Vec<u8>, FoldError> {
    to_bytes(&json!({
        "content": Chat::Discord.text(project, notices),
        "allowed_mentions": {"parse": []},
    }))
}

/// The notice of a test send of the target `target`, named `name`.
pub fn test_notice(target: &str, name: &str, now: Timestamp) -> Notice {
    let mut data = Map::new();
    data.insert("target".to_owned(), json!(target));
    Notice {
        kind: NoticeType::WebhookTest,
        seq: 0,
        at: now,
        data,
        line: vec![Part::text(format!(
            "Test notice for {}: the target works",
            super::short_title(name)
        ))],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(url: Option<&str>) -> NoticeProject {
        NoticeProject {
            slug: "nobs".to_owned(),
            name: "nobs".to_owned(),
            url: url.map(str::to_owned),
        }
    }

    fn notice(seq: u64, title: &str) -> Notice {
        let mut data = Map::new();
        data.insert("task".to_owned(), json!({"id": "T-18"}));
        Notice {
            kind: NoticeType::TaskMoved,
            seq,
            at: Timestamp::from_millis(1_793_180_530_000).unwrap(),
            data,
            line: vec![
                Part::link("T-18", Some("https://f.example/p/nobs/t/T-18".to_owned())),
                Part::text(format!(" {title} moved")),
            ],
        }
    }

    #[test]
    fn the_signature_follows_standard_webhooks() {
        // The example of the Standard Webhooks specification.
        let secret = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
        let body = br#"{"test": 2432232314}"#;
        assert_eq!(
            sign(secret, "msg_p5jXN8AQM9LWM0D4loKWxJek", 1_614_265_330, body).unwrap(),
            "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE="
        );
        let both = signature_header(&[secret, &new_secret(&[7; 24])], "id", 1, body).unwrap();
        let parts: Vec<&str> = both.split(' ').collect();
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|part| part.starts_with("v1,")));
        assert!(sign("whsec_!!!", "id", 1, body).is_err());
        let made = new_secret(&[0; 24]);
        assert!(made.starts_with("whsec_"));
        assert_eq!(secret_key(&made).unwrap(), vec![0; 24]);
    }

    #[test]
    fn a_generic_body_lists_the_notices_with_their_own_type_and_seq() {
        let body = generic_body(
            &project(Some("https://f.example/p/nobs/board")),
            &[notice(4812, "a"), notice(4813, "b")],
            Timestamp::from_millis(1_793_180_531_000).unwrap(),
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["type"], "fold.notices");
        assert_eq!(value["data"]["project"]["slug"], "nobs");
        let notices = value["data"]["notices"].as_array().unwrap();
        assert_eq!(notices.len(), 2);
        assert_eq!(notices[0]["type"], "task.moved");
        assert_eq!(notices[0]["seq"], 4812);
        assert_eq!(notices[1]["task"]["id"], "T-18");
        assert!(notices[0]["timestamp"].as_str().unwrap().ends_with('Z'));
    }

    #[test]
    fn chat_bodies_show_ten_lines_escape_titles_and_link_the_rest() {
        let notices: Vec<Notice> = (0..12).map(|seq| notice(seq, "<b>&*x*")).collect();
        let slack: Value = serde_json::from_slice(
            &slack_body(&project(Some("https://f.example/p/nobs/board")), &notices).unwrap(),
        )
        .unwrap();
        let text = slack["text"].as_str().unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 11);
        assert_eq!(
            lines[0],
            "<https://f.example/p/nobs/board|nobs> · <https://f.example/p/nobs/t/T-18|T-18> &lt;b&gt;&amp;*x* moved"
        );
        assert_eq!(lines[10], "<https://f.example/p/nobs/board|and 2 more>");
        let discord: Value =
            serde_json::from_slice(&discord_body(&project(None), &notices[..2]).unwrap()).unwrap();
        assert_eq!(discord["allowed_mentions"]["parse"], json!([]));
        assert_eq!(
            discord["content"].as_str().unwrap().lines().next().unwrap(),
            "nobs · [T-18](<https://f.example/p/nobs/t/T-18>) \\<b\\>&\\*x\\* moved"
        );
        let long: Vec<Notice> = (0..10).map(|seq| notice(seq, &"w".repeat(400))).collect();
        let content: Value =
            serde_json::from_slice(&discord_body(&project(None), &long).unwrap()).unwrap();
        let content = content["content"].as_str().unwrap();
        assert!(content.chars().count() < 2000);
        assert!(content.ends_with("more"));
        assert_eq!(
            chat_line(
                &project(None),
                &test_notice("nt-3", "Team Slack", Timestamp::EPOCH)
            ),
            "nobs · Test notice for Team Slack: the target works"
        );
    }
}
