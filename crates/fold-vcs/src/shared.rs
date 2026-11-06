use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::atomic::write_atomic;
use crate::ignore::IgnoreRules;
use crate::paths::{RepoPath, os_path};
use crate::platform::DirHandle;

/// The project's list of shared reference paths in `.fold/`: one project-relative path per line. Like `.fold/exclude` it belongs to this machine and is never part of a tree.
pub const SHARED: &str = "shared";

/// The comment fold keeps at the top of `.fold/shared`.
pub const SHARED_NOTE: &str = "# fold writes this file: ignored paths that every workspace links to, read-only. Change it with fld share and fld unshare.";

/// The links fold made in one workspace, in that workspace's own `.fold/`, so a later sync removes the link of a path that is no longer shared and leaves every other link alone.
const LINKS: &str = "shared-links";

/// Every valid path of the list text `text`, in path order. A line that is blank, a comment or no valid tree path is left out, so a hand edit never breaks a create or a sync.
pub fn parse_shared(text: &str) -> Vec<RepoPath> {
    let paths: BTreeSet<RepoPath> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| RepoPath::parse(line.trim_end_matches('/').as_bytes()).ok())
        .collect();
    paths.into_iter().collect()
}

/// The shared paths of the project whose fold folder is `fold_dir`; none when the file is missing. Blocking.
pub fn read_shared(fold_dir: &Path) -> io::Result<Vec<RepoPath>> {
    match std::fs::read_to_string(fold_dir.join(SHARED)) {
        Ok(text) => Ok(parse_shared(&text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

/// Replaces the list with `paths`, under [`SHARED_NOTE`]. Blocking.
pub fn write_shared(fold_dir: &Path, paths: &[RepoPath]) -> io::Result<()> {
    let mut text = format!("{SHARED_NOTE}\n");
    for path in paths {
        text.push_str(&path.to_string());
        text.push('\n');
    }
    write_atomic(&fold_dir.join(SHARED), text.as_bytes())
}

/// Why a path cannot be shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotShareable {
    /// Nothing is at the path in the project folder.
    Missing,
    /// The path or a folder above it is a symbolic link, which fold never follows.
    Link,
    /// No ignore rule leaves the path out, so main tracks it and every workspace holds it already.
    NotIgnored,
    /// The path lies at or below another shared path, or holds one.
    Overlaps,
}

/// A path that may be shared, and whether its link needs an anchored line in `.fold/exclude`: a folder that only a folder pattern such as `design/` leaves out would have its link, which is no folder, recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shareable {
    pub path: RepoPath,
    pub needs_exclude: bool,
}

/// Checks that `path` of the project folder `root` can be shared beside `shared`: a file or folder that is there, reached without a symbolic link, that an ignore rule of `rules` leaves out, and that neither lies within nor holds another shared path, so no link is ever made inside another. Blocking.
pub fn check_shareable(
    root: &Path,
    rules: &IgnoreRules,
    shared: &[RepoPath],
    path: RepoPath,
) -> Result<Shareable, NotShareable> {
    let mut at = root.to_path_buf();
    let mut meta = None;
    for name in path.components() {
        at.push(OsStr::from_bytes(name));
        let found = std::fs::symlink_metadata(&at).map_err(|_| NotShareable::Missing)?;
        if found.file_type().is_symlink() {
            return Err(NotShareable::Link);
        }
        meta = Some(found);
    }
    let is_dir = meta.is_some_and(|meta| meta.is_dir());
    rules.reload();
    if !rules.is_ignored(&path, is_dir) {
        return Err(NotShareable::NotIgnored);
    }
    if shared
        .iter()
        .any(|other| other != &path && (other.is_at_or_below(&path) || path.is_at_or_below(other)))
    {
        return Err(NotShareable::Overlaps);
    }
    Ok(Shareable {
        needs_exclude: is_dir && !rules.is_ignored(&path, false),
        path,
    })
}

/// What [`link_shared`] did in one workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Linked {
    /// The shared paths whose link the workspace holds.
    pub linked: usize,
    /// The shared paths it could not link, because something else is at the path or a folder above it is a link or no folder.
    pub skipped: usize,
}

/// Makes the workspace `ws` hold one symbolic link for each of `shared`, at the same path, to the absolute path of that file or folder in the project folder `root`, so a worker reads the project folder's current copy and no copy takes disk space. Missing folders above a link are made; a folder above it that is a link, or anything already at the path that is not fold's link, leaves that path without one. A link fold made at an earlier create or sync for a path no longer in `shared` is removed, only while it still is fold's link. A shared path that is no longer ignored in `rules` is tracked and in the workspace's files already, so it gets no link. fold calls it only when it creates or syncs a workspace, the points at which it writes into one. Blocking.
pub fn link_shared(
    root: &Path,
    rules: &IgnoreRules,
    ws: &Path,
    shared: &[RepoPath],
) -> io::Result<Linked> {
    let workspace = DirHandle::open(ws)?;
    let previous = std::fs::read_to_string(ws.join(".fold").join(LINKS))
        .map(|text| parse_shared(&text))
        .unwrap_or_default();
    rules.reload();
    let mut outcome = Linked::default();
    for gone in previous.iter().filter(|path| !shared.contains(path)) {
        if let Some((dir, name)) = parent_of(&workspace, gone, false)?
            && dir.read_link(name)?.as_deref() == Some(target_of(root, gone).as_bytes())
        {
            dir.remove_file(name)?;
        }
    }
    let mut kept = Vec::new();
    for path in shared {
        let Ok(meta) = std::fs::symlink_metadata(root.join(os_path(path))) else {
            continue;
        };
        if !rules.is_ignored(path, meta.is_dir()) && !rules.is_ignored(path, false) {
            continue;
        }
        let target = target_of(root, path);
        let Some((dir, name)) = parent_of(&workspace, path, true)? else {
            outcome.skipped += 1;
            continue;
        };
        match dir.read_link(name) {
            Ok(Some(current)) if current == target.as_bytes() => {}
            Ok(_) => {
                outcome.skipped += 1;
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                dir.symlink(target.as_bytes(), name)?;
            }
            Err(error) => return Err(error),
        }
        outcome.linked += 1;
        kept.push(path.clone());
    }
    let own = ws.join(".fold");
    if kept.is_empty() && previous.is_empty() {
        return Ok(outcome);
    }
    std::fs::create_dir_all(&own)?;
    let text: String = kept.iter().map(|path| format!("{path}\n")).collect();
    write_atomic(&own.join(LINKS), text.as_bytes())?;
    Ok(outcome)
}

/// The absolute path a link to `path` holds.
fn target_of(root: &Path, path: &RepoPath) -> std::ffi::OsString {
    root.join(os_path(path)).into_os_string()
}

/// The folder of the workspace that holds `path`, reached one folder at a time with no link followed, and the entry's name; folders that are missing are made when `create` is set. `None` when a folder above it is missing, a link or no folder.
fn parent_of<'p>(
    workspace: &DirHandle,
    path: &'p RepoPath,
    create: bool,
) -> io::Result<Option<(DirHandle, &'p [u8])>> {
    let (parents, name) = path.parents_and_name();
    let mut dir = workspace.try_clone()?;
    for parent in parents {
        let next = if create {
            dir.child_or_create(parent)?.map(|(child, _)| child)
        } else {
            match dir.child(parent) {
                Ok(child) => child,
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            }
        };
        let Some(next) = next else {
            return Ok(None);
        };
        dir = next;
    }
    Ok(Some((dir, name)))
}

/// The ignored files and folders of the project folder `root` that no workspace holds and that `shared` does not share, as a list of at most `limit` paths in path order with the number of all of them. A folder that is ignored whole counts once, with a `/` at its end, and is not read further. fold's own data, Git's folder and the files fold keeps out of main itself, its skills and the local Claude Code settings, are left out, as no worker needs them. Names only: no file is opened. Blocking.
pub fn unshared_ignored(
    root: &Path,
    rules: &IgnoreRules,
    shared: &[RepoPath],
    limit: usize,
) -> io::Result<(Vec<String>, usize)> {
    rules.reload();
    let own: Vec<RepoPath> = crate::ignore::claude_excludes()
        .iter()
        .filter_map(|line| {
            RepoPath::parse(
                line.trim_start_matches('/')
                    .trim_end_matches('/')
                    .as_bytes(),
            )
            .ok()
        })
        .collect();
    let mut found = Vec::new();
    let mut count = 0;
    let mut stack = vec![(root.to_path_buf(), None::<RepoPath>)];
    while let Some((dir, parent)) = stack.pop() {
        let mut names: Vec<_> = match std::fs::read_dir(&dir) {
            Ok(entries) => entries.filter_map(Result::ok).collect(),
            Err(_) => continue,
        };
        names.sort_by_key(std::fs::DirEntry::file_name);
        for entry in names.into_iter().rev() {
            let Ok(path) = RepoPath::child(parent.as_ref(), entry.file_name().as_bytes()) else {
                continue;
            };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let is_dir = kind.is_dir();
            if shared.iter().any(|other| path.is_at_or_below(other))
                || own.iter().any(|other| path.is_at_or_below(other))
            {
                continue;
            }
            let holds_shared = shared.iter().any(|other| other.is_at_or_below(&path));
            if !holds_shared && rules.is_ignored(&path, is_dir) {
                count += 1;
                found.push(if is_dir {
                    format!("{path}/")
                } else {
                    path.to_string()
                });
                continue;
            }
            if is_dir {
                stack.push((entry.path(), Some(path)));
            }
        }
    }
    found.sort();
    found.truncate(limit);
    Ok((found, count))
}
