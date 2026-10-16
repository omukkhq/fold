/// How far into a file the zero-byte check of [`as_text`] looks.
pub const BINARY_PROBE_BYTES: usize = 8192;

/// The bytes as text when they hold no zero byte in their first [`BINARY_PROBE_BYTES`] and are valid UTF-8 throughout; `None` for a binary file. Merge and diff decide text or binary with this one rule, so a file that merges line by line also diffs line by line.
pub fn as_text(bytes: &[u8]) -> Option<&str> {
    let head = &bytes[..bytes.len().min(BINARY_PROBE_BYTES)];
    if head.contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

/// Why a text is not one short line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineFault {
    Empty,
    NotOneLine,
    TooLong,
}

/// `text` trimmed, when it is one non-empty line of at most `max` characters.
pub fn one_line(text: &str, max: usize) -> Result<&str, LineFault> {
    let line = text.trim();
    if line.is_empty() {
        return Err(LineFault::Empty);
    }
    if line.contains(['\n', '\r']) {
        return Err(LineFault::NotOneLine);
    }
    if line.chars().count() > max {
        return Err(LineFault::TooLong);
    }
    Ok(line)
}

/// `text` cut to `limit` characters, the last one an ellipsis when anything was cut.
pub fn shorten(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut short: String = text.chars().take(limit.saturating_sub(1)).collect();
    short.push('…');
    short
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_has_no_zero_byte_in_its_head_and_is_utf8_throughout() {
        assert_eq!(as_text(b"plain\n"), Some("plain\n"));
        assert!(as_text(&[0xff, 0xfe]).is_none());
        let mut late = vec![b'a'; BINARY_PROBE_BYTES];
        late.push(0);
        assert!(as_text(&late).is_some(), "a zero byte past the probe");
        late[10] = 0;
        assert!(as_text(&late).is_none());
    }

    #[test]
    fn one_line_trims_and_names_the_fault() {
        assert_eq!(one_line("  a title \t", 10), Ok("a title"));
        assert_eq!(one_line(" \n ", 10), Err(LineFault::Empty));
        assert_eq!(one_line("a\nb", 10), Err(LineFault::NotOneLine));
        assert_eq!(one_line("a\rb", 10), Err(LineFault::NotOneLine));
        assert_eq!(one_line("ééé", 3), Ok("ééé"));
        assert_eq!(one_line("abcd", 3), Err(LineFault::TooLong));
    }

    #[test]
    fn shorten_ends_a_cut_text_with_an_ellipsis() {
        assert_eq!(shorten("short", 5), "short");
        assert_eq!(shorten("longer", 5), "long…");
        assert_eq!(shorten("ééééé", 4), "ééé…");
    }
}
