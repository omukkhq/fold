use std::fmt::Display;
use std::io::{self, Write};

use fold_core::Timestamp;

pub fn version_line() -> String {
    format!("fld {}", env!("CARGO_PKG_VERSION"))
}

pub fn line(out: &mut impl Write, text: &str) -> io::Result<()> {
    writeln!(out, "{text}")
}

/// The path as one word for a POSIX shell: unchanged when it holds only characters a shell never treats specially, else in single quotes.
pub fn shell_quote(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+=:,@%".contains(c));
    if plain {
        text.into_owned()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

/// `text` with every control character written as its Rust escape, such as `\n` or `\u{1b}`, so text that agents or remote commits wrote cannot move the cursor, change colours or end a line of the terminal.
pub fn escape_control(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            escaped.extend(c.escape_default());
        } else {
            escaped.push(c);
        }
    }
    escaped
}

/// `@ana` for the actor label `human:ana`, `@frontend` for an agent: a person as every screen shows one.
pub fn person(actor: &str) -> String {
    format!("@{}", actor.strip_prefix("human:").unwrap_or(actor))
}

/// `r1: 3 files, 120 bytes`
pub fn revision_summary(revision: &impl Display, files: usize, bytes: u64) -> String {
    let noun = if files == 1 { "file" } else { "files" };
    let unit = if bytes == 1 { "byte" } else { "bytes" };
    format!("{revision}: {files} {noun}, {bytes} {unit}")
}

/// `r2..r4`, or `r3` for one revision.
pub fn revision_range(first: &impl Display, last: &impl Display) -> String {
    let (first, last) = (first.to_string(), last.to_string());
    if first == last {
        first
    } else {
        format!("{first}..{last}")
    }
}

/// `1 commit`, `3 commits`
pub fn count(count: usize, one: &str, many: &str) -> String {
    let noun = if count == 1 { one } else { many };
    format!("{count} {noun}")
}

/// `skipped big.iso (too_large)`
pub fn skipped_line(path: &impl Display, reason: &str) -> String {
    format!("skipped {path} ({reason})")
}

/// `wt-3 ana created 2026-10-15T06:05:47.388Z last used never (laptop)`
pub fn web_token_line(
    id: &impl Display,
    human: &impl Display,
    created_at: &impl Display,
    last_used_at: Option<&impl Display>,
    label: Option<&str>,
) -> String {
    let used = last_used_at.map_or_else(|| "never".to_owned(), ToString::to_string);
    let label = label.map_or_else(String::new, |label| format!(" ({label})"));
    format!("{id} {human} created {created_at} last used {used}{label}")
}

/// One entry of a listing such as `fld log` or `fld status`: a head line, then each detail on its own line after `  ↳ `, and a detail of a detail after `    ↳ `. Plain text with no colour or other escape, so a pipe or a log keeps it as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item(Vec<String>);

impl Item {
    pub fn new(head: impl Into<String>) -> Self {
        Self(vec![head.into()])
    }

    pub fn detail(&mut self, text: impl Display) -> &mut Self {
        self.0.push(format!("  ↳ {text}"));
        self
    }

    pub fn deeper(&mut self, text: impl Display) -> &mut Self {
        self.0.push(format!("    ↳ {text}"));
        self
    }
}

/// The lines of `items`, with one blank line between each two.
pub fn items(items: Vec<Item>) -> Vec<String> {
    let mut lines = Vec::new();
    for item in items {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.extend(item.0);
    }
    lines
}

/// `23 Oct 2026, 17:11`: the time as `zone` reads it, to the minute.
pub fn readable_time<Zone: chrono::TimeZone>(at: Timestamp, zone: &Zone) -> String
where
    Zone::Offset: Display,
{
    chrono::DateTime::from_timestamp_millis(at.as_millis()).map_or_else(
        || at.to_string(),
        |time| {
            time.with_timezone(zone)
                .format("%-d %b %Y, %H:%M")
                .to_string()
        },
    )
}

/// [`readable_time`] in the local time zone of the user, from `TZ` or the system's setting.
pub fn local_time(at: Timestamp) -> String {
    readable_time(at, &chrono::Local)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_characters_are_escaped_and_other_text_is_kept() {
        assert_eq!(escape_control("Add login · ünïcode"), "Add login · ünïcode");
        assert_eq!(
            escape_control("a\nb\tc\rd\u{1b}[31m\u{7f}\u{85}"),
            "a\\nb\\tc\\rd\\u{1b}[31m\\u{7f}\\u{85}"
        );
        assert_eq!(escape_control("quote ' and \\ stay"), "quote ' and \\ stay");
        assert_eq!(escape_control(""), "");
    }

    #[test]
    fn a_person_is_written_as_at_name() {
        assert_eq!(person("human:ana"), "@ana");
        assert_eq!(person("frontend"), "@frontend");
    }

    #[test]
    fn a_path_with_special_characters_is_quoted_for_the_shell() {
        let quote = |text: &str| shell_quote(std::path::Path::new(text));
        assert_eq!(quote("/home/ana/app"), "/home/ana/app");
        assert_eq!(quote("/home/ana/my app"), "'/home/ana/my app'");
        assert_eq!(quote("/tmp/it's"), "'/tmp/it'\\''s'");
        assert_eq!(quote("/tmp/$(rm)"), "'/tmp/$(rm)'");
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn web_token_line_shows_never_for_an_unused_token_and_the_label_last() {
        assert_eq!(
            web_token_line(&"wt-3", &"ana", &"T0", None::<&String>, Some("laptop")),
            "wt-3 ana created T0 last used never (laptop)"
        );
        assert_eq!(
            web_token_line(&"wt-4", &"ana", &"T0", Some(&"T1"), None),
            "wt-4 ana created T0 last used T1"
        );
    }

    #[test]
    fn version_line_names_the_workspace_version() {
        assert_eq!(version_line(), "fld 0.5.2");
    }

    #[test]
    fn line_writes_the_text_and_a_newline() {
        let mut out = Vec::new();
        line(&mut out, "hello").unwrap();
        assert_eq!(out, b"hello\n");
    }

    #[test]
    fn the_revision_summary_counts_files_and_bytes() {
        assert_eq!(revision_summary(&"r1", 0, 0), "r1: 0 files, 0 bytes");
        assert_eq!(revision_summary(&"r1", 1, 1), "r1: 1 file, 1 byte");
        assert_eq!(revision_summary(&"r1", 7, 68), "r1: 7 files, 68 bytes");
    }

    #[test]
    fn a_skipped_line_names_the_path_and_the_reason() {
        assert_eq!(
            skipped_line(&"dir/big.iso", "too_large"),
            "skipped dir/big.iso (too_large)"
        );
    }

    #[test]
    fn ranges_and_counts_read_naturally() {
        assert_eq!(revision_range(&"r2", &"r4"), "r2..r4");
        assert_eq!(revision_range(&"r3", &"r3"), "r3");
        assert_eq!(count(1, "commit", "commits"), "1 commit");
        assert_eq!(count(0, "commit", "commits"), "0 commits");
    }

    #[test]
    fn items_are_a_head_line_and_details_with_a_blank_line_between_items() {
        let mut first = Item::new("r2  Add the form");
        first.detail("head of main").deeper("deeper");
        let mut second = Item::new("r1  Genesis revision");
        second.detail("the start of the history");
        assert_eq!(
            items(vec![first, second]),
            [
                "r2  Add the form",
                "  ↳ head of main",
                "    ↳ deeper",
                "",
                "r1  Genesis revision",
                "  ↳ the start of the history",
            ]
        );
        assert!(items(Vec::new()).is_empty());
    }

    #[test]
    fn a_readable_time_is_in_the_zone_to_the_minute() {
        let at = Timestamp::from_millis(1_792_775_503_162).unwrap();
        assert_eq!(at.to_string(), "2026-10-23T17:11:43.162Z");
        assert_eq!(readable_time(at, &chrono::Utc), "23 Oct 2026, 17:11");
        let dhaka = chrono::FixedOffset::east_opt(6 * 3600).unwrap();
        assert_eq!(readable_time(at, &dhaka), "23 Oct 2026, 23:11");
        let early = Timestamp::from_millis(0).unwrap();
        assert_eq!(readable_time(early, &chrono::Utc), "1 Jan 1970, 00:00");
    }

    #[test]
    fn line_reports_a_write_failure() {
        let mut full: &mut [u8] = &mut [];
        let error = line(&mut full, "x").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WriteZero);
    }
}
