use std::fs;
use std::io;
use std::path::Path;
use std::process::Stdio;

use fold_core::config::{check_branch, check_remote, check_remote_name};
use fold_core::{FoldError, codes};
use serde_json::json;

use crate::git::{CommitId, Git, failed, unexpected_output, wait_error};

/// The project's Git repository inside `.fold/`, bare: the commits of main on the `[git]` branch and, per remote, the branch the last fetch found.
pub const MIRROR_DIR: &str = "git";

/// The branch ref the commits of main are written to in the mirror.
pub fn branch_ref(branch: &str) -> String {
    format!("refs/heads/{branch}")
}

/// The mirror ref that keeps the commit a Git import read, so the commits after it can name it as their parent.
pub const IMPORTED_REF: &str = "refs/fold/imported";

/// The mirror ref that holds the branch of the remote `name` as the last fetch found it.
pub fn remote_ref(name: &str, branch: &str) -> String {
    format!("refs/remotes/{name}/{branch}")
}

/// Creates the bare mirror at `mirror` when it is not a repository yet, and points its remote `name` at `url`. The URL is never put in an error, because it can hold credentials. Blocking.
pub fn prepare_mirror(git: &Git, mirror: &Path, name: &str, url: &str) -> Result<(), FoldError> {
    check_remote_name(name)?;
    check_remote(url)?;
    ensure_mirror(git, mirror)?;
    let verb = if git.succeeds(mirror, &["remote", "get-url", "--", name], "remote")? {
        "set-url"
    } else {
        "add"
    };
    git.output(mirror, &["remote", verb, "--", name, url], "remote")?;
    Ok(())
}

/// Deletes the remote `name` and the refs fetched from it from the mirror; a mirror or remote that does not exist is left as it is. Blocking.
pub fn remove_remote(git: &Git, mirror: &Path, name: &str) -> Result<(), FoldError> {
    check_remote_name(name)?;
    if git.repository(mirror)?.is_none()
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
    ensure_mirror(git, mirror)?;
    let out = git.output(mirror, &["ls-remote", "--symref", url, "HEAD"], "ls-remote")?;
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
    let listed = git.status(
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
    git.output(
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
    fs::create_dir_all(path).map_err(|error| mirror_io(&error))?;
    git.init_bare(path, branch)
}

/// Creates the bare mirror at `mirror` when it is not a repository yet. Blocking.
pub fn ensure_mirror(git: &Git, mirror: &Path) -> Result<(), FoldError> {
    if git.repository(mirror)?.is_none() {
        fs::create_dir_all(mirror).map_err(|error| mirror_io(&error))?;
        git.init_bare(mirror, "main")?;
    }
    Ok(())
}

/// Copies `commit` of the repository `source`, with everything it reaches, into the mirror at `mirror` under [`IMPORTED_REF`], creating the mirror when it is missing. A local operation: `source` is a path. Blocking.
pub fn fetch_imported(
    git: &Git,
    mirror: &Path,
    source: &Path,
    commit: &CommitId,
) -> Result<(), FoldError> {
    ensure_mirror(git, mirror)?;
    // Canonical, so the source is an absolute path that Git cannot read as an option or a URL.
    let source = source.canonicalize().map_err(|error| mirror_io(&error))?;
    let refspec = format!("+{commit}:{IMPORTED_REF}");
    let mut command = git.command(mirror);
    command
        .args(["fetch", "--quiet", "--no-tags", "--end-of-options"])
        .arg(source)
        .arg(refspec);
    let status = Git::spawn(&mut command, "fetch")?
        .wait()
        .map_err(|error| wait_error(&error, "fetch"))?;
    if status.success() {
        Ok(())
    } else {
        Err(failed("fetch", status))
    }
}

/// The best common ancestor of `a` and `b`, or `None` when their histories share no commit. Blocking.
pub fn merge_base(
    git: &Git,
    repo: &Path,
    a: &CommitId,
    b: &CommitId,
) -> Result<Option<CommitId>, FoldError> {
    let mut command = git.command(repo);
    command
        .args(["merge-base", a.as_str(), b.as_str()])
        .stdout(Stdio::piped());
    let output = Git::spawn(&mut command, "merge-base")?
        .wait_with_output()
        .map_err(|error| wait_error(&error, "merge-base"))?;
    match output.status.code() {
        Some(0) => {
            let text =
                String::from_utf8(output.stdout).map_err(|_| unexpected_output("merge-base"))?;
            text.trim_end()
                .parse()
                .map(Some)
                .map_err(|_| unexpected_output("merge-base"))
        }
        Some(1) => Ok(None),
        _ => Err(failed("merge-base", output.status)),
    }
}

/// Whether `ancestor` is `descendant` or one of its ancestors. Blocking.
pub fn is_ancestor(
    git: &Git,
    mirror: &Path,
    ancestor: &CommitId,
    descendant: &CommitId,
) -> Result<bool, FoldError> {
    let status = git.status(
        mirror,
        &[
            "merge-base",
            "--is-ancestor",
            ancestor.as_str(),
            descendant.as_str(),
        ],
        "merge-base",
    )?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(failed("merge-base", status)),
    }
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

/// Pushes the mirror's `local` branch to the `branch` of the remote `name`, never with force. A push the remote rejects because its branch moved is `remote_ahead`; any other failure is `git_failed`. Blocking.
pub fn push_branch(
    git: &Git,
    mirror: &Path,
    name: &str,
    local: &str,
    branch: &str,
) -> Result<(), FoldError> {
    check_remote_name(name)?;
    check_branch(local)?;
    check_branch(branch)?;
    let refspec = format!("{}:{}", branch_ref(local), branch_ref(branch));
    let mut command = git.command(mirror);
    command
        .args(["push", "--porcelain", name, &refspec])
        .stdout(Stdio::piped());
    let output = Git::spawn(&mut command, "push")?
        .wait_with_output()
        .map_err(|error| wait_error(&error, "push"))?;
    if output.status.success() {
        return Ok(());
    }
    // Only the status flag and summary of each ref line are read: the `To <url>` line can hold credentials.
    if rejected(&output.stdout) {
        return Err(remote_ahead(branch, None));
    }
    Err(failed("push", output.status))
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
    .with_hint("run fold pull to bring the remote commits into main, then fold push again");
    match remote_commit {
        Some(commit) => error.with_details([
            ("branch", json!(branch)),
            ("remote_commit", json!(commit.as_str())),
        ]),
        None => error.with_details([("branch", json!(branch))]),
    }
}

fn mirror_io(error: &io::Error) -> FoldError {
    FoldError::new(
        codes::WORKSPACE_IO,
        format!("cannot create or read the Git mirror: {}", error.kind()),
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
        assert!(error.hint().unwrap().contains("fold pull"));
    }

    #[test]
    fn a_mirror_that_cannot_be_created_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("file"), b"x").unwrap();
        let error = prepare_mirror(
            &Git::default(),
            &dir.path().join("file/git"),
            "origin",
            "/r",
        )
        .unwrap_err();
        assert_eq!(error.code(), codes::WORKSPACE_IO);
    }
}
