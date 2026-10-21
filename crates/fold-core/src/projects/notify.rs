use std::time::Duration;

use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::error::{FoldError, codes};
use crate::text::shorten;
use crate::time::Timestamp;

/// The interval of a target that sets none.
pub const DEFAULT_INTERVAL_MINUTES: u32 = 10;

/// The longest interval a target takes: one day.
pub const MAX_INTERVAL_MINUTES: u32 = 24 * 60;

/// How long a new item waits before its notice goes out, so an item a human closes at once, or a burst of items that come together, sends nothing or one digest.
pub const SETTLE: Duration = Duration::from_secs(30);

/// The longest title a notice carries, in characters.
pub const MAX_TITLE_CHARS: usize = 120;

/// The header of a webhook post that holds [`sign`] of its body.
pub const SIGNATURE_HEADER: &str = "x-fold-signature-256";

/// The header of a webhook post that names its [`NoticeEvent`], so a receiver can tell a test from a real notice before it reads the body.
pub const EVENT_HEADER: &str = "x-fold-event";

pub const MIN_SECRET_LEN: usize = 16;
pub const MAX_SECRET_LEN: usize = 512;
pub const MAX_URL_LEN: usize = 2048;
pub const MAX_ADDRESS_LEN: usize = 254;

/// Why a notice goes out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeEvent {
    /// Items came into Needs you.
    NeedsYou,
    /// A human asked for a test send of the target.
    Test,
}

impl NoticeEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            NoticeEvent::NeedsYou => "needs_you",
            NoticeEvent::Test => "test",
        }
    }
}

/// One item of a notice. It names what waits and where a human answers it, never what an agent or a human wrote in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NoticeItem {
    /// The Needs you kind, such as `question` or `human_review`.
    pub kind: String,
    /// The public id the item's screen opens, such as `m-31`, `P-3` or `T-12`; the agent's name for an item about a worker; none for an item about the project folder.
    pub id: Option<String>,
    /// One line of at most [`MAX_TITLE_CHARS`] characters.
    pub title: String,
    /// The item's screen in the web app.
    pub link: String,
    /// UTC milliseconds since the item waits.
    pub since: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NoticeProject {
    pub slug: String,
    pub name: String,
    /// The project's board in the web app.
    pub link: String,
}

/// What one send carries: the body of a webhook post as JSON, and the subject and text of an email.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Notice {
    pub event: NoticeEvent,
    /// None for a test send of a host target, which belongs to no project.
    pub project: Option<NoticeProject>,
    /// Oldest first. One item is a single notice; more are a digest.
    pub items: Vec<NoticeItem>,
    /// UTC milliseconds of the send, so a receiver can refuse an old body that someone sends again.
    pub sent_at: i64,
}

impl Notice {
    /// The JSON body of a webhook post. The signature covers exactly these bytes.
    pub fn body(&self) -> Result<Vec<u8>, FoldError> {
        serde_json::to_vec(self).map_err(|error| {
            FoldError::new(
                codes::INTERNAL_ERROR,
                format!("the notice did not serialise: {error}"),
            )
        })
    }

    /// The subject of an email: the single item's title, or the count of a digest.
    pub fn subject(&self) -> String {
        let project = self
            .project
            .as_ref()
            .map_or_else(String::new, |project| format!(" in {}", project.name));
        match (self.event, self.items.as_slice()) {
            (NoticeEvent::Test, _) => format!("fold: test notice{project}"),
            (NoticeEvent::NeedsYou, [item]) => format!("fold{project}: {}", item.title),
            (NoticeEvent::NeedsYou, items) => {
                format!("fold{project}: {} items need you", items.len())
            }
        }
    }

    /// The plain text of an email: one paragraph per item with its kind, id, title and link.
    pub fn text(&self) -> String {
        let mut text = match self.event {
            NoticeEvent::Test => "This is a test notice from fold. The target works.\n".to_owned(),
            NoticeEvent::NeedsYou => match self.items.len() {
                1 => "One item needs you.\n".to_owned(),
                count => format!("{count} items need you.\n"),
            },
        };
        for item in &self.items {
            text.push('\n');
            text.push_str(&kind_label(&item.kind));
            if let Some(id) = &item.id {
                text.push(' ');
                text.push_str(id);
            }
            text.push_str(": ");
            text.push_str(&item.title);
            text.push_str("\n  ");
            text.push_str(&item.link);
            text.push('\n');
        }
        if let Some(project) = &self.project {
            text.push_str(&format!("\n{}: {}\n", project.name, project.link));
        }
        text
    }
}

/// `human_review` as `Human review`.
fn kind_label(kind: &str) -> String {
    let words = kind.replace('_', " ");
    let mut chars = words.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// HMAC-SHA256 of `body` keyed with `secret`, as `sha256=<lowercase hex>`: the value of [`SIGNATURE_HEADER`]. A receiver computes the same over the raw body it got and compares in constant time.
pub fn sign(secret: &[u8], body: &[u8]) -> Result<String, FoldError> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret)
        .map_err(|_| FoldError::new(codes::INTERNAL_ERROR, "the signing key was refused"))?;
    mac.update(body);
    let digest = mac.finalize().into_bytes();
    let mut text = String::with_capacity(7 + digest.len() * 2);
    text.push_str("sha256=");
    for byte in digest {
        text.push(char::from(HEX[usize::from(byte >> 4)]));
        text.push(char::from(HEX[usize::from(byte & 0xf)]));
    }
    Ok(text)
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// `text` as one line: every run of whitespace or control characters becomes one space, and the line is cut to [`MAX_TITLE_CHARS`]. A title never breaks an email header or a chat line.
pub fn short_title(text: &str) -> String {
    let line = text
        .split(|ch: char| ch.is_whitespace() || ch.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    shorten(&line, MAX_TITLE_CHARS)
}

/// `path`, which starts with `/`, below the web app at `base`.
pub fn web_link(base: &str, path: &str) -> String {
    format!("{}{path}", base.trim_end_matches('/'))
}

/// When the pending items of a target go out: [`SETTLE`] after the oldest of them came, and never sooner than `interval` after the target's last send, so a target gets at most one send in each interval.
pub fn send_at(
    oldest_pending: Timestamp,
    last_sent: Option<Timestamp>,
    interval: Duration,
) -> Timestamp {
    let settled = oldest_pending.checked_add(SETTLE).unwrap_or(oldest_pending);
    match last_sent.and_then(|last| last.checked_add(interval)) {
        Some(next) if next > settled => next,
        _ => settled,
    }
}

/// How an email target's connection to its SMTP server is protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmtpSecurity {
    /// TLS from the first byte, usually on port 465.
    Tls,
    /// A plain connection that `STARTTLS` must upgrade before anything else is sent, usually on port 587.
    Starttls,
    /// No TLS, for a relay on a trusted network. Such a target takes no password, so a password never travels in the clear.
    None,
}

impl SmtpSecurity {
    pub fn as_str(self) -> &'static str {
        match self {
            SmtpSecurity::Tls => "tls",
            SmtpSecurity::Starttls => "starttls",
            SmtpSecurity::None => "none",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            SmtpSecurity::Tls => 465,
            SmtpSecurity::Starttls => 587,
            SmtpSecurity::None => 25,
        }
    }
}

fn invalid(message: impl Into<String>) -> FoldError {
    FoldError::new(codes::INVALID_NOTIFY_TARGET, message)
}

/// An `http` or `https` URL with a host, no whitespace and no user part, of at most [`MAX_URL_LEN`] bytes. The messages never repeat the URL, which can carry a token.
pub fn check_webhook_url(url: &str) -> Result<(), FoldError> {
    if url.len() > MAX_URL_LEN {
        return Err(invalid(format!(
            "the webhook URL is longer than {MAX_URL_LEN} bytes"
        )));
    }
    if url.chars().any(|ch| ch.is_whitespace() || ch.is_control()) {
        return Err(invalid(
            "the webhook URL holds a space or a control character",
        ));
    }
    let Some((_, rest)) = url
        .strip_prefix("https://")
        .map(|rest| ("https", rest))
        .or_else(|| url.strip_prefix("http://").map(|rest| ("http", rest)))
    else {
        return Err(
            invalid("the webhook URL must start with https:// or http://")
                .with_hint("give the full URL, such as https://hooks.example.com/fold"),
        );
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return Err(
            invalid("the webhook URL must not hold a user name or a password")
                .with_hint("give the receiver a signing secret instead"),
        );
    }
    if authority.is_empty() || authority.starts_with(':') {
        return Err(invalid("the webhook URL has no host"));
    }
    Ok(())
}

/// The URL as lists show it: its scheme, host and port, and `/…` when a path or a query follows, which can hold a token.
pub fn shown_url(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let cut = rest.find(['/', '?', '#']);
    let authority = cut.map_or(rest, |at| &rest[..at]);
    let more = cut.is_some_and(|at| rest[at..] != *"/");
    format!("{scheme}://{authority}{}", if more { "/…" } else { "" })
}

/// One address `local@domain`, with no whitespace, control character, comma or angle bracket, so it never adds a recipient or breaks a header.
pub fn check_email(address: &str) -> Result<(), FoldError> {
    let fault = if address.len() > MAX_ADDRESS_LEN {
        Some("is too long")
    } else if address.chars().any(|ch| {
        ch.is_whitespace() || ch.is_control() || matches!(ch, ',' | ';' | '<' | '>' | '"')
    }) {
        Some("holds a space or a character an address cannot have")
    } else {
        match address.split_once('@') {
            Some((local, domain))
                if !local.is_empty() && !domain.is_empty() && !domain.contains('@') =>
            {
                None
            }
            _ => Some("is not one address of the form name@example.com"),
        }
    };
    match fault {
        Some(fault) => Err(invalid(format!("the email address {fault}"))),
        None => Ok(()),
    }
}

/// A host name or IP address of an SMTP server, without a scheme, port or space.
pub fn check_smtp_host(host: &str) -> Result<(), FoldError> {
    if host.is_empty()
        || host.len() > 253
        || host.chars().any(|ch| {
            !(ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | ':' | '[' | ']'))
        })
    {
        return Err(
            invalid("the SMTP host must be a host name or an IP address")
                .with_hint("give the host alone, such as smtp.example.com, and the port apart"),
        );
    }
    Ok(())
}

/// A signing secret of [`MIN_SECRET_LEN`] to [`MAX_SECRET_LEN`] bytes with no whitespace, so it survives a copy into the receiver's configuration.
pub fn check_secret(secret: &str) -> Result<(), FoldError> {
    if secret.len() < MIN_SECRET_LEN || secret.len() > MAX_SECRET_LEN {
        return Err(invalid(format!(
            "the signing secret must be {MIN_SECRET_LEN} to {MAX_SECRET_LEN} bytes"
        )));
    }
    if secret
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return Err(invalid(
            "the signing secret holds a space or a control character",
        ));
    }
    Ok(())
}

/// An interval of 1 to [`MAX_INTERVAL_MINUTES`] minutes.
pub fn check_interval(minutes: u32) -> Result<(), FoldError> {
    if minutes == 0 || minutes > MAX_INTERVAL_MINUTES {
        return Err(invalid(format!(
            "the interval must be 1 to {MAX_INTERVAL_MINUTES} minutes"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_millis(seconds * 1000).unwrap()
    }

    fn item(kind: &str, id: Option<&str>, title: &str) -> NoticeItem {
        NoticeItem {
            kind: kind.to_owned(),
            id: id.map(str::to_owned),
            title: title.to_owned(),
            link: format!("http://fold/p/demo/{kind}"),
            since: 5,
        }
    }

    fn notice(items: Vec<NoticeItem>) -> Notice {
        Notice {
            event: NoticeEvent::NeedsYou,
            project: Some(NoticeProject {
                slug: "demo".to_owned(),
                name: "Demo".to_owned(),
                link: "http://fold/p/demo/board".to_owned(),
            }),
            items,
            sent_at: 9,
        }
    }

    #[test]
    fn the_signature_is_hmac_sha256_in_hex() {
        // RFC 4231 test case 2.
        assert_eq!(
            sign(b"Jefe", b"what do ya want for nothing?").unwrap(),
            "sha256=5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_ne!(
            sign(b"other", b"body").unwrap(),
            sign(b"secret", b"body").unwrap()
        );
    }

    #[test]
    fn a_new_item_waits_the_settle_time_and_a_target_sends_once_per_interval() {
        let interval = Duration::from_secs(600);
        assert_eq!(send_at(at(100), None, interval), at(130));
        assert_eq!(send_at(at(100), Some(at(10)), interval), at(610));
        assert_eq!(send_at(at(1000), Some(at(10)), interval), at(1030));
    }

    #[test]
    fn a_title_is_one_short_line() {
        assert_eq!(short_title("  Fix\nthe\t\u{7}chart  "), "Fix the chart");
        let long = "word ".repeat(60);
        let title = short_title(&long);
        assert_eq!(title.chars().count(), MAX_TITLE_CHARS);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn a_single_item_and_a_digest_read_differently() {
        let single = notice(vec![item("human_review", Some("P-3"), "Database schema")]);
        assert_eq!(single.subject(), "fold in Demo: Database schema");
        assert_eq!(
            single.text(),
            "One item needs you.\n\nHuman review P-3: Database schema\n  http://fold/p/demo/human_review\n\nDemo: http://fold/p/demo/board\n"
        );
        let digest = notice(vec![
            item("question", Some("m-4"), "Chart library"),
            item("head_moved", None, "The Git branch moved"),
        ]);
        assert_eq!(digest.subject(), "fold in Demo: 2 items need you");
        assert!(digest.text().starts_with("2 items need you.\n"));
        assert!(digest.text().contains("Head moved: The Git branch moved\n"));
        let test = Notice {
            event: NoticeEvent::Test,
            project: None,
            items: Vec::new(),
            sent_at: 1,
        };
        assert_eq!(test.subject(), "fold: test notice");
        assert!(test.text().contains("test notice"));
        let body: serde_json::Value = serde_json::from_slice(&single.body().unwrap()).unwrap();
        assert_eq!(body["event"], "needs_you");
        assert_eq!(body["project"]["slug"], "demo");
        assert_eq!(body["items"][0]["id"], "P-3");
        assert_eq!(NoticeEvent::Test.as_str(), "test");
        assert_eq!(kind_label(""), "");
    }

    #[test]
    fn links_join_the_base_without_a_double_slash() {
        assert_eq!(web_link("http://h:1/", "/p/x"), "http://h:1/p/x");
        assert_eq!(
            web_link("https://fold.example.com", "/p/x"),
            "https://fold.example.com/p/x"
        );
    }

    #[test]
    fn a_webhook_url_is_http_with_a_host_and_shows_without_its_path() {
        for good in [
            "https://hooks.example.com/fold",
            "http://127.0.0.1:9000",
            "https://h/a?b=c",
        ] {
            assert!(check_webhook_url(good).is_ok(), "{good}");
        }
        for bad in [
            "ftp://h/x",
            "https://",
            "https://:80/x",
            "https://user:pw@h/x",
            "https://h/a b",
            "hooks.example.com",
        ] {
            let error = check_webhook_url(bad).unwrap_err();
            assert_eq!(error.code(), codes::INVALID_NOTIFY_TARGET, "{bad}");
            assert!(!error.message().contains(bad), "{bad}");
        }
        assert!(check_webhook_url(&format!("https://h/{}", "a".repeat(MAX_URL_LEN))).is_err());
        assert_eq!(
            shown_url("https://hooks.slack.com/T0/B0/XYZ"),
            "https://hooks.slack.com/…"
        );
        assert_eq!(shown_url("http://127.0.0.1:9000"), "http://127.0.0.1:9000");
        assert_eq!(shown_url("http://127.0.0.1:9000/"), "http://127.0.0.1:9000");
        assert_eq!(shown_url("https://h?token=1"), "https://h/…");
    }

    #[test]
    fn an_email_address_is_one_plain_address() {
        assert!(check_email("ana@example.com").is_ok());
        for bad in [
            "ana",
            "@example.com",
            "ana@",
            "a@b@c",
            "ana@example.com, bob@example.com",
            "ana@example.com\r\nBcc: x@y",
            "Ana <ana@example.com>",
        ] {
            assert!(check_email(bad).is_err(), "{bad}");
        }
        assert!(check_email(&format!("{}@example.com", "a".repeat(250))).is_err());
    }

    #[test]
    fn hosts_secrets_and_intervals_have_their_bounds() {
        assert!(check_smtp_host("smtp.example.com").is_ok());
        assert!(check_smtp_host("[::1]").is_ok());
        for bad in ["", "smtp://h", "h 1", "h/x"] {
            assert!(check_smtp_host(bad).is_err(), "{bad}");
        }
        assert!(check_secret("0123456789abcdef").is_ok());
        assert!(check_secret("short").is_err());
        assert!(check_secret("0123456789abcdef 1").is_err());
        assert!(check_secret(&"a".repeat(MAX_SECRET_LEN + 1)).is_err());
        assert!(check_interval(1).is_ok());
        assert!(check_interval(MAX_INTERVAL_MINUTES).is_ok());
        assert!(check_interval(0).is_err());
        assert!(check_interval(MAX_INTERVAL_MINUTES + 1).is_err());
        assert_eq!(SmtpSecurity::Tls.default_port(), 465);
        assert_eq!(SmtpSecurity::Starttls.default_port(), 587);
        assert_eq!(SmtpSecurity::None.default_port(), 25);
        assert_eq!(SmtpSecurity::Starttls.as_str(), "starttls");
        assert_eq!(SmtpSecurity::None.as_str(), "none");
        assert_eq!(SmtpSecurity::Tls.as_str(), "tls");
    }
}
