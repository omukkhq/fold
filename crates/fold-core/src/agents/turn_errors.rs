use crate::agents::roles::WorkerState;

/// The longest error text fold keeps for a worker, in bytes, so also in characters. Claude Code's error of a turn is one line or a few; the cap only stops a process that writes without end.
pub const MAX_ERROR_LEN: usize = 1000;

/// How many of the last stderr lines of a failed process fold keeps: the cause comes last.
pub const STDERR_LINES: usize = 20;

/// How many turns in a row must fail before any token reached the model before fold asks a human about the worker. One failed turn can be a passing network fault; a worker whose every turn fails this way cannot work at all.
pub const MODEL_UNREACHABLE_TURNS: u64 = 3;

/// What a human does about a worker that cannot reach the model. fold cannot tell the cause from Claude Code's text with certainty, so the hint names every usual one.
pub const MODEL_UNREACHABLE_HINT: &str =
    "check the Claude Code sign-in, model and usage limit, or run `fld doctor`";

/// Whether a worker in `state` with `failed_turns` turns in a row that reached no model waits for a human. A stopped worker waits for nobody: its count outlives the stop, and a start that runs it again shows it once more.
pub fn model_unreachable(failed_turns: u64, state: WorkerState) -> bool {
    state != WorkerState::Stopped && failed_turns >= MODEL_UNREACHABLE_TURNS
}

/// The error of a result line as fold keeps it: without terminal escapes, control characters and blank lines, cut to [`MAX_ERROR_LEN`] at its end; `None` for a text with nothing left.
pub fn error_text(raw: &str) -> Option<String> {
    let lines = clean_lines(raw);
    if lines.is_empty() {
        return None;
    }
    Some(keep_start(&lines.join("\n")))
}

/// The last [`STDERR_LINES`] lines of a process's stderr as fold keeps them, cleaned as [`error_text`] cleans and cut to [`MAX_ERROR_LEN`] at their start; `None` when nothing is left. Bytes that are not UTF-8 become replacement characters.
pub fn stderr_text(bytes: &[u8]) -> Option<String> {
    let lines = clean_lines(&String::from_utf8_lossy(bytes));
    let tail = &lines[lines.len().saturating_sub(STDERR_LINES)..];
    if tail.is_empty() {
        return None;
    }
    Some(keep_end(&tail.join("\n")))
}

/// The non-blank lines of `raw` with their ends trimmed. A carriage return starts a line, as a terminal shows it; a tab becomes a space; escape sequences and other control characters are dropped.
fn clean_lines(raw: &str) -> Vec<String> {
    let mut text = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => skip_escape(&mut chars),
            '\n' | '\r' => text.push('\n'),
            '\t' => text.push(' '),
            c if c.is_control() => {}
            c => text.push(c),
        }
    }
    text.lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect()
}

/// Skips the rest of an escape sequence whose ESC was read: a CSI sequence up to its final byte, an OSC sequence up to its BEL or string terminator, or the one character of any other escape.
fn skip_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.next() {
        Some('[') => {
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
        Some(']') => {
            while let Some(c) = chars.next() {
                if c == '\u{7}' {
                    break;
                }
                if c == '\u{1b}' {
                    chars.next_if_eq(&'\\');
                    break;
                }
            }
        }
        _ => {}
    }
}

const ELLIPSIS: char = '…';

fn keep_start(text: &str) -> String {
    if text.len() <= MAX_ERROR_LEN {
        return text.to_owned();
    }
    let mut end = MAX_ERROR_LEN - ELLIPSIS.len_utf8();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut kept = text[..end].to_owned();
    kept.push(ELLIPSIS);
    kept
}

fn keep_end(text: &str) -> String {
    if text.len() <= MAX_ERROR_LEN {
        return text.to_owned();
    }
    let mut start = text.len() - (MAX_ERROR_LEN - ELLIPSIS.len_utf8());
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let mut kept = String::from(ELLIPSIS);
    kept.push_str(&text[start..]);
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_loses_escapes_controls_and_blank_lines() {
        assert_eq!(
            error_text("\u{1b}[31mNo access\u{1b}[0m\r\n\n\t to the model \u{7}\n"),
            Some("No access\n  to the model".to_owned())
        );
        assert_eq!(
            error_text("\u{1b}]0;title\u{7}left\u{1b}]8;;x\u{1b}\\ right\u{1b}c!"),
            Some("left right!".to_owned())
        );
        assert_eq!(error_text("unended\u{1b}]8;;x"), Some("unended".to_owned()));
        assert_eq!(error_text("cut\u{1b}["), Some("cut".to_owned()));
        assert_eq!(error_text(" \n\u{1b}[0m\r\n"), None);
        assert_eq!(error_text(""), None);
    }

    #[test]
    fn a_long_error_keeps_its_start_and_a_long_stderr_its_end() {
        let long = format!("{}é", "a".repeat(MAX_ERROR_LEN));
        let start = error_text(&long).unwrap();
        assert!(start.len() <= MAX_ERROR_LEN);
        assert!(start.starts_with("aaa") && start.ends_with(ELLIPSIS));
        let wide = "é".repeat(MAX_ERROR_LEN);
        assert!(error_text(&wide).unwrap().len() <= MAX_ERROR_LEN);
        let end = stderr_text(format!("é{}", "b".repeat(MAX_ERROR_LEN)).as_bytes()).unwrap();
        assert!(end.len() <= MAX_ERROR_LEN);
        assert!(end.starts_with(ELLIPSIS) && end.ends_with("bbb"));
        assert!(stderr_text(wide.as_bytes()).unwrap().len() <= MAX_ERROR_LEN);
        assert_eq!(error_text("short"), Some("short".to_owned()));
    }

    #[test]
    fn stderr_keeps_its_last_lines() {
        let lines: String = (0..30).map(|n| format!("line {n}\n\n")).collect();
        let kept = stderr_text(lines.as_bytes()).unwrap();
        assert_eq!(kept.lines().count(), STDERR_LINES);
        assert!(kept.starts_with("line 10\n") && kept.ends_with("line 29"));
        assert_eq!(
            stderr_text(b"bad \xff byte"),
            Some("bad \u{fffd} byte".to_owned())
        );
        assert_eq!(stderr_text(b"\n \n"), None);
    }

    #[test]
    fn a_worker_waits_for_a_human_after_enough_failed_turns() {
        let idle = WorkerState::Idle;
        assert!(!model_unreachable(0, idle));
        assert!(!model_unreachable(MODEL_UNREACHABLE_TURNS - 1, idle));
        assert!(model_unreachable(MODEL_UNREACHABLE_TURNS, idle));
        assert!(model_unreachable(MODEL_UNREACHABLE_TURNS + 5, idle));
        assert!(model_unreachable(
            MODEL_UNREACHABLE_TURNS,
            WorkerState::Failed
        ));
        assert!(!model_unreachable(
            MODEL_UNREACHABLE_TURNS,
            WorkerState::Stopped
        ));
    }
}
