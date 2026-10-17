use similar::{Algorithm, DiffOp, capture_diff_slices};

/// A run of consecutive lines of a merged file that one member brought in, and the line where the same run starts in that member's own version of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OriginRun {
    /// The first line of the run in the merged file, from 1.
    pub start_line: u32,
    /// The last line of the run in the merged file, inclusive.
    pub end_line: u32,
    /// The member's index in merge order, from 0.
    pub member: usize,
    /// The line of `start_line` in the member's own version, from 1. The run's lines follow it one for one.
    pub member_start_line: u32,
}

/// Which member brought each line of one file into a merge that applies members one after another onto main. `steps[0]` is main's text of the file and `steps[i + 1]` the text after member `i` merged; `members[i]` is member `i`'s own version of the file. `None` is no text file at the path.
///
/// Each line of the last step is traced back step by step through line diffs: the step whose diff inserts or replaces it is the member that brought it, and a line that reaches `steps[0]` is main's. A member's line is then found in the member's own version by a diff of that version against the last step, so a comment on the merged file can be stored on the member's lines. A line that diff cannot find in the member's version has no origin, and the runs break around it. Lines of main have no run.
pub fn line_origins(steps: &[Option<&str>], members: &[Option<&str>]) -> Vec<OriginRun> {
    let Some(Some(last)) = steps.last() else {
        return Vec::new();
    };
    let last_lines = split_lines(last);
    let mut position: Vec<Option<usize>> = (0..last_lines.len()).map(Some).collect();
    let mut owner: Vec<Option<usize>> = vec![None; last_lines.len()];
    for step in (1..steps.len()).rev() {
        let before = steps[step - 1].unwrap_or("");
        let after = steps[step].unwrap_or("");
        if before == after {
            continue;
        }
        let kept = matched(&split_lines(before), &split_lines(after));
        for (line, at) in position.iter_mut().enumerate() {
            let Some(index) = *at else {
                continue;
            };
            match kept.get(index).copied().flatten() {
                Some(earlier) => *at = Some(earlier),
                None => {
                    owner[line] = Some(step - 1);
                    *at = None;
                }
            }
        }
    }
    let mut member_line: Vec<Option<usize>> = vec![None; last_lines.len()];
    for (member, text) in members.iter().enumerate() {
        let Some(text) = text else {
            continue;
        };
        if !owner.contains(&Some(member)) {
            continue;
        }
        let found = matched(&split_lines(text), &last_lines);
        for (line, slot) in member_line.iter_mut().enumerate() {
            if owner[line] == Some(member) {
                *slot = found.get(line).copied().flatten();
            }
        }
    }
    let mut runs: Vec<OriginRun> = Vec::new();
    for (line, (owner, at)) in owner.iter().zip(&member_line).enumerate() {
        let (Some(member), Some(at)) = (*owner, *at) else {
            continue;
        };
        let (line, at) = (number(line), number(at));
        if let Some(run) = runs.last_mut()
            && run.member == member
            && run.end_line.checked_add(1) == Some(line)
            && run
                .member_start_line
                .checked_add(run.end_line - run.start_line + 1)
                == Some(at)
        {
            run.end_line = line;
            continue;
        }
        runs.push(OriginRun {
            start_line: line,
            end_line: line,
            member,
            member_start_line: at,
        });
    }
    runs
}

/// For each line of `new`, the line of `old` that a Myers diff keeps it as.
fn matched(old: &[&str], new: &[&str]) -> Vec<Option<usize>> {
    let mut map = vec![None; new.len()];
    for op in capture_diff_slices(Algorithm::Myers, old, new) {
        if let DiffOp::Equal {
            old_index,
            new_index,
            len,
        } = op
        {
            for offset in 0..len {
                if let Some(slot) = map.get_mut(new_index + offset) {
                    *slot = Some(old_index + offset);
                }
            }
        }
    }
    map
}

/// Lines as the three-way merge splits them, each with its newline, so a last line without one is still a line.
fn split_lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

fn number(index: usize) -> u32 {
    u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn run(start_line: u32, end_line: u32, member: usize, member_start_line: u32) -> OriginRun {
        OriginRun {
            start_line,
            end_line,
            member,
            member_start_line,
        }
    }

    #[test]
    fn each_line_names_the_member_that_brought_it() {
        let main = "a\nb\nc\n";
        // Member 0 inserts two lines after a; member 1, made on main, changes c.
        let first = "a\nx\ny\nb\nc\n";
        let second = "a\nb\nC\n";
        let after_second = "a\nx\ny\nb\nC\n";
        let runs = line_origins(
            &[Some(main), Some(first), Some(after_second)],
            &[Some(first), Some(second)],
        );
        assert_eq!(runs, vec![run(2, 3, 0, 2), run(5, 5, 1, 3)]);
    }

    #[test]
    fn a_line_a_later_member_replaces_belongs_to_the_later_member() {
        let main = "a\n";
        let first = "a\nb\n";
        let second = "a\nB\n";
        let runs = line_origins(
            &[Some(main), Some(first), Some(second)],
            &[Some(first), Some(second)],
        );
        assert_eq!(runs, vec![run(2, 2, 1, 2)]);
    }

    #[test]
    fn a_new_file_belongs_to_its_member_and_a_deleted_file_has_no_lines() {
        let runs = line_origins(&[None, Some("x\ny")], &[Some("x\ny")]);
        assert_eq!(runs, vec![run(1, 2, 0, 1)]);
        assert!(line_origins(&[Some("x\n"), None], &[None]).is_empty());
        assert!(line_origins(&[], &[]).is_empty());
    }

    #[test]
    fn an_unchanged_file_has_no_origin() {
        let text = Some("a\nb\n");
        assert!(line_origins(&[text, text, text], &[text, text]).is_empty());
    }

    #[test]
    fn a_member_line_missing_from_its_version_breaks_the_run() {
        // The member's own version is not what the merge took from it, so its lines are not found there.
        let runs = line_origins(&[Some("a\n"), Some("a\nb\nc\n")], &[Some("a\nc\n")]);
        assert_eq!(runs, vec![run(3, 3, 0, 2)]);
    }

    fn texts() -> impl Strategy<Value = String> {
        proptest::collection::vec(
            proptest::sample::select(vec!["a\n", "b\n", "c\n", "d\n"]),
            0..8,
        )
        .prop_map(|lines| lines.concat())
    }

    proptest! {
        /// Every run is inside the last text and the member's version, the runs come in line order without overlap, and a run's lines are the member's lines it names.
        #[test]
        fn runs_name_the_members_own_lines(main in texts(), versions in proptest::collection::vec(texts(), 1..4)) {
            let mut steps: Vec<Option<&str>> = vec![Some(main.as_str())];
            steps.extend(versions.iter().map(|text| Some(text.as_str())));
            let members: Vec<Option<&str>> = versions.iter().map(|text| Some(text.as_str())).collect();
            let runs = line_origins(&steps, &members);
            let last = split_lines(versions.last().unwrap());
            let mut previous_end = 0;
            for run in runs {
                prop_assert!(run.start_line > previous_end);
                prop_assert!(run.start_line <= run.end_line);
                previous_end = run.end_line;
                let own = split_lines(&versions[run.member]);
                let len = (run.end_line - run.start_line + 1) as usize;
                let merged = &last[run.start_line as usize - 1..run.end_line as usize];
                let start = run.member_start_line as usize - 1;
                prop_assert!(start + len <= own.len());
                prop_assert_eq!(merged, &own[start..start + len]);
            }
        }
    }
}
