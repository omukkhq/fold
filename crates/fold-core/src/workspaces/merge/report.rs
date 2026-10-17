use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use super::{Conflict, ConflictHunk, ConflictKind};
use crate::ids::{ProposalId, RevisionId};
use crate::workspaces::object::ObjectHash;
use crate::workspaces::tree::{EntryKind, FlatEntry};

/// How many conflicting paths the author's message names before it counts the rest.
const NAMED_PATHS: usize = 5;

/// Why a version of a proposal could not be integrated: every path that conflicts between its base, main and the version. It is stored with the `conflicted` integration and read back as stored, so a reader never recomputes the merge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictReport {
    pub proposal: ProposalId,
    pub version: u32,
    pub base_revision: RevisionId,
    pub main_revision: RevisionId,
    pub trees: ReportTrees,
    /// In path order.
    pub files: Vec<ReportFile>,
}

/// The three root trees of the merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportTrees {
    #[serde(with = "hash_text")]
    pub base: ObjectHash,
    #[serde(with = "hash_text")]
    pub main: ObjectHash,
    #[serde(with = "hash_text")]
    pub version: ObjectHash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportKind {
    Text,
    BothModifiedBinary,
    ModifyDelete,
    AddAdd,
    TypeChange,
}

impl From<ConflictKind> for ReportKind {
    fn from(kind: ConflictKind) -> Self {
        match kind {
            ConflictKind::BothModifiedText => ReportKind::Text,
            ConflictKind::BothModifiedBinary => ReportKind::BothModifiedBinary,
            ConflictKind::ModifyDelete => ReportKind::ModifyDelete,
            ConflictKind::AddAdd => ReportKind::AddAdd,
            ConflictKind::TypeChange => ReportKind::TypeChange,
        }
    }
}

/// One conflicting path. An entry is `None` where that side has nothing at the path, or a directory: flat trees hold files and symlinks only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportFile {
    /// The path's bytes as UTF-8, with any invalid sequence replaced.
    pub path: String,
    pub kind: ReportKind,
    pub base: Option<ReportEntry>,
    pub main: Option<ReportEntry>,
    pub version: Option<ReportEntry>,
    /// Empty unless `kind` is `text`.
    pub hunks: Vec<ReportHunk>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportEntry {
    pub kind: ReportEntryKind,
    #[serde(with = "hash_text")]
    pub hash: ObjectHash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportEntryKind {
    File,
    Executable,
    Symlink,
    Directory,
}

impl From<FlatEntry> for ReportEntry {
    fn from(entry: FlatEntry) -> Self {
        let kind = match entry.kind {
            EntryKind::File => ReportEntryKind::File,
            EntryKind::Executable => ReportEntryKind::Executable,
            EntryKind::Symlink => ReportEntryKind::Symlink,
            EntryKind::Directory => ReportEntryKind::Directory,
        };
        ReportEntry {
            kind,
            hash: entry.hash,
        }
    }
}

/// One span of a text file that main and the version changed differently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportHunk {
    pub base: HunkSide,
    pub main: HunkSide,
    pub version: HunkSide,
}

/// The lines of one side, with their terminators. `start` is the line number, from 1, of the first line; an empty side starts at the line that follows the span's position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HunkSide {
    pub start: usize,
    pub count: usize,
    pub lines: Vec<String>,
}

impl HunkSide {
    fn new(start: usize, lines: &[String]) -> Self {
        HunkSide {
            start: start.saturating_add(1),
            count: lines.len(),
            lines: lines.to_vec(),
        }
    }
}

impl From<&ConflictHunk> for ReportHunk {
    /// In the merge `ours` is the version and `theirs` is main.
    fn from(hunk: &ConflictHunk) -> Self {
        ReportHunk {
            base: HunkSide::new(hunk.base_start, &hunk.base),
            main: HunkSide::new(hunk.theirs_start, &hunk.theirs),
            version: HunkSide::new(hunk.ours_start, &hunk.ours),
        }
    }
}

impl From<&Conflict> for ReportFile {
    fn from(conflict: &Conflict) -> Self {
        ReportFile {
            path: String::from_utf8_lossy(&conflict.path).into_owned(),
            kind: conflict.kind.into(),
            base: conflict.base.map(ReportEntry::from),
            main: conflict.theirs.map(ReportEntry::from),
            version: conflict.ours.map(ReportEntry::from),
            hunks: conflict.hunks.iter().map(ReportHunk::from).collect(),
        }
    }
}

impl ConflictReport {
    /// The report of merging `version` of `proposal`, based on `base_revision`, onto main at `main_revision`. `conflicts` come from the tree merge with the version as `ours` and main as `theirs`.
    pub fn new(
        proposal: ProposalId,
        version: u32,
        base_revision: RevisionId,
        main_revision: RevisionId,
        trees: ReportTrees,
        conflicts: &[Conflict],
    ) -> Self {
        let mut files: Vec<ReportFile> = conflicts.iter().map(ReportFile::from).collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        ConflictReport {
            proposal,
            version,
            base_revision,
            main_revision,
            trees,
            files,
        }
    }

    /// The message fold sends the author's agent. It names paths only, never file content or hunks, and the tools that lead to a new version.
    pub fn author_message(&self) -> String {
        let mut text = format!(
            "{} v{} conflicts with main at {} and was not integrated.\nConflicting paths ({}): ",
            self.proposal,
            self.version,
            self.main_revision,
            self.files.len()
        );
        let named: Vec<&str> = self
            .files
            .iter()
            .take(NAMED_PATHS)
            .map(|file| file.path.as_str())
            .collect();
        text.push_str(&named.join(", "));
        let rest = self.files.len().saturating_sub(NAMED_PATHS);
        if rest > 0 {
            let _ = write!(text, " and {rest} more");
        }
        text.push_str(".\nNext: call get_proposal to read the conflict report, then sync_workspace to merge main into your workspace. Resolve each conflicted path, then call update_proposal to send a new version for review.");
        text
    }
}

/// An object hash as its 64 hex characters.
mod hash_text {
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::workspaces::object::ObjectHash;

    pub fn serialize<S: Serializer>(hash: &ObjectHash, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(hash)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ObjectHash, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::workspaces::object::{ObjectKind, hash_object};
    use crate::workspaces::tree::FlatTree;

    fn hash(text: &str) -> ObjectHash {
        hash_object(ObjectKind::Blob, text.as_bytes())
    }

    fn entry(kind: EntryKind, text: &str) -> Option<FlatEntry> {
        Some(FlatEntry {
            kind,
            hash: hash(text),
        })
    }

    fn file(text: &str) -> Option<FlatEntry> {
        entry(EntryKind::File, text)
    }

    fn conflict(
        path: &str,
        kind: ConflictKind,
        sides: [Option<FlatEntry>; 3],
        hunks: Vec<ConflictHunk>,
    ) -> Conflict {
        let [base, ours, theirs] = sides;
        Conflict {
            path: path.as_bytes().to_vec(),
            kind,
            base,
            ours,
            theirs,
            theirs_below: FlatTree::new(),
            hunks,
        }
    }

    fn trees() -> ReportTrees {
        ReportTrees {
            base: hash("base tree"),
            main: hash("main tree"),
            version: hash("version tree"),
        }
    }

    fn report(conflicts: &[Conflict]) -> ConflictReport {
        ConflictReport::new(
            ProposalId::from_number(3).unwrap(),
            2,
            RevisionId::from_number(5).unwrap(),
            RevisionId::from_number(8).unwrap(),
            trees(),
            conflicts,
        )
    }

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| (*line).to_owned()).collect()
    }

    fn text_conflict() -> Conflict {
        conflict(
            "src/lib.rs",
            ConflictKind::BothModifiedText,
            [file("a\nold\n"), file("a\nmine\n"), file("a\nmain\n")],
            vec![ConflictHunk {
                base: lines(&["old\n"]),
                ours: lines(&["mine\n", "more\n"]),
                theirs: Vec::new(),
                base_start: 1,
                ours_start: 1,
                theirs_start: 1,
            }],
        )
    }

    #[test]
    fn a_text_conflict_reports_its_hunks_with_main_and_version_named() {
        let built = report(&[text_conflict()]);
        assert_eq!(built.files.len(), 1);
        let file = &built.files[0];
        assert_eq!(file.path, "src/lib.rs");
        assert_eq!(file.kind, ReportKind::Text);
        assert_eq!(file.base.unwrap().hash, hash("a\nold\n"));
        assert_eq!(file.version.unwrap().hash, hash("a\nmine\n"));
        assert_eq!(file.main.unwrap().hash, hash("a\nmain\n"));
        assert_eq!(
            file.hunks,
            [ReportHunk {
                base: HunkSide {
                    start: 2,
                    count: 1,
                    lines: lines(&["old\n"]),
                },
                main: HunkSide {
                    start: 2,
                    count: 0,
                    lines: Vec::new(),
                },
                version: HunkSide {
                    start: 2,
                    count: 2,
                    lines: lines(&["mine\n", "more\n"]),
                },
            }]
        );
    }

    #[test]
    fn each_other_kind_reports_its_entries_and_no_hunks() {
        let cases = [
            (
                ConflictKind::BothModifiedBinary,
                ReportKind::BothModifiedBinary,
                [file("\0a"), file("\0b"), file("\0c")],
            ),
            (
                ConflictKind::ModifyDelete,
                ReportKind::ModifyDelete,
                [file("old\n"), None, file("main\n")],
            ),
            (
                ConflictKind::AddAdd,
                ReportKind::AddAdd,
                [None, file("mine\n"), file("main\n")],
            ),
            (
                ConflictKind::TypeChange,
                ReportKind::TypeChange,
                [
                    file("old\n"),
                    entry(EntryKind::Symlink, "../x"),
                    entry(EntryKind::Executable, "main\n"),
                ],
            ),
            (
                ConflictKind::TypeChange,
                ReportKind::TypeChange,
                [file("old\n"), None, file("main\n")],
            ),
        ];
        for (kind, expected, sides) in cases {
            let built = report(&[conflict("p", kind, sides, Vec::new())]);
            let file = &built.files[0];
            assert_eq!(file.kind, expected);
            assert_eq!(file.base, sides[0].map(ReportEntry::from));
            assert_eq!(file.version, sides[1].map(ReportEntry::from));
            assert_eq!(file.main, sides[2].map(ReportEntry::from));
            assert!(file.hunks.is_empty());
        }
    }

    #[test]
    fn entry_kinds_keep_their_names() {
        let names: Vec<Value> = [
            EntryKind::File,
            EntryKind::Executable,
            EntryKind::Symlink,
            EntryKind::Directory,
        ]
        .into_iter()
        .map(|kind| {
            serde_json::to_value(ReportEntry::from(FlatEntry {
                kind,
                hash: hash(""),
            }))
            .unwrap()["kind"]
                .clone()
        })
        .collect();
        assert_eq!(
            names,
            [
                json!("file"),
                json!("executable"),
                json!("symlink"),
                json!("directory")
            ]
        );
    }

    #[test]
    fn files_come_in_path_order() {
        let built = report(&[
            conflict(
                "z",
                ConflictKind::AddAdd,
                [None, file("1"), file("2")],
                Vec::new(),
            ),
            text_conflict(),
        ]);
        let paths: Vec<&str> = built.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["src/lib.rs", "z"]);
    }

    #[test]
    fn serde_round_trips() {
        let built = report(&[
            text_conflict(),
            conflict(
                "gone",
                ConflictKind::ModifyDelete,
                [file("a"), file("b"), None],
                Vec::new(),
            ),
        ]);
        let text = serde_json::to_string(&built).unwrap();
        assert_eq!(
            serde_json::from_str::<ConflictReport>(&text).unwrap(),
            built
        );
        assert!(
            serde_json::from_str::<ConflictReport>(&text.replace(&hash("a").to_string(), "zz"))
                .is_err()
        );
    }

    #[test]
    fn field_names_are_stable() {
        let built = report(&[text_conflict()]);
        let value = serde_json::to_value(&built).unwrap();
        let tree = trees();
        let base = hash("a\nold\n").to_string();
        let mine = hash("a\nmine\n").to_string();
        let main = hash("a\nmain\n").to_string();
        assert_eq!(
            value,
            json!({
                "proposal": "P-3",
                "version": 2,
                "base_revision": "r5",
                "main_revision": "r8",
                "trees": {
                    "base": tree.base.to_string(),
                    "main": tree.main.to_string(),
                    "version": tree.version.to_string(),
                },
                "files": [{
                    "path": "src/lib.rs",
                    "kind": "text",
                    "base": {"kind": "file", "hash": base},
                    "main": {"kind": "file", "hash": main},
                    "version": {"kind": "file", "hash": mine},
                    "hunks": [{
                        "base": {"start": 2, "count": 1, "lines": ["old\n"]},
                        "main": {"start": 2, "count": 0, "lines": []},
                        "version": {"start": 2, "count": 2, "lines": ["mine\n", "more\n"]},
                    }],
                }],
            })
        );
    }

    #[test]
    fn a_path_that_is_not_utf8_is_reported_readably() {
        let mut odd = text_conflict();
        odd.path = b"bad\xffname".to_vec();
        assert_eq!(report(&[odd]).files[0].path, "bad\u{fffd}name");
    }

    #[test]
    fn the_author_message_names_five_paths_and_counts_the_rest() {
        let conflicts: Vec<Conflict> = (1..=7)
            .map(|n| {
                conflict(
                    &format!("f{n}"),
                    ConflictKind::AddAdd,
                    [None, file("1"), file("2")],
                    Vec::new(),
                )
            })
            .collect();
        let message = report(&conflicts).author_message();
        assert!(message.starts_with("P-3 v2 conflicts with main at r8"));
        assert!(message.contains("Conflicting paths (7): f1, f2, f3, f4, f5 and 2 more."));
        assert!(!message.contains("f6"));
        assert!(message.contains("get_proposal"));
        assert!(message.contains("sync_workspace"));
    }

    #[test]
    fn the_author_message_holds_no_content() {
        let message = report(&[text_conflict()]).author_message();
        assert!(message.contains("Conflicting paths (1): src/lib.rs."));
        for content in ["old", "mine", "main\n", "more"] {
            assert!(!message.contains(content), "{content:?} in {message:?}");
        }
    }
}
