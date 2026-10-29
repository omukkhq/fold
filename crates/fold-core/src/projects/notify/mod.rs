use std::collections::BTreeSet;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{FoldError, codes};
use crate::tasks::define_names;
use crate::tasks::status::TaskStatus;
use crate::text::shorten;
use crate::time::Timestamp;

mod payload;
mod rules;
mod schedule;

pub use payload::{
    CHAT_LINES, chat_line, discord_body, generic_body, new_secret, sign, signature_header,
    slack_body, test_notice,
};
pub use rules::{
    Cause, EventFacts, Lookup, NeedsYouNotice, cause_of, classify, needs_you_notice, notice,
};
pub use schedule::{
    Answer, Judgement, MAX_RETRY_AFTER, OFF_AFTER_FAILURES, RETRIES, SEND_TIMEOUT, due_at, judge,
    next_try,
};

/// The grouping window of a target that sets none.
pub const DEFAULT_WINDOW_SECONDS: u32 = 15;

/// The longest grouping window a target takes: one hour.
pub const MAX_WINDOW_SECONDS: u32 = 3600;

/// The most notices one generic send carries; the rest go in the next send.
pub const MAX_NOTICES_PER_SEND: usize = 100;

/// The sends of each target that the send log keeps.
pub const LOG_KEPT: i64 = 50;

/// The longest title a notice carries, in characters.
pub const MAX_TITLE_CHARS: usize = 120;

pub const MAX_URL_LEN: usize = 2048;
pub const MAX_NAME_CHARS: usize = 64;

define_names!(
    /// What a notice is about. Every type but the Needs you pair and `webhook.test` comes from one stored event, as [`classify`] maps it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum NoticeType {
        NeedsYouOpened = "needs_you.opened",
        NeedsYouClosed = "needs_you.closed",
        TaskCreated = "task.created",
        TaskMoved = "task.moved",
        TaskBlocked = "task.blocked",
        TaskSentBack = "task.sent_back",
        ProposalOpened = "proposal.opened",
        ReviewApproved = "review.approved",
        ReviewChangesRequested = "review.changes_requested",
        ProposalIntegrated = "proposal.integrated",
        ProposalClosed = "proposal.closed",
        IntegrationFailed = "integration.failed",
        ChecksFailed = "checks.failed",
        WorkerFailed = "worker.failed",
        GitFailed = "git.failed",
        MessageToHuman = "message.to_human",
        RequestApproved = "request.approved",
        ProjectPaused = "project.paused",
        WebhookTest = "webhook.test",
    }
);

impl NoticeType {
    /// The types a target can select: every type but `webhook.test`, which a test send always carries.
    pub fn selectable() -> impl Iterator<Item = NoticeType> {
        Self::ALL
            .into_iter()
            .filter(|kind| *kind != NoticeType::WebhookTest)
    }
}

impl Serialize for NoticeType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for NoticeType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        NoticeType::parse(&name)
            .ok_or_else(|| serde::de::Error::custom("not a notice type of fold"))
    }
}

define_names!(
    /// A named selection of notice types. `custom` is the selection a person made type by type.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Preset {
        NeedsYouOnly = "needs_you_only",
        Recommended = "recommended",
        All = "all",
        Custom = "custom",
    }
);

impl Preset {
    /// The types of the preset; `custom` has none of its own.
    pub fn types(self) -> BTreeSet<NoticeType> {
        match self {
            Preset::NeedsYouOnly => BTreeSet::from([NoticeType::NeedsYouOpened]),
            Preset::Recommended => BTreeSet::from([
                NoticeType::NeedsYouOpened,
                NoticeType::TaskMoved,
                NoticeType::TaskBlocked,
                NoticeType::ReviewChangesRequested,
                NoticeType::ProposalIntegrated,
                NoticeType::IntegrationFailed,
                NoticeType::ChecksFailed,
                NoticeType::WorkerFailed,
                NoticeType::GitFailed,
            ]),
            Preset::All => NoticeType::selectable().collect(),
            Preset::Custom => BTreeSet::new(),
        }
    }

    /// The preset whose types are exactly `types`, else `custom`.
    pub fn of(types: &BTreeSet<NoticeType>) -> Preset {
        [Preset::NeedsYouOnly, Preset::Recommended, Preset::All]
            .into_iter()
            .find(|preset| preset.types() == *types)
            .unwrap_or(Preset::Custom)
    }
}

define_names!(
    /// Where a target sends: a generic receiver that gets a signed JSON body, or a Slack or Discord incoming webhook that gets a short message it shows as it is.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum TargetKind {
        Webhook = "webhook",
        Slack = "slack",
        Discord = "discord",
    }
);

/// The card-move columns of a target that sets none: the moves a person waits for.
pub fn default_moves_to() -> BTreeSet<TaskStatus> {
    BTreeSet::from([TaskStatus::Review, TaskStatus::Done])
}

/// Which notices a target gets: its types, and for `task.moved` only the moves into `moves_to`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeFilter {
    pub types: BTreeSet<NoticeType>,
    #[serde(default = "default_moves_to")]
    pub moves_to: BTreeSet<TaskStatus>,
}

impl Default for NoticeFilter {
    fn default() -> Self {
        Self {
            types: Preset::Recommended.types(),
            moves_to: default_moves_to(),
        }
    }
}

impl NoticeFilter {
    /// Whether a target may want a notice of `kind` at all, before its facts are loaded.
    pub fn wants_type(&self, kind: NoticeType) -> bool {
        kind == NoticeType::WebhookTest || self.types.contains(&kind)
    }

    pub fn matches(&self, notice: &Notice) -> bool {
        if !self.wants_type(notice.kind) {
            return false;
        }
        if notice.kind != NoticeType::TaskMoved {
            return true;
        }
        notice
            .data
            .get("to")
            .and_then(serde_json::Value::as_str)
            .and_then(TaskStatus::parse)
            .is_some_and(|to| self.moves_to.contains(&to))
    }
}

/// One piece of a chat line: words of fold's own or a title, or a label with the link it opens when fold knows its web address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "part", rename_all = "snake_case")]
pub enum Part {
    Text { text: String },
    Link { label: String, url: Option<String> },
}

impl Part {
    pub fn text(text: impl Into<String>) -> Part {
        Part::Text { text: text.into() }
    }

    pub fn link(label: impl Into<String>, url: Option<String>) -> Part {
        Part::Link {
            label: label.into(),
            url,
        }
    }
}

/// One notice as the outbox keeps it: its type, the `seq` of the event it comes from, its time, the fields of the generic body, and the line a chat shows. It holds ids, titles, columns, names and links, never a message body, a review text, a diff, a path or a token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notice {
    #[serde(rename = "type")]
    pub kind: NoticeType,
    pub seq: u64,
    pub at: Timestamp,
    pub data: serde_json::Map<String, serde_json::Value>,
    pub line: Vec<Part>,
}

impl Notice {
    /// The short form the send log shows: the type and the first id the notice names, such as `task.moved T-18`.
    pub fn summary(&self) -> String {
        let id = ["task", "proposal", "batch", "request"]
            .iter()
            .find_map(|key| self.data.get(*key)?.get("id")?.as_str())
            .or_else(|| {
                ["item", "message", "agent", "target"]
                    .iter()
                    .find_map(|key| self.data.get(*key)?.as_str())
            });
        match id {
            Some(id) => format!("{} {id}", self.kind),
            None => self.kind.as_str().to_owned(),
        }
    }
}

/// The project as every notice of a send names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeProject {
    pub slug: String,
    pub name: String,
    /// The project's board, when fold knows its web address.
    pub url: Option<String>,
}

/// The links of one project's notices: below the web address a person set, or none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Links {
    base: Option<String>,
    slug: String,
}

impl Links {
    pub fn new(base: Option<&str>, slug: &str) -> Self {
        Self {
            base: base.map(|base| base.trim_end_matches('/').to_owned()),
            slug: slug.to_owned(),
        }
    }

    /// `path` below the project, such as `t/T-12`.
    pub fn to(&self, path: &str) -> Option<String> {
        self.base
            .as_ref()
            .map(|base| format!("{base}/p/{}/{path}", self.slug))
    }

    pub fn project(&self, name: &str) -> NoticeProject {
        NoticeProject {
            slug: self.slug.clone(),
            name: name.to_owned(),
            url: self.to("board"),
        }
    }
}

/// `text` as one line: every run of whitespace or control characters becomes one space, and the line is cut to [`MAX_TITLE_CHARS`].
pub fn short_title(text: &str) -> String {
    let line = text
        .split(|ch: char| ch.is_whitespace() || ch.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    shorten(&line, MAX_TITLE_CHARS)
}

/// A column as a person reads it: `In progress`.
pub fn column_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Backlog => "Backlog",
        TaskStatus::Todo => "To do",
        TaskStatus::InProgress => "In progress",
        TaskStatus::Review => "Review",
        TaskStatus::Done => "Done",
    }
}

fn invalid(message: impl Into<String>) -> FoldError {
    FoldError::new(codes::INVALID_NOTIFY_TARGET, message)
}

/// An `http` or `https` URL with a host, no whitespace and no user part, of at most [`MAX_URL_LEN`] bytes. The messages never repeat the URL, which can carry a token.
pub fn check_url(url: &str) -> Result<(), FoldError> {
    if url.len() > MAX_URL_LEN {
        return Err(invalid(format!(
            "the target URL is longer than {MAX_URL_LEN} bytes"
        )));
    }
    if url.chars().any(|ch| ch.is_whitespace() || ch.is_control()) {
        return Err(invalid(
            "the target URL holds a space or a control character",
        ));
    }
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return Err(
            invalid("the target URL must start with https:// or http://")
                .with_hint("give the full URL that the receiver or the chat gave you"),
        );
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return Err(invalid(
            "the target URL must not hold a user name or a password",
        ));
    }
    if authority.is_empty() || authority.starts_with(':') {
        return Err(invalid("the target URL has no host"));
    }
    Ok(())
}

/// The host of a URL that [`check_url`] took, without brackets or port: what a log line names.
pub fn url_host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    match authority.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    }
}

/// True for `localhost` and a loopback address (`127.0.0.0/8`, `::1`): the only hosts a notice goes to without TLS, since the bytes never leave the machine.
pub fn is_loopback_host(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// A URL that [`check_url`] took: `https` to any host, a private network too, or `http` only to a loopback host, because a notice's titles and links would cross the network unencrypted.
pub fn check_transport(url: &str) -> Result<(), FoldError> {
    if !url.starts_with("http://") || is_loopback_host(url_host(url)) {
        return Ok(());
    }
    Err(FoldError::new(
        codes::PLAINTEXT_NOTIFY_TARGET,
        "an http:// target gets the notices unencrypted, so it must be on this machine",
    )
    .with_hint("give an https:// URL, or http:// only to localhost or a loopback address"))
}

/// The URL as lists show it: its scheme, host and port, and `/…` when a path or a query follows, which can hold a token.
pub fn shown_url(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let cut = rest.find(['/', '?', '#']);
    let authority = cut.map_or(rest, |at| &rest[..at]);
    let more = cut.is_some_and(|at| rest[at..] != *"/");
    format!("{scheme}://{authority}{}", if more { "/…" } else { "" })
}

/// A target name of one line of 1 to [`MAX_NAME_CHARS`] characters, trimmed.
pub fn check_name(name: &str) -> Result<String, FoldError> {
    let name = name.trim();
    if name.is_empty()
        || name.chars().count() > MAX_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err(invalid(format!(
            "the target name must be one line of 1 to {MAX_NAME_CHARS} characters"
        )));
    }
    Ok(name.to_owned())
}

/// A grouping window of 0 to [`MAX_WINDOW_SECONDS`] seconds; 0 sends each notice alone.
pub fn check_window(seconds: u32) -> Result<Duration, FoldError> {
    if seconds > MAX_WINDOW_SECONDS {
        return Err(invalid(format!(
            "the grouping window must be 0 to {MAX_WINDOW_SECONDS} seconds"
        )));
    }
    Ok(Duration::from_secs(u64::from(seconds)))
}

/// A selection of types that a target can make: at least one type, and never `webhook.test`, which every test send carries.
pub fn check_types(types: &BTreeSet<NoticeType>) -> Result<(), FoldError> {
    if types.is_empty() {
        return Err(invalid("a target needs at least one event")
            .with_hint("choose a preset such as recommended, or name the events"));
    }
    if types.contains(&NoticeType::WebhookTest) {
        return Err(invalid(
            "webhook.test is not an event a target selects: every test send carries it",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notice_types_read_and_write_their_dotted_names() {
        for kind in NoticeType::ALL {
            assert_eq!(NoticeType::parse(kind.as_str()), Some(kind));
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(serde_json::from_str::<NoticeType>(&json).unwrap(), kind);
        }
        assert!(serde_json::from_str::<NoticeType>("\"task.flew\"").is_err());
        assert_eq!(NoticeType::selectable().count(), NoticeType::ALL.len() - 1);
    }

    #[test]
    fn presets_hold_their_types_and_a_selection_names_its_preset() {
        assert_eq!(Preset::Recommended.types().len(), 9);
        assert_eq!(Preset::NeedsYouOnly.types().len(), 1);
        assert!(!Preset::All.types().contains(&NoticeType::WebhookTest));
        assert!(Preset::Custom.types().is_empty());
        for preset in [Preset::NeedsYouOnly, Preset::Recommended, Preset::All] {
            assert_eq!(Preset::of(&preset.types()), preset);
        }
        let mut custom = Preset::Recommended.types();
        custom.insert(NoticeType::TaskCreated);
        assert_eq!(Preset::of(&custom), Preset::Custom);
        assert_eq!(NoticeFilter::default().types, Preset::Recommended.types());
    }

    fn moved(to: &str) -> Notice {
        let mut data = serde_json::Map::new();
        data.insert("to".to_owned(), serde_json::json!(to));
        data.insert("task".to_owned(), serde_json::json!({"id": "T-3"}));
        Notice {
            kind: NoticeType::TaskMoved,
            seq: 4,
            at: Timestamp::EPOCH,
            data,
            line: Vec::new(),
        }
    }

    #[test]
    fn a_card_move_matches_only_its_columns() {
        let filter = NoticeFilter::default();
        assert!(filter.matches(&moved("review")));
        assert!(filter.matches(&moved("done")));
        assert!(!filter.matches(&moved("in_progress")));
        assert!(!filter.matches(&moved("nowhere")));
        let mut test = moved("review");
        test.kind = NoticeType::WebhookTest;
        assert!(
            NoticeFilter {
                types: BTreeSet::from([NoticeType::ChecksFailed]),
                moves_to: BTreeSet::new(),
            }
            .matches(&test)
        );
        test.kind = NoticeType::TaskCreated;
        assert!(!filter.matches(&test));
        assert_eq!(moved("review").summary(), "task.moved T-3");
        let mut bare = moved("review");
        bare.data.clear();
        assert_eq!(bare.summary(), "task.moved");
    }

    #[test]
    fn a_url_is_http_with_a_host_and_shows_without_its_path() {
        for good in [
            "https://hooks.example.com/fold",
            "http://127.0.0.1:9000",
            "https://h/a?b=c",
        ] {
            assert!(check_url(good).is_ok(), "{good}");
        }
        for bad in [
            "ftp://h/x",
            "https://",
            "https://:80/x",
            "https://user:pw@h/x",
            "https://h/a b",
            "hooks.example.com",
        ] {
            let error = check_url(bad).unwrap_err();
            assert_eq!(error.code(), codes::INVALID_NOTIFY_TARGET, "{bad}");
            assert!(!error.message().contains(bad), "{bad}");
        }
        assert!(check_url(&format!("https://h/{}", "a".repeat(MAX_URL_LEN))).is_err());
        assert_eq!(
            shown_url("https://hooks.slack.com/T0/B0/XYZ"),
            "https://hooks.slack.com/…"
        );
        assert_eq!(shown_url("http://127.0.0.1:9000"), "http://127.0.0.1:9000");
        assert_eq!(shown_url("http://127.0.0.1:9000/"), "http://127.0.0.1:9000");
        assert_eq!(shown_url("https://h?token=1"), "https://h/…");
        assert_eq!(
            url_host("https://hooks.slack.com:443/T0/x"),
            "hooks.slack.com"
        );
        assert_eq!(url_host("http://[::1]:9/x"), "::1");
    }

    #[test]
    fn plain_text_goes_only_to_a_loopback_host_and_https_anywhere() {
        for fine in [
            "http://127.0.0.1:9000/x",
            "http://127.8.9.1",
            "http://localhost:80/hook",
            "http://LOCALHOST",
            "http://[::1]:9/x",
            "https://hooks.example.com/x",
            "https://10.0.0.5/x",
            "https://ntfy.lan/x",
        ] {
            assert!(check_transport(fine).is_ok(), "{fine}");
        }
        for remote in [
            "http://hooks.example.com/x",
            "http://10.0.0.5:8080",
            "http://128.0.0.1",
            "http://[::2]/x",
            "http://localhost.example.com",
        ] {
            let error = check_transport(remote).unwrap_err();
            assert_eq!(error.code(), codes::PLAINTEXT_NOTIFY_TARGET, "{remote}");
            assert!(!error.message().contains(remote), "{remote}");
        }
    }

    #[test]
    fn names_windows_and_selections_have_their_bounds() {
        assert_eq!(check_name("  Team Slack ").unwrap(), "Team Slack");
        assert!(check_name("").is_err());
        assert!(check_name("a\nb").is_err());
        assert!(check_name(&"a".repeat(MAX_NAME_CHARS + 1)).is_err());
        assert_eq!(check_window(0).unwrap(), Duration::ZERO);
        assert!(check_window(MAX_WINDOW_SECONDS).is_ok());
        assert!(check_window(MAX_WINDOW_SECONDS + 1).is_err());
        assert!(check_types(&Preset::Recommended.types()).is_ok());
        assert!(check_types(&BTreeSet::new()).is_err());
        assert!(check_types(&BTreeSet::from([NoticeType::WebhookTest])).is_err());
    }

    #[test]
    fn titles_columns_and_links_read_as_a_person_reads_them() {
        assert_eq!(short_title("  Fix\nthe\t\u{7}chart  "), "Fix the chart");
        assert_eq!(
            short_title(&"word ".repeat(60)).chars().count(),
            MAX_TITLE_CHARS
        );
        assert_eq!(column_label(TaskStatus::InProgress), "In progress");
        assert_eq!(column_label(TaskStatus::Todo), "To do");
        assert_eq!(column_label(TaskStatus::Backlog), "Backlog");
        let links = Links::new(Some("https://fold.example.org/"), "nobs");
        assert_eq!(
            links.to("t/T-18").as_deref(),
            Some("https://fold.example.org/p/nobs/t/T-18")
        );
        assert_eq!(
            links.project("nobs").url.as_deref(),
            Some("https://fold.example.org/p/nobs/board")
        );
        assert_eq!(Links::new(None, "nobs").to("board"), None);
    }
}
