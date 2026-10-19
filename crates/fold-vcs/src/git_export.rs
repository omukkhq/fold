use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufWriter, Read, Write};
use std::path::Path;
use std::process::{ChildStdin, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use fold_core::object::ObjectHash;
use fold_core::tree::{EntryKind, FlatEntry, TreeEntry};
use fold_core::{FoldError, RevisionId, Timestamp, codes};
use serde_json::json;

use crate::git::{CommitId, Git, check_rev, failed, unexpected_output, wait_error};
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

impl Signature {
    /// The author and committer of the commits in a repository whose config names no identity a commit can carry.
    pub fn fold() -> Self {
        Self {
            name: "fold".to_owned(),
            email: "fold@localhost".to_owned(),
        }
    }
}

/// The identity the config of `repo` names, from the repository's own, the global and the system files, which is the author and the committer of every commit fold writes there: the commits are the human's project history, and fold's own names stay in fold. [`Signature::fold`] stands in when the name or the email is unset or blank, or holds what an author line cannot carry. Blocking.
pub fn repository_identity(git: &Git, repo: &Path) -> Result<Signature, FoldError> {
    let value = |key: &str| -> Result<Option<String>, FoldError> {
        Ok(git
            .config_get(repo, key)?
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty()))
    };
    let identity = match (value("user.name")?, value("user.email")?) {
        (Some(name), Some(email)) => Signature { name, email },
        _ => return Ok(Signature::fold()),
    };
    Ok(match check_signature(&identity) {
        Ok(()) => identity,
        Err(_) => Signature::fold(),
    })
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
    /// The parents after the first, in order: the remote commit a pull, or the sync task that resolved one, brought into this revision, which makes the remote accept the commit as a fast-forward of its branch, and the human's commit of a human change it brought onto main, which gives the project folder that holds the human's files a way to follow main.
    pub merges: Vec<CommitId>,
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

/// A commit that is no fold revision: the commit `fld record` makes of the project folder for the human who runs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportCommit {
    pub tree: ObjectHash,
    pub created_at: Timestamp,
    pub message: String,
    pub author: Signature,
    pub committer: Signature,
}

/// Writes `commit` as one commit on `target.refname` that continues `base`, as [`write_commits`] writes a revision, and returns its id. Blocking.
pub fn write_commit(
    objects: &ObjectStore,
    git: &Git,
    target: &ExportTarget<'_>,
    base: &ExportBase,
    commit: &ExportCommit,
    scratch: &Path,
) -> Result<CommitId, FoldError> {
    // The number only pairs the commit with its input; it is not written into the commit.
    let number = RevisionId::from_number(1).ok_or_else(|| unexpected_output("fast-import"))?;
    let revision = ExportRevision {
        number,
        tree: commit.tree,
        created_at: commit.created_at,
        message: commit.message.clone(),
        author: commit.author.clone(),
        committer: commit.committer.clone(),
        merges: Vec::new(),
    };
    write_commits(objects, git, target, Some(base), &[revision], scratch)?
        .pop()
        .map(|(_, id)| id)
        .ok_or_else(|| unexpected_output("fast-import"))
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
    let mut previous = base.map(|base| base.tree);
    out.write_all(b"feature done\n").map_err(pipe)?;
    for (index, revision) in revisions.iter().enumerate() {
        let changed =
            tree_changes(objects, previous.as_ref(), &revision.tree).map_err(StreamError::Fold)?;
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
        for merge in &revision.merges {
            out.write_all(format!("merge {merge}\n").as_bytes())
                .map_err(pipe)?;
        }
        // Deletions come first, so a path that turns from a file into a directory, or back, is free before it is written.
        for path in &changed.deleted {
            out.write_all(b"D ").map_err(pipe)?;
            out.write_all(&quote_path(path)).map_err(pipe)?;
            out.write_all(b"\n").map_err(pipe)?;
        }
        for (path, entry) in &changed.written {
            write_file(objects, &mut out, path, entry)?;
        }
        out.write_all(b"\n").map_err(pipe)?;
        previous = Some(revision.tree);
    }
    out.write_all(b"done\n").map_err(pipe)?;
    out.flush().map_err(pipe)
}

/// What one commit changes, each list in path order.
#[derive(Debug, Default, PartialEq, Eq)]
struct TreeChanges {
    /// Files and symlinks of the old tree that the new one does not have.
    deleted: Vec<Vec<u8>>,
    /// Files and symlinks of the new tree that the old one does not have with the same kind and content.
    written: Vec<(Vec<u8>, FlatEntry)>,
}

/// The changes from the tree `old` to the tree `new`. A directory with the same hash on both sides is never read, so a commit costs only the directories it changed.
fn tree_changes(
    objects: &ObjectStore,
    old: Option<&ObjectHash>,
    new: &ObjectHash,
) -> Result<TreeChanges, FoldError> {
    let mut changes = TreeChanges::default();
    diff_dir(objects, old, Some(new), &[], &mut changes)?;
    changes.deleted.sort();
    changes.written.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(changes)
}

fn diff_dir(
    objects: &ObjectStore,
    old: Option<&ObjectHash>,
    new: Option<&ObjectHash>,
    prefix: &[u8],
    changes: &mut TreeChanges,
) -> Result<(), FoldError> {
    if old == new {
        return Ok(());
    }
    let entries = |tree: Option<&ObjectHash>| -> Result<BTreeMap<Vec<u8>, TreeEntry>, FoldError> {
        Ok(match tree {
            Some(tree) => load_tree(objects, tree)?
                .entries
                .into_iter()
                .map(|entry| (entry.name.clone(), entry))
                .collect(),
            None => BTreeMap::new(),
        })
    };
    let olds = entries(old)?;
    let news = entries(new)?;
    let names: BTreeSet<&Vec<u8>> = olds.keys().chain(news.keys()).collect();
    for name in names {
        let (before, after) = (olds.get(name), news.get(name));
        if before == after {
            continue;
        }
        let mut path = prefix.to_vec();
        if !path.is_empty() {
            path.push(b'/');
        }
        path.extend_from_slice(name);
        let dir = |entry: Option<&TreeEntry>| {
            entry
                .filter(|entry| entry.kind == EntryKind::Directory)
                .map(|entry| entry.hash)
        };
        let (old_dir, new_dir) = (dir(before), dir(after));
        if old_dir.is_some() || new_dir.is_some() {
            diff_dir(objects, old_dir.as_ref(), new_dir.as_ref(), &path, changes)?;
        }
        let is_file = |entry: &&TreeEntry| entry.kind != EntryKind::Directory;
        match (before.filter(is_file), after.filter(is_file)) {
            (_, Some(entry)) => changes.written.push((
                path,
                FlatEntry {
                    kind: entry.kind,
                    hash: entry.hash,
                },
            )),
            (Some(_), None) => changes.deleted.push(path),
            (None, None) => {}
        }
    }
    Ok(())
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
    use fold_core::object::{ObjectKind, hash_object};
    use fold_core::tree::FlatTree;

    use super::*;
    use crate::revision_store::write_initial_tree;

    fn tree(objects: &ObjectStore, files: &[(&str, EntryKind, &[u8])]) -> ObjectHash {
        let entries: FlatTree = files
            .iter()
            .map(|(path, kind, content)| {
                let hash = objects.write(ObjectKind::Blob, content).unwrap();
                (path.as_bytes().to_vec(), FlatEntry { kind: *kind, hash })
            })
            .collect();
        write_initial_tree(objects, entries).unwrap()
    }

    #[test]
    fn a_commit_lists_only_what_changed_also_where_files_and_directories_swap() {
        let dir = tempfile::tempdir().unwrap();
        let objects = ObjectStore::open(dir.path()).unwrap();
        let file = EntryKind::File;
        let old = tree(
            &objects,
            &[
                ("a", file, b"x"),
                ("d/b", file, b"y"),
                ("d/c", file, b"z"),
                ("f", file, b"w"),
                ("same/deep/s", file, b"s"),
            ],
        );
        let new = tree(
            &objects,
            &[
                ("a", EntryKind::Executable, b"x"),
                ("d", file, b"now a file"),
                ("f/g", file, b"w"),
                ("h", EntryKind::Symlink, b"a"),
                ("same/deep/s", file, b"s"),
            ],
        );
        let changes = tree_changes(&objects, Some(&old), &new).unwrap();
        assert_eq!(
            changes.deleted,
            [b"d/b".to_vec(), b"d/c".to_vec(), b"f".to_vec()]
        );
        let written: Vec<(&[u8], EntryKind)> = changes
            .written
            .iter()
            .map(|(path, entry)| (path.as_slice(), entry.kind))
            .collect();
        assert_eq!(
            written,
            [
                (&b"a"[..], EntryKind::Executable),
                (b"d", file),
                (b"f/g", file),
                (b"h", EntryKind::Symlink),
            ]
        );
        assert_eq!(
            changes.written[1].1.hash,
            hash_object(ObjectKind::Blob, b"now a file")
        );
        assert_eq!(
            tree_changes(&objects, Some(&new), &new).unwrap(),
            TreeChanges::default()
        );
        let first = tree_changes(&objects, None, &old).unwrap();
        assert!(first.deleted.is_empty());
        assert_eq!(first.written.len(), 5);
    }

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
        assert!(check_signature(&Signature::fold()).is_ok());
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

    /// A repository whose `git` reads `global` as the global config and no system config.
    fn isolated(global: &str) -> (tempfile::TempDir, Git) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("gitconfig");
        fs::write(&config, global).unwrap();
        let script = dir.path().join("git-isolated");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nGIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL='{}' exec git \"$@\"\n",
                config.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let git = Git::new(script);
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();
        git.init_checkout(&repo, "main").unwrap();
        (dir, git)
    }

    #[test]
    fn the_identity_comes_from_the_repository_config_and_else_is_fold() {
        let (dir, git) = isolated("[user]\n\tname = Global\n\temail = global@example.com\n");
        let repo = dir.path().join("repo");
        let global = Signature {
            name: "Global".to_owned(),
            email: "global@example.com".to_owned(),
        };
        assert_eq!(repository_identity(&git, &repo).unwrap(), global);
        git.config_set_local(&repo, "user.name", "Ana Lima")
            .unwrap();
        let local = repository_identity(&git, &repo).unwrap();
        assert_eq!(
            (local.name.as_str(), local.email.as_str()),
            ("Ana Lima", "global@example.com")
        );
        git.config_set_local(&repo, "user.name", "Ana <x>").unwrap();
        assert_eq!(repository_identity(&git, &repo).unwrap(), Signature::fold());

        let (dir, git) = isolated("[user]\n\tname = Only a name\n");
        assert_eq!(
            repository_identity(&git, &dir.path().join("repo")).unwrap(),
            Signature::fold()
        );
        let (dir, git) = isolated("[user]\n\tname = \" \"\n\temail = a@b\n");
        assert_eq!(
            repository_identity(&git, &dir.path().join("repo")).unwrap(),
            Signature::fold()
        );
    }
}
