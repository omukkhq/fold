use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use fold_core::tree::{TreeError, validate_name};
use fold_core::{FoldError, codes};
use serde_json::json;
use thiserror::Error;

use crate::objects::ObjectError;

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
}

impl WorkspaceError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidPath(_) => codes::INVALID_PATH,
            Self::TargetNotEmpty(_) => codes::TARGET_NOT_EMPTY,
            Self::Object(error) => error.code(),
            Self::Tree(error) => error.code(),
            Self::Io(_) => codes::WORKSPACE_IO,
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
                folded.with_details(json!({ "path": path }))
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

/// Joins `path` under `root` one component at a time and checks every parent that exists. A parent that is a symlink or not a directory is refused, so no write can leave `root`. The last component may be a symlink; it is never followed here.
pub fn resolve(root: &Path, path: &RepoPath) -> Result<PathBuf, WorkspaceError> {
    let names: Vec<&[u8]> = path.components().collect();
    let mut current = root.to_path_buf();
    for (position, name) in names.iter().enumerate() {
        current.push(os_component(name)?);
        if position + 1 == names.len() {
            break;
        }
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(WorkspaceError::invalid(path.as_bytes())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(current)
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
    }
}
