use std::fmt::Display;
use std::io::{self, Write};

pub fn version_line() -> String {
    format!("fold {}", env!("CARGO_PKG_VERSION"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_names_the_workspace_version() {
        assert_eq!(version_line(), "fold 0.1.0");
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
