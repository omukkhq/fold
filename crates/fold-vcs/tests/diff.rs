use fold_vcs::diff::{
    DEFAULT_CONTEXT, DiffLine, Hunk, LineKind, count_lines, diff_text, is_binary, line_count,
};
use proptest::prelude::*;

/// A hunk as unified-diff text, with each line's terminator shown escaped and `\` marking a missing final newline.
#[cfg(test)]
fn render(hunks: &[Hunk]) -> Vec<String> {
    hunks
        .iter()
        .map(|hunk| {
            let mut out = format!(
                "@@ -{},{} +{},{} @@",
                hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
            );
            for line in &hunk.lines {
                let sign = match line.kind {
                    LineKind::Context => ' ',
                    LineKind::Added => '+',
                    LineKind::Removed => '-',
                };
                let text = line.text.replace('\r', "\\r").replace('\n', "\\n");
                out.push_str(&format!("|{sign}{text}"));
                if line.no_newline {
                    out.push('\\');
                }
            }
            out
        })
        .collect()
}

#[test]
fn a_table_of_texts_gives_the_expected_hunks() {
    let cases: &[(&str, &str, &str, &[&str])] = &[
        ("equal texts", "a\nb\n", "a\nb\n", &[]),
        ("empty files", "", "", &[]),
        (
            "an empty file gains lines",
            "",
            "a\nb\n",
            &["@@ -0,0 +1,2 @@|+a\\n|+b\\n"],
        ),
        (
            "a file is emptied",
            "a\nb\n",
            "",
            &["@@ -1,2 +0,0 @@|-a\\n|-b\\n"],
        ),
        (
            "added only",
            "a\nb\nc\n",
            "a\nb\nnew\nc\n",
            &["@@ -1,3 +1,4 @@| a\\n| b\\n|+new\\n| c\\n"],
        ),
        (
            "removed only",
            "a\nb\nc\nd\n",
            "a\nc\nd\n",
            &["@@ -1,4 +1,3 @@| a\\n|-b\\n| c\\n| d\\n"],
        ),
        (
            "a change at the first line",
            "one\n2\n3\n4\n5\n6\n",
            "ONE\n2\n3\n4\n5\n6\n",
            &["@@ -1,4 +1,4 @@|-one\\n|+ONE\\n| 2\\n| 3\\n| 4\\n"],
        ),
        (
            "a change at the last line",
            "1\n2\n3\n4\n5\nsix\n",
            "1\n2\n3\n4\n5\nSIX\n",
            &["@@ -3,4 +3,4 @@| 3\\n| 4\\n| 5\\n|-six\\n|+SIX\\n"],
        ),
        (
            "no trailing newline on either side",
            "a\nb",
            "a\nc",
            &["@@ -1,2 +1,2 @@| a\\n|-b\\|+c\\"],
        ),
        (
            "a trailing newline is added",
            "a\nb",
            "a\nb\n",
            &["@@ -1,2 +1,2 @@| a\\n|-b\\|+b\\n"],
        ),
        (
            "CRLF lines stay intact",
            "a\r\nb\r\nc\r\n",
            "a\r\nB\r\nc\r\n",
            &["@@ -1,3 +1,3 @@| a\\r\\n|-b\\r\\n|+B\\r\\n| c\\r\\n"],
        ),
        (
            "a line ending change is a change",
            "a\r\nb\n",
            "a\nb\n",
            &["@@ -1,2 +1,2 @@|-a\\r\\n|+a\\n| b\\n"],
        ),
        (
            "an insertion at the top of a long file",
            "1\n2\n3\n4\n5\n",
            "0\n1\n2\n3\n4\n5\n",
            &["@@ -1,3 +1,4 @@|+0\\n| 1\\n| 2\\n| 3\\n"],
        ),
    ];
    for (name, old, new, expected) in cases {
        assert_eq!(
            render(&diff_text(old, new, DEFAULT_CONTEXT)),
            *expected,
            "{name}"
        );
    }
}

#[test]
fn distant_changes_make_separate_hunks_with_line_numbers() {
    let old: String = (1..=20).map(|n| format!("{n}\n")).collect();
    let new: String = (1..=20)
        .filter(|n| *n != 19)
        .map(|n| match n {
            2 => "two\n".to_owned(),
            n => format!("{n}\n"),
        })
        .collect();
    let hunks = diff_text(&old, &new, DEFAULT_CONTEXT);
    assert_eq!(
        render(&hunks),
        [
            "@@ -1,5 +1,5 @@| 1\\n|-2\\n|+two\\n| 3\\n| 4\\n| 5\\n",
            "@@ -16,5 +16,4 @@| 16\\n| 17\\n| 18\\n|-19\\n| 20\\n",
        ]
    );
    let removed = &hunks[1].lines[3];
    assert_eq!(
        removed,
        &DiffLine {
            kind: LineKind::Removed,
            old_no: Some(19),
            new_no: None,
            text: "19\n".to_owned(),
            no_newline: false,
        }
    );
    let context = &hunks[1].lines[4];
    assert_eq!((context.old_no, context.new_no), (Some(20), Some(19)));
    let added = &hunks[0].lines[2];
    assert_eq!((added.old_no, added.new_no), (None, Some(2)));
}

#[test]
fn context_widens_or_narrows_a_hunk() {
    let old = "1\n2\n3\n4\n5\n";
    let new = "1\n2\nthree\n4\n5\n";
    assert_eq!(
        render(&diff_text(old, new, 0)),
        ["@@ -3,1 +3,1 @@|-3\\n|+three\\n"]
    );
    assert_eq!(
        render(&diff_text(old, new, 1)),
        ["@@ -2,3 +2,3 @@| 2\\n|-3\\n|+three\\n| 4\\n"]
    );
}

#[test]
fn line_counts_match_the_hunks() {
    assert_eq!(line_count(""), 0);
    assert_eq!(line_count("a"), 1);
    assert_eq!(line_count("a\nb\n"), 2);
    let counts = count_lines("a\nb\nc\n", "a\nB\nc\nd\n");
    assert_eq!((counts.added, counts.removed), (2, 1));
}

#[test]
fn binary_detection_reads_a_zero_byte_in_the_first_8_kb_and_invalid_utf8() {
    let mut early = vec![b'a'; 9000];
    early[8191] = 0;
    assert!(is_binary(&early));
    let mut late = vec![b'a'; 9000];
    late[8192] = 0;
    assert!(!is_binary(&late));
    assert!(is_binary(&[b'a', 0xff, b'b']));
    assert!(is_binary(b"\0"));
    assert!(!is_binary(b""));
    assert!(!is_binary("caf\u{e9}\n".as_bytes()));
}

/// Applies the hunks to `old` the way a patch tool does, checking every context and removed line against the old text.
#[cfg(test)]
fn apply(old: &str, hunks: &[Hunk]) -> String {
    let lines: Vec<&str> = old.split_inclusive('\n').collect();
    let mut out = String::new();
    let mut next = 0usize;
    for hunk in hunks {
        let first = if hunk.old_lines == 0 {
            hunk.old_start as usize
        } else {
            hunk.old_start as usize - 1
        };
        assert!(first >= next, "hunks overlap or run backwards");
        for line in &lines[next..first] {
            out.push_str(line);
        }
        next = first;
        for line in &hunk.lines {
            match line.kind {
                LineKind::Context | LineKind::Removed => {
                    assert_eq!(lines[next], line.text);
                    assert_eq!(line.old_no, Some(next as u32 + 1));
                    next += 1;
                    if line.kind == LineKind::Context {
                        out.push_str(&line.text);
                    }
                }
                LineKind::Added => out.push_str(&line.text),
            }
        }
    }
    for line in &lines[next..] {
        out.push_str(line);
    }
    out
}

#[cfg(test)]
fn text_strategy() -> impl Strategy<Value = String> {
    // Few distinct lines, so the two texts share runs and the diff has real context.
    prop::collection::vec(
        prop::sample::select(vec!["a\n", "b\n", "c\n", "d\r\n", "e", "\n"]),
        0..24,
    )
    .prop_map(|lines| lines.concat())
}

proptest! {
    #[test]
    fn applying_the_hunks_to_the_old_text_gives_the_new_text(
        old in text_strategy(),
        new in text_strategy(),
        context in 0usize..5,
    ) {
        let hunks = diff_text(&old, &new, context);
        prop_assert_eq!(apply(&old, &hunks), new.clone());
        let mut added = 0;
        let mut removed = 0;
        for hunk in &hunks {
            let count = |kinds: &[LineKind]| {
                hunk.lines.iter().filter(|line| kinds.contains(&line.kind)).count() as u32
            };
            prop_assert_eq!(hunk.old_lines, count(&[LineKind::Context, LineKind::Removed]));
            prop_assert_eq!(hunk.new_lines, count(&[LineKind::Context, LineKind::Added]));
            added += count(&[LineKind::Added]);
            removed += count(&[LineKind::Removed]);
            for line in &hunk.lines {
                prop_assert_eq!(line.old_no.is_some(), line.kind != LineKind::Added);
                prop_assert_eq!(line.new_no.is_some(), line.kind != LineKind::Removed);
                prop_assert_eq!(line.no_newline, !line.text.ends_with(['\n', '\r']));
            }
        }
        let counts = count_lines(&old, &new);
        prop_assert_eq!((counts.added, counts.removed), (added, removed));
        prop_assert_eq!(hunks.is_empty(), old == new);
    }
}
