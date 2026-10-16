use std::collections::{BTreeSet, HashMap};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{ChildStdin, ChildStdout, Stdio};

use fold_core::FoldError;
use fold_core::object::ObjectHash;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree, validate_name};

use crate::git::{Git, check_rev, failed, unexpected_output, wait_error};
use crate::import::ImportResult;
use crate::objects::{ObjectError, ObjectStore};
use crate::paths::{RepoPath, WorkspaceError, is_reserved_name};
use crate::revision_store::write_initial_tree;
use crate::scan::{SkipReason, Skipped};

const BUFFER_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct GitImportOptions {
    pub max_file_bytes: u64,
}

/// Stores the committed tree of `rev` in `repo` and returns its root: no history, no working tree and no ignore rule, except that `.git` and `.fold` never import. The tree is listed with `git ls-tree` and each blob read once with `git cat-file --batch` rather than taken from `git archive`, because an archive leaves out `export-ignore` paths, rewrites `export-subst` files and holds only the subdirectory it runs in. Nothing is written into `repo`. Blobs of a failed run stay behind as unreferenced objects. Blocking: async callers go through `spawn_blocking`.
pub fn import_commit(
    objects: &ObjectStore,
    git: &Git,
    repo: &Path,
    rev: &str,
    opts: &GitImportOptions,
) -> Result<ImportResult, FoldError> {
    check_rev(rev)?;
    let listing = git.output(
        repo,
        &[
            "ls-tree",
            "-r",
            "-z",
            "-l",
            "--full-tree",
            "--end-of-options",
            rev,
        ],
        "ls-tree",
    )?;
    let listed = parse_listing(&listing, opts)?;
    let mut collected = Collected {
        entries: FlatTree::new(),
        bytes: 0,
        skipped: listed.skipped,
    };
    if !listed.wanted.is_empty() {
        read_blobs(objects, git, repo, &listed.wanted, &mut collected)?;
    }
    let files = collected.entries.len();
    let tree = write_initial_tree(objects, collected.entries)?;
    Ok(ImportResult {
        tree,
        files,
        bytes: collected.bytes,
        skipped: collected.skipped,
    })
}

/// One blob of the listing that imports.
#[derive(Debug, PartialEq, Eq)]
struct Wanted {
    path: RepoPath,
    kind: EntryKind,
    blob: String,
    size: u64,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Listing {
    wanted: Vec<Wanted>,
    skipped: Vec<Skipped>,
}

struct Collected {
    entries: FlatTree,
    bytes: u64,
    skipped: Vec<Skipped>,
}

/// Reads `git ls-tree -r -z -l` output. Submodules and paths under `.git` or `.fold` are left out, and files over the limit are skipped. A path that is not a clean relative path, repeats, or is both a file and a directory fails with `invalid_path`.
fn parse_listing(listing: &[u8], opts: &GitImportOptions) -> Result<Listing, FoldError> {
    let mut listed = Listing::default();
    let mut seen: BTreeSet<Vec<u8>> = BTreeSet::new();
    let Some(records) = listing.strip_suffix(b"\0") else {
        return if listing.is_empty() {
            Ok(listed)
        } else {
            Err(unexpected_output("ls-tree"))
        };
    };
    for record in records.split(|&byte| byte == 0) {
        let tab = record
            .iter()
            .position(|&byte| byte == b'\t')
            .ok_or_else(|| unexpected_output("ls-tree"))?;
        let (head, raw) = (&record[..tab], &record[tab + 1..]);
        let head = std::str::from_utf8(head).map_err(|_| unexpected_output("ls-tree"))?;
        let fields: Vec<&str> = head.split_ascii_whitespace().collect();
        let [mode, kind, blob, size] = fields[..] else {
            return Err(unexpected_output("ls-tree"));
        };
        let Some(path) = entry_path(raw)? else {
            continue;
        };
        if !seen.insert(raw.to_vec()) {
            return Err(WorkspaceError::invalid(raw).into());
        }
        let kind = match (mode, kind) {
            ("160000", "commit") => continue,
            ("100644" | "100664", "blob") => EntryKind::File,
            ("100755", "blob") => EntryKind::Executable,
            ("120000", "blob") => EntryKind::Symlink,
            _ => return Err(unexpected_output("ls-tree")),
        };
        if blob.len() != 40 || !blob.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(unexpected_output("ls-tree"));
        }
        let size: u64 = size.parse().map_err(|_| unexpected_output("ls-tree"))?;
        if size > opts.max_file_bytes {
            listed.skipped.push(Skipped {
                path,
                reason: SkipReason::TooLarge,
            });
            continue;
        }
        listed.wanted.push(Wanted {
            path,
            kind,
            blob: blob.to_owned(),
            size,
        });
    }
    // A path below another one is a file inside a file.
    for path in &seen {
        let mut prefix = path.as_slice();
        while let Some(slash) = prefix.iter().rposition(|&byte| byte == b'/') {
            prefix = &prefix[..slash];
            if seen.contains(prefix) {
                return Err(WorkspaceError::invalid(path).into());
            }
        }
    }
    listed.skipped.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(listed)
}

/// `None` for a path under `.git` or `.fold`, which never imports; an absolute path or one with an empty, `.` or `..` component fails the import.
fn entry_path(raw: &[u8]) -> Result<Option<RepoPath>, FoldError> {
    let names = || raw.split(|&byte| byte == b'/');
    if names().any(|name| validate_name(name).is_err()) {
        return Err(WorkspaceError::invalid(raw).into());
    }
    if names().any(is_reserved_name) {
        return Ok(None);
    }
    Ok(Some(RepoPath::parse(raw)?))
}

/// `Stream` is a fault of `git` or of its output, whose exit status then explains it; `Fold` is the object store's.
enum Failure {
    Stream,
    Fold(FoldError),
}

fn read_blobs(
    objects: &ObjectStore,
    git: &Git,
    repo: &Path,
    wanted: &[Wanted],
    collected: &mut Collected,
) -> Result<(), FoldError> {
    let mut command = git.command(repo);
    command
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let mut child = Git::spawn(&mut command, "cat-file")?;
    let read = match (child.stdin.take(), child.stdout.take()) {
        (Some(stdin), Some(stdout)) => read_each(objects, stdin, stdout, wanted, collected),
        _ => Err(Failure::Stream),
    };
    // `read_each` dropped both pipes, so a `git` stopped early sees the end of its input or a broken pipe instead of blocking.
    let status = child
        .wait()
        .map_err(|error| wait_error(&error, "cat-file"))?;
    match read {
        Err(Failure::Fold(error)) => Err(error),
        _ if !status.success() => Err(failed("cat-file", status)),
        Err(Failure::Stream) => Err(unexpected_output("cat-file")),
        Ok(()) => Ok(()),
    }
}

/// Asks for one blob at a time and reads its answer before asking for the next, so neither side waits on a full pipe.
fn read_each(
    objects: &ObjectStore,
    mut input: ChildStdin,
    output: ChildStdout,
    wanted: &[Wanted],
    collected: &mut Collected,
) -> Result<(), Failure> {
    let mut output = BufReader::with_capacity(BUFFER_SIZE, output);
    let stream = |_: io::Error| Failure::Stream;
    let mut header = Vec::new();
    // A blob listed at several paths is read and stored once.
    let mut stored: HashMap<&str, ObjectHash> = HashMap::new();
    for item in wanted {
        if let Some(hash) = stored.get(item.blob.as_str()) {
            collected.bytes += item.size;
            collected.entries.insert(
                item.path.as_bytes().to_vec(),
                FlatEntry {
                    kind: item.kind,
                    hash: *hash,
                },
            );
            continue;
        }
        input
            .write_all(format!("{}\n", item.blob).as_bytes())
            .map_err(stream)?;
        header.clear();
        output.read_until(b'\n', &mut header).map_err(stream)?;
        if header != format!("{} blob {}\n", item.blob, item.size).as_bytes() {
            return Err(Failure::Stream);
        }
        let mut body = Watched {
            inner: (&mut output).take(item.size),
            failed: false,
        };
        let written = objects.write_blob_from(&mut body);
        if body.failed {
            return Err(Failure::Stream);
        }
        let written =
            written.map_err(|error: ObjectError| Failure::Fold(FoldError::from(error)))?;
        let mut end = [0u8; 1];
        output.read_exact(&mut end).map_err(stream)?;
        if written.size != item.size || end != *b"\n" {
            return Err(Failure::Stream);
        }
        stored.insert(&item.blob, written.hash);
        collected.bytes += written.size;
        collected.entries.insert(
            item.path.as_bytes().to_vec(),
            FlatEntry {
                kind: item.kind,
                hash: written.hash,
            },
        );
    }
    Ok(())
}

/// Remembers a failed read, so an error the object store returns can be told apart from one of the stream it read.
struct Watched<R> {
    inner: R,
    failed: bool,
}

impl<R: Read> Read for Watched<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf).inspect_err(|error| {
            if error.kind() != io::ErrorKind::Interrupted {
                self.failed = true;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use fold_core::codes;

    use super::*;

    const BLOB: &str = "0123456789abcdef0123456789abcdef01234567";

    fn listing(records: &[(&str, &str, &str, &str)]) -> Vec<u8> {
        records
            .iter()
            .flat_map(|(mode, kind, size, path)| {
                format!("{mode} {kind} {BLOB} {size:>7}\t{path}\0").into_bytes()
            })
            .collect()
    }

    fn listing_error(listing: &[u8]) -> &'static str {
        let opts = GitImportOptions {
            max_file_bytes: u64::MAX,
        };
        parse_listing(listing, &opts).unwrap_err().code()
    }

    #[test]
    fn paths_are_checked_before_use() {
        assert_eq!(
            entry_path(b"src/main.rs").unwrap(),
            Some(RepoPath::parse(b"src/main.rs").unwrap())
        );
        for skipped in [&b".fold/x"[..], b".git", b"a/.git/config", b"a/.fold"] {
            assert_eq!(entry_path(skipped).unwrap(), None);
        }
        for bad in [
            &b"../x"[..],
            b"/etc/passwd",
            b"a//b",
            b"a/./b",
            b"",
            b"x/../.fold/y",
        ] {
            assert_eq!(entry_path(bad).unwrap_err().code(), codes::INVALID_PATH);
        }
    }

    #[test]
    fn a_listing_gives_kinds_from_modes_and_leaves_out_submodules() {
        let text = listing(&[
            ("100644", "blob", "3", "a.txt"),
            ("100664", "blob", "3", "old-mode.txt"),
            ("100755", "blob", "9", "bin/run"),
            ("120000", "blob", "5", "link"),
            ("160000", "commit", "-", "vendor/sub"),
            ("100644", "blob", "1", ".fold/state"),
            ("100644", "blob", "20", "big.bin"),
        ]);
        let opts = GitImportOptions { max_file_bytes: 10 };
        let listed = parse_listing(&text, &opts).unwrap();
        let kinds: Vec<(&[u8], EntryKind, u64)> = listed
            .wanted
            .iter()
            .map(|item| (item.path.as_bytes(), item.kind, item.size))
            .collect();
        assert_eq!(
            kinds,
            [
                (&b"a.txt"[..], EntryKind::File, 3),
                (b"old-mode.txt", EntryKind::File, 3),
                (b"bin/run", EntryKind::Executable, 9),
                (b"link", EntryKind::Symlink, 5),
            ]
        );
        assert_eq!(
            listed.skipped,
            [Skipped {
                path: RepoPath::parse(b"big.bin").unwrap(),
                reason: SkipReason::TooLarge,
            }]
        );
        assert_eq!(parse_listing(b"", &opts).unwrap(), Listing::default());
    }

    #[test]
    fn a_path_that_leaves_the_tree_repeats_or_is_a_file_and_a_directory_is_invalid() {
        for bad in [
            listing(&[("100644", "blob", "1", "../escape")]),
            listing(&[("100644", "blob", "1", "/etc/passwd")]),
            listing(&[("100644", "blob", "1", "a/../../b")]),
            listing(&[("100644", "blob", "1", "a"), ("100644", "blob", "1", "a")]),
            listing(&[
                ("100644", "blob", "1", "a"),
                ("100644", "blob", "1", "a/b/c"),
            ]),
        ] {
            assert_eq!(listing_error(&bad), codes::INVALID_PATH);
        }
    }

    #[test]
    fn a_listing_fold_cannot_read_is_git_failed() {
        for bad in [
            b"unterminated".to_vec(),
            b"no tab\0".to_vec(),
            b"100644 blob\tx\0".to_vec(),
            b"\xff\tx\0".to_vec(),
            listing(&[("040000", "tree", "-", "dir")]),
            listing(&[("100644", "blob", "x", "a")]),
            b"100644 blob abc 1\ta\0".to_vec(),
        ] {
            assert_eq!(listing_error(&bad), codes::GIT_FAILED);
        }
    }

    #[test]
    fn interrupted_reads_are_not_failures() {
        struct Interrupted;
        impl Read for Interrupted {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::ErrorKind::Interrupted.into())
            }
        }
        let mut watched = Watched {
            inner: Interrupted,
            failed: false,
        };
        assert!(watched.read(&mut [0; 4]).is_err());
        assert!(!watched.failed);
    }
}
