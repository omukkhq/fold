use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

use crate::error::{FoldError, codes};
use crate::object::{ObjectHash, ObjectKind, hash_object, varint};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TreeError {
    #[error("entry name is empty")]
    EmptyName,
    #[error("entry name is `.` or `..`")]
    DotName,
    #[error("entry name holds `/` or NUL")]
    BadNameByte,
    #[error("entry name appears twice")]
    DuplicateName,
    #[error("entries are not sorted by name")]
    Unsorted,
    #[error("entry kind byte {0} is unknown")]
    UnknownKind(u8),
    #[error("tree bytes end inside an entry")]
    Truncated,
    #[error("entry name length is not a valid varint")]
    BadLength,
    #[error("a path is both a file and a directory")]
    PathConflict,
    #[error("a flat entry names a directory")]
    DirectoryInFlatMap,
}

impl TreeError {
    pub fn code(&self) -> &'static str {
        codes::TREE_INVALID
    }
}

impl From<TreeError> for FoldError {
    fn from(error: TreeError) -> Self {
        FoldError::new(error.code(), error.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum EntryKind {
    File = 1,
    Executable = 2,
    Symlink = 3,
    Directory = 4,
}

impl EntryKind {
    pub fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::File),
            2 => Some(Self::Executable),
            3 => Some(Self::Symlink),
            4 => Some(Self::Directory),
            _ => None,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub name: Vec<u8>,
    pub kind: EntryKind,
    pub hash: ObjectHash,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tree {
    pub entries: Vec<TreeEntry>,
}

pub fn validate_name(name: &[u8]) -> Result<(), TreeError> {
    if name.is_empty() {
        Err(TreeError::EmptyName)
    } else if name == b"." || name == b".." {
        Err(TreeError::DotName)
    } else if name.iter().any(|&byte| byte == b'/' || byte == 0) {
        Err(TreeError::BadNameByte)
    } else {
        Ok(())
    }
}

impl Tree {
    /// Entries may be given in any order; the payload is always sorted by name bytes.
    pub fn encode(&self) -> Result<Vec<u8>, TreeError> {
        let mut sorted: Vec<&TreeEntry> = self.entries.iter().collect();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        let mut out = Vec::new();
        let mut previous: Option<&[u8]> = None;
        for entry in sorted {
            validate_name(&entry.name)?;
            if previous == Some(entry.name.as_slice()) {
                return Err(TreeError::DuplicateName);
            }
            previous = Some(&entry.name);
            out.push(entry.kind.as_u8());
            out.extend_from_slice(entry.hash.as_bytes());
            varint::encode(entry.name.len() as u64, &mut out);
            out.extend_from_slice(&entry.name);
        }
        Ok(out)
    }

    pub fn decode(mut bytes: &[u8]) -> Result<Self, TreeError> {
        let mut entries: Vec<TreeEntry> = Vec::new();
        while let Some((&kind_byte, rest)) = bytes.split_first() {
            let kind = EntryKind::from_u8(kind_byte).ok_or(TreeError::UnknownKind(kind_byte))?;
            let (hash_bytes, rest) = rest.split_first_chunk::<32>().ok_or(TreeError::Truncated)?;
            let (length, used) = varint::decode(rest).map_err(|_| TreeError::BadLength)?;
            let rest = &rest[used..];
            let length = usize::try_from(length).map_err(|_| TreeError::Truncated)?;
            if rest.len() < length {
                return Err(TreeError::Truncated);
            }
            let (name, rest) = rest.split_at(length);
            validate_name(name)?;
            if let Some(last) = entries.last() {
                match last.name.as_slice().cmp(name) {
                    std::cmp::Ordering::Less => {}
                    std::cmp::Ordering::Equal => return Err(TreeError::DuplicateName),
                    std::cmp::Ordering::Greater => return Err(TreeError::Unsorted),
                }
            }
            entries.push(TreeEntry {
                name: name.to_vec(),
                kind,
                hash: ObjectHash::from_bytes(*hash_bytes),
            });
            bytes = rest;
        }
        Ok(Self { entries })
    }

    pub fn hash(&self) -> Result<ObjectHash, TreeError> {
        Ok(hash_object(ObjectKind::Tree, &self.encode()?))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlatEntry {
    pub kind: EntryKind,
    pub hash: ObjectHash,
}

/// Slash-joined paths to their entries. Directories are not in the map, so an empty directory is not tracked.
pub type FlatTree = BTreeMap<Vec<u8>, FlatEntry>;

/// How many files a set of changed paths adds, modifies and deletes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChangeCounts {
    pub files_added: u32,
    pub files_modified: u32,
    pub files_deleted: u32,
}

/// How one file differs between two states of a comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    /// A deleted path and an added path with the same blob. The pairing is for display only; merging never uses it.
    Renamed,
    /// The same blob with another entry kind, such as a file that became executable.
    ModeChanged,
}

impl ChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::ModeChanged => "mode_changed",
        }
    }
}

/// The changed files of a comparison counted by kind, with their line totals, for the one-line summary the live views show.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChangeTally {
    pub modified: u32,
    pub added: u32,
    pub deleted: u32,
    pub renamed: u32,
    pub mode_changed: u32,
    pub added_lines: u32,
    pub removed_lines: u32,
}

impl ChangeTally {
    /// Counts one changed file. A binary file adds no lines.
    pub fn add(&mut self, kind: ChangeKind, added_lines: u32, removed_lines: u32) {
        let count = match kind {
            ChangeKind::Modified => &mut self.modified,
            ChangeKind::Added => &mut self.added,
            ChangeKind::Deleted => &mut self.deleted,
            ChangeKind::Renamed => &mut self.renamed,
            ChangeKind::ModeChanged => &mut self.mode_changed,
        };
        *count = count.saturating_add(1);
        self.added_lines = self.added_lines.saturating_add(added_lines);
        self.removed_lines = self.removed_lines.saturating_add(removed_lines);
    }

    pub fn files(&self) -> u32 {
        [
            self.modified,
            self.added,
            self.deleted,
            self.renamed,
            self.mode_changed,
        ]
        .into_iter()
        .fold(0, u32::saturating_add)
    }

    /// `3 files: 2 modified, 1 added (+40 -5)`, or `no changes`. The kinds always come in the same order and a kind with no file is left out.
    pub fn summary(&self) -> String {
        let files = self.files();
        if files == 0 {
            return "no changes".to_owned();
        }
        let kinds: Vec<String> = [
            (self.modified, "modified"),
            (self.added, "added"),
            (self.deleted, "deleted"),
            (self.renamed, "renamed"),
            (self.mode_changed, "mode changed"),
        ]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, name)| format!("{count} {name}"))
        .collect();
        let noun = if files == 1 { "file" } else { "files" };
        format!(
            "{files} {noun}: {} (+{} -{})",
            kinds.join(", "),
            self.added_lines,
            self.removed_lines
        )
    }
}

/// Supplies tree objects, so the core stays free of I/O.
pub type TreeLoader<'a> = &'a dyn Fn(&ObjectHash) -> Result<Tree, FoldError>;

pub fn flatten(root: &ObjectHash, load: TreeLoader<'_>) -> Result<FlatTree, FoldError> {
    let mut flat = FlatTree::new();
    flatten_into(root, &[], load, &mut flat)?;
    Ok(flat)
}

fn flatten_into(
    hash: &ObjectHash,
    prefix: &[u8],
    load: TreeLoader<'_>,
    flat: &mut FlatTree,
) -> Result<(), FoldError> {
    let tree = load(hash)?;
    for entry in tree.entries {
        let mut path = prefix.to_vec();
        if !path.is_empty() {
            path.push(b'/');
        }
        path.extend_from_slice(&entry.name);
        if entry.kind == EntryKind::Directory {
            flatten_into(&entry.hash, &path, load, flat)?;
        } else {
            flat.insert(
                path,
                FlatEntry {
                    kind: entry.kind,
                    hash: entry.hash,
                },
            );
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeBuild {
    pub root: ObjectHash,
    /// Each rebuilt tree whose hash differs from the base tree at its path, with its encoded payload. One may name an object the store already holds, such as an identical directory elsewhere in the base.
    pub new_trees: Vec<(ObjectHash, Vec<u8>)>,
}

/// A change maps a path to its new entry, or to `None` to delete it. The result is validated, not the change map: a directory becomes a file only when every path under it is deleted, and a file becomes a directory only when the file itself is deleted. Deleting a path that holds no file, such as a directory or a path below a file, changes nothing. Only the ancestors of changed paths are rebuilt; every other subtree keeps its base hash, and a directory left empty is dropped.
pub fn apply_changes(
    base_root: Option<ObjectHash>,
    changes: &BTreeMap<Vec<u8>, Option<FlatEntry>>,
    load: TreeLoader<'_>,
) -> Result<TreeBuild, FoldError> {
    let mut items: Vec<(&[u8], Option<FlatEntry>)> = Vec::with_capacity(changes.len());
    for (path, change) in changes {
        for name in path.split(|&byte| byte == b'/') {
            validate_name(name)?;
        }
        if matches!(change, Some(entry) if entry.kind == EntryKind::Directory) {
            return Err(TreeError::DirectoryInFlatMap.into());
        }
        items.push((path, *change));
    }
    let mut new_trees = Vec::new();
    let root = match build_dir(base_root, &items, load, &mut new_trees)? {
        Some(root) => root,
        None => {
            let tree = Tree::default();
            let payload = tree.encode()?;
            let hash = hash_object(ObjectKind::Tree, &payload);
            if base_root != Some(hash) {
                new_trees.push((hash, payload));
            }
            hash
        }
    };
    Ok(TreeBuild { root, new_trees })
}

type Change<'a> = (&'a [u8], Option<FlatEntry>);

fn build_dir(
    base: Option<ObjectHash>,
    changes: &[Change<'_>],
    load: TreeLoader<'_>,
    new_trees: &mut Vec<(ObjectHash, Vec<u8>)>,
) -> Result<Option<ObjectHash>, FoldError> {
    let mut entries: BTreeMap<Vec<u8>, TreeEntry> = BTreeMap::new();
    if let Some(hash) = &base {
        for entry in load(hash)?.entries {
            entries.insert(entry.name.clone(), entry);
        }
    }
    let mut groups: BTreeMap<&[u8], Vec<Change<'_>>> = BTreeMap::new();
    let mut whole: BTreeMap<&[u8], Option<FlatEntry>> = BTreeMap::new();
    for &(path, change) in changes {
        match path.iter().position(|&byte| byte == b'/') {
            None => {
                whole.insert(path, change);
            }
            Some(at) => groups
                .entry(&path[..at])
                .or_default()
                .push((&path[at + 1..], change)),
        }
    }
    let names: BTreeSet<&[u8]> = whole.keys().chain(groups.keys()).copied().collect();
    for name in names {
        let existing = entries.get(name).cloned();
        let deeper = groups.get(name);
        let replacement = match (whole.get(name), deeper) {
            (Some(Some(entry)), deeper) => {
                if let Some(entry_dir) = existing.filter(|e| e.kind == EntryKind::Directory) {
                    let emptied = match deeper {
                        Some(deeper) => {
                            sub_entry(name, Some(entry_dir.hash), deeper, load, new_trees)?
                        }
                        None => Some(entry_dir),
                    };
                    if emptied.is_some() {
                        return Err(TreeError::PathConflict.into());
                    }
                } else if let Some(deeper) = deeper
                    && sub_entry(name, None, deeper, load, new_trees)?.is_some()
                {
                    return Err(TreeError::PathConflict.into());
                }
                Some(TreeEntry {
                    name: name.to_vec(),
                    kind: entry.kind,
                    hash: entry.hash,
                })
            }
            (Some(None), None) => existing.filter(|entry| entry.kind == EntryKind::Directory),
            (Some(None), Some(deeper)) => {
                let base_dir = existing
                    .filter(|entry| entry.kind == EntryKind::Directory)
                    .map(|entry| entry.hash);
                sub_entry(name, base_dir, deeper, load, new_trees)?
            }
            (None, Some(deeper)) => match existing {
                Some(entry) if entry.kind == EntryKind::Directory => {
                    sub_entry(name, Some(entry.hash), deeper, load, new_trees)?
                }
                Some(entry) => {
                    if sub_entry(name, None, deeper, load, new_trees)?.is_some() {
                        return Err(TreeError::PathConflict.into());
                    }
                    Some(entry)
                }
                None => sub_entry(name, None, deeper, load, new_trees)?,
            },
            (None, None) => existing,
        };
        match replacement {
            Some(entry) => entries.insert(name.to_vec(), entry),
            None => entries.remove(name),
        };
    }
    if entries.is_empty() {
        return Ok(None);
    }
    let tree = Tree {
        entries: entries.into_values().collect(),
    };
    let payload = tree.encode()?;
    let hash = hash_object(ObjectKind::Tree, &payload);
    if base != Some(hash) {
        new_trees.push((hash, payload));
    }
    Ok(Some(hash))
}

fn sub_entry(
    name: &[u8],
    base_dir: Option<ObjectHash>,
    changes: &[Change<'_>],
    load: TreeLoader<'_>,
    new_trees: &mut Vec<(ObjectHash, Vec<u8>)>,
) -> Result<Option<TreeEntry>, FoldError> {
    Ok(
        build_dir(base_dir, changes, load, new_trees)?.map(|hash| TreeEntry {
            name: name.to_vec(),
            kind: EntryKind::Directory,
            hash,
        }),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use proptest::prelude::*;

    use super::*;

    fn blob(text: &str) -> ObjectHash {
        hash_object(ObjectKind::Blob, text.as_bytes())
    }

    fn file(text: &str) -> FlatEntry {
        FlatEntry {
            kind: EntryKind::File,
            hash: blob(text),
        }
    }

    fn tally(files: &[(ChangeKind, u32, u32)]) -> ChangeTally {
        let mut tally = ChangeTally::default();
        for &(kind, added, removed) in files {
            tally.add(kind, added, removed);
        }
        tally
    }

    #[test]
    fn a_summary_without_files_says_so() {
        let empty = tally(&[]);
        assert_eq!(empty.files(), 0);
        assert_eq!(empty.summary(), "no changes");
    }

    #[test]
    fn a_summary_of_one_file_is_singular() {
        let one = tally(&[(ChangeKind::Modified, 3, 1)]);
        assert_eq!(one.summary(), "1 file: 1 modified (+3 -1)");
    }

    #[test]
    fn a_summary_counts_each_kind_in_a_fixed_order() {
        let all = tally(&[
            (ChangeKind::ModeChanged, 0, 0),
            (ChangeKind::Renamed, 0, 0),
            (ChangeKind::Deleted, 0, 7),
            (ChangeKind::Added, 12, 0),
            (ChangeKind::Modified, 28, 5),
            (ChangeKind::Modified, 0, 0),
        ]);
        assert_eq!(all.files(), 6);
        assert_eq!(
            all.summary(),
            "6 files: 2 modified, 1 added, 1 deleted, 1 renamed, 1 mode changed (+40 -12)"
        );
    }

    #[test]
    fn a_summary_leaves_out_kinds_without_files_and_sums_the_lines() {
        let some = tally(&[
            (ChangeKind::Added, 10, 0),
            (ChangeKind::Modified, 25, 3),
            (ChangeKind::Modified, 5, 2),
        ]);
        assert_eq!(some.summary(), "3 files: 2 modified, 1 added (+40 -5)");
        let deleted = tally(&[(ChangeKind::Deleted, 0, 4), (ChangeKind::Deleted, 0, 1)]);
        assert_eq!(deleted.summary(), "2 files: 2 deleted (+0 -5)");
    }

    #[test]
    fn a_tally_saturates_instead_of_overflowing() {
        let mut big = tally(&[(ChangeKind::Added, u32::MAX, 0)]);
        big.add(ChangeKind::Added, 1, 0);
        big.mode_changed = u32::MAX;
        assert_eq!(big.added_lines, u32::MAX);
        assert_eq!(big.files(), u32::MAX);
    }

    #[test]
    fn change_kinds_have_stable_names() {
        let names: Vec<&str> = [
            ChangeKind::Added,
            ChangeKind::Modified,
            ChangeKind::Deleted,
            ChangeKind::Renamed,
            ChangeKind::ModeChanged,
        ]
        .into_iter()
        .map(ChangeKind::as_str)
        .collect();
        assert_eq!(
            names,
            ["added", "modified", "deleted", "renamed", "mode_changed"]
        );
    }

    fn entry(name: &str, kind: EntryKind, hash: ObjectHash) -> TreeEntry {
        TreeEntry {
            name: name.as_bytes().to_vec(),
            kind,
            hash,
        }
    }

    /// An in-memory object store for the tests.
    #[derive(Default)]
    struct Store(HashMap<ObjectHash, Vec<u8>>);

    impl Store {
        fn add(&mut self, build: &TreeBuild) {
            for (hash, payload) in &build.new_trees {
                self.0.insert(*hash, payload.clone());
            }
        }

        fn load(&self, hash: &ObjectHash) -> Result<Tree, FoldError> {
            let payload = self
                .0
                .get(hash)
                .ok_or_else(|| FoldError::new("missing", "no such tree"))?;
            Ok(Tree::decode(payload)?)
        }

        fn build(
            &mut self,
            base: Option<ObjectHash>,
            changes: &[(&str, Option<FlatEntry>)],
        ) -> TreeBuild {
            let map = to_changes(changes);
            let build = apply_changes(base, &map, &|hash| self.load(hash)).unwrap();
            self.add(&build);
            build
        }

        fn flat(&self, root: &ObjectHash) -> FlatTree {
            flatten(root, &|hash| self.load(hash)).unwrap()
        }
    }

    fn to_changes(changes: &[(&str, Option<FlatEntry>)]) -> BTreeMap<Vec<u8>, Option<FlatEntry>> {
        changes
            .iter()
            .map(|(path, change)| (path.as_bytes().to_vec(), *change))
            .collect()
    }

    fn fixed_tree() -> Tree {
        Tree {
            entries: vec![
                entry("src", EntryKind::Directory, blob("dir")),
                entry("README.md", EntryKind::File, blob("readme")),
                entry("run.sh", EntryKind::Executable, blob("run")),
                entry("link", EntryKind::Symlink, blob("README.md")),
            ],
        }
    }

    #[test]
    fn golden_tree_bytes_and_hash() {
        let bytes = fixed_tree().encode().unwrap();
        assert_eq!(hex(&bytes), GOLDEN_TREE_BYTES);
        assert_eq!(fixed_tree().hash().unwrap().to_string(), GOLDEN_TREE_HASH);
        assert_eq!(Tree::decode(&bytes).unwrap().encode().unwrap(), bytes);
    }

    const GOLDEN_TREE_BYTES: &str = "01f2d0934db0f1b5282e7b904631b2893e33c3090d652abd08a589e5201103552409524541444d452e6d6403d0456a6181d8f5e0b4bd9792ddd8e386a323bf335bbc6700b51090f85d17e8ba046c696e6b026b8801ada62311218c81a28d75640b262f6fc1bc5e75004f89aa8dcb47dba8ff0672756e2e73680485f88d9437c77d4b85ff38c1c3405a484ce2e8dcf8d212d4a546b1742793b67c03737263";
    const GOLDEN_TREE_HASH: &str =
        "367be859728c90dde3ad44f2e762cf378ca0f7745ef75b0b5e9abb3e1fb6ab3b";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn empty_tree_is_empty_payload() {
        assert_eq!(Tree::default().encode().unwrap(), Vec::<u8>::new());
        assert_eq!(Tree::decode(&[]).unwrap(), Tree::default());
    }

    #[test]
    fn encode_refuses_bad_names_and_duplicates() {
        for name in ["", ".", "..", "a/b", "a\0b"] {
            let tree = Tree {
                entries: vec![entry(name, EntryKind::File, blob("x"))],
            };
            assert!(tree.encode().is_err(), "{name:?}");
        }
        let tree = Tree {
            entries: vec![
                entry("a", EntryKind::File, blob("x")),
                entry("a", EntryKind::Symlink, blob("y")),
            ],
        };
        assert_eq!(tree.encode(), Err(TreeError::DuplicateName));
    }

    fn raw(kind: u8, hash: &[u8], name: &[u8]) -> Vec<u8> {
        let mut out = vec![kind];
        out.extend_from_slice(hash);
        out.push(name.len() as u8);
        out.extend_from_slice(name);
        out
    }

    #[test]
    fn decode_refusals() {
        let h = [7u8; 32];
        let one = raw(1, &h, b"a");
        assert!(Tree::decode(&one).is_ok());
        let cat = |parts: &[&[u8]]| parts.concat();
        assert_eq!(
            Tree::decode(&cat(&[&raw(1, &h, b"b"), &raw(1, &h, b"a")])),
            Err(TreeError::Unsorted)
        );
        assert_eq!(
            Tree::decode(&cat(&[&raw(1, &h, b"a"), &raw(2, &h, b"a")])),
            Err(TreeError::DuplicateName)
        );
        for name in [&b"a/b"[..], b"a\0", b".", b"..", b""] {
            assert!(Tree::decode(&raw(1, &h, name)).is_err(), "{name:?}");
        }
        assert_eq!(
            Tree::decode(&raw(0, &h, b"a")),
            Err(TreeError::UnknownKind(0))
        );
        assert_eq!(
            Tree::decode(&raw(5, &h, b"a")),
            Err(TreeError::UnknownKind(5))
        );
        assert_eq!(Tree::decode(&one[..20]), Err(TreeError::Truncated));
        assert_eq!(
            Tree::decode(&one[..one.len() - 1]),
            Err(TreeError::Truncated)
        );
        assert_eq!(Tree::decode(&one[..33]), Err(TreeError::BadLength));
        // A trailing byte is read as the start of a further entry, which is cut short.
        let mut trailing = one.clone();
        trailing.push(0);
        assert_eq!(Tree::decode(&trailing), Err(TreeError::UnknownKind(0)));
        trailing.pop();
        trailing.push(1);
        assert_eq!(Tree::decode(&trailing), Err(TreeError::Truncated));
        let mut huge = vec![1u8];
        huge.extend_from_slice(&h);
        huge.extend_from_slice(&[0xff; 9]);
        huge.push(0x01);
        assert_eq!(Tree::decode(&huge), Err(TreeError::Truncated));
    }

    #[test]
    fn errors_map_to_the_tree_invalid_code() {
        let error = FoldError::from(TreeError::Unsorted);
        assert_eq!(error.code(), "tree_invalid");
    }

    fn arb_name() -> impl Strategy<Value = Vec<u8>> {
        proptest::collection::vec(
            any::<u8>().prop_filter("no slash or NUL", |&b| b != b'/' && b != 0),
            1..8,
        )
        .prop_filter("not a dot name", |name| name != b"." && name != b"..")
    }

    fn arb_tree() -> impl Strategy<Value = Tree> {
        proptest::collection::btree_map(arb_name(), (1u8..=4, any::<[u8; 32]>()), 0..12).prop_map(
            |map| Tree {
                entries: map
                    .into_iter()
                    .map(|(name, (kind, hash))| TreeEntry {
                        name,
                        kind: EntryKind::from_u8(kind).unwrap(),
                        hash: ObjectHash::from_bytes(hash),
                    })
                    .collect(),
            },
        )
    }

    proptest! {
        #[test]
        fn tree_round_trip(tree in arb_tree()) {
            let bytes = tree.encode().unwrap();
            prop_assert_eq!(Tree::decode(&bytes).unwrap(), tree);
        }

        #[test]
        fn entry_order_does_not_change_bytes(tree in arb_tree(), seed in any::<u64>()) {
            let mut shuffled = tree.clone();
            let len = shuffled.entries.len();
            if len > 1 {
                for i in 0..len {
                    let j = (seed as usize).wrapping_mul(i + 7).wrapping_add(i) % len;
                    shuffled.entries.swap(i, j);
                }
            }
            prop_assert_eq!(shuffled.encode().unwrap(), tree.encode().unwrap());
        }
    }

    fn sample(store: &mut Store) -> TreeBuild {
        store.build(
            None,
            &[
                ("README.md", Some(file("readme"))),
                ("src/main.rs", Some(file("main"))),
                ("src/deep/a/b.rs", Some(file("b"))),
                ("src/deep/c.rs", Some(file("c"))),
                ("docs/guide.md", Some(file("guide"))),
            ],
        )
    }

    #[test]
    fn flatten_returns_nested_paths() {
        let mut store = Store::default();
        let build = sample(&mut store);
        let flat = store.flat(&build.root);
        let paths: Vec<&[u8]> = flat.keys().map(Vec::as_slice).collect();
        assert_eq!(
            paths,
            [
                &b"README.md"[..],
                b"docs/guide.md",
                b"src/deep/a/b.rs",
                b"src/deep/c.rs",
                b"src/main.rs",
            ]
        );
        assert_eq!(flat[&b"src/main.rs"[..]], file("main"));
    }

    #[test]
    fn changing_a_deep_file_rebuilds_only_its_ancestors() {
        let mut store = Store::default();
        let base = sample(&mut store);
        let next = store.build(Some(base.root), &[("src/deep/a/b.rs", Some(file("b2")))]);
        assert_eq!(next.new_trees.len(), 4);
        let before = Tree::decode(&store.0[&base.root]).unwrap();
        let after = Tree::decode(&store.0[&next.root]).unwrap();
        let find = |tree: &Tree, name: &str| {
            tree.entries
                .iter()
                .find(|e| e.name == name.as_bytes())
                .unwrap()
                .hash
        };
        assert_ne!(find(&before, "src"), find(&after, "src"));
        assert_eq!(find(&before, "docs"), find(&after, "docs"));
        let src_before = Tree::decode(&store.0[&find(&before, "src")]).unwrap();
        let src_after = Tree::decode(&store.0[&find(&after, "src")]).unwrap();
        assert_ne!(find(&src_before, "deep"), find(&src_after, "deep"));
        assert_eq!(find(&src_before, "main.rs"), find(&src_after, "main.rs"));
        let flat = store.flat(&next.root);
        assert_eq!(flat[&b"src/deep/a/b.rs"[..]], file("b2"));
        assert_eq!(flat.len(), 5);
    }

    #[test]
    fn an_unchanged_result_adds_no_trees() {
        let mut store = Store::default();
        let base = sample(&mut store);
        let same = store.build(Some(base.root), &[("README.md", Some(file("readme")))]);
        assert_eq!(same.root, base.root);
        assert!(same.new_trees.is_empty());
    }

    #[test]
    fn deleting_the_last_file_removes_the_directory() {
        let mut store = Store::default();
        let base = sample(&mut store);
        let next = store.build(Some(base.root), &[("docs/guide.md", None)]);
        let flat = store.flat(&next.root);
        assert!(flat.keys().all(|path| !path.starts_with(b"docs")));
        let root = store.load(&next.root).unwrap();
        assert!(root.entries.iter().all(|e| e.name != b"docs"));
        let nested = store.build(Some(base.root), &[("src/deep/a/b.rs", None)]);
        let src = store.flat(&nested.root);
        assert!(src.keys().all(|path| !path.starts_with(b"src/deep/a")));
        assert!(src.contains_key(&b"src/deep/c.rs"[..]));
    }

    #[test]
    fn deleting_everything_gives_the_empty_tree() {
        let mut store = Store::default();
        let base = store.build(None, &[("a", Some(file("a")))]);
        let next = store.build(Some(base.root), &[("a", None)]);
        assert_eq!(next.root, Tree::default().hash().unwrap());
        assert!(store.flat(&next.root).is_empty());
        let again = store.build(Some(next.root), &[("gone", None)]);
        assert_eq!(again.root, next.root);
        assert!(again.new_trees.is_empty());
        let empty = store.build(None, &[]);
        assert_eq!(empty.root, next.root);
    }

    #[test]
    fn building_from_no_base_matches_flattening_and_rebuilding() {
        let mut store = Store::default();
        let base = sample(&mut store);
        let flat = store.flat(&base.root);
        let changes: BTreeMap<Vec<u8>, Option<FlatEntry>> = flat
            .iter()
            .map(|(path, e)| (path.clone(), Some(*e)))
            .collect();
        let rebuilt = apply_changes(None, &changes, &|hash| store.load(hash)).unwrap();
        assert_eq!(rebuilt.root, base.root);
    }

    #[test]
    fn a_file_and_its_directory_prefix_are_refused() {
        let store = Store::default();
        let map = to_changes(&[("a", Some(file("a"))), ("a/b", Some(file("b")))]);
        let error = apply_changes(None, &map, &|hash| store.load(hash)).unwrap_err();
        assert_eq!(error.code(), "tree_invalid");
    }

    #[test]
    fn file_to_directory_swaps_need_the_delete() {
        let mut store = Store::default();
        let base = store.build(None, &[("a", Some(file("a")))]);
        let map = to_changes(&[("a/b", Some(file("b")))]);
        let error = apply_changes(Some(base.root), &map, &|hash| store.load(hash)).unwrap_err();
        assert_eq!(error.code(), "tree_invalid");
        let swapped = store.build(Some(base.root), &[("a", None), ("a/b", Some(file("b")))]);
        let flat = store.flat(&swapped.root);
        assert_eq!(
            flat.keys().map(Vec::as_slice).collect::<Vec<_>>(),
            [&b"a/b"[..]]
        );
        let map = to_changes(&[("a", Some(file("a2"))), ("a/b", Some(file("b")))]);
        assert!(apply_changes(Some(swapped.root), &map, &|hash| store.load(hash)).is_err());
    }

    #[test]
    fn directory_to_file_swaps_need_every_delete() {
        let mut store = Store::default();
        let base = store.build(
            None,
            &[
                ("a/b", Some(file("b"))),
                ("a/c", Some(file("c"))),
                ("a/d/e", Some(file("e"))),
                ("z", Some(file("z"))),
            ],
        );
        let load = |hash: &ObjectHash| store.load(hash);
        for map in [
            to_changes(&[("a", Some(file("a")))]),
            to_changes(&[("a", Some(file("a"))), ("a/b", None)]),
            to_changes(&[("a", Some(file("a"))), ("a/b", None), ("a/c", None)]),
            to_changes(&[("a", Some(file("a"))), ("a/b", Some(file("b2")))]),
        ] {
            let error = apply_changes(Some(base.root), &map, &load).unwrap_err();
            assert_eq!(error.code(), "tree_invalid");
        }
        let all = to_changes(&[
            ("a", Some(file("a"))),
            ("a/b", None),
            ("a/c", None),
            ("a/d/e", None),
        ]);
        let swapped = apply_changes(Some(base.root), &all, &load).unwrap();
        store.add(&swapped);
        let flat = store.flat(&swapped.root);
        assert_eq!(
            flat.keys().map(Vec::as_slice).collect::<Vec<_>>(),
            [&b"a"[..], b"z"]
        );
        assert_eq!(flat[&b"a"[..]], file("a"));
    }

    #[test]
    fn a_file_replaced_by_a_file_tolerates_deletes_below_it() {
        let mut store = Store::default();
        let base = store.build(None, &[("a", Some(file("a")))]);
        let next = store.build(Some(base.root), &[("a", Some(file("a2"))), ("a/b", None)]);
        assert_eq!(store.flat(&next.root)[&b"a"[..]], file("a2"));
        let same = store.build(Some(base.root), &[("a/b", None), ("a/b/c", None)]);
        assert_eq!(same.root, base.root);
        assert!(same.new_trees.is_empty());
    }

    #[test]
    fn both_swap_spellings_agree_with_the_acceptance_cases() {
        let mut store = Store::default();
        let dir = store.build(None, &[("a/b", Some(file("b"))), ("a/c", Some(file("c")))]);
        let file_base = store.build(None, &[("a", Some(file("a")))]);
        let paths = |store: &mut Store, base: &TreeBuild, changes: &[(&str, Option<FlatEntry>)]| {
            let map = to_changes(changes);
            match apply_changes(Some(base.root), &map, &|hash| store.load(hash)) {
                Ok(build) => {
                    store.add(&build);
                    let flat = store.flat(&build.root);
                    Ok(flat.into_keys().collect::<Vec<_>>())
                }
                Err(error) => Err(error.code()),
            }
        };
        let to_file = [("a", Some(file("a"))), ("a/b", None), ("a/c", None)];
        assert_eq!(paths(&mut store, &dir, &to_file), Ok(vec![b"a".to_vec()]));
        let bare = [("a", Some(file("a")))];
        assert_eq!(paths(&mut store, &dir, &bare), Err("tree_invalid"));
        let to_dir = [("a", None), ("a/b", Some(file("b")))];
        assert_eq!(
            paths(&mut store, &file_base, &to_dir),
            Ok(vec![b"a/b".to_vec()])
        );
        let bare = [("a/b", Some(file("b")))];
        assert_eq!(paths(&mut store, &file_base, &bare), Err("tree_invalid"));
    }

    fn arb_path() -> impl Strategy<Value = Vec<u8>> {
        proptest::collection::vec(prop_oneof![Just("a"), Just("b"), Just("c")], 1..=3)
            .prop_map(|names| names.join("/").into_bytes())
    }

    fn arb_leaf() -> impl Strategy<Value = FlatEntry> {
        (
            prop_oneof![Just(EntryKind::File), Just(EntryKind::Symlink)],
            0u8..3,
        )
            .prop_map(|(kind, content)| FlatEntry {
                kind,
                hash: hash_object(ObjectKind::Blob, &[content]),
            })
    }

    fn has_parent_in(path: &[u8], flat: &FlatTree) -> bool {
        flat.keys()
            .any(|other| path.starts_with(other) && path.get(other.len()) == Some(&b'/'))
    }

    fn is_prefix_free(flat: &FlatTree) -> bool {
        flat.keys().all(|path| !has_parent_in(path, flat))
    }

    fn arb_flat() -> impl Strategy<Value = FlatTree> {
        proptest::collection::btree_map(arb_path(), arb_leaf(), 0..8).prop_map(|flat| {
            flat.iter()
                .filter(|(path, _)| !has_parent_in(path, &flat))
                .map(|(path, entry)| (path.clone(), *entry))
                .collect()
        })
    }

    fn build_flat(store: &mut Store, flat: &FlatTree) -> TreeBuild {
        let changes = flat
            .iter()
            .map(|(path, e)| (path.clone(), Some(*e)))
            .collect();
        let build = apply_changes(None, &changes, &|hash| store.load(hash)).unwrap();
        store.add(&build);
        build
    }

    proptest! {
        #[test]
        fn changes_are_refused_only_when_the_result_is_invalid(
            base in arb_flat(),
            changes in proptest::collection::btree_map(
                arb_path(),
                proptest::option::of(arb_leaf()),
                0..8,
            ),
        ) {
            let mut store = Store::default();
            let base_build = build_flat(&mut store, &base);
            let mut expected = base.clone();
            for (path, change) in &changes {
                match change {
                    Some(entry) => expected.insert(path.clone(), *entry),
                    None => expected.remove(path),
                };
            }
            let result = apply_changes(Some(base_build.root), &changes, &|hash| store.load(hash));
            prop_assert_eq!(result.is_ok(), is_prefix_free(&expected));
            if let Ok(build) = result {
                store.add(&build);
                prop_assert_eq!(&store.flat(&build.root), &expected);
                prop_assert_eq!(build.root, build_flat(&mut store, &expected).root);
            }
        }

        #[test]
        fn applying_a_snapshot_diff_gives_the_target(base in arb_flat(), target in arb_flat()) {
            let mut store = Store::default();
            let base_build = build_flat(&mut store, &base);
            let mut diff: BTreeMap<Vec<u8>, Option<FlatEntry>> = base
                .keys()
                .filter(|path| !target.contains_key(*path))
                .map(|path| (path.clone(), None))
                .collect();
            for (path, entry) in &target {
                if base.get(path) != Some(entry) {
                    diff.insert(path.clone(), Some(*entry));
                }
            }
            let build = apply_changes(Some(base_build.root), &diff, &|hash| store.load(hash)).unwrap();
            prop_assert_eq!(build.root, build_flat(&mut store, &target).root);
        }
    }

    #[test]
    fn invalid_change_paths_are_refused() {
        let store = Store::default();
        for path in ["", "/a", "a//b", "a/", "./a", "a/../b"] {
            let map = to_changes(&[(path, Some(file("x")))]);
            assert!(
                apply_changes(None, &map, &|hash| store.load(hash)).is_err(),
                "{path:?}"
            );
        }
        let dir = FlatEntry {
            kind: EntryKind::Directory,
            hash: blob("d"),
        };
        let map = to_changes(&[("a", Some(dir))]);
        assert!(apply_changes(None, &map, &|hash| store.load(hash)).is_err());
    }

    #[test]
    fn loader_errors_pass_through() {
        let missing = blob("nowhere");
        let error = flatten(&missing, &|_| Err(FoldError::new("missing", "no tree"))).unwrap_err();
        assert_eq!(error.code(), "missing");
    }
}
