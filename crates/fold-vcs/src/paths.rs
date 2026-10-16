use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::ops::Bound;
use std::path::{Path, PathBuf};

use fold_core::tree::{TreeError, validate_name};
use fold_core::{FoldError, codes};
use serde_json::json;
use thiserror::Error;

use crate::objects::ObjectError;
use crate::platform::DirHandle;

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("path `{0}` is not a valid workspace path")]
    InvalidPath(String),
    #[error("target `{0}` exists and is not empty")]
    TargetNotEmpty(String),
    #[error(transparent)]
    Object(#[from] ObjectError),
    #[error(transparent)]
    Tree(#[from] TreeError),
    #[error("workspace I/O failed: {0}")]
    Io(#[from] io::Error),
    /// The paths hold content that no object records, so replacing them would lose it.
    #[error("{} paths hold changes no checkpoint records", .0.len())]
    Unrecorded(Vec<String>),
    /// The paths a sync would overwrite or remove that no longer match the checkpoint taken before it.
    #[error("{} workspace paths changed since the checkpoint", .0.len())]
    ChangedDuringSync(Vec<String>),
}

impl WorkspaceError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidPath(_) => codes::INVALID_PATH,
            Self::TargetNotEmpty(_) => codes::TARGET_NOT_EMPTY,
            Self::Object(error) => error.code(),
            Self::Tree(error) => error.code(),
            Self::Io(_) => codes::WORKSPACE_IO,
            Self::Unrecorded(_) => codes::UNRECORDED_CHANGES,
            Self::ChangedDuringSync(_) => codes::WORKSPACE_CHANGED_DURING_SYNC,
        }
    }

    pub(crate) fn invalid(path: &[u8]) -> Self {
        Self::InvalidPath(String::from_utf8_lossy(path).into_owned())
    }
}

impl From<WorkspaceError> for FoldError {
    fn from(error: WorkspaceError) -> Self {
        let folded = FoldError::new(error.code(), error.to_string());
        match &error {
            WorkspaceError::InvalidPath(path) | WorkspaceError::TargetNotEmpty(path) => {
                folded.with_details([("path", json!(path))])
            }
            WorkspaceError::Unrecorded(paths) | WorkspaceError::ChangedDuringSync(paths) => {
                folded.with_details([("paths", json!(paths))])
            }
            _ => folded,
        }
    }
}

/// Names never tracked at any depth, as a file or a directory: Git's and fold's metadata, which hold fold's staging directory among other things.
pub fn is_reserved_name(name: &[u8]) -> bool {
    name == b".git" || name == b".fold"
}

/// A workspace-relative path held as bytes: slash-joined, with no empty, `.` or `..` component, no NUL, no leading `/`, and no `.git` or `.fold` component, so no tree can hold one. The ordering is bytewise, which is the order trees and the stat index use.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepoPath(Vec<u8>);

impl RepoPath {
    pub fn parse(bytes: &[u8]) -> Result<Self, WorkspaceError> {
        let valid = !bytes.is_empty()
            && bytes
                .split(|&byte| byte == b'/')
                .all(|name| validate_name(name).is_ok() && !is_reserved_name(name));
        if valid {
            Ok(Self(bytes.to_vec()))
        } else {
            Err(WorkspaceError::invalid(bytes))
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn components(&self) -> impl Iterator<Item = &[u8]> {
        self.0.split(|&byte| byte == b'/')
    }

    /// The names of the directories above the entry, from the top, and the entry's own name.
    pub(crate) fn parents_and_name(&self) -> (Vec<&[u8]>, &[u8]) {
        match self.0.iter().rposition(|&byte| byte == b'/') {
            Some(at) => (
                self.0[..at].split(|&byte| byte == b'/').collect(),
                &self.0[at + 1..],
            ),
            None => (Vec::new(), &self.0),
        }
    }

    /// True for this path itself and every path inside it. A sibling that only begins with the same bytes, such as `src-old` or `src.rs` beside `src`, is not inside it.
    pub fn is_at_or_below(&self, ancestor: &Self) -> bool {
        self.0
            .strip_prefix(ancestor.0.as_slice())
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(b"/"))
    }

    /// The entries of `map` at this path and below it, in key order.
    pub fn at_or_below<'m, V>(
        &self,
        map: &'m BTreeMap<Vec<u8>, V>,
    ) -> impl Iterator<Item = (&'m Vec<u8>, &'m V)> + use<'m, V> {
        entries_at_or_below(map, &self.0)
    }

    /// The path of the entry `name` inside `parent`, or the top-level entry when `parent` is `None`.
    pub fn child(parent: Option<&Self>, name: &[u8]) -> Result<Self, WorkspaceError> {
        match parent {
            Some(parent) => {
                let mut joined = parent.0.clone();
                joined.push(b'/');
                joined.extend_from_slice(name);
                Self::parse(&joined)
            }
            None => Self::parse(name),
        }
    }
}

/// The entries of `map` at the path `prefix` and below it; an empty `prefix` is the workspace root and gives every entry. Keys sort bytewise, and a sibling such as `src-old`, `src.rs`, `src x` or `src+` sorts between `src` and `src/`, so the entries below are the range from `src/` up to `src0`, the byte after `/`.
pub(crate) fn entries_at_or_below<'m, V>(
    map: &'m BTreeMap<Vec<u8>, V>,
    prefix: &[u8],
) -> impl Iterator<Item = (&'m Vec<u8>, &'m V)> + use<'m, V> {
    let (exact, bounds) = if prefix.is_empty() {
        (None, (Bound::Unbounded, Bound::Unbounded))
    } else {
        let mut lower = prefix.to_vec();
        lower.push(b'/');
        let mut upper = prefix.to_vec();
        upper.push(b'/' + 1);
        (
            map.get_key_value(prefix),
            (Bound::Included(lower), Bound::Excluded(upper)),
        )
    };
    exact.into_iter().chain(map.range::<Vec<u8>, _>(bounds))
}

impl fmt::Display for RepoPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&String::from_utf8_lossy(&self.0))
    }
}

#[cfg(unix)]
fn os_component(name: &[u8]) -> Result<OsString, WorkspaceError> {
    use std::os::unix::ffi::OsStringExt;
    Ok(OsString::from_vec(name.to_vec()))
}

/// Platforms without byte-exact file names accept only UTF-8 components, and none that could name a drive or a separator.
#[cfg(not(unix))]
fn os_component(name: &[u8]) -> Result<OsString, WorkspaceError> {
    match std::str::from_utf8(name) {
        Ok(text) if !text.contains(['\\', ':']) => Ok(OsString::from(text)),
        _ => Err(WorkspaceError::invalid(name)),
    }
}

/// The path as relative OS components, with no check against the file system.
pub(crate) fn os_path(path: &RepoPath) -> Result<PathBuf, WorkspaceError> {
    path.components()
        .map(os_component)
        .collect::<Result<PathBuf, _>>()
}

/// The bytes of one file name or link target, or `None` where the platform cannot name them as bytes.
pub(crate) fn os_bytes(name: &std::ffi::OsStr) -> Option<Vec<u8>> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(name.as_bytes().to_vec())
    }
    #[cfg(not(unix))]
    {
        name.to_str().map(|text| text.as_bytes().to_vec())
    }
}

/// Joins `path` under `root` and checks every parent that exists, as [`open_parent`] opens them. A parent that is a symlink or not a directory is refused. The last component may be a symlink; it is never followed here.
pub fn resolve(root: &Path, path: &RepoPath) -> Result<PathBuf, WorkspaceError> {
    let located = root.join(os_path(path)?);
    open_parent(root, path)?;
    Ok(located)
}

/// The directory that holds `path`, reached from `root` one handle at a time with no symlink followed, and the entry's name in it. `None` when the root or a parent is missing; a parent that is a symlink or not a directory is refused.
pub(crate) fn open_parent<'p>(
    root: &Path,
    path: &'p RepoPath,
) -> Result<Option<(DirHandle, &'p [u8])>, WorkspaceError> {
    let mut dir = match DirHandle::open(root) {
        Ok(dir) => dir,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let (parents, name) = path.parents_and_name();
    for parent in parents {
        dir = match dir.child(parent) {
            Ok(Some(child)) => child,
            Ok(None) => return Err(WorkspaceError::invalid(path.as_bytes())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
    }
    Ok(Some((dir, name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_joins_and_validates() {
        let parent = RepoPath::parse(b"a/b").unwrap();
        assert_eq!(
            RepoPath::child(Some(&parent), b"c").unwrap().as_bytes(),
            b"a/b/c"
        );
        assert_eq!(RepoPath::child(None, b"c").unwrap().to_string(), "c");
        assert!(RepoPath::child(Some(&parent), b"..").is_err());
    }

    fn path(text: &str) -> RepoPath {
        RepoPath::parse(text.as_bytes()).unwrap()
    }

    #[test]
    fn a_sibling_that_begins_with_the_same_name_is_never_at_or_below_it() {
        for sibling in ["src-old", "src.rs", "src x", "src+1", "src0", "srcs/lib.rs"] {
            let mut map = BTreeMap::new();
            for key in ["sr", "src", sibling, "src/a", "src/deep/b", "srd"] {
                map.insert(key.as_bytes().to_vec(), ());
            }
            let found: Vec<&[u8]> = path("src")
                .at_or_below(&map)
                .map(|(key, ())| key.as_slice())
                .collect();
            assert_eq!(
                found,
                [b"src".as_slice(), b"src/a", b"src/deep/b"],
                "{sibling}"
            );
            assert!(!path(sibling).is_at_or_below(&path("src")), "{sibling}");
        }
        assert!(path("src").is_at_or_below(&path("src")));
        assert!(path("src/lib.rs").is_at_or_below(&path("src")));
        assert!(!path("lib.rs").is_at_or_below(&path("src")));
    }

    #[test]
    fn the_entries_below_a_directory_come_back_without_its_own_key_or_from_the_root() {
        let mut map = BTreeMap::new();
        for key in ["a", "lib-x", "lib/b", "lib/c/d"] {
            map.insert(key.as_bytes().to_vec(), ());
        }
        let below = |prefix: &[u8]| -> Vec<Vec<u8>> {
            entries_at_or_below(&map, prefix)
                .map(|(key, ())| key.clone())
                .collect()
        };
        assert_eq!(below(b"lib"), [b"lib/b".to_vec(), b"lib/c/d".to_vec()]);
        assert_eq!(below(b"").len(), 4);
        assert!(below(b"none").is_empty());
    }

    #[test]
    fn errors_map_to_codes_and_details() {
        let invalid = WorkspaceError::invalid(b"../x");
        assert_eq!(invalid.code(), "invalid_path");
        let folded = FoldError::from(invalid);
        assert_eq!(folded.details().unwrap()["path"], "../x");
        let io = WorkspaceError::from(io::Error::other("x"));
        assert_eq!(io.code(), "workspace_io");
        assert!(FoldError::from(io).details().is_none());
        assert_eq!(
            WorkspaceError::TargetNotEmpty("t".into()).code(),
            "target_not_empty"
        );
        let tree = WorkspaceError::from(TreeError::Unsorted);
        assert_eq!(tree.code(), "tree_invalid");
        let changed = FoldError::from(WorkspaceError::ChangedDuringSync(vec!["a".into()]));
        assert_eq!(changed.code(), "workspace_changed_during_sync");
        assert_eq!(changed.details().unwrap()["paths"][0], "a");
        assert_eq!(
            changed.message(),
            "1 workspace paths changed since the checkpoint"
        );
    }
}
