use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use fold_core::config::{check_branch, check_remote, check_remote_name};
use fold_core::{FoldError, ProjectPaths, codes};
use serde_json::json;

use crate::git::{CommitId, Git, failed, unexpected_output, wait_error};

/// The ref fold writes its commits to. The `[git]` branch, which the project folder has checked out, moves to this ref's commit only by fast-forward, so a branch tip that is not an ancestor of it is a commit made outside fold, also after a stop between the write and the move.
pub const FOLD_HEAD_REF: &str = "refs/fold/head";

/// The branch ref the commits of main are written to.
pub fn branch_ref(branch: &str) -> String {
    format!("refs/heads/{branch}")
}

/// The ref that holds the branch of the remote `name` as the last fetch found it.
pub fn remote_ref(name: &str, branch: &str) -> String {
    format!("refs/remotes/{name}/{branch}")
}

/// fold's data folder, which `git status` in the project must never list.
const EXCLUDE_LINE: &str = "/.fold/";

/// Where `info/exclude` of the project's repository points: the project's `.fold/exclude`, relative to the link.
pub const EXCLUDE_LINK: &str = "../../.fold/exclude";

/// The project's Git repository at [`ProjectPaths::git_dir`], whose working tree is the project folder, after making sure its `info/exclude` is the project's `.fold/exclude` with fold's data folder in it. It never creates one: `fld init` does. Anything but a real directory in its place, a symlink included, is refused, so fold never writes Git data through it and Git never finds an enclosing repository instead. Blocking.
pub fn project_repository(paths: &ProjectPaths) -> Result<PathBuf, FoldError> {
    let repo = paths.git_dir();
    match fs::symlink_metadata(&repo) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return Err(no_repository().with_details([("reason", json!("not_a_directory"))]));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(no_repository().with_details([("reason", json!("missing"))]));
        }
        Err(error) => return Err(repository_io(&error)),
    }
    exclude_fold_dir(&repo)?;
    Ok(repo)
}

fn no_repository() -> FoldError {
    FoldError::new(
        codes::INVALID_REQUEST,
        "the project folder has no Git repository of its own in .git",
    )
    .with_hint("re-create the project with `fld init`")
}

/// Makes `info/exclude` of `repo`, the project's `.git`, a symlink to the project's `.fold/exclude`, so Git and fold read the same local rules, and makes sure that file lists [`EXCLUDE_LINE`]. The lines an `info/exclude` of its own holds move into `.fold/exclude` first, after the lines already there, each once. The file is written to a temporary name and renamed into place, and so is the link. Blocking.
pub fn exclude_fold_dir(repo: &Path) -> Result<(), FoldError> {
    let io = |error: io::Error| repository_io(&error);
    let _writing = crate::lock(&EXCLUDE_WRITES);
    let fold_dir = repo.parent().unwrap_or(repo).join(".fold");
    let target = fold_dir.join("exclude");
    let link = repo.join("info").join("exclude");
    let linked = fs::read_link(&link).is_ok_and(|to| to == Path::new(EXCLUDE_LINK));
    let moved = if linked {
        String::new()
    } else {
        match fs::read_to_string(&link) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(io(error)),
        }
    };
    let current = match fs::read_to_string(&target) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(io(error)),
    };
    let mut lines: Vec<&str> = current.lines().collect();
    for line in moved.lines().chain([EXCLUDE_LINE]) {
        if !line.trim().is_empty() && !lines.contains(&line) {
            lines.push(line);
        }
    }
    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    if text != current {
        write_exclude(&fold_dir, &text).map_err(io)?;
    }
    if !linked {
        let info = repo.join("info");
        fs::create_dir_all(&info).map_err(io)?;
        let temporary = info.join(".exclude.fold");
        let _ = fs::remove_file(&temporary);
        std::os::unix::fs::symlink(EXCLUDE_LINK, &temporary)
            .and_then(|()| fs::rename(&temporary, &link))
            .map_err(io)?;
    }
    Ok(())
}

/// Held while `.fold/exclude` is read and written again, so two writers in this process never drop each other's lines.
static EXCLUDE_WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Replaces `.fold/exclude` in `fold_dir` with `text` by renaming a file of a name of its own into place, so a reader sees the old file or the new one. The caller holds [`EXCLUDE_WRITES`]. A missing `fold_dir` is `NotFound` and is not created, so a project whose `.fold/` was deleted never gets it back from here.
fn write_exclude(fold_dir: &Path, text: &str) -> io::Result<()> {
    let temporary = fold_dir.join(format!(".exclude-{}.tmp", std::process::id()));
    let written =
        fs::write(&temporary, text).and_then(|()| fs::rename(&temporary, fold_dir.join("exclude")));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

/// Adds each of `patterns` that `.fold/exclude` in `fold_dir` does not list yet at its end, keeping [`EXCLUDE_LINE`] in it, and returns the patterns it added. Blocking.
pub fn add_exclude_patterns(
    fold_dir: &Path,
    patterns: &[String],
) -> Result<Vec<String>, FoldError> {
    let io = |error: io::Error| repository_io(&error);
    let _writing = crate::lock(&EXCLUDE_WRITES);
    let current = match fs::read_to_string(fold_dir.join("exclude")) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(io(error)),
    };
    let mut lines: Vec<String> = current.lines().map(str::to_owned).collect();
    let mut added = Vec::new();
    for pattern in [EXCLUDE_LINE.to_owned()].iter().chain(patterns) {
        if !lines.contains(pattern) {
            lines.push(pattern.clone());
            if pattern != EXCLUDE_LINE {
                added.push(pattern.clone());
            }
        }
    }
    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    if text != current {
        write_exclude(fold_dir, &text).map_err(io)?;
    }
    Ok(added)
}

/// Moves `branch` of `repo` to `new`, only while it points to `old`, or only while it does not exist when `old` is `None`. Blocking.
pub fn move_branch(
    git: &Git,
    repo: &Path,
    branch: &str,
    new: &CommitId,
    old: Option<&CommitId>,
) -> Result<(), FoldError> {
    check_branch(branch)?;
    let old = old.map_or("0000000000000000000000000000000000000000", CommitId::as_str);
    git.output(
        repo,
        &["update-ref", &branch_ref(branch), new.as_str(), old],
        "update-ref",
    )?;
    Ok(())
}

/// `args` for a Git command in the project folder, naming the repository and the working tree, so Git never looks for a repository above the folder.
fn in_folder(paths: &ProjectPaths, args: &[&str]) -> Vec<OsString> {
    let mut git_dir = OsString::from("--git-dir=");
    git_dir.push(paths.git_dir());
    let mut work_tree = OsString::from("--work-tree=");
    work_tree.push(paths.root());
    let mut all = vec![git_dir, work_tree];
    all.extend(args.iter().map(OsString::from));
    all
}

/// Sets the index of the project folder to the tree of `commit` and refreshes its file stats, without touching a file: for the first commit, whose tree is the folder's files. Blocking.
pub fn index_to(git: &Git, paths: &ProjectPaths, commit: &CommitId) -> Result<(), FoldError> {
    let root = paths.root();
    git.output(
        root,
        &in_folder(paths, &["read-tree", commit.as_str()]),
        "read-tree",
    )?;
    let refreshed = git.status(
        root,
        &in_folder(paths, &["update-index", "-q", "--refresh"]),
        "update-index",
    )?;
    // 1 names a path whose file differs from the index, such as one the import skipped.
    match refreshed.code() {
        Some(0 | 1) => Ok(()),
        _ => Err(failed("update-index", refreshed)),
    }
}

/// The branch `HEAD` of `repo` names, or `None` when `HEAD` is detached. Blocking.
pub fn checked_out_branch(git: &Git, repo: &Path) -> Result<Option<String>, FoldError> {
    // `-q` exits 1, with nothing on stderr, for a detached `HEAD`.
    let Some(out) = git.output_or_none(repo, &["symbolic-ref", "-q", "HEAD"], "symbolic-ref")?
    else {
        return Ok(None);
    };
    let text = String::from_utf8(out).map_err(|_| unexpected_output("symbolic-ref"))?;
    Ok(text
        .trim_end()
        .strip_prefix("refs/heads/")
        .map(str::to_owned))
}

/// Whether the index and the tracked files of the project folder match `commit`, as Git checks before `updateInstead` updates a working tree; untracked files do not count. Blocking.
pub fn folder_clean(git: &Git, paths: &ProjectPaths, commit: &CommitId) -> Result<bool, FoldError> {
    let root = paths.root();
    let quiet = |args: &[&str], what: &'static str| git.check(root, &in_folder(paths, args), what);
    // Refreshing the stats first keeps a file whose stats alone changed from counting as changed; its exit code is what the two checks below answer.
    git.status(
        root,
        &in_folder(
            paths,
            &["update-index", "-q", "--ignore-submodules", "--refresh"],
        ),
        "update-index",
    )?;
    Ok(quiet(
        &["diff-files", "--quiet", "--ignore-submodules", "--"],
        "diff-files",
    )? && quiet(
        &[
            "diff-index",
            "--quiet",
            "--cached",
            "--ignore-submodules",
            commit.as_str(),
            "--",
        ],
        "diff-index",
    )?)
}

/// Whether the index or a tracked file of the project folder differs from `HEAD`, read without writing the index, as a check that must change nothing needs. Untracked files do not count. Blocking.
pub fn folder_has_changes(git: &Git, paths: &ProjectPaths) -> Result<bool, FoldError> {
    let out = git.output(
        paths.root(),
        &in_folder(
            paths,
            &[
                "--no-optional-locks",
                "status",
                "--porcelain",
                "--untracked-files=no",
                "--ignore-submodules",
            ],
        ),
        "status",
    )?;
    Ok(!out.is_empty())
}

/// The ref that names where fold last left the `[git]` branch. A tip somewhere else is a change of the branch that fold did not make.
pub const FOLD_BRANCH_REF: &str = "refs/fold/branch";

/// Git's ref of the newest stash.
pub const STASH_REF: &str = "refs/stash";

/// The ref that holds [`STASH_REF`] as it was when fold turned Git sync on, so a stash made since is a change fold did not make; none is set when there was no stash.
pub const FOLD_STASH_REF: &str = "refs/fold/stash";

/// What the `[git]` branch and `HEAD` of the project folder show, against where fold left them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchState {
    /// `HEAD` names the branch, whose tip is where fold left it, or a commit fold wrote after that, or nothing yet.
    Fold { tip: Option<CommitId> },
    /// The tip descends from `from`, where fold left the branch, with commits fold did not write.
    Ahead { tip: CommitId, from: CommitId },
    /// A change fold did not make: fold neither updates the folder nor moves the branch until it is undone.
    Moved(Moved),
}

/// Why the branch counts as moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Moved {
    /// `HEAD` names another branch, or no branch when it is `None`.
    OtherBranch(Option<String>),
    /// The tip is neither fold's commit nor one of its descendants, as after a reset, an amend or a rebase; `fold` is where fold left it.
    Rewritten { tip: CommitId, fold: CommitId },
    /// A stash was made since fold turned Git sync on, and is still there.
    Stash,
}

impl Moved {
    /// The name the `git.head_moved` event and a refusal give the reason.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::OtherBranch(Some(_)) => "other_branch",
            Self::OtherBranch(None) => "detached_head",
            Self::Rewritten { .. } => "branch_moved",
            Self::Stash => "stash",
        }
    }

    /// The command that brings the folder back to what fold left, without losing a file: the human's own files stay, and `fld record` records what differs.
    pub fn hint(&self, branch: &str) -> String {
        match self {
            Self::OtherBranch(_) => format!("run `git switch {branch}` in the project folder"),
            Self::Rewritten { fold, .. } => format!(
                "run `git reset --soft {}` in the project folder; your files stay as they are and `fld record` records them",
                &fold.as_str()[..12]
            ),
            Self::Stash => "run `git stash pop` in the project folder".to_owned(),
        }
    }
}

/// Reads [`BranchState`] for `branch` of the project folder's repository. `fold_commit` tells whether a commit is one fold wrote, as the sync map knows them, and `fold_last` is the newest of them, which stands in for [`FOLD_BRANCH_REF`] until fold first moved the branch. Blocking.
pub fn branch_state(
    git: &Git,
    paths: &ProjectPaths,
    branch: &str,
    fold_commit: &dyn Fn(&CommitId) -> bool,
    fold_last: Option<&CommitId>,
) -> Result<BranchState, FoldError> {
    let repo = paths.git_dir();
    let checked_out = checked_out_branch(git, &repo)?;
    if checked_out.as_deref() != Some(branch) {
        // A repository with no commit yet, as `git init` leaves it, gets fold's branch when fold writes its first commit.
        let unborn = |name: &str| -> Result<bool, FoldError> {
            Ok(git.ref_commit(&repo, &branch_ref(name))?.is_none())
        };
        if let Some(other) = &checked_out
            && unborn(other)?
            && unborn(branch)?
        {
            return Ok(BranchState::Fold { tip: None });
        }
        return Ok(BranchState::Moved(Moved::OtherBranch(checked_out)));
    }
    // A stash list emptied, or a stash dropped, is no change of the folder's files, so only a stash made since counts.
    if let Some(stash) = git.ref_commit(&repo, STASH_REF)?
        && stash_made_since(
            git,
            &repo,
            &stash,
            git.ref_commit(&repo, FOLD_STASH_REF)?.as_ref(),
        )?
    {
        return Ok(BranchState::Moved(Moved::Stash));
    }
    let Some(tip) = git.ref_commit(&repo, &branch_ref(branch))? else {
        return Ok(BranchState::Fold { tip: None });
    };
    let Some(left) = git
        .ref_commit(&repo, FOLD_BRANCH_REF)?
        .or_else(|| fold_last.cloned())
    else {
        return Ok(BranchState::Fold { tip: Some(tip) });
    };
    if left == tip {
        return Ok(BranchState::Fold { tip: Some(tip) });
    }
    if !is_ancestor(git, &repo, &left, &tip)? {
        return Ok(BranchState::Moved(Moved::Rewritten { tip, fold: left }));
    }
    // A commit fold wrote past where it left the branch is a move whose ref write a stop cut off, and a commit of the human that a revision of fold's merged is one main holds.
    if fold_commit(&tip)
        || fold_last.is_some_and(|last| {
            *last != tip && is_ancestor(git, &repo, &tip, last).unwrap_or(false)
        })
    {
        return Ok(BranchState::Fold { tip: Some(tip) });
    }
    Ok(BranchState::Ahead { tip, from: left })
}

/// Whether `stash`, the newest stash, was made after `seen`, the newest one when fold turned Git sync on, which [`FOLD_STASH_REF`] keeps. A stash commit is not built on the stash before it, so their committer times order them: once the human drops the newest stash, the one below it is older than `seen`. Blocking.
fn stash_made_since(
    git: &Git,
    repo: &Path,
    stash: &CommitId,
    seen: Option<&CommitId>,
) -> Result<bool, FoldError> {
    let Some(seen) = seen else {
        return Ok(true);
    };
    if stash == seen {
        return Ok(false);
    }
    let time = |commit: &CommitId| -> Result<i64, FoldError> {
        let out = git.output(repo, &["log", "-1", "--format=%ct", commit.as_str()], "log")?;
        String::from_utf8(out)
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .ok_or_else(|| unexpected_output("log"))
    };
    Ok(time(stash)? > time(seen)?)
}

/// A cheap fingerprint of the refs [`branch_state`] reads, from their files alone with no `git` process: `HEAD`, the branch's loose ref, `packed-refs` and the stash. It changes when one of them changes, so a watch reads the state only then. Blocking.
pub fn branch_stamp(paths: &ProjectPaths, branch: &str) -> Vec<u8> {
    let repo = paths.git_dir();
    let mut stamp = Vec::new();
    for file in [
        repo.join("HEAD"),
        repo.join("refs/heads").join(branch),
        repo.join("refs/stash"),
        repo.join("packed-refs"),
    ] {
        match fs::read(&file) {
            Ok(bytes) => stamp.extend_from_slice(&bytes),
            Err(_) => stamp.push(0),
        }
        stamp.push(b'\n');
    }
    stamp
}

/// Moves `branch` from `tip` to `new`, a commit that descends from it, while it still points to `tip`, or creates it on `new` when `tip` is `None`, and then records the move in [`FOLD_BRANCH_REF`]. Before the branch moves, the index follows: every path that differs between the two commits gets `new`'s entry when the index still holds `tip`'s, and a path whose index entry is the human's own keeps it, so `git status` shows only the human's changes. An unborn branch is created on `new` with `HEAD` naming it and the index set to `new` whole. False, with nothing moved, when an index entry of a path that differs is the human's own: committing it would undo main's change. Never by force. Blocking.
pub fn move_branch_and_index(
    git: &Git,
    paths: &ProjectPaths,
    branch: &str,
    tip: Option<&CommitId>,
    new: &CommitId,
) -> Result<bool, FoldError> {
    let repo = paths.git_dir();
    match tip {
        None => {
            // The index comes first, so the branch exists only once the folder's Git state is whole.
            index_to(git, paths, new)?;
            move_branch(git, &repo, branch, new, None)?;
            git.output(
                &repo,
                &["symbolic-ref", "HEAD", &branch_ref(branch)],
                "symbolic-ref",
            )?;
        }
        Some(tip) if tip == new => {}
        Some(tip) => {
            if !index_follow(git, paths, tip, new)? {
                return Ok(false);
            }
            move_branch(git, &repo, branch, new, Some(tip))?;
        }
    }
    set_ref(git, &repo, FOLD_BRANCH_REF, Some(new))?;
    Ok(true)
}

/// A mode and an object id, as an index or tree entry names them in Git's text.
type GitEntry = (Vec<u8>, Vec<u8>);

/// One path of `git diff-tree -r -z` between two commits: its old and new mode and object.
struct TreeChange {
    path: Vec<u8>,
    old: Option<GitEntry>,
    new: Option<GitEntry>,
}

const ZERO_MODE: &[u8] = b"000000";

fn entry(mode: &[u8], object: &[u8]) -> Option<GitEntry> {
    (mode != ZERO_MODE).then(|| (mode.to_vec(), object.to_vec()))
}

/// The paths that differ between `old` and `new`. Blocking.
fn tree_changes(
    git: &Git,
    repo: &Path,
    old: &CommitId,
    new: &CommitId,
) -> Result<Vec<TreeChange>, FoldError> {
    let out = git.output(
        repo,
        &[
            "diff-tree",
            "-r",
            "-z",
            "--no-renames",
            "--no-commit-id",
            old.as_str(),
            new.as_str(),
        ],
        "diff-tree",
    )?;
    let mut fields = out.split(|&byte| byte == 0).filter(|part| !part.is_empty());
    let mut changes = Vec::new();
    while let Some(header) = fields.next() {
        let header = header
            .strip_prefix(b":")
            .ok_or_else(|| unexpected_output("diff-tree"))?;
        let parts: Vec<&[u8]> = header.split(|&byte| byte == b' ').collect();
        let [old_mode, new_mode, old_object, new_object, _status] = parts[..] else {
            return Err(unexpected_output("diff-tree"));
        };
        let path = fields
            .next()
            .ok_or_else(|| unexpected_output("diff-tree"))?;
        changes.push(TreeChange {
            path: path.to_vec(),
            old: entry(old_mode, old_object),
            new: entry(new_mode, new_object),
        });
    }
    Ok(changes)
}

/// The stage-0 entries of the folder's index, by path. Blocking.
fn index_entries(
    git: &Git,
    paths: &ProjectPaths,
) -> Result<std::collections::HashMap<Vec<u8>, Option<GitEntry>>, FoldError> {
    let out = git.output(
        paths.root(),
        &in_folder(paths, &["ls-files", "-s", "-z"]),
        "ls-files",
    )?;
    let mut entries = std::collections::HashMap::new();
    for line in out.split(|&byte| byte == 0).filter(|line| !line.is_empty()) {
        let (head, path) = line
            .iter()
            .position(|&byte| byte == b'\t')
            .map(|at| (&line[..at], &line[at + 1..]))
            .ok_or_else(|| unexpected_output("ls-files"))?;
        let parts: Vec<&[u8]> = head.split(|&byte| byte == b' ').collect();
        let [mode, object, stage] = parts[..] else {
            return Err(unexpected_output("ls-files"));
        };
        // An entry with a merge stage is the human's own unfinished merge.
        let found = (stage == b"0").then(|| (mode.to_vec(), object.to_vec()));
        entries.insert(path.to_vec(), found);
    }
    Ok(entries)
}

/// Brings the index entries of the paths that differ between `old` and `new` from `old`'s to `new`'s, as [`move_branch_and_index`] describes. Blocking.
fn index_follow(
    git: &Git,
    paths: &ProjectPaths,
    old: &CommitId,
    new: &CommitId,
) -> Result<bool, FoldError> {
    let changes = tree_changes(git, &paths.git_dir(), old, new)?;
    if changes.is_empty() {
        return Ok(true);
    }
    let index = index_entries(git, paths)?;
    let mut info = Vec::new();
    for change in &changes {
        let now = index.get(&change.path).cloned().unwrap_or(None);
        let present = index.contains_key(&change.path);
        if present && now.is_none() {
            return Ok(false);
        }
        if now == change.new {
            continue;
        }
        if now != change.old {
            return Ok(false);
        }
        match &change.new {
            Some((mode, object)) => {
                info.extend_from_slice(mode);
                info.push(b' ');
                info.extend_from_slice(object);
            }
            None => info.extend_from_slice(b"0 0000000000000000000000000000000000000000"),
        }
        info.push(b'\t');
        info.extend_from_slice(&change.path);
        info.push(0);
    }
    write_index_info(git, paths, &info)?;
    refresh_index(git, paths)?;
    Ok(true)
}

/// Puts the index entries of `reset`, paths of the project folder, back to their entries in `commit`, and removes those `commit` does not hold, or every one of them without a commit, then refreshes the index's file stats. A change the human staged on those paths is undone; every other entry stays. Blocking.
pub fn reset_index_paths(
    git: &Git,
    paths: &ProjectPaths,
    commit: Option<&CommitId>,
    reset: &[Vec<u8>],
) -> Result<(), FoldError> {
    if reset.is_empty() {
        return Ok(());
    }
    let mut committed = std::collections::HashMap::new();
    if let Some(commit) = commit {
        let out = git.output(
            &paths.git_dir(),
            &["ls-tree", "-r", "-z", "--full-tree", commit.as_str()],
            "ls-tree",
        )?;
        for line in out.split(|&byte| byte == 0).filter(|line| !line.is_empty()) {
            let (head, path) = line
                .iter()
                .position(|&byte| byte == b'\t')
                .map(|at| (&line[..at], &line[at + 1..]))
                .ok_or_else(|| unexpected_output("ls-tree"))?;
            let parts: Vec<&[u8]> = head.split(|&byte| byte == b' ').collect();
            let [mode, _kind, object] = parts[..] else {
                return Err(unexpected_output("ls-tree"));
            };
            committed.insert(path.to_vec(), (mode.to_vec(), object.to_vec()));
        }
    }
    let mut info = Vec::new();
    for path in reset {
        match committed.get(path) {
            Some((mode, object)) => {
                info.extend_from_slice(mode);
                info.push(b' ');
                info.extend_from_slice(object);
            }
            None => info.extend_from_slice(b"0 0000000000000000000000000000000000000000"),
        }
        info.push(b'\t');
        info.extend_from_slice(path);
        info.push(0);
    }
    write_index_info(git, paths, &info)?;
    refresh_index(git, paths)
}

/// Feeds `info`, lines of `git update-index -z --index-info`, to the folder's index. Blocking.
fn write_index_info(git: &Git, paths: &ProjectPaths, info: &[u8]) -> Result<(), FoldError> {
    if info.is_empty() {
        return Ok(());
    }
    let mut command = git.command(paths.root());
    command
        .args(in_folder(paths, &["update-index", "-z", "--index-info"]))
        .stdin(Stdio::piped());
    let mut child = Git::spawn(&mut command, "update-index")?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write as _;
        stdin
            .write_all(info)
            .map_err(|error| wait_error(&error, "update-index"))?;
    }
    let status = child
        .wait()
        .map_err(|error| wait_error(&error, "update-index"))?;
    if status.success() {
        Ok(())
    } else {
        Err(failed("update-index", status))
    }
}

/// Refreshes the file stats of the folder's index; a path whose file differs from its entry is no failure. Blocking.
fn refresh_index(git: &Git, paths: &ProjectPaths) -> Result<(), FoldError> {
    let refreshed = git.status(
        paths.root(),
        &in_folder(paths, &["update-index", "-q", "--refresh"]),
        "update-index",
    )?;
    match refreshed.code() {
        Some(0 | 1) => Ok(()),
        _ => Err(failed("update-index", refreshed)),
    }
}

/// Points the remote `name` of the project's repository `mirror` at `url`. The URL is never put in an error, because it can hold credentials. Blocking.
pub fn prepare_mirror(git: &Git, mirror: &Path, name: &str, url: &str) -> Result<(), FoldError> {
    check_remote_name(name)?;
    check_remote(url)?;
    let verb = if git.succeeds(mirror, &["remote", "get-url", "--", name], "remote")? {
        "set-url"
    } else {
        "add"
    };
    git.output(mirror, &["remote", verb, "--", name, url], "remote")?;
    Ok(())
}

/// Deletes the remote `name` and the refs fetched from it from the project's repository `mirror`; a repository or remote that does not exist is left as it is. Blocking.
pub fn remove_remote(git: &Git, mirror: &Path, name: &str) -> Result<(), FoldError> {
    check_remote_name(name)?;
    if !mirror.join("HEAD").is_file()
        || !git.succeeds(mirror, &["remote", "get-url", "--", name], "remote")?
    {
        return Ok(());
    }
    git.output(mirror, &["remote", "remove", name], "remote")?;
    Ok(())
}

/// The branch the `HEAD` of the repository at `url` points to, or `None` when it has no `HEAD` branch, as an empty repository has none. `mirror` is the repository Git runs in. Blocking.
pub fn default_branch(git: &Git, mirror: &Path, url: &str) -> Result<Option<String>, FoldError> {
    check_remote(url)?;
    let out = git.network_output(mirror, &["ls-remote", "--symref", url, "HEAD"], "ls-remote")?;
    let text = String::from_utf8(out).map_err(|_| unexpected_output("ls-remote"))?;
    Ok(text.lines().find_map(|line| {
        let (target, name) = line.strip_prefix("ref: ")?.split_once('\t')?;
        let branch = target.strip_prefix("refs/heads/")?;
        (name == "HEAD" && check_branch(branch).is_ok()).then(|| branch.to_owned())
    }))
}

/// Fetches `branch` of the remote `name` into [`remote_ref`] and returns its commit, or `None` when the remote has no such branch, which counts as empty. Blocking.
pub fn fetch_branch(
    git: &Git,
    mirror: &Path,
    name: &str,
    branch: &str,
) -> Result<Option<CommitId>, FoldError> {
    check_remote_name(name)?;
    check_branch(branch)?;
    let head = format!("refs/heads/{branch}");
    let tracking = remote_ref(name, branch);
    let listed = git.network_status(
        mirror,
        &["ls-remote", "--exit-code", "--heads", name, &head],
        "ls-remote",
    )?;
    match listed.code() {
        Some(0) => {}
        // `--exit-code` gives 2 when the remote has no matching ref.
        Some(2) => {
            git.output(mirror, &["update-ref", "-d", &tracking], "update-ref")?;
            return Ok(None);
        }
        _ => return Err(failed("ls-remote", listed)),
    }
    let refspec = format!("+{head}:{tracking}");
    git.network_output(
        mirror,
        &["fetch", "--quiet", "--no-tags", name, &refspec],
        "fetch",
    )?;
    git.ref_commit(mirror, &tracking)
}

/// Creates a bare repository at `url` with `HEAD` on `branch` when `url` is an absolute local path that does not exist, so a push to a new local path makes its repository. Blocking.
pub fn create_local_remote(git: &Git, url: &str, branch: &str) -> Result<(), FoldError> {
    let path = Path::new(url);
    if !path.is_absolute() || fs::symlink_metadata(path).is_ok() {
        return Ok(());
    }
    fs::create_dir_all(path).map_err(|error| repository_io(&error))?;
    git.init_bare(path, branch).inspect_err(|_| {
        // A half-made directory would make every retry see an existing path and skip the creation.
        let _ = fs::remove_dir_all(path);
    })
}

/// The best common ancestor of `a` and `b`, or `None` when their histories share no commit. Blocking.
pub fn merge_base(
    git: &Git,
    repo: &Path,
    a: &CommitId,
    b: &CommitId,
) -> Result<Option<CommitId>, FoldError> {
    let Some(out) =
        git.output_or_none(repo, &["merge-base", a.as_str(), b.as_str()], "merge-base")?
    else {
        return Ok(None);
    };
    let text = String::from_utf8(out).map_err(|_| unexpected_output("merge-base"))?;
    text.trim_end()
        .parse()
        .map(Some)
        .map_err(|_| unexpected_output("merge-base"))
}

/// Whether `ancestor` is `descendant` or one of its ancestors. Blocking.
pub fn is_ancestor(
    git: &Git,
    mirror: &Path,
    ancestor: &CommitId,
    descendant: &CommitId,
) -> Result<bool, FoldError> {
    git.check(
        mirror,
        &[
            "merge-base",
            "--is-ancestor",
            ancestor.as_str(),
            descendant.as_str(),
        ],
        "merge-base",
    )
}

/// Points `refname` at `commit`, or deletes it for `None`. Blocking.
pub fn set_ref(
    git: &Git,
    mirror: &Path,
    refname: &str,
    commit: Option<&CommitId>,
) -> Result<(), FoldError> {
    match commit {
        Some(commit) => git.output(
            mirror,
            &["update-ref", refname, commit.as_str()],
            "update-ref",
        ),
        None => git.output(mirror, &["update-ref", "-d", refname], "update-ref"),
    }?;
    Ok(())
}

/// Pushes `commit` of the project's repository `mirror` to the `branch` of the remote `name`, never with force. A push the remote rejects because its branch moved is `remote_ahead`; any other failure is `git_failed`. Blocking.
pub fn push_branch(
    git: &Git,
    mirror: &Path,
    name: &str,
    commit: &CommitId,
    branch: &str,
) -> Result<(), FoldError> {
    check_remote_name(name)?;
    check_branch(branch)?;
    let refspec = format!("{commit}:{}", branch_ref(branch));
    let mut command = git.command(mirror);
    command
        .args(["push", "--porcelain", name, &refspec])
        .stdout(Stdio::piped());
    let (status, stdout) = git.network_run(command, "push")?;
    if status.success() {
        return Ok(());
    }
    // Only the status flag and summary of each ref line are read: the `To <url>` line can hold credentials.
    if rejected(&stdout) {
        return Err(remote_ahead(branch, None));
    }
    Err(failed("push", status))
}

/// Whether a porcelain ref line reports a push the remote refused because it is not a fast-forward of its branch.
fn rejected(porcelain: &[u8]) -> bool {
    porcelain.split(|&byte| byte == b'\n').any(|line| {
        let mut fields = line.split(|&byte| byte == b'\t');
        fields.next() == Some(b"!")
            && fields
                .nth(1)
                .is_some_and(|summary| summary.starts_with(b"[rejected]"))
    })
}

/// The remote branch holds commits that main does not: pushing would drop them.
pub fn remote_ahead(branch: &str, remote_commit: Option<&CommitId>) -> FoldError {
    let error = FoldError::new(
        codes::REMOTE_AHEAD,
        format!("the remote branch {branch} has commits that fold main does not have"),
    )
    .with_hint("run `fld pull` to bring the remote commits into main, then `fld push` again");
    match remote_commit {
        Some(commit) => error.with_details([
            ("branch", json!(branch)),
            ("remote_commit", json!(commit.as_str())),
        ]),
        None => error.with_details([("branch", json!(branch))]),
    }
}

fn repository_io(error: &io::Error) -> FoldError {
    FoldError::new(
        codes::WORKSPACE_IO,
        format!(
            "cannot create, move or read the project's Git repository: {}",
            error.kind()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_rejected_ref_line_means_the_remote_moved() {
        let line = b"To /x/remote.git\n!\trefs/fold/main:refs/heads/main\t[rejected] (fetch first)\nDone\n";
        assert!(rejected(line));
        let hook =
            b"!\trefs/fold/main:refs/heads/main\t[remote rejected] (pre-receive hook declined)\n";
        assert!(!rejected(hook));
        assert!(!rejected(
            b"=\trefs/fold/main:refs/heads/main\t[up to date]\n"
        ));
        assert!(!rejected(b""));
    }

    #[test]
    fn a_moved_head_names_its_commands_between_backticks() {
        let fold: CommitId = "b".repeat(40).parse().unwrap();
        assert_eq!(
            Moved::OtherBranch(Some("dev".to_owned())).hint("main"),
            "run `git switch main` in the project folder"
        );
        assert_eq!(
            Moved::Rewritten {
                tip: fold.clone(),
                fold
            }
            .hint("main"),
            "run `git reset --soft bbbbbbbbbbbb` in the project folder; your files stay as they are and `fld record` records them"
        );
        assert_eq!(
            Moved::Stash.hint("main"),
            "run `git stash pop` in the project folder"
        );
    }

    #[test]
    fn refs_name_the_branch() {
        assert_eq!(branch_ref("release/1.0"), "refs/heads/release/1.0");
        assert_eq!(remote_ref("origin", "main"), "refs/remotes/origin/main");
        let error = remote_ahead("main", None);
        assert_eq!(error.code(), codes::REMOTE_AHEAD);
        assert!(error.hint().unwrap().contains("fld pull"));
    }

    #[test]
    fn a_project_without_its_own_repository_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::new(dir.path());
        let missing = project_repository(&paths).unwrap_err();
        assert_eq!(missing.code(), codes::INVALID_REQUEST);
        assert!(missing.hint().unwrap().contains("fld init"));
        std::os::unix::fs::symlink(dir.path(), paths.git_dir()).unwrap();
        assert_eq!(
            project_repository(&paths).unwrap_err().code(),
            codes::INVALID_REQUEST
        );
        fs::remove_file(paths.git_dir()).unwrap();
        fs::create_dir_all(paths.git_dir().join("info")).unwrap();
        fs::write(paths.git_dir().join("info/exclude"), "# mine").unwrap();
        fs::create_dir(paths.fold_dir()).unwrap();
        project_repository(&paths).unwrap();
        project_repository(&paths).unwrap();
        assert_eq!(
            fs::read_to_string(paths.git_dir().join("info/exclude")).unwrap(),
            "# mine\n/.fold/\n"
        );
    }
}
