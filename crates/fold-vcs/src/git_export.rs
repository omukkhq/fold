use std::ffi::OsString;
use std::fs;
use std::io::{self, BufWriter, Read, Write};
use std::path::Path;
use std::process::{ChildStdin, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use fold_core::object::ObjectHash;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree, flatten};
use fold_core::{FoldError, RevisionId, Timestamp, codes};
use serde_json::json;

use crate::git::{CommitId, Git, check_rev, failed, unexpected_output, wait_error};
use crate::git_sync_map::SyncMap;
use crate::objects::{ObjectError, ObjectStore};
use crate::snapshot::load_tree;

const BUFFER_SIZE: usize = 64 * 1024;

static MARKS_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A Git author or committer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub name: String,
    pub email: String,
}

/// One fold revision as the commit it becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRevision {
    pub number: RevisionId,
    pub tree: ObjectHash,
    pub created_at: Timestamp,
    pub message: String,
    pub author: Signature,
    pub committer: Signature,
    /// The second parent: the remote commit a pull, or the sync task that resolved one, brought into this revision. Writing it makes the remote accept the commit as a fast-forward of its branch.
    pub merge: Option<CommitId>,
}

/// The last exported revision: the commit the next one continues and the fold tree it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportBase {
    pub commit: CommitId,
    pub tree: ObjectHash,
}

#[derive(Debug, Clone, Copy)]
pub struct ExportTarget<'a> {
    pub repo: &'a Path,
    /// The ref the commits go to, such as `refs/heads/main`.
    pub refname: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetState {
    /// The path does not exist or is an empty directory: [`create_target`] makes the repository.
    Missing,
    /// A repository whose ref is where the export continues.
    Ready,
}

/// Checks that an export can continue the chain that ends at `last` (the last mapped commit): `target.refname` must point to it, or must not exist when nothing is mapped yet. A missing or empty path continues only an empty chain. A non-empty directory must be the top of a SHA-1 repository, working tree or bare. Changes nothing.
pub fn check_target(
    git: &Git,
    target: &ExportTarget<'_>,
    last: Option<&(RevisionId, CommitId)>,
) -> Result<TargetState, FoldError> {
    check_rev(target.refname)?;
    let repo = target.repo;
    let state = match fs::symlink_metadata(repo) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => TargetState::Missing,
        Err(error) => return Err(target_io(&error)),
        Ok(meta) if !meta.is_dir() => return Err(not_a_repository(repo)),
        Ok(_) => {
            let mut listing = fs::read_dir(repo).map_err(|error| target_io(&error))?;
            if listing.next().is_none() {
                TargetState::Missing
            } else if git.repository(repo)?.is_some() {
                TargetState::Ready
            } else {
                return Err(not_a_repository(repo));
            }
        }
    };
    let continues = match (state, last) {
        (TargetState::Missing, None) => true,
        (TargetState::Missing, Some(_)) => false,
        (TargetState::Ready, None) => git.ref_commit(repo, target.refname)?.is_none(),
        (TargetState::Ready, Some((_, commit))) => {
            git.has_commit(repo, commit)?
                && git.ref_commit(repo, target.refname)?.as_ref() == Some(commit)
        }
    };
    if continues {
        Ok(state)
    } else {
        Err(diverged(target, last))
    }
}

/// Makes the bare repository [`check_target`] found missing, with `HEAD` on the target branch.
pub fn create_target(git: &Git, target: &ExportTarget<'_>) -> Result<(), FoldError> {
    let branch = target.refname.strip_prefix("refs/heads/").ok_or_else(|| {
        FoldError::new(
            codes::INVALID_REQUEST,
            "a new repository is created only for a branch ref",
        )
        .with_details([("ref", json!(target.refname))])
    })?;
    fs::create_dir_all(target.repo).map_err(|error| target_io(&error))?;
    git.init_bare(target.repo, branch)
}

/// The ref [`recover_stopped`] writes the expected commits to, to compare them with the branch before the branch moves.
const RECOVERY_REF: &str = "refs/fold/export-recovery";

/// Recovers an export that stopped after `git fast-import` and before the sync map was appended to: `target.refname` is not the last mapped commit `base.commit` (or exists although nothing is mapped, when `base` is `None`), but is one of the commits fold writes for `revisions`, the revisions after it. A commit has the same id whenever its tree, parent, message, author and time are the same, so the expected chain is written to a scratch ref first and the branch is compared with it. When the branch is one of those commits it moves on to the newest, and every revision with its commit is returned for the map. When it is anything else, such as a commit fold did not write, the branch does not move, the commits written for the comparison stay unreferenced in the repository, and the result is `None`. Blocking: async callers go through `spawn_blocking`.
pub fn recover_stopped(
    objects: &ObjectStore,
    git: &Git,
    target: &ExportTarget<'_>,
    base: Option<&ExportBase>,
    revisions: &[ExportRevision],
    scratch: &Path,
) -> Result<Option<Vec<(RevisionId, CommitId)>>, FoldError> {
    check_rev(target.refname)?;
    if revisions.is_empty() || git.repository(target.repo)?.is_none() {
        return Ok(None);
    }
    if let Some(base) = base
        && !git.has_commit(target.repo, &base.commit)?
    {
        return Ok(None);
    }
    let Some(current) = git.ref_commit(target.repo, target.refname)? else {
        return Ok(None);
    };
    if git.checked_out(target.repo, target.refname)? {
        return Err(checked_out(target));
    }
    let scratch_target = ExportTarget {
        repo: target.repo,
        refname: RECOVERY_REF,
    };
    // A scratch ref left by a recovery that stopped would make fast-import refuse a chain that does not continue it.
    drop_recovery_ref(git, target.repo)?;
    let written = write_commits(objects, git, &scratch_target, base, revisions, scratch);
    let dropped = drop_recovery_ref(git, target.repo);
    let added = written?;
    dropped?;
    let Some(tip) = added.last().map(|(_, commit)| commit.clone()) else {
        return Ok(None);
    };
    if !added.iter().any(|(_, commit)| *commit == current) {
        return Ok(None);
    }
    if current != tip {
        git.update_ref(target.repo, target.refname, Some(&tip), &current)?;
    }
    Ok(Some(added))
}

fn drop_recovery_ref(git: &Git, repo: &Path) -> Result<(), FoldError> {
    match git.ref_commit(repo, RECOVERY_REF)? {
        Some(tip) => git.update_ref(repo, RECOVERY_REF, None, &tip),
        None => Ok(()),
    }
}

fn diverged(target: &ExportTarget<'_>, last: Option<&(RevisionId, CommitId)>) -> FoldError {
    let message = match last {
        Some((revision, commit)) => format!(
            "{} in {} is not {commit}, the commit {revision} was exported as",
            target.refname,
            target.repo.display()
        ),
        None => format!(
            "{} already exists in {}, and nothing was exported from this project yet",
            target.refname,
            target.repo.display()
        ),
    };
    let error = FoldError::new(codes::EXPORT_DIVERGED, message).with_hint(
        "export into the repository the last export wrote, or a clone of it whose branch has no new commits",
    );
    match last {
        Some((revision, commit)) => error.with_details([
            ("revision", json!(revision.to_string())),
            ("commit", json!(commit.as_str())),
        ]),
        None => error,
    }
}

fn checked_out(target: &ExportTarget<'_>) -> FoldError {
    FoldError::new(
        codes::INVALID_REQUEST,
        format!(
            "{} is checked out in {}, and moving it would leave its working tree on the old commit",
            target.refname,
            target.repo.display()
        ),
    )
    .with_hint("check out another branch or detach HEAD in that repository, or export into a bare clone of it")
    .with_details([("ref", json!(target.refname))])
}

fn not_a_repository(repo: &Path) -> FoldError {
    FoldError::new(
        codes::INVALID_REQUEST,
        format!(
            "{} is not empty and is not a Git repository",
            repo.display()
        ),
    )
    .with_hint("export into a Git repository, an empty directory or a path that does not exist")
}

fn target_io(error: &io::Error) -> FoldError {
    FoldError::new(
        codes::WORKSPACE_IO,
        format!("cannot read the export target: {}", error.kind()),
    )
}

/// Writes `revisions` as one chain of commits on `target.refname` through `git fast-import`, the first continuing `base` or starting the history when there is none, then appends each revision and its commit to `map` in one write. A failed run appends nothing, so a retry exports the same revisions again. See [`write_commits`] for the rest. Blocking: async callers go through `spawn_blocking`.
pub fn export(
    objects: &ObjectStore,
    git: &Git,
    target: &ExportTarget<'_>,
    base: Option<&ExportBase>,
    revisions: &[ExportRevision],
    map: &mut SyncMap,
    scratch: &Path,
) -> Result<Vec<(RevisionId, CommitId)>, FoldError> {
    let added = write_commits(objects, git, target, base, revisions, scratch)?;
    map.append(&added)?;
    Ok(added)
}

/// Writes `revisions` as one chain of commits on `target.refname` through `git fast-import`, the first continuing `base` or starting the history when there is none, and returns each revision with its commit. Each commit records only the paths that differ from the previous revision's tree, and its id depends only on the revisions, so writing the same revisions again gives the same commits. A ref that a working tree has checked out is refused with `invalid_request`, as Git refuses a push to it, because fast-import moves the ref without touching the files. fast-import refuses to move the ref unless the old commit is an ancestor of the new one. `scratch` holds the marks file while the command runs. Blocking: async callers go through `spawn_blocking`.
pub fn write_commits(
    objects: &ObjectStore,
    git: &Git,
    target: &ExportTarget<'_>,
    base: Option<&ExportBase>,
    revisions: &[ExportRevision],
    scratch: &Path,
) -> Result<Vec<(RevisionId, CommitId)>, FoldError> {
    if revisions.is_empty() {
        return Ok(Vec::new());
    }
    check_rev(target.refname)?;
    for revision in revisions {
        check_signature(&revision.author)?;
        check_signature(&revision.committer)?;
    }
    if git.checked_out(target.repo, target.refname)? {
        return Err(checked_out(target));
    }
    let marks = scratch.join(format!(
        "git-export-{}-{}.marks",
        std::process::id(),
        MARKS_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = run_fast_import(objects, git, target, base, revisions, &marks);
    // The marks file is read already or useless after a failure.
    let _ = fs::remove_file(&marks);
    let commits = result?;
    Ok(revisions
        .iter()
        .map(|revision| revision.number)
        .zip(commits)
        .collect())
}

fn run_fast_import(
    objects: &ObjectStore,
    git: &Git,
    target: &ExportTarget<'_>,
    base: Option<&ExportBase>,
    revisions: &[ExportRevision],
    marks: &Path,
) -> Result<Vec<CommitId>, FoldError> {
    let mut marks_arg = OsString::from("--export-marks=");
    marks_arg.push(marks);
    let mut command = git.command(target.repo);
    command
        .arg("fast-import")
        .arg("--quiet")
        .arg(marks_arg)
        .stdin(Stdio::piped());
    let mut child = Git::spawn(&mut command, "fast-import")?;
    let written = match child.stdin.take() {
        Some(stdin) => write_stream(objects, stdin, target.refname, base, revisions),
        None => Err(StreamError::Pipe),
    };
    if let Err(StreamError::Fold(error)) = written {
        // Without the closing `done` fast-import discards the stream; killing it first makes sure it never reads a partial one as whole.
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let status = child
        .wait()
        .map_err(|error| wait_error(&error, "fast-import"))?;
    if !status.success() {
        return Err(failed("fast-import", status));
    }
    if written.is_err() {
        return Err(unexpected_output("fast-import"));
    }
    let text = fs::read_to_string(marks).map_err(|_| unexpected_output("fast-import"))?;
    parse_marks(&text, revisions.len()).ok_or_else(|| unexpected_output("fast-import"))
}

enum StreamError {
    /// fast-import stopped reading; its exit status says why.
    Pipe,
    Fold(FoldError),
}

fn write_stream(
    objects: &ObjectStore,
    stdin: ChildStdin,
    refname: &str,
    base: Option<&ExportBase>,
    revisions: &[ExportRevision],
) -> Result<(), StreamError> {
    let mut out = BufWriter::with_capacity(BUFFER_SIZE, stdin);
    let pipe = |_: io::Error| StreamError::Pipe;
    let load = |hash: &ObjectHash| load_tree(objects, hash);
    let mut previous = match base {
        Some(base) => flatten(&base.tree, &load).map_err(StreamError::Fold)?,
        None => FlatTree::new(),
    };
    out.write_all(b"feature done\n").map_err(pipe)?;
    for (index, revision) in revisions.iter().enumerate() {
        let current = flatten(&revision.tree, &load).map_err(StreamError::Fold)?;
        let seconds = revision.created_at.as_millis().div_euclid(1000);
        let mut head = format!("commit {refname}\nmark :{}\n", index + 1);
        for (role, signature) in [
            ("author", &revision.author),
            ("committer", &revision.committer),
        ] {
            head.push_str(&format!(
                "{role} {} <{}> {seconds} +0000\n",
                signature.name, signature.email
            ));
        }
        head.push_str(&format!("data {}\n", revision.message.len()));
        out.write_all(head.as_bytes()).map_err(pipe)?;
        out.write_all(revision.message.as_bytes()).map_err(pipe)?;
        let from = match (index, base) {
            (0, Some(base)) => Some(base.commit.to_string()),
            (0, None) => None,
            (index, _) => Some(format!(":{index}")),
        };
        out.write_all(b"\n").map_err(pipe)?;
        if let Some(from) = from {
            out.write_all(format!("from {from}\n").as_bytes())
                .map_err(pipe)?;
        }
        if let Some(merge) = &revision.merge {
            out.write_all(format!("merge {merge}\n").as_bytes())
                .map_err(pipe)?;
        }
        // Deletions come first, so a path that turns from a file into a directory, or back, is free before it is written.
        for path in previous.keys().filter(|path| !current.contains_key(*path)) {
            out.write_all(b"D ").map_err(pipe)?;
            out.write_all(&quote_path(path)).map_err(pipe)?;
            out.write_all(b"\n").map_err(pipe)?;
        }
        for (path, entry) in &current {
            if previous.get(path) != Some(entry) {
                write_file(objects, &mut out, path, entry)?;
            }
        }
        out.write_all(b"\n").map_err(pipe)?;
        previous = current;
    }
    out.write_all(b"done\n").map_err(pipe)?;
    out.flush().map_err(pipe)
}

fn write_file(
    objects: &ObjectStore,
    out: &mut impl Write,
    path: &[u8],
    entry: &FlatEntry,
) -> Result<(), StreamError> {
    let pipe = |_: io::Error| StreamError::Pipe;
    let mode = match entry.kind {
        EntryKind::File => "100644",
        EntryKind::Executable => "100755",
        EntryKind::Symlink => "120000",
        EntryKind::Directory => {
            return Err(StreamError::Fold(FoldError::new(
                codes::TREE_INVALID,
                "a flattened tree holds a directory entry",
            )));
        }
    };
    let mut blob = objects
        .open_blob(&entry.hash)
        .map_err(|error| StreamError::Fold(error.into()))?;
    out.write_all(format!("M {mode} inline ").as_bytes())
        .map_err(pipe)?;
    out.write_all(&quote_path(path)).map_err(pipe)?;
    out.write_all(format!("\ndata {}\n", blob.size()).as_bytes())
        .map_err(pipe)?;
    let mut buffer = vec![0; BUFFER_SIZE];
    loop {
        let read = match blob.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(StreamError::Fold(ObjectError::from(error).into())),
        };
        out.write_all(&buffer[..read]).map_err(pipe)?;
    }
    out.write_all(b"\n").map_err(pipe)
}

/// fast-import reads a path to the end of the line and accepts C-style quoting, which it requires for a path that starts with a double quote or holds a line break.
fn quote_path(path: &[u8]) -> Vec<u8> {
    let needs_quotes = path.first() == Some(&b'"')
        || path
            .iter()
            .any(|&byte| byte == b'\\' || byte == b'"' || byte < 0x20 || byte == 0x7f);
    if !needs_quotes {
        return path.to_vec();
    }
    let mut quoted = vec![b'"'];
    for &byte in path {
        match byte {
            b'"' => quoted.extend_from_slice(b"\\\""),
            b'\\' => quoted.extend_from_slice(b"\\\\"),
            b'\n' => quoted.extend_from_slice(b"\\n"),
            b'\t' => quoted.extend_from_slice(b"\\t"),
            byte if byte < 0x20 || byte == 0x7f => {
                quoted.extend_from_slice(format!("\\{byte:03o}").as_bytes());
            }
            byte => quoted.push(byte),
        }
    }
    quoted.push(b'"');
    quoted
}

/// The commits of marks `:1` to `:count`, in mark order.
fn parse_marks(text: &str, count: usize) -> Option<Vec<CommitId>> {
    let mut commits: Vec<Option<CommitId>> = vec![None; count];
    for line in text.lines() {
        let (mark, commit) = line.strip_prefix(':')?.split_once(' ')?;
        let mark: usize = mark.parse().ok()?;
        let slot = commits.get_mut(mark.checked_sub(1)?)?;
        *slot = Some(commit.parse().ok()?);
    }
    commits.into_iter().collect()
}

fn check_signature(signature: &Signature) -> Result<(), FoldError> {
    let bad = |text: &str| text.contains(['<', '>', '\n', '\0']);
    if bad(&signature.name) || bad(&signature.email) {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            "a Git author name or email must not hold `<`, `>` or a line break",
        )
        .with_details([("name", json!(signature.name))]));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_quoted_only_when_fast_import_needs_it() {
        assert_eq!(quote_path(b"src/a b.rs"), b"src/a b.rs");
        assert_eq!(quote_path("ü/x".as_bytes()), "ü/x".as_bytes());
        assert_eq!(quote_path(b"\"q"), b"\"\\\"q\"");
        assert_eq!(quote_path(b"a\nb"), b"\"a\\nb\"");
        assert_eq!(quote_path(b"a\\b\tc"), b"\"a\\\\b\\tc\"");
        assert_eq!(quote_path(b"a\x01\x7f"), b"\"a\\001\\177\"");
    }

    #[test]
    fn marks_give_one_commit_per_revision() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let commits = parse_marks(&format!(":2 {b}\n:1 {a}\n"), 2).unwrap();
        assert_eq!(commits, [a.parse().unwrap(), b.parse().unwrap()]);
        for bad in [
            format!(":1 {a}\n"),
            format!("1 {a}\n:2 {b}\n"),
            format!(":0 {a}\n:2 {b}\n"),
            format!(":1 {a}\n:3 {b}\n"),
            format!(":1 xyz\n:2 {b}\n"),
            format!(":x {a}\n:2 {b}\n"),
        ] {
            assert_eq!(parse_marks(&bad, 2), None, "{bad}");
        }
    }

    #[test]
    fn signatures_cannot_break_the_header() {
        let ok = Signature {
            name: "fold".to_owned(),
            email: "fold@localhost".to_owned(),
        };
        assert!(check_signature(&ok).is_ok());
        for (name, email) in [("a<b", "e"), ("a", "e>"), ("a\nb", "e")] {
            let bad = Signature {
                name: name.to_owned(),
                email: email.to_owned(),
            };
            assert_eq!(
                check_signature(&bad).unwrap_err().code(),
                codes::INVALID_REQUEST
            );
        }
    }
}
