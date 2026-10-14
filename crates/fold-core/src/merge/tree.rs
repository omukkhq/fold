use std::collections::BTreeSet;

use crate::error::FoldError;
use crate::object::{ObjectHash, ObjectKind, hash_object};
use crate::tree::{EntryKind, FlatEntry, FlatTree};

use super::{ConflictHunk, merge3};

/// Supplies blob content, so the core stays free of I/O.
pub type BlobLoader<'a> = &'a dyn Fn(&ObjectHash) -> Result<Vec<u8>, FoldError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConflictKind {
    BothModifiedText,
    BothModifiedBinary,
    ModifyDelete,
    AddAdd,
    TypeChange,
}

impl ConflictKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BothModifiedText => "both_modified_text",
            Self::BothModifiedBinary => "both_modified_binary",
            Self::ModifyDelete => "modify_delete",
            Self::AddAdd => "add_add",
            Self::TypeChange => "type_change",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub path: Vec<u8>,
    pub kind: ConflictKind,
    pub base: Option<FlatEntry>,
    pub ours: Option<FlatEntry>,
    pub theirs: Option<FlatEntry>,
    /// Main's entries below `path`, keyed by their path relative to it: main's side of `path` when main has a directory there, which sync writes as the directory `<path>.fold-main`. Empty when main has a file, a symlink or nothing at `path`.
    pub theirs_below: FlatTree,
    /// Non-empty only for `BothModifiedText`.
    pub hunks: Vec<ConflictHunk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeMerge {
    /// Every path that resolved. A conflicted path is absent.
    pub merged: FlatTree,
    /// Each line-merged file, for the caller to store before it refers to the hash.
    pub new_blobs: Vec<(ObjectHash, Vec<u8>)>,
    /// In path order.
    pub conflicts: Vec<Conflict>,
}

/// Three-way merge of flat trees, path by path. `ours` is the workspace or proposal version and `theirs` is main. A rename is a delete plus an add here. A failing `load` fails the merge, since a half-merged result would hide a conflict.
pub fn merge_trees(
    base: &FlatTree,
    ours: &FlatTree,
    theirs: &FlatTree,
    load: BlobLoader<'_>,
) -> Result<TreeMerge, FoldError> {
    let paths: BTreeSet<&Vec<u8>> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .collect();
    let mut result = TreeMerge {
        merged: FlatTree::new(),
        new_blobs: Vec::new(),
        conflicts: Vec::new(),
    };
    for path in paths {
        let triple = Triple {
            base: base.get(path).copied(),
            ours: ours.get(path).copied(),
            theirs: theirs.get(path).copied(),
        };
        match merge_path(&triple, load)? {
            Resolved::Entry(entry, blob) => {
                result.merged.insert(path.clone(), entry);
                if let Some(blob) = blob
                    && !result.new_blobs.iter().any(|(hash, _)| *hash == blob.0)
                {
                    result.new_blobs.push(blob);
                }
            }
            Resolved::Absent => {}
            Resolved::Conflict(kind, hunks) => {
                result.conflicts.push(triple.conflict(path, kind, hunks))
            }
        }
    }
    resolve_file_directory_clashes(base, ours, theirs, &mut result);
    collapse_nested_conflicts(theirs, &mut result);
    Ok(result)
}

#[derive(Clone, Copy)]
struct Triple {
    base: Option<FlatEntry>,
    ours: Option<FlatEntry>,
    theirs: Option<FlatEntry>,
}

impl Triple {
    fn conflict(&self, path: &[u8], kind: ConflictKind, hunks: Vec<ConflictHunk>) -> Conflict {
        Conflict {
            path: path.to_vec(),
            kind,
            base: self.base,
            ours: self.ours,
            theirs: self.theirs,
            theirs_below: FlatTree::new(),
            hunks,
        }
    }
}

enum Resolved {
    Entry(FlatEntry, Option<(ObjectHash, Vec<u8>)>),
    Absent,
    Conflict(ConflictKind, Vec<ConflictHunk>),
}

fn taken(entry: Option<FlatEntry>) -> Resolved {
    entry.map_or(Resolved::Absent, |entry| Resolved::Entry(entry, None))
}

fn is_regular(kind: EntryKind) -> bool {
    matches!(kind, EntryKind::File | EntryKind::Executable)
}

fn merge_path(triple: &Triple, load: BlobLoader<'_>) -> Result<Resolved, FoldError> {
    let Triple { base, ours, theirs } = *triple;
    if ours == theirs {
        return Ok(taken(ours));
    }
    if ours == base {
        return Ok(taken(theirs));
    }
    if theirs == base {
        return Ok(taken(ours));
    }
    let (Some(base), Some(ours), Some(theirs)) = (base, ours, theirs) else {
        // Neither side equals base and the sides differ: with no base both added, otherwise one deleted what the other changed.
        let kind = if base.is_none() {
            ConflictKind::AddAdd
        } else {
            ConflictKind::ModifyDelete
        };
        return Ok(Resolved::Conflict(kind, Vec::new()));
    };
    if is_regular(ours.kind) != is_regular(theirs.kind) {
        return Ok(Resolved::Conflict(ConflictKind::TypeChange, Vec::new()));
    }
    // Two symlinks that differ, or a symlink base that both sides turned into different files, have no text to merge and no mode to pick.
    if !is_regular(ours.kind) || !is_regular(base.kind) {
        return Ok(Resolved::Conflict(
            ConflictKind::BothModifiedBinary,
            Vec::new(),
        ));
    }
    // Between regular files a mode change is independent of content: the side that moved off the base mode wins.
    let kind = if ours.kind == base.kind {
        theirs.kind
    } else {
        ours.kind
    };
    if ours.hash == theirs.hash {
        return Ok(Resolved::Entry(
            FlatEntry {
                kind,
                hash: ours.hash,
            },
            None,
        ));
    }
    if ours.hash == base.hash {
        return Ok(Resolved::Entry(
            FlatEntry {
                kind,
                hash: theirs.hash,
            },
            None,
        ));
    }
    if theirs.hash == base.hash {
        return Ok(Resolved::Entry(
            FlatEntry {
                kind,
                hash: ours.hash,
            },
            None,
        ));
    }
    let base_bytes = load(&base.hash)?;
    let ours_bytes = load(&ours.hash)?;
    let theirs_bytes = load(&theirs.hash)?;
    let (Some(base_text), Some(ours_text), Some(theirs_text)) = (
        as_text(&base_bytes),
        as_text(&ours_bytes),
        as_text(&theirs_bytes),
    ) else {
        return Ok(Resolved::Conflict(
            ConflictKind::BothModifiedBinary,
            Vec::new(),
        ));
    };
    let merged = merge3(base_text, ours_text, theirs_text);
    if merged.is_clean() {
        let bytes = merged.into_text().unwrap_or_default().into_bytes();
        let hash = hash_object(ObjectKind::Blob, &bytes);
        return Ok(Resolved::Entry(
            FlatEntry { kind, hash },
            Some((hash, bytes)),
        ));
    }
    let hunks = merged
        .segments
        .into_iter()
        .filter_map(|segment| match segment {
            super::Segment::Conflict(hunk) => Some(hunk),
            super::Segment::Resolved(_) => None,
        })
        .collect();
    Ok(Resolved::Conflict(ConflictKind::BothModifiedText, hunks))
}

/// Text means no zero byte in the first 8 KiB and valid UTF-8 throughout.
fn as_text(bytes: &[u8]) -> Option<&str> {
    let head = &bytes[..bytes.len().min(8192)];
    if head.contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

/// Conflict files are written beside their path, and a merged file is written at its path, so a path and one of its parent directories can never both be a file. Whichever resolved file sits on the other side of such a clash becomes a `TypeChange` conflict, until no merged path is a parent or a child of another merged path or of a conflict path. Demoting is the only direction a path moves, so the loop ends.
fn resolve_file_directory_clashes(
    base: &FlatTree,
    ours: &FlatTree,
    theirs: &FlatTree,
    result: &mut TreeMerge,
) {
    let mut demoted_any = false;
    loop {
        let conflicted: BTreeSet<&[u8]> =
            result.conflicts.iter().map(|c| c.path.as_slice()).collect();
        let mut demote = BTreeSet::new();
        for path in result.merged.keys() {
            for index in directory_prefixes(path) {
                if result.merged.contains_key(&path[..index]) {
                    demote.insert(path[..index].to_vec());
                }
                if conflicted.contains(&path[..index]) {
                    demote.insert(path.clone());
                }
            }
        }
        for path in &conflicted {
            for index in directory_prefixes(path) {
                if result.merged.contains_key(&path[..index]) {
                    demote.insert(path[..index].to_vec());
                }
            }
        }
        if demote.is_empty() {
            break;
        }
        demoted_any = true;
        for path in demote {
            result.merged.remove(&path);
            let triple = Triple {
                base: base.get(&path).copied(),
                ours: ours.get(&path).copied(),
                theirs: theirs.get(&path).copied(),
            };
            result
                .conflicts
                .push(triple.conflict(&path, ConflictKind::TypeChange, Vec::new()));
        }
    }
    if !demoted_any {
        return;
    }
    result.conflicts.sort_by(|a, b| a.path.cmp(&b.path));
    let merged = &result.merged;
    result
        .new_blobs
        .retain(|(hash, _)| merged.values().any(|entry| entry.hash == *hash));
}

/// A conflict keeps the workspace's side at its path and writes main's side beside it, so a conflict below another conflict would have to be written inside the other's file. Only the top conflict of a nested set stays: it becomes a `TypeChange`, since one side has a file there and another a directory, and every conflict carries main's whole side of its path.
fn collapse_nested_conflicts(theirs: &FlatTree, result: &mut TreeMerge) {
    let paths: BTreeSet<Vec<u8>> = result.conflicts.iter().map(|c| c.path.clone()).collect();
    let mut tops = BTreeSet::new();
    result.conflicts.retain(|conflict| {
        let top = directory_prefixes(&conflict.path)
            .find(|&index| paths.contains(&conflict.path[..index]));
        if let Some(index) = top {
            tops.insert(conflict.path[..index].to_vec());
        }
        top.is_none()
    });
    for conflict in &mut result.conflicts {
        if tops.contains(&conflict.path) {
            conflict.kind = ConflictKind::TypeChange;
            conflict.hunks.clear();
        }
        conflict.theirs_below = entries_below(theirs, &conflict.path);
    }
}

/// The entries of `tree` below `path`, keyed relative to it.
fn entries_below(tree: &FlatTree, path: &[u8]) -> FlatTree {
    let mut prefix = path.to_vec();
    prefix.push(b'/');
    tree.range(prefix.clone()..)
        .take_while(|(other, _)| other.starts_with(&prefix))
        .map(|(other, entry)| (other[prefix.len()..].to_vec(), *entry))
        .collect()
}

/// The lengths of the proper directory prefixes of `path`.
fn directory_prefixes(path: &[u8]) -> impl Iterator<Item = usize> + '_ {
    path.iter()
        .enumerate()
        .filter_map(|(index, byte)| (*byte == b'/').then_some(index))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use proptest::collection::vec;
    use proptest::prelude::*;

    use super::*;
    use crate::error::codes;

    #[derive(Default)]
    struct Blobs(RefCell<HashMap<ObjectHash, Vec<u8>>>);

    impl Blobs {
        fn add(&self, bytes: &[u8]) -> ObjectHash {
            let hash = hash_object(ObjectKind::Blob, bytes);
            self.0.borrow_mut().insert(hash, bytes.to_vec());
            hash
        }

        fn entry(&self, kind: EntryKind, text: &str) -> FlatEntry {
            FlatEntry {
                kind,
                hash: self.add(text.as_bytes()),
            }
        }

        fn file(&self, text: &str) -> FlatEntry {
            self.entry(EntryKind::File, text)
        }

        fn merge(
            &self,
            base: &FlatTree,
            ours: &FlatTree,
            theirs: &FlatTree,
        ) -> Result<TreeMerge, FoldError> {
            let load = |hash: &ObjectHash| {
                self.0
                    .borrow()
                    .get(hash)
                    .cloned()
                    .ok_or_else(|| FoldError::new(codes::INVALID_HASH, "missing blob"))
            };
            merge_trees(base, ours, theirs, &load)
        }
    }

    fn tree(entries: &[(&str, Option<FlatEntry>)]) -> FlatTree {
        entries
            .iter()
            .filter_map(|(path, entry)| entry.map(|entry| (path.as_bytes().to_vec(), entry)))
            .collect()
    }

    fn kinds(merge: &TreeMerge) -> Vec<(&[u8], ConflictKind)> {
        merge
            .conflicts
            .iter()
            .map(|c| (c.path.as_slice(), c.kind))
            .collect()
    }

    #[test]
    fn equal_sides_take_either_including_both_absent() {
        let blobs = Blobs::default();
        let x = blobs.file("x\n");
        let base = tree(&[("a", Some(blobs.file("old\n"))), ("gone", Some(x))]);
        let side = tree(&[("a", Some(x))]);
        let merge = blobs.merge(&base, &side, &side).unwrap();
        assert_eq!(merge.merged, side);
        assert!(merge.conflicts.is_empty() && merge.new_blobs.is_empty());
    }

    #[test]
    fn an_unchanged_side_takes_the_other_including_deletes() {
        let blobs = Blobs::default();
        let base = tree(&[
            ("a", Some(blobs.file("1\n"))),
            ("b", Some(blobs.file("2\n"))),
        ]);
        let changed = tree(&[
            ("a", Some(blobs.file("3\n"))),
            ("c", Some(blobs.file("4\n"))),
        ]);
        for (ours, theirs) in [(&base, &changed), (&changed, &base)] {
            let merge = blobs.merge(&base, ours, theirs).unwrap();
            assert_eq!(merge.merged, changed);
            assert!(merge.conflicts.is_empty());
        }
    }

    #[test]
    fn two_different_additions_are_add_add() {
        let blobs = Blobs::default();
        let ours = tree(&[("n", Some(blobs.file("1\n")))]);
        let theirs = tree(&[("n", Some(blobs.file("2\n")))]);
        let merge = blobs.merge(&FlatTree::new(), &ours, &theirs).unwrap();
        assert_eq!(kinds(&merge), [(b"n".as_slice(), ConflictKind::AddAdd)]);
        assert!(merge.merged.is_empty());
        let conflict = &merge.conflicts[0];
        assert_eq!(conflict.base, None);
        assert_eq!(conflict.ours, ours.get(b"n".as_slice()).copied());
        assert_eq!(conflict.theirs, theirs.get(b"n".as_slice()).copied());
        assert!(conflict.hunks.is_empty());
    }

    #[test]
    fn modify_against_delete_conflicts_in_both_directions() {
        let blobs = Blobs::default();
        let base = tree(&[("f", Some(blobs.file("1\n")))]);
        let edited = tree(&[("f", Some(blobs.file("2\n")))]);
        let deleted = FlatTree::new();
        for (ours, theirs) in [(&edited, &deleted), (&deleted, &edited)] {
            let merge = blobs.merge(&base, ours, theirs).unwrap();
            assert_eq!(
                kinds(&merge),
                [(b"f".as_slice(), ConflictKind::ModifyDelete)]
            );
            assert!(merge.merged.is_empty());
        }
    }

    #[test]
    fn a_file_against_a_symlink_is_a_type_change() {
        let blobs = Blobs::default();
        let base = tree(&[("l", Some(blobs.file("t\n")))]);
        let file = tree(&[("l", Some(blobs.file("changed\n")))]);
        let link = tree(&[("l", Some(blobs.entry(EntryKind::Symlink, "target")))]);
        let merge = blobs.merge(&base, &file, &link).unwrap();
        assert_eq!(kinds(&merge), [(b"l".as_slice(), ConflictKind::TypeChange)]);
        let added = blobs.merge(&FlatTree::new(), &file, &link).unwrap();
        assert_eq!(kinds(&added), [(b"l".as_slice(), ConflictKind::AddAdd)]);
    }

    #[test]
    fn a_file_against_a_directory_of_the_same_name_conflicts_at_the_file() {
        let blobs = Blobs::default();
        let base = FlatTree::new();
        let ours = tree(&[
            ("a", Some(blobs.file("file\n"))),
            ("z", Some(blobs.file("z\n"))),
        ]);
        let theirs = tree(&[("a/b", Some(blobs.file("nested\n")))]);
        let merge = blobs.merge(&base, &ours, &theirs).unwrap();
        assert_eq!(kinds(&merge), [(b"a".as_slice(), ConflictKind::TypeChange)]);
        let paths: Vec<_> = merge.merged.keys().map(|p| p.as_slice()).collect();
        assert_eq!(paths, [b"z".as_slice()]);
        assert_eq!(merge.conflicts[0].ours, ours.get(b"a".as_slice()).copied());
        assert_eq!(merge.conflicts[0].theirs, None);
        assert_eq!(
            merge.conflicts[0].theirs_below,
            tree(&[("b", theirs.get(b"a/b".as_slice()).copied())])
        );
        assert_writable(&merge);
    }

    /// No conflict path or merged path is below a merged file or another conflict, so sync can write every merged file and every `.fold-main` side.
    fn assert_writable(merge: &TreeMerge) {
        for conflict in &merge.conflicts {
            for index in directory_prefixes(&conflict.path) {
                assert!(!merge.merged.contains_key(&conflict.path[..index]));
                assert!(
                    !merge
                        .conflicts
                        .iter()
                        .any(|other| other.path == conflict.path[..index])
                );
            }
            assert!(!has_child(&merge.merged, &conflict.path));
        }
    }

    /// Checks the one conflict at `a` that each side order gives: main's side is `theirs` at `a` when main has a file there, otherwise `theirs_below`.
    fn assert_collapsed(merge: &TreeMerge, theirs: &FlatTree, below: &[(&str, &str)]) {
        assert_eq!(kinds(merge), [(b"a".as_slice(), ConflictKind::TypeChange)]);
        let conflict = &merge.conflicts[0];
        assert_eq!(conflict.theirs, theirs.get(b"a".as_slice()).copied());
        let expected: FlatTree = below
            .iter()
            .map(|(relative, full)| (relative.as_bytes().to_vec(), theirs[full.as_bytes()]))
            .collect();
        assert_eq!(conflict.theirs_below, expected);
        assert!(conflict.hunks.is_empty());
        assert_writable(merge);
    }

    #[test]
    fn a_conflict_below_a_merged_file_collapses_into_a_conflict_at_the_file() {
        let blobs = Blobs::default();
        let base = tree(&[("a/b", Some(blobs.file("1\n")))]);
        let ours = tree(&[("a", Some(blobs.file("file\n")))]);
        let theirs = tree(&[("a/b", Some(blobs.file("2\n")))]);
        let merge = blobs.merge(&base, &ours, &theirs).unwrap();
        assert!(merge.merged.is_empty() && merge.new_blobs.is_empty());
        assert_collapsed(&merge, &theirs, &[("b", "a/b")]);
        let swapped = blobs.merge(&base, &theirs, &ours).unwrap();
        assert!(swapped.merged.is_empty());
        assert_collapsed(&swapped, &ours, &[]);
    }

    #[test]
    fn a_merged_path_below_a_conflicted_file_collapses_into_that_conflict() {
        let blobs = Blobs::default();
        let base = tree(&[("a", Some(blobs.file("1\n")))]);
        let edited = tree(&[("a", Some(blobs.file("2\n")))]);
        let nested = tree(&[("a/b", Some(blobs.file("x\n")))]);
        let merge = blobs.merge(&base, &edited, &nested).unwrap();
        assert!(merge.merged.is_empty());
        assert_collapsed(&merge, &nested, &[("b", "a/b")]);
        let swapped = blobs.merge(&base, &nested, &edited).unwrap();
        assert!(swapped.merged.is_empty());
        assert_collapsed(&swapped, &edited, &[]);
    }

    #[test]
    fn a_deeper_chain_collapses_into_its_top_conflict_with_main_s_whole_subtree() {
        let blobs = Blobs::default();
        let z = Some(blobs.file("z\n"));
        let base = tree(&[("x/a/b/c", Some(blobs.file("1\n"))), ("x/z", z)]);
        let ours = tree(&[("x/a", Some(blobs.file("file\n"))), ("x/z", z)]);
        let theirs = tree(&[
            ("x/a/b/c", Some(blobs.file("2\n"))),
            ("x/a/d", Some(blobs.file("d\n"))),
            ("x/z", z),
        ]);
        for (o, t, below) in [
            (&ours, &theirs, vec![("b/c", "x/a/b/c"), ("d", "x/a/d")]),
            (&theirs, &ours, vec![]),
        ] {
            let merge = blobs.merge(&base, o, t).unwrap();
            assert_eq!(
                kinds(&merge),
                [(b"x/a".as_slice(), ConflictKind::TypeChange)]
            );
            let paths: Vec<_> = merge.merged.keys().map(|p| p.as_slice()).collect();
            assert_eq!(paths, [b"x/z".as_slice()]);
            let conflict = &merge.conflicts[0];
            assert_eq!(conflict.theirs, t.get(b"x/a".as_slice()).copied());
            let expected: FlatTree = below
                .iter()
                .map(|(relative, full)| (relative.as_bytes().to_vec(), t[full.as_bytes()]))
                .collect();
            assert_eq!(conflict.theirs_below, expected);
            assert_writable(&merge);
        }
    }

    #[test]
    fn a_clash_drops_the_merged_blob_of_the_conflicted_file() {
        let blobs = Blobs::default();
        let base = tree(&[("a", Some(blobs.file("1\n2\n3\n4\n5\n6\n7\n")))]);
        let ours = tree(&[("a", Some(blobs.file("one\n2\n3\n4\n5\n6\n7\n")))]);
        let theirs = tree(&[
            ("a", Some(blobs.file("1\n2\n3\n4\n5\n6\nseven\n"))),
            ("a/b", Some(blobs.file("x\n"))),
        ]);
        let merge = blobs.merge(&base, &ours, &theirs).unwrap();
        assert_eq!(kinds(&merge), [(b"a".as_slice(), ConflictKind::TypeChange)]);
        assert!(merge.new_blobs.is_empty());
    }

    #[test]
    fn a_mode_change_on_one_side_merges_with_the_content_change_on_the_other() {
        let blobs = Blobs::default();
        let base_entry = blobs.file("1\n");
        let base = tree(&[("run", Some(base_entry))]);
        let exec = FlatEntry {
            kind: EntryKind::Executable,
            hash: base_entry.hash,
        };
        let edited = blobs.file("2\n");
        let ours = tree(&[("run", Some(exec))]);
        let theirs = tree(&[("run", Some(edited))]);
        for (o, t) in [(&ours, &theirs), (&theirs, &ours)] {
            let merge = blobs.merge(&base, o, t).unwrap();
            assert!(merge.conflicts.is_empty());
            assert_eq!(
                merge.merged[b"run".as_slice()],
                FlatEntry {
                    kind: EntryKind::Executable,
                    hash: edited.hash
                }
            );
        }
        let only_mode = blobs.merge(&base, &ours, &base).unwrap();
        assert_eq!(only_mode.merged, ours);
    }

    #[test]
    fn a_mode_change_beside_a_text_merge_keeps_the_mode() {
        let blobs = Blobs::default();
        let text = "1\n2\n3\n4\n5\n6\n7\n";
        let base = tree(&[("run", Some(blobs.file(text)))]);
        let ours = tree(&[(
            "run",
            Some(blobs.entry(EntryKind::Executable, "one\n2\n3\n4\n5\n6\n7\n")),
        )]);
        let theirs = tree(&[("run", Some(blobs.file("1\n2\n3\n4\n5\n6\nseven\n")))]);
        let merge = blobs.merge(&base, &ours, &theirs).unwrap();
        assert!(merge.conflicts.is_empty());
        assert_eq!(merge.merged[b"run".as_slice()].kind, EntryKind::Executable);
        let swapped = blobs.merge(&base, &theirs, &ours).unwrap();
        assert_eq!(swapped.merged, merge.merged);
    }

    #[test]
    fn identical_edits_with_different_modes_take_the_changed_mode() {
        let blobs = Blobs::default();
        let base = tree(&[("f", Some(blobs.file("1\n")))]);
        let ours = tree(&[("f", Some(blobs.entry(EntryKind::Executable, "2\n")))]);
        let theirs = tree(&[("f", Some(blobs.file("2\n")))]);
        let merge = blobs.merge(&base, &ours, &theirs).unwrap();
        assert!(merge.conflicts.is_empty() && merge.new_blobs.is_empty());
        assert_eq!(merge.merged, ours);
    }

    #[test]
    fn two_binary_edits_conflict_as_binary() {
        let blobs = Blobs::default();
        let bin = |bytes: &[u8]| FlatEntry {
            kind: EntryKind::File,
            hash: blobs.add(bytes),
        };
        let base = tree(&[("b", Some(bin(b"\0one")))]);
        let ours = tree(&[("b", Some(bin(b"\0two")))]);
        let theirs = tree(&[("b", Some(bin(b"\0three")))]);
        let merge = blobs.merge(&base, &ours, &theirs).unwrap();
        assert_eq!(
            kinds(&merge),
            [(b"b".as_slice(), ConflictKind::BothModifiedBinary)]
        );
        assert!(merge.conflicts[0].hunks.is_empty());
    }

    #[test]
    fn invalid_utf8_and_a_zero_byte_past_the_probe_decide_text() {
        assert!(as_text(b"plain\n").is_some());
        assert!(as_text(&[0xff, 0xfe]).is_none());
        let mut late = vec![b'a'; 8192];
        late.push(0);
        assert!(as_text(&late).is_some());
        late.insert(0, 0);
        assert!(as_text(&late).is_none());
    }

    #[test]
    fn a_binary_among_text_conflicts_as_binary() {
        let blobs = Blobs::default();
        let base = tree(&[("b", Some(blobs.file("text\n")))]);
        let ours = tree(&[("b", Some(blobs.file("ours\n")))]);
        let theirs = tree(&[(
            "b",
            Some(FlatEntry {
                kind: EntryKind::File,
                hash: blobs.add(&[0, 1, 2]),
            }),
        )]);
        let merge = blobs.merge(&base, &ours, &theirs).unwrap();
        assert_eq!(
            kinds(&merge),
            [(b"b".as_slice(), ConflictKind::BothModifiedBinary)]
        );
    }

    #[test]
    fn a_symlink_base_turned_into_different_files_conflicts_in_both_orders() {
        let blobs = Blobs::default();
        let target = blobs.add(b"t");
        let base = tree(&[(
            "l",
            Some(FlatEntry {
                kind: EntryKind::Symlink,
                hash: target,
            }),
        )]);
        let exec = tree(&[(
            "l",
            Some(FlatEntry {
                kind: EntryKind::Executable,
                hash: target,
            }),
        )]);
        let file = tree(&[(
            "l",
            Some(FlatEntry {
                kind: EntryKind::File,
                hash: target,
            }),
        )]);
        for (ours, theirs) in [(&exec, &file), (&file, &exec)] {
            let merge = blobs.merge(&base, ours, theirs).unwrap();
            assert_eq!(
                kinds(&merge),
                [(b"l".as_slice(), ConflictKind::BothModifiedBinary)]
            );
        }
    }

    #[test]
    fn two_changed_symlinks_conflict_as_binary() {
        let blobs = Blobs::default();
        let link = |target: &str| Some(blobs.entry(EntryKind::Symlink, target));
        let merge = blobs
            .merge(
                &tree(&[("l", link("a"))]),
                &tree(&[("l", link("b"))]),
                &tree(&[("l", link("c"))]),
            )
            .unwrap();
        assert_eq!(
            kinds(&merge),
            [(b"l".as_slice(), ConflictKind::BothModifiedBinary)]
        );
    }

    #[test]
    fn a_symlink_base_edited_into_files_conflicts_as_binary() {
        let blobs = Blobs::default();
        let base = tree(&[("l", Some(blobs.entry(EntryKind::Symlink, "t")))]);
        let ours = tree(&[("l", Some(blobs.file("a\n")))]);
        let theirs = tree(&[("l", Some(blobs.file("b\n")))]);
        let merge = blobs.merge(&base, &ours, &theirs).unwrap();
        assert_eq!(
            kinds(&merge),
            [(b"l".as_slice(), ConflictKind::BothModifiedBinary)]
        );
    }

    #[test]
    fn edits_in_different_places_become_a_new_blob() {
        let blobs = Blobs::default();
        let base = tree(&[("f", Some(blobs.file("1\n2\n3\n4\n5\n6\n7\n")))]);
        let ours = tree(&[("f", Some(blobs.file("one\n2\n3\n4\n5\n6\n7\n")))]);
        let theirs = tree(&[("f", Some(blobs.file("1\n2\n3\n4\n5\n6\nseven\n")))]);
        let merge = blobs.merge(&base, &ours, &theirs).unwrap();
        let bytes = b"one\n2\n3\n4\n5\n6\nseven\n".to_vec();
        let hash = hash_object(ObjectKind::Blob, &bytes);
        assert_eq!(merge.new_blobs, [(hash, bytes)]);
        assert_eq!(
            merge.merged[b"f".as_slice()],
            FlatEntry {
                kind: EntryKind::File,
                hash
            }
        );
        assert!(merge.conflicts.is_empty());
    }

    #[test]
    fn identical_merged_content_stores_one_blob() {
        let blobs = Blobs::default();
        let base_text = "1\n2\n3\n4\n5\n6\n7\n";
        let base = tree(&[
            ("a", Some(blobs.file(base_text))),
            ("b", Some(blobs.file(base_text))),
        ]);
        let ours = tree(&[
            ("a", Some(blobs.file("one\n2\n3\n4\n5\n6\n7\n"))),
            ("b", Some(blobs.file("one\n2\n3\n4\n5\n6\n7\n"))),
        ]);
        let theirs = tree(&[
            ("a", Some(blobs.file("1\n2\n3\n4\n5\n6\nseven\n"))),
            ("b", Some(blobs.file("1\n2\n3\n4\n5\n6\nseven\n"))),
        ]);
        let merge = blobs.merge(&base, &ours, &theirs).unwrap();
        assert_eq!(merge.new_blobs.len(), 1);
        assert_eq!(merge.merged.len(), 2);
    }

    #[test]
    fn edits_on_the_same_lines_carry_all_three_versions() {
        let blobs = Blobs::default();
        let base = tree(&[
            ("f", Some(blobs.file("a\nold\nb\n"))),
            ("ok", Some(blobs.file("1\n"))),
        ]);
        let ours = tree(&[
            ("f", Some(blobs.file("a\nmine\nb\n"))),
            ("ok", Some(blobs.file("1\n"))),
        ]);
        let theirs = tree(&[
            ("f", Some(blobs.file("a\nmain\nb\n"))),
            ("ok", Some(blobs.file("2\n"))),
        ]);
        let merge = blobs.merge(&base, &ours, &theirs).unwrap();
        assert_eq!(
            kinds(&merge),
            [(b"f".as_slice(), ConflictKind::BothModifiedText)]
        );
        assert!(!merge.merged.contains_key(b"f".as_slice()));
        assert!(merge.merged.contains_key(b"ok".as_slice()));
        assert_eq!(
            merge.conflicts[0].hunks,
            [ConflictHunk {
                base: vec!["old\n".into()],
                ours: vec!["mine\n".into()],
                theirs: vec!["main\n".into()],
            }]
        );
    }

    #[test]
    fn conflicts_come_in_path_order_and_runs_agree() {
        let blobs = Blobs::default();
        let paths = ["z", "m", "a/b", "b"];
        let mut base = FlatTree::new();
        let mut ours = FlatTree::new();
        let mut theirs = FlatTree::new();
        for path in paths {
            base.insert(path.as_bytes().to_vec(), blobs.file("0\n"));
            ours.insert(path.as_bytes().to_vec(), blobs.file("1\n"));
            theirs.insert(path.as_bytes().to_vec(), blobs.file("2\n"));
        }
        let first = blobs.merge(&base, &ours, &theirs).unwrap();
        let second = blobs.merge(&base, &ours, &theirs).unwrap();
        assert_eq!(first, second);
        let listed: Vec<_> = first.conflicts.iter().map(|c| c.path.clone()).collect();
        let mut sorted = listed.clone();
        sorted.sort();
        assert_eq!(listed, sorted);
        assert_eq!(listed.len(), 4);
    }

    #[test]
    fn a_failing_loader_fails_the_merge() {
        let blobs = Blobs::default();
        let missing = |hash: &ObjectHash| {
            let _ = hash;
            Err(FoldError::new(codes::INVALID_HASH, "unreadable"))
        };
        let base = tree(&[("f", Some(blobs.file("1\n")))]);
        let ours = tree(&[("f", Some(blobs.file("2\n")))]);
        let theirs = tree(&[("f", Some(blobs.file("3\n")))]);
        let error = merge_trees(&base, &ours, &theirs, &missing).unwrap_err();
        assert_eq!(error.code(), codes::INVALID_HASH);
    }

    #[test]
    fn kind_names_are_stable() {
        let names: Vec<_> = [
            ConflictKind::BothModifiedText,
            ConflictKind::BothModifiedBinary,
            ConflictKind::ModifyDelete,
            ConflictKind::AddAdd,
            ConflictKind::TypeChange,
        ]
        .into_iter()
        .map(ConflictKind::as_str)
        .collect();
        assert_eq!(
            names,
            [
                "both_modified_text",
                "both_modified_binary",
                "modify_delete",
                "add_add",
                "type_change"
            ]
        );
    }

    /// `d` is also the directory of `d/x` and `d/y`, and `e` and `e/f` are directories of `e/f/g`, so merges over these names can produce file-directory clashes, including ones that a demotion carries more than one level down.
    const NAMES: [&str; 9] = ["a", "b", "c", "d", "d/x", "d/y", "e/f/g", "e", "e/f"];
    /// No name here is the directory of another, so trees over them never clash.
    const DISJOINT: [&str; 6] = ["a", "b", "c", "d/x", "d/y", "e/f/g"];

    fn content() -> impl Strategy<Value = Vec<u8>> {
        prop_oneof![
            8 => vec(prop::sample::select(vec!["a\n", "b\n", "c\n", "d\n", "e\n"]), 0..6)
                .prop_map(|lines| lines.concat().into_bytes()),
            1 => Just(vec![0u8, 1, 2]),
        ]
    }

    fn entry_kind() -> impl Strategy<Value = EntryKind> {
        prop_oneof![
            6 => Just(EntryKind::File),
            2 => Just(EntryKind::Executable),
            1 => Just(EntryKind::Symlink),
        ]
    }

    type Spec = Vec<Option<(EntryKind, Vec<u8>)>>;

    fn spec(len: usize) -> impl Strategy<Value = Spec> {
        vec(prop::option::of((entry_kind(), content())), len)
    }

    fn build(blobs: &Blobs, names: &[&str], spec: &Spec) -> FlatTree {
        spec.iter()
            .zip(names)
            .filter_map(|(item, name)| {
                item.as_ref().map(|(kind, bytes)| {
                    (
                        name.as_bytes().to_vec(),
                        FlatEntry {
                            kind: *kind,
                            hash: blobs.add(bytes),
                        },
                    )
                })
            })
            .collect()
    }

    fn merged_of(blobs: &Blobs, base: &FlatTree, ours: &FlatTree, theirs: &FlatTree) -> TreeMerge {
        blobs.merge(base, ours, theirs).unwrap()
    }

    /// A built tree over `NAMES` with every clashing path dropped, so it is a valid flat tree.
    fn valid(blobs: &Blobs, spec: &Spec) -> FlatTree {
        without_clashes(build(blobs, &NAMES, spec))
    }

    fn has_child(tree: &FlatTree, path: &[u8]) -> bool {
        tree.keys().any(|other| {
            other.len() > path.len() && other.starts_with(path) && other[path.len()] == b'/'
        })
    }

    fn matches_entries(conflict: &Conflict, sides: [&FlatTree; 3]) -> bool {
        let regular = |e: &Option<FlatEntry>| e.is_some_and(|e| is_regular(e.kind));
        match conflict.kind {
            ConflictKind::AddAdd => {
                conflict.base.is_none()
                    && conflict.ours.is_some()
                    && conflict.theirs.is_some()
                    && conflict.ours != conflict.theirs
                    && conflict.hunks.is_empty()
            }
            ConflictKind::ModifyDelete => {
                conflict.base.is_some()
                    && (conflict.ours.is_none() != conflict.theirs.is_none())
                    && conflict.hunks.is_empty()
            }
            ConflictKind::TypeChange => {
                conflict.hunks.is_empty()
                    && (conflict.ours.is_some()
                        && conflict.theirs.is_some()
                        && regular(&conflict.ours) != regular(&conflict.theirs)
                        || sides.iter().any(|side| has_child(side, &conflict.path)))
            }
            ConflictKind::BothModifiedText => {
                !conflict.hunks.is_empty()
                    && regular(&conflict.base)
                    && regular(&conflict.ours)
                    && regular(&conflict.theirs)
            }
            ConflictKind::BothModifiedBinary => {
                conflict.hunks.is_empty()
                    && conflict.base.is_some()
                    && conflict.ours.is_some()
                    && conflict.theirs.is_some()
                    && regular(&conflict.ours) == regular(&conflict.theirs)
            }
        }
    }

    /// Drops every path that is the directory prefix of another, so a generated tree is a valid flat tree.
    fn without_clashes(mut tree: FlatTree) -> FlatTree {
        let clashing: Vec<Vec<u8>> = tree
            .keys()
            .filter(|path| has_child(&tree, path))
            .cloned()
            .collect();
        for path in clashing {
            tree.remove(&path);
        }
        tree
    }

    #[derive(Debug, Clone)]
    enum Op {
        Keep,
        Delete,
        Replace,
        InsertAfter,
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            3 => Just(Op::Keep),
            1 => Just(Op::Delete),
            1 => Just(Op::Replace),
            1 => Just(Op::InsertAfter),
        ]
    }

    /// Applies one side's edits to `base`; added lines carry the side's tag, so every added line is unique. Returns the text, the lines it added and the base lines it removed.
    fn edit(base: &[String], ops: &[Op], tag: &str) -> (String, Vec<String>, Vec<String>) {
        let (mut text, mut added, mut removed) = (String::new(), Vec::new(), Vec::new());
        for (index, line) in base.iter().enumerate() {
            let fresh = format!("{tag}{index}\n");
            match ops.get(index).unwrap_or(&Op::Keep) {
                Op::Keep => text.push_str(line),
                Op::Delete => removed.push(line.clone()),
                Op::Replace => {
                    removed.push(line.clone());
                    text.push_str(&fresh);
                    added.push(fresh);
                }
                Op::InsertAfter => {
                    text.push_str(line);
                    text.push_str(&fresh);
                    added.push(fresh);
                }
            }
        }
        (text, added, removed)
    }

    fn single(blobs: &Blobs, text: &str) -> FlatTree {
        tree(&[("f", Some(blobs.file(text)))])
    }

    proptest! {
        #[test]
        fn merging_against_an_unchanged_side_gives_the_changed_side(b in spec(NAMES.len()), x in spec(NAMES.len())) {
            let blobs = Blobs::default();
            let base = valid(&blobs, &b);
            let side = valid(&blobs, &x);
            let one = merged_of(&blobs, &base, &side, &base);
            let two = merged_of(&blobs, &base, &base, &side);
            prop_assert_eq!(&one.merged, &side);
            prop_assert_eq!(&two.merged, &side);
            prop_assert!(one.conflicts.is_empty() && two.conflicts.is_empty());
        }

        #[test]
        fn merging_identical_sides_gives_that_side(b in spec(NAMES.len()), x in spec(NAMES.len())) {
            let blobs = Blobs::default();
            let base = valid(&blobs, &b);
            let side = valid(&blobs, &x);
            let merge = merged_of(&blobs, &base, &side, &side);
            prop_assert_eq!(&merge.merged, &side);
            prop_assert!(merge.conflicts.is_empty() && merge.new_blobs.is_empty());
        }

        #[test]
        fn edits_to_different_paths_merge_cleanly_in_either_order(
            b in spec(DISJOINT.len()),
            ours_edit in spec(DISJOINT.len()),
            theirs_edit in spec(DISJOINT.len()),
            owner in vec(any::<bool>(), DISJOINT.len()),
        ) {
            let blobs = Blobs::default();
            let base_spec = b;
            let mut ours_spec = base_spec.clone();
            let mut theirs_spec = base_spec.clone();
            let mut expected_spec = base_spec.clone();
            for index in 0..DISJOINT.len() {
                if owner[index] {
                    ours_spec[index] = ours_edit[index].clone();
                    expected_spec[index] = ours_edit[index].clone();
                } else {
                    theirs_spec[index] = theirs_edit[index].clone();
                    expected_spec[index] = theirs_edit[index].clone();
                }
            }
            let base = build(&blobs, &DISJOINT, &base_spec);
            let ours = build(&blobs, &DISJOINT, &ours_spec);
            let theirs = build(&blobs, &DISJOINT, &theirs_spec);
            let expected = build(&blobs, &DISJOINT, &expected_spec);
            let forward = merged_of(&blobs, &base, &ours, &theirs);
            let backward = merged_of(&blobs, &base, &theirs, &ours);
            prop_assert!(forward.conflicts.is_empty() && backward.conflicts.is_empty());
            prop_assert_eq!(&forward.merged, &expected);
            prop_assert_eq!(&backward.merged, &expected);
        }

        #[test]
        fn edits_to_separate_lines_of_one_file_merge_cleanly_in_either_order(
            ours_ops in vec(op(), 4),
            theirs_ops in vec(op(), 4),
        ) {
            // Ours touches base lines 0 and 4 and theirs touches 2 and 6; lines 1, 3 and 5 stay between them.
            let base: Vec<String> = (0..8).map(|i| format!("base{i}\n")).collect();
            let mut ours_full = vec![Op::Keep; 8];
            let mut theirs_full = vec![Op::Keep; 8];
            for (slot, index) in [0usize, 4].into_iter().enumerate() {
                ours_full[index] = ours_ops[slot].clone();
            }
            for (slot, index) in [2usize, 6].into_iter().enumerate() {
                theirs_full[index] = theirs_ops[slot].clone();
            }
            let blobs = Blobs::default();
            let base_text = base.concat();
            let (ours_text, ..) = edit(&base, &ours_full, "o");
            let (theirs_text, ..) = edit(&base, &theirs_full, "t");
            let base_tree = single(&blobs, &base_text);
            let ours = single(&blobs, &ours_text);
            let theirs = single(&blobs, &theirs_text);
            let forward = merged_of(&blobs, &base_tree, &ours, &theirs);
            let backward = merged_of(&blobs, &base_tree, &theirs, &ours);
            prop_assert!(forward.conflicts.is_empty() && backward.conflicts.is_empty());
            prop_assert_eq!(&forward.merged, &backward.merged);
            let expected = {
                let mut text = String::new();
                for (index, line) in base.iter().enumerate() {
                    let (tag, ops) = if index % 4 == 0 { ("o", &ours_full) } else { ("t", &theirs_full) };
                    match &ops[index] {
                        Op::Keep => text.push_str(line),
                        Op::Delete => {}
                        Op::Replace => text.push_str(&format!("{tag}{index}\n")),
                        Op::InsertAfter => {
                            text.push_str(line);
                            text.push_str(&format!("{tag}{index}\n"));
                        }
                    }
                }
                text
            };
            prop_assert_eq!(forward.merged[b"f".as_slice()].hash, hash_object(ObjectKind::Blob, expected.as_bytes()));
        }

        #[test]
        fn a_clean_merge_keeps_every_added_line_and_no_removed_line(
            ours_ops in vec(op(), 0..10),
            theirs_ops in vec(op(), 0..10),
            len in 0usize..10,
        ) {
            let base: Vec<String> = (0..len).map(|i| format!("base{i}\n")).collect();
            let (ours_text, ours_added, ours_removed) = edit(&base, &ours_ops, "o");
            let (theirs_text, theirs_added, theirs_removed) = edit(&base, &theirs_ops, "t");
            let blobs = Blobs::default();
            let base_tree = single(&blobs, &base.concat());
            let ours = single(&blobs, &ours_text);
            let theirs = single(&blobs, &theirs_text);
            let merge = merged_of(&blobs, &base_tree, &ours, &theirs);
            if merge.conflicts.is_empty() {
                let text = match merge.new_blobs.first() {
                    Some((_, bytes)) => String::from_utf8(bytes.clone()).unwrap(),
                    None => {
                        let hash = merge.merged.get(b"f".as_slice()).map(|e| e.hash);
                        match hash {
                            Some(hash) => String::from_utf8(blobs.0.borrow()[&hash].clone()).unwrap(),
                            None => String::new(),
                        }
                    }
                };
                let lines: Vec<&str> = text.split_inclusive('\n').collect();
                for line in ours_added.iter().chain(&theirs_added) {
                    prop_assert!(lines.contains(&line.as_str()), "lost added {line:?}");
                }
                for line in ours_removed.iter().chain(&theirs_removed) {
                    prop_assert!(!lines.contains(&line.as_str()), "kept removed {line:?}");
                }
            }
        }

        #[test]
        fn swapping_the_sides_gives_the_same_merged_map(b in spec(NAMES.len()), o in spec(NAMES.len()), t in spec(NAMES.len())) {
            let blobs = Blobs::default();
            let base = valid(&blobs, &b);
            let ours = valid(&blobs, &o);
            let theirs = valid(&blobs, &t);
            let forward = merged_of(&blobs, &base, &ours, &theirs);
            let backward = merged_of(&blobs, &base, &theirs, &ours);
            prop_assert_eq!(&forward.merged, &backward.merged);
            let paths = |m: &TreeMerge| m.conflicts.iter().map(|c| (c.path.clone(), c.kind)).collect::<Vec<_>>();
            prop_assert_eq!(paths(&forward), paths(&backward));
        }

        #[test]
        fn conflicts_match_their_entries_and_never_overlap_merged(b in spec(NAMES.len()), o in spec(NAMES.len()), t in spec(NAMES.len())) {
            let blobs = Blobs::default();
            let base = valid(&blobs, &b);
            let ours = valid(&blobs, &o);
            let theirs = valid(&blobs, &t);
            let merge = merged_of(&blobs, &base, &ours, &theirs);
            let mut previous: Option<&Vec<u8>> = None;
            for conflict in &merge.conflicts {
                prop_assert!(matches_entries(conflict, [&base, &ours, &theirs]), "{conflict:?}");
                prop_assert!(!merge.merged.contains_key(&conflict.path));
                for index in directory_prefixes(&conflict.path) {
                    prop_assert!(!merge.merged.contains_key(&conflict.path[..index]), "{conflict:?}");
                }
                prop_assert!(!has_child(&merge.merged, &conflict.path), "{conflict:?}");
                for other in &merge.conflicts {
                    prop_assert!(
                        !(other.path.len() > conflict.path.len()
                            && other.path.starts_with(&conflict.path)
                            && other.path[conflict.path.len()] == b'/'),
                        "{conflict:?} above {other:?}"
                    );
                }
                let below: FlatTree = theirs
                    .iter()
                    .filter(|(path, _)| {
                        path.len() > conflict.path.len()
                            && path.starts_with(&conflict.path)
                            && path[conflict.path.len()] == b'/'
                    })
                    .map(|(path, entry)| (path[conflict.path.len() + 1..].to_vec(), *entry))
                    .collect();
                prop_assert_eq!(&conflict.theirs_below, &below);
                prop_assert!(previous.is_none_or(|p| *p < conflict.path));
                previous = Some(&conflict.path);
                prop_assert_eq!(conflict.base, base.get(&conflict.path).copied());
                prop_assert_eq!(conflict.ours, ours.get(&conflict.path).copied());
                prop_assert_eq!(conflict.theirs, theirs.get(&conflict.path).copied());
            }
        }
    }
}
