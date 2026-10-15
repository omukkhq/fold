use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use fold_core::{FoldError, RevisionId, codes};
use serde_json::json;

use crate::git::CommitId;
use crate::platform::sync_dir;

/// The file name inside `.fold/`.
pub const SYNC_MAP_FILE: &str = "git-sync.map";

/// The file inside `.fold/` whose lock guards the map.
pub const SYNC_LOCK_FILE: &str = "git-sync.lock";

/// The project's Git sync lock, held until it is dropped.
#[derive(Debug)]
pub struct SyncLock {
    _file: File,
}

/// Which fold revision is which Git commit: one line `r<n> <commit>` for each revision that is that commit, in revision order. The Git import writes the first line and every export appends, so the last line is where the next export continues the chain of commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncMap {
    path: PathBuf,
    entries: Vec<(RevisionId, CommitId)>,
}

impl SyncMap {
    /// Reads the map in `fold_dir`; a missing file is an empty map.
    pub fn load(fold_dir: &Path) -> Result<Self, FoldError> {
        let path = fold_dir.join(SYNC_MAP_FILE);
        let text = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(io_error(&error)),
        };
        let text = String::from_utf8(text).map_err(|_| invalid(1))?;
        let mut entries: Vec<(RevisionId, CommitId)> = Vec::new();
        if !text.is_empty() && !text.ends_with('\n') {
            return Err(invalid(text.lines().count()));
        }
        for (index, line) in text.lines().enumerate() {
            let entry = parse_line(line).ok_or_else(|| invalid(index + 1))?;
            if entries.last().is_some_and(|(last, _)| *last >= entry.0) {
                return Err(invalid(index + 1));
            }
            entries.push(entry);
        }
        Ok(Self { path, entries })
    }

    /// Waits for the project's Git sync lock, then reads the map. A command that appends holds the lock from before it reads the map until after it appends, so two exports never both continue the same commit. It is the operating system's file lock, so it holds between processes and ends with the process that holds it.
    pub fn lock(fold_dir: &Path) -> Result<(SyncLock, Self), FoldError> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(fold_dir.join(SYNC_LOCK_FILE))
            .and_then(|file| file.lock().map(|()| file))
            .map_err(|error| {
                FoldError::new(
                    codes::WORKSPACE_IO,
                    format!("cannot lock {SYNC_LOCK_FILE}: {}", error.kind()),
                )
            })?;
        Ok((SyncLock { _file: file }, Self::load(fold_dir)?))
    }

    pub fn entries(&self) -> &[(RevisionId, CommitId)] {
        &self.entries
    }

    /// The newest mapped revision and its commit.
    pub fn last(&self) -> Option<&(RevisionId, CommitId)> {
        self.entries.last()
    }

    pub fn commit_of(&self, revision: RevisionId) -> Option<&CommitId> {
        self.entries
            .iter()
            .find(|(number, _)| *number == revision)
            .map(|(_, commit)| commit)
    }

    pub fn revision_of(&self, commit: &CommitId) -> Option<RevisionId> {
        self.entries
            .iter()
            .find(|(_, mapped)| mapped == commit)
            .map(|(number, _)| *number)
    }

    /// Appends `added` in one flushed write. Each revision must be newer than the last one mapped.
    pub fn append(&mut self, added: &[(RevisionId, CommitId)]) -> Result<(), FoldError> {
        let mut last = self.last().map(|(number, _)| *number);
        let mut text = String::new();
        for (number, commit) in added {
            if last.is_some_and(|last| last >= *number) {
                return Err(FoldError::new(
                    codes::GIT_SYNC_MAP_INVALID,
                    format!("{number} is not newer than the last mapped revision"),
                )
                .with_details([("revision", json!(number.to_string()))]));
            }
            last = Some(*number);
            text.push_str(&format!("{number} {commit}\n"));
        }
        if added.is_empty() {
            return Ok(());
        }
        let created = !self.path.exists();
        let written = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut file| {
                file.write_all(text.as_bytes())?;
                file.sync_all()
            });
        written.map_err(|error| io_error(&error))?;
        if created && let Some(dir) = self.path.parent() {
            sync_dir(dir).map_err(|error| io_error(&error))?;
        }
        self.entries.extend_from_slice(added);
        Ok(())
    }
}

fn parse_line(line: &str) -> Option<(RevisionId, CommitId)> {
    let (number, commit) = line.split_once(' ')?;
    Some((number.parse().ok()?, commit.parse().ok()?))
}

fn invalid(line: usize) -> FoldError {
    FoldError::new(
        codes::GIT_SYNC_MAP_INVALID,
        format!("{SYNC_MAP_FILE} line {line} is not `r<n> <commit>` in revision order"),
    )
    .with_hint("restore .fold/git-sync.map from a backup")
    .with_details([("line", json!(line))])
}

fn io_error(error: &io::Error) -> FoldError {
    FoldError::new(
        codes::WORKSPACE_IO,
        format!("cannot read or write {SYNC_MAP_FILE}: {}", error.kind()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn rev(number: u32) -> RevisionId {
        RevisionId::from_number(number).unwrap()
    }

    fn commit(text: &str) -> CommitId {
        text.parse().unwrap()
    }

    #[test]
    fn a_missing_file_is_an_empty_map_and_appends_create_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut map = SyncMap::load(dir.path()).unwrap();
        assert!(map.entries().is_empty() && map.last().is_none());
        map.append(&[]).unwrap();
        assert!(!dir.path().join(SYNC_MAP_FILE).exists());
        map.append(&[(rev(1), commit(A))]).unwrap();
        map.append(&[(rev(3), commit(B))]).unwrap();
        let text = fs::read_to_string(dir.path().join(SYNC_MAP_FILE)).unwrap();
        assert_eq!(text, format!("r1 {A}\nr3 {B}\n"));
        let again = SyncMap::load(dir.path()).unwrap();
        assert_eq!(again, map);
        assert_eq!(again.last(), Some(&(rev(3), commit(B))));
        assert_eq!(again.commit_of(rev(1)), Some(&commit(A)));
        assert_eq!(again.commit_of(rev(2)), None);
        assert_eq!(again.revision_of(&commit(B)), Some(rev(3)));
        assert_eq!(again.revision_of(&commit(&"c".repeat(40))), None);
    }

    #[test]
    fn appends_out_of_order_are_refused_and_write_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut map = SyncMap::load(dir.path()).unwrap();
        map.append(&[(rev(2), commit(A))]).unwrap();
        for added in [
            vec![(rev(2), commit(B))],
            vec![(rev(4), commit(B)), (rev(3), commit(A))],
        ] {
            let error = map.append(&added).unwrap_err();
            assert_eq!(error.code(), codes::GIT_SYNC_MAP_INVALID);
        }
        assert_eq!(map.entries().len(), 1);
        let text = fs::read_to_string(dir.path().join(SYNC_MAP_FILE)).unwrap();
        assert_eq!(text, format!("r2 {A}\n"));
    }

    #[test]
    fn damaged_files_name_the_bad_line() {
        let dir = tempfile::tempdir().unwrap();
        let cases: [(Vec<u8>, usize); 6] = [
            (b"r1 abc\n".to_vec(), 1),
            (b"\xff\n".to_vec(), 1),
            (format!("r1 {A}\nr1 {B}\n").into_bytes(), 2),
            (format!("r1 {A}").into_bytes(), 1),
            (format!("x1 {A}\n").into_bytes(), 1),
            (b"r1\n".to_vec(), 1),
        ];
        for (bytes, line) in cases {
            fs::write(dir.path().join(SYNC_MAP_FILE), bytes).unwrap();
            let error = SyncMap::load(dir.path()).unwrap_err();
            assert_eq!(error.code(), codes::GIT_SYNC_MAP_INVALID);
            assert_eq!(error.details().unwrap()["line"], line);
        }
    }

    #[test]
    fn the_lock_lets_one_holder_in_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let (held, mut map) = SyncMap::lock(dir.path()).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let fold_dir = dir.path().to_owned();
        let waiter = std::thread::spawn(move || {
            let locked = SyncMap::lock(&fold_dir).map(|(_, map)| map.entries().len());
            sender.send(locked).unwrap();
        });
        let wait = std::time::Duration::from_millis(200);
        assert!(receiver.recv_timeout(wait).is_err());
        map.append(&[(rev(1), commit(A))]).unwrap();
        drop(held);
        let seen = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        assert_eq!(seen.unwrap(), 1);
        waiter.join().unwrap();
        let error = SyncMap::lock(&dir.path().join("absent")).unwrap_err();
        assert_eq!(error.code(), codes::WORKSPACE_IO);
    }

    #[test]
    fn an_unreadable_map_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(SYNC_MAP_FILE)).unwrap();
        let error = SyncMap::load(dir.path()).unwrap_err();
        assert_eq!(error.code(), codes::WORKSPACE_IO);
        let mut map = SyncMap {
            path: dir.path().join(SYNC_MAP_FILE),
            entries: Vec::new(),
        };
        let error = map.append(&[(rev(1), commit(A))]).unwrap_err();
        assert_eq!(error.code(), codes::WORKSPACE_IO);
        assert!(map.entries().is_empty());
    }
}
