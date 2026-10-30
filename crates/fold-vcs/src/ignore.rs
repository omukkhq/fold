use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ::ignore::Match;
use ::ignore::gitignore::{Gitignore, GitignoreBuilder};
use fold_core::tree::FlatTree;

use crate::lock;
use crate::paths::{RepoPath, entries_at_or_below, open_parent, os_path};

pub const GITIGNORE: &str = ".gitignore";
pub const FOLDIGNORE: &str = ".foldignore";
/// The project's local ignore file in `.fold/`, which `.git/info/exclude` links to.
pub const EXCLUDE: &str = "exclude";
/// The comment fold keeps at the top of every project's `.fold/exclude`, above the user's lines.
pub const EXCLUDE_NOTE: &str =
    "# fold writes this file. To keep a path, add a ! line to .foldignore.";

/// The lines fold keeps in every project's `.fold/exclude`, after the user's own: one for each folder of fold's own skills (`layout::fold_skills`), which fold refreshes after each upgrade and every session reads from the project root, and one for Claude Code's settings of one person on one machine. Each skill has a line of its own, so a skill of the user's whose name starts with `fold-` stays tracked, and a role added in an upgrade gets its line at the next open. A `!` line of a `.gitignore` or `.foldignore` brings one back, and a file main tracks already stays tracked.
pub fn claude_excludes() -> Vec<String> {
    fold_core::layout::fold_skills()
        .map(|skill| format!("/{}/{skill}/", fold_core::layout::SKILLS_DIR))
        .chain([format!("/{}", fold_core::layout::LOCAL_SETTINGS_FILE)])
        .collect()
}
/// fold's data folder, whose `exclude` file holds the project's local rules.
const FOLD_DIR: &str = ".fold";

/// A folder and the name of one ignore file in it.
type MatcherKey = (Vec<u8>, &'static str);

/// The `.gitignore` and `.foldignore` files of one workspace, read from the workspace itself, and the project's `.fold/exclude`: no global excludes, no `.ignore` files, no parent folder. Each ignore file applies below its own folder, and the exclude file to the whole tree. A `.foldignore` rule at any level wins over a `.gitignore` rule at any level, so a `.foldignore` can add patterns or bring a file back with `!`; within each kind the deepest file that has a rule for the path decides. This is the order the `ignore` crate's walker gives a custom ignore file. The exclude file comes last, as `.git/info/exclude` does for Git, so a `!` line of either kind of file brings back a path it excludes and Git and fold always agree.
///
/// Files are read when first needed and kept until [`IgnoreRules::reload`], which every scan calls first, so the rules always follow the files on disk.
#[derive(Debug)]
pub struct IgnoreRules {
    /// `None` for rules that ignore nothing.
    root: Option<PathBuf>,
    /// The project's `.fold/exclude`: in the root's own `.fold/` for the project folder, in the `.fold/` that holds it for a workspace and for fold's check directory.
    exclude: Option<PathBuf>,
    cache: Mutex<HashMap<MatcherKey, Option<Arc<Gitignore>>>>,
}

impl IgnoreRules {
    pub fn new(root: &Path) -> Self {
        // Any root that is neither a workspace nor fold's check directory is a project folder, also one that lies below a folder named `.fold`.
        let fold_dir = root
            .parent()
            .filter(|parent| {
                parent.ends_with(fold_core::layout::WS_DIR)
                    || parent.ends_with(fold_core::layout::CHECKS_DIR)
            })
            .and_then(Path::parent)
            .map_or_else(|| root.join(FOLD_DIR), Path::to_path_buf);
        Self {
            root: Some(root.to_path_buf()),
            exclude: Some(fold_dir.join(EXCLUDE)),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Rules that ignore nothing, for a directory that shows a tree as it is: a tracked file stays tracked there even when an ignore file in the same tree matches it.
    pub fn none() -> Self {
        Self {
            root: None,
            exclude: None,
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

    /// What the rules make of `path` once the tracked tree is known, its parents included: Git's rule, under which an ignore rule applies only to what `tracked` does not hold. Every scan, import, watch and comparison of fold decides by it.
    pub fn cover(&self, tracked: Option<&FlatTree>, path: &RepoPath, is_dir: bool) -> Cover {
        Cover::of(self.is_ignored(path, is_dir), tracked, path, is_dir)
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
        if let Some(matcher) = self.matcher(&[], EXCLUDE) {
            matchers.push((0, matcher));
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

    /// The file is reached from the root one directory handle at a time and opened once, with no symlink followed below the root, so the file read is the one checked.
    fn build(&self, dir: &[u8], name: &str) -> Option<Gitignore> {
        if name == EXCLUDE {
            return self.build_exclude();
        }
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
        rules_of(&folder, &source, &text)
    }

    /// The exclude file is fold's own, so it is read by its path; anything but a regular file there is no rules.
    fn build_exclude(&self) -> Option<Gitignore> {
        let (root, exclude) = (self.root.as_ref()?, self.exclude.as_ref()?);
        if !std::fs::symlink_metadata(exclude).ok()?.is_file() {
            return None;
        }
        let text = std::fs::read(exclude).ok()?;
        rules_of(root, exclude, &text)
    }
}

/// Whether `pattern` is one line of `.gitignore` syntax that Git and fold read as a rule: not blank, not a comment, and a glob the matcher accepts.
pub fn is_ignore_pattern(pattern: &str) -> bool {
    !pattern.trim().is_empty()
        && !pattern.starts_with('#')
        && !pattern.chars().any(char::is_control)
        && GitignoreBuilder::new("/").add_line(None, pattern).is_ok()
}

/// Whether `pattern` is a `!` line that would bring fold's data folder back for Git after the `/.fold/` line of the exclude file, so `git status` would list it and a `git add` would take it in.
pub fn brings_back_fold_dir(pattern: &str) -> bool {
    let mut builder = GitignoreBuilder::new("/");
    builder.add_line(None, pattern).is_ok()
        && builder
            .build()
            .is_ok_and(|rules| rules.matched(Path::new(FOLD_DIR), true).is_whitelist())
}

/// The rules of one ignore file read from `source`, matching below `folder`. Lines are read as the `ignore` crate reads a file: up to the first line that is not UTF-8, with a byte order mark at the start dropped.
fn rules_of(folder: &Path, source: &Path, text: &[u8]) -> Option<Gitignore> {
    let mut builder = GitignoreBuilder::new(folder);
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
        let _ = builder.add_line(Some(source.to_path_buf()), line);
    }
    builder.build().ok()
}

/// The ignore files of one directory in the order they decide: each `.foldignore` from the deepest folder up, then each `.gitignore` the same way, then the project's exclude file. A walk resolves it once per directory and checks every entry against it without taking the cache lock again.
pub(crate) struct DirRules {
    /// Each matcher with the length of its folder's path plus the slash after it, where the path it matches starts.
    matchers: Vec<(usize, Arc<Gitignore>)>,
}

impl DirRules {
    /// True when an ignore rule matches the entry `path` of this directory.
    pub(crate) fn is_ignored(&self, path: &RepoPath, is_dir: bool) -> bool {
        self.matches(path.as_bytes(), is_dir)
    }

    /// [`IgnoreRules::cover`] for the entry `path` of this directory, whose parents a walk already found not covered.
    pub(crate) fn cover(&self, tracked: Option<&FlatTree>, path: &RepoPath, is_dir: bool) -> Cover {
        Cover::of(self.is_ignored(path, is_dir), tracked, path, is_dir)
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

/// What the ignore rules make of one path. As in Git, a rule applies only to a path that is not tracked: a tracked file stays tracked when a rule matches it, so its changes and its deletion count, and an ignored directory still holds the tracked paths below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cover {
    /// No ignore rule covers the path.
    Open,
    /// An ignore rule covers the path, or a parent of it, but the tracked tree holds the path, or for a directory a path below it. Only tracked paths count at and below it, so a walk reads no more of an ignored directory than the tracked paths in it.
    Tracked,
    /// An ignore rule covers the path and nothing tracked is there.
    Ignored,
}

impl Cover {
    /// The cover of a path that an ignore rule covers when `ignored` is true.
    pub(crate) fn of(
        ignored: bool,
        tracked: Option<&FlatTree>,
        path: &RepoPath,
        is_dir: bool,
    ) -> Self {
        if !ignored {
            Self::Open
        } else if tracks(tracked, path, is_dir) {
            Self::Tracked
        } else {
            Self::Ignored
        }
    }

    /// An ignore rule covers the path, tracked or not.
    pub fn is_covered(self) -> bool {
        self != Self::Open
    }
}

/// Whether `tracked` holds `path`: the entry itself for a file, any entry below it for a directory.
fn tracks(tracked: Option<&FlatTree>, path: &RepoPath, is_dir: bool) -> bool {
    let Some(tracked) = tracked else {
        return false;
    };
    let bytes = path.as_bytes();
    if is_dir {
        entries_at_or_below(tracked, bytes).any(|(found, _)| found.len() > bytes.len())
    } else {
        tracked.contains_key(bytes)
    }
}

/// The names directly inside `dir` that `tracked` holds, as a file or as a directory with entries below it, in name order: what a walk of an ignored directory looks at instead of listing it.
pub(crate) fn tracked_names(
    tracked: Option<&FlatTree>,
    dir: Option<&RepoPath>,
) -> BTreeSet<Vec<u8>> {
    let Some(tracked) = tracked else {
        return BTreeSet::new();
    };
    let prefix = dir.map_or(&[][..], RepoPath::as_bytes);
    let start = if prefix.is_empty() {
        0
    } else {
        prefix.len() + 1
    };
    entries_at_or_below(tracked, prefix)
        .filter_map(|(path, _)| path.get(start..))
        .filter_map(|rest| rest.split(|&byte| byte == b'/').next())
        .filter(|name| !name.is_empty())
        .map(<[u8]>::to_vec)
        .collect()
}

/// The tracked tree of one workspace, which its scanner and the watcher share: the scanner sets it when the tree it compares against moves, and the watcher decides by it which changes below an ignored directory count.
#[derive(Debug, Default)]
pub struct TrackedSet(Mutex<Arc<FlatTree>>);

impl TrackedSet {
    pub fn new(tree: FlatTree) -> Self {
        Self(Mutex::new(Arc::new(tree)))
    }

    pub fn set(&self, tree: FlatTree) {
        *lock(&self.0) = Arc::new(tree);
    }

    pub fn get(&self) -> Arc<FlatTree> {
        Arc::clone(&lock(&self.0))
    }
}
