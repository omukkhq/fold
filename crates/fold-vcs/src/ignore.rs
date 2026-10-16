use std::collections::HashMap;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ::ignore::Match;
use ::ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::lock;
use crate::paths::{RepoPath, open_parent, os_path};

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
        lock(&self.cache).clear();
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

    /// The check a directory walk makes for one entry. The parents were already walked, so they are known not to be ignored and only the entry itself is matched. A walk that checks every entry of a directory resolves [`IgnoreRules::dir_rules`] once instead.
    pub(crate) fn is_ignored_below(&self, path: &RepoPath, is_dir: bool) -> bool {
        self.matches(path.as_bytes(), is_dir)
    }

    /// The ignore files that apply to the entries directly inside `dir`, `None` being the root.
    pub(crate) fn dir_rules(&self, dir: Option<&RepoPath>) -> DirRules {
        self.rules_in(dir.map_or(&[][..], RepoPath::as_bytes))
    }

    fn rules_in(&self, dir: &[u8]) -> DirRules {
        let mut folders: Vec<&[u8]> = vec![&[]];
        if !dir.is_empty() {
            folders.extend(
                dir.iter()
                    .enumerate()
                    .filter(|&(_, &byte)| byte == b'/')
                    .map(|(at, _)| &dir[..at]),
            );
            folders.push(dir);
        }
        let mut matchers = Vec::new();
        for name in [FOLDIGNORE, GITIGNORE] {
            for folder in folders.iter().rev() {
                if let Some(matcher) = self.matcher(folder, name) {
                    let start = if folder.is_empty() {
                        0
                    } else {
                        folder.len() + 1
                    };
                    matchers.push((start, matcher));
                }
            }
        }
        DirRules { matchers }
    }

    fn matches(&self, path: &[u8], is_dir: bool) -> bool {
        let parent = path
            .iter()
            .rposition(|&byte| byte == b'/')
            .map_or(&[][..], |at| &path[..at]);
        self.rules_in(parent).matches(path, is_dir)
    }

    fn matcher(&self, dir: &[u8], name: &'static str) -> Option<Arc<Gitignore>> {
        let mut cache = lock(&self.cache);
        let key = (dir.to_vec(), name);
        if let Some(found) = cache.get(&key) {
            return found.clone();
        }
        let built = self.build(dir, name).map(Arc::new);
        cache.insert(key, built.clone());
        built
    }

    /// The file is reached from the root one directory handle at a time and opened once, with no symlink followed below the root, so the file read is the one checked. Lines are read as the `ignore` crate reads a file: up to the first line that is not UTF-8, with a byte order mark at the start dropped.
    fn build(&self, dir: &[u8], name: &str) -> Option<Gitignore> {
        let root = self.root.as_ref()?;
        let folder = if dir.is_empty() {
            None
        } else {
            Some(RepoPath::parse(dir).ok()?)
        };
        let file = RepoPath::child(folder.as_ref(), name.as_bytes()).ok()?;
        let (parent, file_name) = open_parent(root, &file).ok()??;
        let (mut opened, _) = parent.open_regular(file_name).ok()??;
        let mut text = Vec::new();
        opened.read_to_end(&mut text).ok()?;
        let folder = match &folder {
            Some(folder) => root.join(os_path(folder)),
            None => root.clone(),
        };
        let source = folder.join(name);
        let mut builder = GitignoreBuilder::new(&folder);
        for (at, line) in text.lines().enumerate() {
            let Ok(line) = line else {
                break;
            };
            let line = if at == 0 {
                line.trim_start_matches('\u{feff}')
            } else {
                &line
            };
            // A line that does not parse is skipped, as Git skips it.
            let _ = builder.add_line(Some(source.clone()), line);
        }
        builder.build().ok()
    }
}

/// The ignore files of one directory in the order they decide: each `.foldignore` from the deepest folder up, then each `.gitignore` the same way. A walk resolves it once per directory and checks every entry against it without taking the cache lock again.
pub(crate) struct DirRules {
    /// Each matcher with the length of its folder's path plus the slash after it, where the path it matches starts.
    matchers: Vec<(usize, Arc<Gitignore>)>,
}

impl DirRules {
    /// True when an ignore rule matches the entry `path` of this directory.
    pub(crate) fn is_ignored(&self, path: &RepoPath, is_dir: bool) -> bool {
        self.matches(path.as_bytes(), is_dir)
    }

    fn matches(&self, path: &[u8], is_dir: bool) -> bool {
        for (start, matcher) in &self.matchers {
            let Some(Ok(relative)) = path.get(*start..).map(RepoPath::parse) else {
                return false;
            };
            let relative = os_path(&relative);
            match matcher.matched(&relative, is_dir) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => {}
            }
        }
        false
    }
}
