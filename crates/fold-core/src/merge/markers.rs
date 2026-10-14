use super::{Merge3, Segment};

const OURS: &str = "<<<<<<<";
const BASE: &str = "|||||||";
const SEPARATOR: &str = "=======";
const THEIRS: &str = ">>>>>>>";

/// Writes each conflict hunk with fold's diff3 markers. Marker lines end in `\n`; a side whose last line has no terminator gets one so a marker never shares a line with text. The labels are the caller's, for example `workspace`, `base r12` and `main r13`.
pub fn render_markers(
    merge: &Merge3,
    ours_label: &str,
    base_label: &str,
    theirs_label: &str,
) -> String {
    let mut out = String::new();
    for segment in &merge.segments {
        match segment {
            Segment::Resolved(lines) => lines.iter().for_each(|line| out.push_str(line)),
            Segment::Conflict(hunk) => {
                marker(&mut out, OURS, ours_label);
                side(&mut out, &hunk.ours);
                marker(&mut out, BASE, base_label);
                side(&mut out, &hunk.base);
                marker(&mut out, SEPARATOR, "");
                side(&mut out, &hunk.theirs);
                marker(&mut out, THEIRS, theirs_label);
            }
        }
    }
    out
}

fn marker(out: &mut String, prefix: &str, label: &str) {
    out.push_str(prefix);
    if !label.is_empty() {
        out.push(' ');
        out.push_str(label);
    }
    out.push('\n');
}

fn side(out: &mut String, lines: &[String]) {
    for line in lines {
        out.push_str(line);
    }
    if lines.last().is_some_and(|last| !last.ends_with('\n')) {
        out.push('\n');
    }
}

/// True when a line starts a fold marker. A bare `=======` never counts because ordinary files contain it.
pub fn contains_marker_line(text: &str) -> bool {
    text.lines().any(|line| {
        line.starts_with("<<<<<<< workspace")
            || numbered(line, "||||||| base r")
            || numbered(line, ">>>>>>> main r")
    })
}

fn numbered(line: &str, prefix: &str) -> bool {
    line.strip_prefix(prefix)
        .and_then(|rest| rest.chars().next())
        .is_some_and(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::merge3;

    #[test]
    fn renders_a_known_conflict() {
        let m = merge3("a\nb\nc\n", "a\nours\nc\n", "a\ntheirs\nc\n");
        assert_eq!(
            render_markers(&m, "workspace", "base r12", "main r13"),
            "a\n<<<<<<< workspace\nours\n||||||| base r12\nb\n=======\ntheirs\n>>>>>>> main r13\nc\n"
        );
    }

    #[test]
    fn unterminated_sides_get_a_newline() {
        let m = merge3("a\nb", "a\nours", "a\ntheirs");
        assert_eq!(
            render_markers(&m, "workspace", "base r1", "main r2"),
            "a\n<<<<<<< workspace\nours\n||||||| base r1\nb\n=======\ntheirs\n>>>>>>> main r2\n"
        );
    }

    #[test]
    fn clean_merge_renders_plain_text() {
        let m = merge3("a\nb\n", "A\nb\n", "a\nb\n");
        assert_eq!(render_markers(&m, "w", "b", "m"), "A\nb\n");
    }

    #[test]
    fn empty_sides_render_back_to_back_markers() {
        let m = merge3("a\n", "a\nx\n", "a\ny\n");
        assert_eq!(
            render_markers(&m, "workspace", "base r1", "main r2"),
            "a\n<<<<<<< workspace\nx\n||||||| base r1\n=======\ny\n>>>>>>> main r2\n"
        );
    }

    #[test]
    fn detects_each_marker_prefix() {
        assert!(contains_marker_line("a\n<<<<<<< workspace\nb\n"));
        assert!(contains_marker_line("a\n||||||| base r12\nb\n"));
        assert!(contains_marker_line(">>>>>>> main r3"));
    }

    #[test]
    fn ignores_lookalikes() {
        assert!(!contains_marker_line("a\n=======\nb\n"));
        assert!(!contains_marker_line("see <<<<<<< in the middle\n"));
        assert!(!contains_marker_line("||||||| base rx\n>>>>>>> main r\n"));
        assert!(!contains_marker_line(""));
    }
}
