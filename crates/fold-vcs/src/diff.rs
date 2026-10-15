use std::time::Duration;

use similar::{ChangeTag, DiffOp, TextDiff};

/// How many unchanged lines a hunk shows around each change.
pub const DEFAULT_CONTEXT: usize = 3;

/// How far into a file the zero-byte check looks.
pub const BINARY_PROBE_BYTES: usize = 8192;

/// The largest file whose content a read answers with or whose hunks a diff builds. A larger file is answered with its size only, as a binary file is, so one request never carries megabytes of text.
pub const MAX_INLINE_BYTES: usize = 1024 * 1024;

/// How long one line diff may search for the smallest edit. Past it `similar` stops and reports the rest of the pair as replaced, so an unusual pair of large texts gives a coarser diff, never a slow one; the hunks still turn the old text into the new.
pub const DIFF_TIMEOUT: Duration = Duration::from_millis(500);

/// A file is binary when its first 8 KB hold a zero byte or the whole content is not valid UTF-8. A binary file is compared by hash only and has no hunks.
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_PROBE_BYTES)].contains(&0) || std::str::from_utf8(bytes).is_err()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// One line of a hunk. `text` keeps the line's terminator (`\n`, `\r\n` or `\r`), so joining the lines of a side gives its text back byte for byte. Both numbers are set on a context line; review comments use `old_no` for the old side and `new_no` for the new side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    pub text: String,
    /// The last line of a file that does not end with a line terminator.
    pub no_newline: bool,
}

/// A unified hunk. `old_lines` is the number of context and removed lines, `new_lines` the number of context and added lines. A start is the 1-based number of the first line of its side, or the line before the hunk (`0` at the top) when that side has no lines in it, as unified diffs write it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

/// Line counts of a text change, without building hunks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LineCounts {
    pub added: u32,
    pub removed: u32,
}

/// The hunks that turn `old` into `new`, with `context` unchanged lines around each change. Equal texts give no hunks.
pub fn diff_text(old: &str, new: &str, context: usize) -> Vec<Hunk> {
    diff_within(old, new, context, DIFF_TIMEOUT)
}

/// How many lines `new` adds to and removes from `old`.
pub fn count_lines(old: &str, new: &str) -> LineCounts {
    count_within(old, new, DIFF_TIMEOUT)
}

fn diff_within(old: &str, new: &str, context: usize, timeout: Duration) -> Vec<Hunk> {
    let diff = TextDiff::configure().timeout(timeout).diff_lines(old, new);
    // `similar` doubles the context while grouping, so a context near `usize::MAX` would overflow.
    diff.grouped_ops(context.min(usize::MAX / 4))
        .iter()
        .filter(|group| !group.is_empty())
        .map(|group| hunk(&diff, group))
        .collect()
}

fn count_within(old: &str, new: &str, timeout: Duration) -> LineCounts {
    let diff = TextDiff::configure().timeout(timeout).diff_lines(old, new);
    let mut counts = LineCounts::default();
    for op in diff.ops() {
        let (old_range, new_range) = (op.old_range(), op.new_range());
        match op {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete { .. } => counts.removed = add(counts.removed, old_range.len()),
            DiffOp::Insert { .. } => counts.added = add(counts.added, new_range.len()),
            DiffOp::Replace { .. } => {
                counts.removed = add(counts.removed, old_range.len());
                counts.added = add(counts.added, new_range.len());
            }
        }
    }
    counts
}

/// The number of lines of `text`, a final line without a terminator included.
pub fn line_count(text: &str) -> u32 {
    count_lines("", text).added
}

fn add(count: u32, more: usize) -> u32 {
    count.saturating_add(u32::try_from(more).unwrap_or(u32::MAX))
}

fn number(index: usize) -> u32 {
    u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX)
}

fn hunk(diff: &TextDiff<'_, '_, str>, group: &[DiffOp]) -> Hunk {
    let mut lines = Vec::new();
    for op in group {
        for change in diff.iter_changes(op) {
            let kind = match change.tag() {
                ChangeTag::Equal => LineKind::Context,
                ChangeTag::Insert => LineKind::Added,
                ChangeTag::Delete => LineKind::Removed,
            };
            lines.push(DiffLine {
                kind,
                old_no: change.old_index().map(number),
                new_no: change.new_index().map(number),
                text: change.value().to_owned(),
                no_newline: change.missing_newline(),
            });
        }
    }
    let old_lines = count(&lines, LineKind::Removed);
    let new_lines = count(&lines, LineKind::Added);
    // The group's first op starts both ranges, also when one of them is empty.
    let (old_first, new_first) = group
        .first()
        .map_or((0, 0), |op| (op.old_range().start, op.new_range().start));
    Hunk {
        old_start: start(old_first, old_lines),
        old_lines,
        new_start: start(new_first, new_lines),
        new_lines,
        lines,
    }
}

fn count(lines: &[DiffLine], changed: LineKind) -> u32 {
    let found = lines
        .iter()
        .filter(|line| line.kind == LineKind::Context || line.kind == changed)
        .count();
    u32::try_from(found).unwrap_or(u32::MAX)
}

fn start(first_index: usize, lines: u32) -> u32 {
    let at = u32::try_from(first_index).unwrap_or(u32::MAX);
    if lines == 0 { at } else { at.saturating_add(1) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every third line differs, and the common first and last lines leave a middle that a search must work through.
    fn interleaved() -> (String, String) {
        let old: String = (0..300).map(|n| format!("line {n}\n")).collect();
        let new: String = (0..300)
            .map(|n| match n % 3 {
                1 if n != 1 && n != 298 => format!("edit {n}\n"),
                _ => format!("line {n}\n"),
            })
            .collect();
        (old, new)
    }

    fn side(hunks: &[Hunk], dropped: LineKind) -> String {
        hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .filter(|line| line.kind != dropped)
            .map(|line| line.text.as_str())
            .collect()
    }

    #[test]
    fn a_spent_deadline_gives_a_coarser_diff_that_still_turns_old_into_new() {
        let (old, new) = interleaved();
        let exact = count_within(&old, &new, DIFF_TIMEOUT);
        assert_eq!((exact.added, exact.removed), (98, 98));
        let coarse = count_within(&old, &new, Duration::ZERO);
        assert!(coarse.removed > exact.removed, "{coarse:?}");
        assert_eq!(coarse.removed - exact.removed, coarse.added - exact.added);
        let hunks = diff_within(&old, &new, usize::MAX, Duration::ZERO);
        assert_eq!(side(&hunks, LineKind::Added), old);
        assert_eq!(side(&hunks, LineKind::Removed), new);
    }
}
