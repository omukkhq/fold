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

/// The project's Git repository at [`ProjectPaths::git_dir`], whose working tree is the project folder, after making sure `info/exclude` lists fold's data folder. It never creates one: `fld init` does. Anything but a real directory in its place, a symlink included, is refused, so fold never writes Git data through it and Git never finds an enclosing repository instead. Blocking.
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
    .with_hint("re-create the project with fld init")
}

/// Adds [`EXCLUDE_LINE`] to `info/exclude` of `repo` once. Blocking.
pub fn exclude_fold_dir(repo: &Path) -> Result<(), FoldError> {
    let exclude = repo.join("info").join("exclude");
    let text = match fs::read_to_string(&exclude) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(repository_io(&error)),
    };
    if text.lines().any(|line| line.trim() == EXCLUDE_LINE) {
        return Ok(());
    }
    let separator = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    fs::create_dir_all(repo.join("info"))
        .and_then(|()| {
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&exclude)?;
            io::Write::write_all(&mut file, format!("{separator}{EXCLUDE_LINE}\n").as_bytes())
        })
        .map_err(|error| repository_io(&error))
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

/// What [`follow_main`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Followed {
    /// The branch moved, and with it the folder's files and index when the branch is checked out.
    Moved,
    /// The folder has changes that are not committed, or a file in the way of the update, so nothing moved.
    Dirty,
}

/// Moves `branch` from `tip` to `new`, a commit that descends from it, and updates the project folder as Git's `receive.denyCurrentBranch=updateInstead` does: only when the index and the tracked files match `tip`, the files and the index are brought to `new` first and the branch moves after, while it still points to `tip`. A branch that is not checked out moves alone. Untracked files stay, and one that the update would overwrite makes the folder dirty. Never by force. Blocking.
pub fn follow_main(
    git: &Git,
    paths: &ProjectPaths,
    branch: &str,
    tip: &CommitId,
    new: &CommitId,
) -> Result<Followed, FoldError> {
    let repo = paths.git_dir();
    if checked_out_branch(git, &repo)?.as_deref() == Some(branch) {
        if !folder_clean(git, paths, tip)? {
            return Ok(Followed::Dirty);
        }
        let updated = git.status(
            paths.root(),
            &in_folder(
                paths,
                &["read-tree", "-u", "-m", tip.as_str(), new.as_str()],
            ),
            "read-tree",
        )?;
        if !updated.success() {
            return Ok(Followed::Dirty);
        }
    }
    move_branch(git, &repo, branch, new, Some(tip))?;
    Ok(Followed::Moved)
}

/// [`follow_main`] for a folder that holds the commit `via` rather than its branch tip `tip`: `fld record` made `via` of the folder's changes on top of `tip`, and `new` descends from it. The files are first compared with `via` through an index file of fold's own, so a folder that holds other changes keeps its index as the human left it, also across the many runs of the commit writer until the next record. Only when they match is the index set to `via` and are the files and the index brought to `new`, and the branch moves from `tip` to `new`; a file that changes in between sets the index back to `tip` and nothing else changes. Blocking.
pub fn follow_main_via(
    git: &Git,
    paths: &ProjectPaths,
    branch: &str,
    tip: &CommitId,
    via: &CommitId,
    new: &CommitId,
) -> Result<Followed, FoldError> {
    let repo = paths.git_dir();
    if checked_out_branch(git, &repo)?.as_deref() != Some(branch) || !files_match(git, paths, via)?
    {
        return Ok(Followed::Dirty);
    }
    index_to(git, paths, via)?;
    let moved = folder_clean(git, paths, via)?
        && git
            .status(
                paths.root(),
                &in_folder(
                    paths,
                    &["read-tree", "-u", "-m", via.as_str(), new.as_str()],
                ),
                "read-tree",
            )?
            .success();
    if !moved {
        index_to(git, paths, tip)?;
        return Ok(Followed::Dirty);
    }
    move_branch(git, &repo, branch, new, Some(tip))?;
    Ok(Followed::Moved)
}

static INDEX_FILES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Whether the tracked files of the project folder match the tree of `commit`, read through an index file of fold's own in the repository that holds that tree, so the folder's index is never written. Untracked files do not count. Blocking.
fn files_match(git: &Git, paths: &ProjectPaths, commit: &CommitId) -> Result<bool, FoldError> {
    let index = paths.git_dir().join(format!(
        "fold-index-{}-{}",
        std::process::id(),
        INDEX_FILES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let run = |args: &[&str], what: &'static str| {
        let mut command = git.command(paths.root());
        command
            .args(in_folder(paths, args))
            .env("GIT_INDEX_FILE", &index);
        Git::spawn(&mut command, what)?
            .wait()
            .map_err(|error| wait_error(&error, what))
    };
    let matched = (|| {
        let read = run(&["read-tree", commit.as_str()], "read-tree")?;
        if !read.success() {
            return Err(failed("read-tree", read));
        }
        // The fresh index has no file stats; the refresh compares each file's content, and its exit code is what the check below answers.
        run(
            &["update-index", "-q", "--ignore-submodules", "--refresh"],
            "update-index",
        )?;
        let diff = run(
            &["diff-files", "--quiet", "--ignore-submodules", "--"],
            "diff-files",
        )?;
        match diff.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(failed("diff-files", diff)),
        }
    })();
    let _ = fs::remove_file(&index);
    matched
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
    .with_hint("run fld pull to bring the remote commits into main, then fld push again");
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
        project_repository(&paths).unwrap();
        project_repository(&paths).unwrap();
        assert_eq!(
            fs::read_to_string(paths.git_dir().join("info/exclude")).unwrap(),
            "# mine\n/.fold/\n"
        );
    }
}
