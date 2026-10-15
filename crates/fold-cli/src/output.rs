use std::fmt::Display;
use std::io::{self, Write};

pub fn version_line() -> String {
    format!("fld {}", env!("CARGO_PKG_VERSION"))
}

pub fn line(out: &mut impl Write, text: &str) -> io::Result<()> {
    writeln!(out, "{text}")
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(version_line(), "fld 0.1.0");
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
    fn line_reports_a_write_failure() {
        let mut full: &mut [u8] = &mut [];
        let error = line(&mut full, "x").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WriteZero);
    }
}
