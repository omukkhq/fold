use super::{Merge3, Segment};
use crate::ids::RevisionId;

const OURS: &str = "<<<<<<<";
const BASE: &str = "|||||||";
const SEPARATOR: &str = "=======";
const THEIRS: &str = ">>>>>>>";
const WORKSPACE_LABEL: &str = "workspace";
const BASE_LABEL: &str = "base";
const MAIN_LABEL: &str = "main";

/// The labels of a sync's markers, ours, base and theirs: `workspace`, `base r<n>` and `main r<n>`, the forms [`contains_marker_line`] recognises.
pub fn sync_labels(base: RevisionId, main: RevisionId) -> [String; 3] {
    [
        WORKSPACE_LABEL.to_owned(),
        format!("{BASE_LABEL} {base}"),
        format!("{MAIN_LABEL} {main}"),
    ]
}

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

/// True when a line starts a fold marker with one of the [`sync_labels`]. A bare `=======` never counts because ordinary files contain it.
pub fn contains_marker_line(text: &str) -> bool {
    text.lines().any(|line| {
        labelled(line, OURS, WORKSPACE_LABEL).is_some()
            || numbered(line, BASE, BASE_LABEL)
            || numbered(line, THEIRS, MAIN_LABEL)
    })
}

/// The rest of `line` after the marker and its label.
fn labelled<'l>(line: &'l str, marker: &str, label: &str) -> Option<&'l str> {
    line.strip_prefix(marker)?
        .strip_prefix(' ')?
        .strip_prefix(label)
}

/// A marker whose label names a revision, `r` and its number.
fn numbered(line: &str, marker: &str, label: &str) -> bool {
    labelled(line, marker, label)
        .and_then(|rest| rest.strip_prefix(" r"))
        .and_then(|rest| rest.chars().next())
        .is_some_and(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspaces::merge::merge3;

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
    fn the_sync_labels_render_markers_that_are_detected() {
        let r = |n| RevisionId::from_number(n).unwrap();
        let [ours, base, theirs] = sync_labels(r(12), r(13));
        assert_eq!(
            [ours.as_str(), base.as_str(), theirs.as_str()],
            ["workspace", "base r12", "main r13"]
        );
        let m = merge3("a\n", "b\n", "c\n");
        assert!(contains_marker_line(&render_markers(
            &m, &ours, &base, &theirs
        )));
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
