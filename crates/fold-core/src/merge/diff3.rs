use similar::{Algorithm, DiffOp, capture_diff_slices};

/// One piece of a merged file: lines taken as they are, or a span both sides changed differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Resolved(Vec<String>),
    Conflict(ConflictHunk),
}

/// All three versions of the whole conflicting span, so a caller can render or resolve it without the inputs. Each `*_start` is the index, from 0, of the span's first line in that side's file; an empty span starts where its lines would be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictHunk {
    pub base: Vec<String>,
    pub ours: Vec<String>,
    pub theirs: Vec<String>,
    pub base_start: usize,
    pub ours_start: usize,
    pub theirs_start: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merge3 {
    pub segments: Vec<Segment>,
}

impl Merge3 {
    pub fn is_clean(&self) -> bool {
        self.segments
            .iter()
            .all(|segment| matches!(segment, Segment::Resolved(_)))
    }

    /// The merged text, or `None` when a conflict remains.
    pub fn into_text(self) -> Option<String> {
        let mut text = String::new();
        for segment in self.segments {
            match segment {
                Segment::Resolved(lines) => text.extend(lines),
                Segment::Conflict(_) => return None,
            }
        }
        Some(text)
    }
}

/// A changed span of the base on one side: `base[start..end]` becomes `lines`.
struct Region {
    start: usize,
    end: usize,
    lines: Vec<String>,
}

/// Three-way line merge. `ours` is the workspace or proposal side, `theirs` is main. Lines keep their terminator, so line endings are never normalised.
pub fn merge3(base: &str, ours: &str, theirs: &str) -> Merge3 {
    if ours == theirs {
        return whole(ours);
    }
    if ours == base {
        return whole(theirs);
    }
    if theirs == base {
        return whole(ours);
    }

    let base_lines = split_lines(base);
    let ours_lines = split_lines(ours);
    let theirs_lines = split_lines(theirs);
    let ours_regions = regions(&base_lines, &ours_lines);
    let theirs_regions = regions(&base_lines, &theirs_lines);

    let mut segments = Vec::new();
    let mut pos = 0;
    // The line in ours and in theirs that base line `pos` lines up with.
    let (mut ours_pos, mut theirs_pos) = (0, 0);
    let (mut oi, mut ti) = (0, 0);
    while oi < ours_regions.len() || ti < theirs_regions.len() {
        // Both sides' regions that overlap or touch form one cluster; a lone region is a clean one-sided change.
        let first = next_start(&ours_regions, oi, &theirs_regions, ti);
        let (mut span_end, mut o_end, mut t_end) = (first, oi, ti);
        loop {
            let mut grew = false;
            if let Some(r) = ours_regions.get(o_end)
                && r.start <= span_end
            {
                span_end = span_end.max(r.end);
                o_end += 1;
                grew = true;
            }
            if let Some(r) = theirs_regions.get(t_end)
                && r.start <= span_end
            {
                span_end = span_end.max(r.end);
                t_end += 1;
                grew = true;
            }
            if !grew {
                break;
            }
        }
        push_resolved(&mut segments, base_lines[pos..first].to_vec());
        let ours_cluster = &ours_regions[oi..o_end];
        let theirs_cluster = &theirs_regions[ti..t_end];
        let ours_start = ours_pos + (first - pos);
        let theirs_start = theirs_pos + (first - pos);
        if theirs_cluster.is_empty() || ours_cluster.is_empty() {
            let only = if theirs_cluster.is_empty() {
                ours_cluster
            } else {
                theirs_cluster
            };
            for region in only {
                push_resolved(&mut segments, region.lines.clone());
            }
        } else {
            let ours_text = apply(&base_lines, first, span_end, ours_cluster);
            let theirs_text = apply(&base_lines, first, span_end, theirs_cluster);
            if ours_text == theirs_text {
                push_resolved(&mut segments, ours_text);
            } else {
                segments.push(Segment::Conflict(ConflictHunk {
                    base: base_lines[first..span_end].to_vec(),
                    ours: ours_text,
                    theirs: theirs_text,
                    base_start: first,
                    ours_start,
                    theirs_start,
                }));
            }
        }
        ours_pos = ours_start + applied_len(first, span_end, ours_cluster);
        theirs_pos = theirs_start + applied_len(first, span_end, theirs_cluster);
        pos = span_end;
        oi = o_end;
        ti = t_end;
    }
    push_resolved(&mut segments, base_lines[pos..].to_vec());
    Merge3 { segments }
}

fn whole(text: &str) -> Merge3 {
    let mut segments = Vec::new();
    push_resolved(&mut segments, split_lines(text));
    Merge3 { segments }
}

fn split_lines(text: &str) -> Vec<String> {
    text.split_inclusive('\n').map(str::to_owned).collect()
}

fn push_resolved(segments: &mut Vec<Segment>, lines: Vec<String>) {
    if lines.is_empty() {
        return;
    }
    if let Some(Segment::Resolved(previous)) = segments.last_mut() {
        previous.extend(lines);
    } else {
        segments.push(Segment::Resolved(lines));
    }
}

fn next_start(ours: &[Region], oi: usize, theirs: &[Region], ti: usize) -> usize {
    match (ours.get(oi), theirs.get(ti)) {
        (Some(o), Some(t)) => o.start.min(t.start),
        (Some(r), None) | (None, Some(r)) => r.start,
        (None, None) => 0,
    }
}

/// Replays one side's regions over `base[start..end]`.
fn apply(base: &[String], start: usize, end: usize, regions: &[Region]) -> Vec<String> {
    let mut out = Vec::new();
    let mut pos = start;
    for region in regions {
        out.extend_from_slice(&base[pos..region.start]);
        out.extend_from_slice(&region.lines);
        pos = region.end;
    }
    out.extend_from_slice(&base[pos..end]);
    out
}

/// How many lines [`apply`] gives for `base[start..end]`, without building them.
fn applied_len(start: usize, end: usize, regions: &[Region]) -> usize {
    regions.iter().fold(end - start, |len, region| {
        len - (region.end - region.start) + region.lines.len()
    })
}

/// Myers diff of base against `side`, reduced to maximal changed regions in base order. Consecutive non-equal ops are one region, so regions of one side never touch.
fn regions(base: &[String], side: &[String]) -> Vec<Region> {
    let base_refs: Vec<&str> = base.iter().map(String::as_str).collect();
    let side_refs: Vec<&str> = side.iter().map(String::as_str).collect();
    let mut out: Vec<Region> = Vec::new();
    let mut open = false;
    for op in capture_diff_slices(Algorithm::Myers, &base_refs, &side_refs) {
        let (start, end, new_index, new_len) = match op {
            DiffOp::Equal { .. } => {
                open = false;
                continue;
            }
            DiffOp::Delete {
                old_index,
                old_len,
                new_index,
            } => (old_index, old_index + old_len, new_index, 0),
            DiffOp::Insert {
                old_index,
                new_index,
                new_len,
            } => (old_index, old_index, new_index, new_len),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => (old_index, old_index + old_len, new_index, new_len),
        };
        let lines = side[new_index..new_index + new_len].to_vec();
        match out.last_mut() {
            Some(last) if open => {
                last.end = end;
                last.lines.extend(lines);
            }
            _ => out.push(Region { start, end, lines }),
        }
        open = true;
    }
    out
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn text(m: Merge3) -> String {
        m.into_text().expect("clean merge")
    }

    fn conflict(m: &Merge3) -> &ConflictHunk {
        let hunks: Vec<_> = m
            .segments
            .iter()
            .filter_map(|s| match s {
                Segment::Conflict(h) => Some(h),
                Segment::Resolved(_) => None,
            })
            .collect();
        assert_eq!(hunks.len(), 1);
        hunks[0]
    }

    fn v(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|l| (*l).to_owned()).collect()
    }

    #[test]
    fn clean_table() {
        let cases: &[(&str, &str, &str, &str, &str)] = &[
            (
                "ours only",
                "a\nb\nc\n",
                "a\nB\nc\n",
                "a\nb\nc\n",
                "a\nB\nc\n",
            ),
            (
                "theirs only",
                "a\nb\nc\n",
                "a\nb\nc\n",
                "a\nb\nC\n",
                "a\nb\nC\n",
            ),
            (
                "same edit",
                "a\nb\nc\n",
                "a\nX\nc\n",
                "a\nX\nc\n",
                "a\nX\nc\n",
            ),
            (
                "different places",
                "a\nb\nc\nd\ne\n",
                "A\nb\nc\nd\ne\n",
                "a\nb\nc\nd\nE\n",
                "A\nb\nc\nd\nE\n",
            ),
            (
                "insert at start",
                "a\nb\n",
                "z\na\nb\n",
                "a\nB\n",
                "z\na\nB\n",
            ),
            ("insert at end", "a\nb\n", "a\nb\n", "a\nB\n", "a\nB\n"),
            (
                "insert at end ours",
                "a\nb\nc\n",
                "a\nb\nc\nz\n",
                "A\nb\nc\n",
                "A\nb\nc\nz\n",
            ),
            (
                "delete and edit elsewhere",
                "a\nb\nc\nd\ne\n",
                "a\nd\ne\n",
                "a\nb\nc\nd\nE\n",
                "a\nd\nE\n",
            ),
            (
                "theirs deletes, ours edits elsewhere",
                "a\nb\nc\nd\ne\n",
                "a\nb\nc\nd\nE\n",
                "a\nd\ne\n",
                "a\nd\nE\n",
            ),
            (
                "identical insertions",
                "a\nb\n",
                "a\nx\nb\n",
                "a\nx\nb\n",
                "a\nx\nb\n",
            ),
        ];
        for (name, base, ours, theirs, expected) in cases {
            let m = merge3(base, ours, theirs);
            assert!(m.is_clean(), "{name}");
            assert_eq!(text(m), *expected, "{name}");
        }
    }

    #[test]
    fn missing_trailing_newline_on_one_side() {
        assert_eq!(text(merge3("a\nb\nc", "A\nb\nc", "a\nb\nc\n")), "A\nb\nc\n");
        assert_eq!(text(merge3("a\nb\nc\n", "a\nb\nc", "A\nb\nc\n")), "A\nb\nc");
        assert_eq!(text(merge3("a\nb\nc", "a\nb\nc", "A\nb\nc")), "A\nb\nc");
        let m = merge3("a\nb\nc", "a\nb\nC", "a\nb\nc\n");
        let hunk = conflict(&m);
        assert_eq!(hunk.base, v(&["c"]));
        assert_eq!(hunk.ours, v(&["C"]));
        assert_eq!(hunk.theirs, v(&["c\n"]));
        assert!(!merge3("a\nb\n", "a\nb", "a\nb\nz\n").is_clean());
    }

    #[test]
    fn overlapping_edits_conflict_with_three_versions() {
        let m = merge3("a\nb\nc\nd\n", "a\nB1\nC1\nd\n", "a\nB2\nc\nd\n");
        assert_eq!(
            m.segments,
            vec![
                Segment::Resolved(v(&["a\n"])),
                Segment::Conflict(ConflictHunk {
                    base: v(&["b\n", "c\n"]),
                    ours: v(&["B1\n", "C1\n"]),
                    theirs: v(&["B2\n", "c\n"]),
                    base_start: 1,
                    ours_start: 1,
                    theirs_start: 1,
                }),
                Segment::Resolved(v(&["d\n"])),
            ]
        );
    }

    #[test]
    fn touching_edits_conflict() {
        let m = merge3("a\nb\nc\nd\n", "a\nB\nc\nd\n", "a\nb\nC\nd\n");
        let hunk = conflict(&m);
        assert_eq!(hunk.base, v(&["b\n", "c\n"]));
        assert_eq!(hunk.ours, v(&["B\n", "c\n"]));
        assert_eq!(hunk.theirs, v(&["b\n", "C\n"]));
    }

    #[test]
    fn different_insertions_at_one_position_conflict() {
        let m = merge3("a\nb\n", "a\nx\nb\n", "a\ny\nb\n");
        let hunk = conflict(&m);
        assert!(hunk.base.is_empty());
        assert_eq!(hunk.ours, v(&["x\n"]));
        assert_eq!(hunk.theirs, v(&["y\n"]));
    }

    #[test]
    fn insertion_touching_an_edit_conflicts() {
        let m = merge3("a\nb\nc\n", "a\nx\nb\nc\n", "a\nB\nc\n");
        assert!(!m.is_clean());
    }

    #[test]
    fn chained_regions_form_one_hunk() {
        let m = merge3("a\nb\nc\nd\ne\n", "a\nB\nc\nD\ne\n", "a\nb\nC\nd\ne\n");
        let hunk = conflict(&m);
        assert_eq!(hunk.base, v(&["b\n", "c\n", "d\n"]));
        assert_eq!(hunk.ours, v(&["B\n", "c\n", "D\n"]));
        assert_eq!(hunk.theirs, v(&["b\n", "C\n", "d\n"]));
    }

    #[test]
    fn crlf_is_kept_on_every_line() {
        let m = merge3(
            "a\r\nb\r\nc\r\nd\r\ne\r\n",
            "A\r\nb\r\nc\r\nd\r\ne\r\n",
            "a\r\nb\r\nc\r\nd\r\nE\r\n",
        );
        assert_eq!(text(m), "A\r\nb\r\nc\r\nd\r\nE\r\n");
        let m = merge3("a\r\nb\r\n", "a\r\nX\r\n", "a\r\nY\r\n");
        let hunk = conflict(&m);
        assert_eq!(hunk.ours, v(&["X\r\n"]));
        assert_eq!(hunk.theirs, v(&["Y\r\n"]));
        assert_eq!(hunk.base, v(&["b\r\n"]));
    }

    #[test]
    fn line_endings_are_not_normalised() {
        let m = merge3("a\nb\n", "a\r\nb\n", "a\nb\n");
        assert_eq!(text(m), "a\r\nb\n");
    }

    #[test]
    fn shortcuts() {
        for x in ["", "a\n", "a\nb"] {
            assert_eq!(text(merge3(x, x, x)), x);
        }
        assert_eq!(text(merge3("b\n", "o\n", "b\n")), "o\n");
        assert_eq!(text(merge3("b\n", "b\n", "t\n")), "t\n");
        assert_eq!(text(merge3("b\n", "o\n", "o\n")), "o\n");
        assert!(merge3("", "", "").segments.is_empty());
    }

    #[test]
    fn empty_base() {
        assert_eq!(text(merge3("", "a\n", "")), "a\n");
        assert!(!merge3("", "a\n", "b\n").is_clean());
    }

    #[test]
    fn into_text_refuses_conflicts() {
        assert_eq!(merge3("a\n", "b\n", "c\n").into_text(), None);
    }

    /// Lines added or removed earlier on one side move that side's later spans.
    #[test]
    fn hunk_starts_follow_each_side() {
        let m = merge3("a\nb\nc\nd\ne\n", "x\ny\na\nb\nc\nD1\ne\n", "b\nc\nD2\ne\n");
        let hunks: Vec<_> = m
            .segments
            .iter()
            .filter_map(|s| match s {
                Segment::Conflict(h) => Some(h),
                Segment::Resolved(_) => None,
            })
            .collect();
        let starts: Vec<_> = hunks
            .iter()
            .map(|h| (h.base_start, h.ours_start, h.theirs_start))
            .collect();
        assert_eq!(starts, [(0, 0, 0), (3, 5, 2)]);
        assert_eq!(hunks[0].theirs, Vec::<String>::new());
        assert_eq!(hunks[1].ours, v(&["D1\n"]));
        assert_eq!(hunks[1].theirs, v(&["D2\n"]));
    }

    #[test]
    fn deterministic() {
        let a = merge3("a\nb\nc\n", "a\nx\nc\n", "a\ny\nc\n");
        let b = merge3("a\nb\nc\n", "a\nx\nc\n", "a\ny\nc\n");
        assert_eq!(a, b);
    }

    fn file() -> impl Strategy<Value = String> {
        proptest::collection::vec(prop_oneof!["a\n", "b\n", "c\n", "d\n"], 0..8)
            .prop_map(|lines| lines.concat())
    }

    /// The lines at `start` of `side`, as many as `lines` holds.
    fn at(side: &str, start: usize, lines: &[String]) -> Option<Vec<String>> {
        split_lines(side)
            .get(start..start + lines.len())
            .map(<[String]>::to_vec)
    }

    proptest! {
        #[test]
        fn each_hunk_start_points_at_its_lines_in_that_side(base in file(), ours in file(), theirs in file()) {
            for segment in merge3(&base, &ours, &theirs).segments {
                if let Segment::Conflict(hunk) = segment {
                    prop_assert_eq!(at(&base, hunk.base_start, &hunk.base), Some(hunk.base.clone()));
                    prop_assert_eq!(at(&ours, hunk.ours_start, &hunk.ours), Some(hunk.ours.clone()));
                    prop_assert_eq!(at(&theirs, hunk.theirs_start, &hunk.theirs), Some(hunk.theirs.clone()));
                }
            }
        }
    }
}
