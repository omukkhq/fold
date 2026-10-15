use std::fs;
use std::io;
use std::path::Path;
use std::process::Stdio;

use fold_core::config::{check_branch, check_remote};
use fold_core::{FoldError, codes};
use serde_json::json;

use crate::git::{CommitId, Git, failed, wait_error};

/// The bare mirror inside `.fold/`: the fetched remote branch and the commits fold writes, with no working tree.
pub const MIRROR_DIR: &str = "git";

/// The mirror's name for the remote repository.
pub const ORIGIN: &str = "origin";

/// The mirror ref the push commits are written to before they are pushed to `refs/heads/<branch>`.
pub fn fold_ref(branch: &str) -> String {
    format!("refs/fold/{branch}")
}

/// The mirror ref that holds the remote branch as the last fetch found it.
pub fn remote_ref(branch: &str) -> String {
    format!("refs/remotes/{ORIGIN}/{branch}")
}

/// Creates the bare mirror at `mirror` when it is not a repository yet, and points its `origin` at `remote`. The remote is never put in an error, because it can hold credentials. Blocking.
pub fn prepare_mirror(git: &Git, mirror: &Path, remote: &str) -> Result<(), FoldError> {
    check_remote(remote)?;
    if git.repository(mirror)?.is_none() {
        fs::create_dir_all(mirror).map_err(|error| mirror_io(&error))?;
        git.init_bare(mirror, "main")?;
    }
    let verb = if git.succeeds(mirror, &["remote", "get-url", ORIGIN], "remote")? {
        "set-url"
    } else {
        "add"
    };
    git.output(mirror, &["remote", verb, "--", ORIGIN, remote], "remote")?;
    Ok(())
}

/// Fetches `branch` of `origin` into [`remote_ref`] and returns its commit, or `None` when the remote has no such branch, which counts as empty. Blocking.
pub fn fetch_branch(git: &Git, mirror: &Path, branch: &str) -> Result<Option<CommitId>, FoldError> {
    check_branch(branch)?;
    let head = format!("refs/heads/{branch}");
    let tracking = remote_ref(branch);
    let listed = git.status(
        mirror,
        &["ls-remote", "--exit-code", "--heads", ORIGIN, &head],
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
        &["fetch", "--quiet", "--no-tags", ORIGIN, &refspec],
        "fetch",
    )?;
    git.ref_commit(mirror, &tracking)
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

/// Pushes [`fold_ref`] to `refs/heads/<branch>` of `origin`, never with force. A push the remote rejects because its branch moved is `remote_ahead`; any other failure is `git_failed`. Blocking.
pub fn push_branch(git: &Git, mirror: &Path, branch: &str) -> Result<(), FoldError> {
    check_branch(branch)?;
    let refspec = format!("{}:refs/heads/{branch}", fold_ref(branch));
    let mut command = git.command(mirror);
    command
        .args(["push", "--porcelain", ORIGIN, &refspec])
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
        format!("cannot create the Git mirror: {}", error.kind()),
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
        assert_eq!(fold_ref("release/1.0"), "refs/fold/release/1.0");
        assert_eq!(remote_ref("main"), "refs/remotes/origin/main");
        let error = remote_ahead("main", None);
        assert_eq!(error.code(), codes::REMOTE_AHEAD);
        assert!(error.hint().unwrap().contains("fold pull"));
    }

    #[test]
    fn a_mirror_that_cannot_be_created_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("file"), b"x").unwrap();
        let error =
            prepare_mirror(&Git::default(), &dir.path().join("file/git"), "/r").unwrap_err();
        assert_eq!(error.code(), codes::WORKSPACE_IO);
    }
}
