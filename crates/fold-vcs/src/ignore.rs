use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use ::ignore::Match;
use ::ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::paths::{RepoPath, os_path, resolve};

pub const GITIGNORE: &str = ".gitignore";
pub const FOLDIGNORE: &str = ".foldignore";

/// A folder and the name of one ignore file in it.
type MatcherKey = (Vec<u8>, &'static str);

/// The `.gitignore` and `.foldignore` files of one workspace, read from the workspace itself and nowhere else: no global excludes, no `.git/info/exclude`, no `.ignore` files, no parent folder. Each file applies below its own folder. A `.foldignore` rule at any level wins over a `.gitignore` rule at any level, so a `.foldignore` can add patterns or bring a file back with `!`; within each kind the deepest file that has a rule for the path decides. This is the order the `ignore` crate's walker gives a custom ignore file.
///
/// Files are read when first needed and kept until [`IgnoreRules::reload`], which every scan calls first, so the rules always follow the files on disk.
#[derive(Debug)]
pub struct IgnoreRules {
    /// `None` for rules that ignore nothing.
    root: Option<PathBuf>,
    cache: Mutex<HashMap<MatcherKey, Option<Arc<Gitignore>>>>,
}

impl IgnoreRules {
    pub fn new(root: &Path) -> Self {
        Self {
            root: Some(root.to_path_buf()),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Rules that ignore nothing, for a directory that shows a tree as it is: a tracked file stays tracked there even when an ignore file in the same tree matches it.
    pub fn none() -> Self {
        Self {
            root: None,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Forgets every file read so far.
    pub fn reload(&self) {
        self.cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// True when an ignore rule matches the path or any parent directory. A directory that is ignored hides everything below it, even a `!` rule.
    pub fn is_ignored(&self, path: &RepoPath, is_dir: bool) -> bool {
        let bytes = path.as_bytes();
        let parents = bytes
            .iter()
            .enumerate()
            .filter(|&(_, &byte)| byte == b'/')
            .map(|(at, _)| at);
        for end in parents {
            if self.matches(&bytes[..end], true) {
                return true;
            }
        }
        self.matches(bytes, is_dir)
    }

    /// The check a directory walk makes for each entry. The parents were already walked, so they are known not to be ignored and only the entry itself is matched.
    pub(crate) fn is_ignored_below(&self, path: &RepoPath, is_dir: bool) -> bool {
        self.matches(path.as_bytes(), is_dir)
    }

    fn matches(&self, path: &[u8], is_dir: bool) -> bool {
        let starts: Vec<usize> = std::iter::once(0)
            .chain(
                path.iter()
                    .enumerate()
                    .filter(|&(_, &byte)| byte == b'/')
                    .map(|(at, _)| at + 1),
            )
            .collect();
        for name in [FOLDIGNORE, GITIGNORE] {
            for &start in starts.iter().rev() {
                let dir = &path[..start.saturating_sub(1)];
                let Some(matcher) = self.matcher(dir, name) else {
                    continue;
                };
                let Ok(relative) = RepoPath::parse(&path[start..]).and_then(|rel| os_path(&rel))
                else {
                    return false;
                };
                match matcher.matched(&relative, is_dir) {
                    Match::Ignore(_) => return true,
                    Match::Whitelist(_) => return false,
                    Match::None => {}
                }
            }
        }
        false
    }

    fn matcher(&self, dir: &[u8], name: &'static str) -> Option<Arc<Gitignore>> {
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        let key = (dir.to_vec(), name);
        if let Some(found) = cache.get(&key) {
            return found.clone();
        }
        let built = self.build(dir, name).map(Arc::new);
        cache.insert(key, built.clone());
        built
    }

    fn build(&self, dir: &[u8], name: &str) -> Option<Gitignore> {
        let root = self.root.as_ref()?;
        let folder = if dir.is_empty() {
            root.clone()
        } else {
            resolve(root, &RepoPath::parse(dir).ok()?).ok()?
        };
        let file = folder.join(name);
        if !fs::symlink_metadata(&folder).ok()?.is_dir()
            || !fs::symlink_metadata(&file).ok()?.is_file()
        {
            return None;
        }
        let mut builder = GitignoreBuilder::new(&folder);
        // A line that does not parse is skipped, as Git skips it.
        let _ = builder.add(&file);
        builder.build().ok()
    }
}
