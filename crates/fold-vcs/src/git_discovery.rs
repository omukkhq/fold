use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// The variable that lists the folders Git never goes up into while it looks for a repository.
pub const CEILING_DIRECTORIES: &str = "GIT_CEILING_DIRECTORIES";

/// The variable that lets Git go up across a mount point while it looks for a repository.
pub const ACROSS_FILESYSTEM: &str = "GIT_DISCOVERY_ACROSS_FILESYSTEM";

/// The values of the variables that limit how far up Git looks for the repository around a folder, as the caller's environment gives them. [`crate::git::Git`] hands them to `git` unchanged and reads them the way Git does when it walks up itself, so both ways find the same repository.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Discovery {
    pub ceiling_directories: Option<OsString>,
    pub across_filesystem: Option<OsString>,
}

impl Discovery {
    /// The values that `var` gives for the two variables.
    pub fn from_vars(var: impl Fn(&str) -> Option<OsString>) -> Self {
        Self {
            ceiling_directories: var(CEILING_DIRECTORIES),
            across_filesystem: var(ACROSS_FILESYSTEM),
        }
    }

    /// The ceiling folders as Git reads the list: colon-separated absolute paths, a relative one left out, and every one before the first empty entry resolved to its real path, a path that does not resolve left out. An entry after the empty one keeps its bytes but one trailing slash, because Git compares it with the folder byte by byte.
    fn ceilings(&self) -> Vec<PathBuf> {
        let Some(value) = &self.ceiling_directories else {
            return Vec::new();
        };
        let mut resolve = true;
        let mut ceilings = Vec::new();
        for entry in value.as_bytes().split(|&byte| byte == b':') {
            let entry = Path::new(OsStr::from_bytes(entry));
            if entry.as_os_str().is_empty() {
                resolve = false;
            } else if entry.is_absolute() {
                let entry = if resolve {
                    fs::canonicalize(entry).ok()
                } else {
                    let bytes = entry.as_os_str().as_bytes();
                    let bytes = match bytes.strip_suffix(b"/") {
                        Some(kept) if !kept.is_empty() => kept,
                        _ => bytes,
                    };
                    Some(PathBuf::from(OsStr::from_bytes(bytes)))
                };
                ceilings.extend(entry);
            }
        }
        ceilings
    }

    /// Whether the walk may go up into a folder on another file system, which Git reads as a boolean that is false when unset.
    fn crosses_filesystems(&self) -> bool {
        let Some(value) = self
            .across_filesystem
            .as_ref()
            .and_then(|value| value.to_str())
        else {
            return false;
        };
        let value = value.trim().to_ascii_lowercase();
        match value.as_str() {
            "true" | "yes" | "on" => true,
            _ => value.parse::<i64>().is_ok_and(|number| number != 0),
        }
    }
}

/// The repository that holds a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enclosing {
    /// The folder is in the working tree whose top this is.
    WorkingTree(PathBuf),
    /// The folder is this Git directory or inside it: the `.git` of a repository, or a bare repository.
    GitDirectory(PathBuf),
}

/// The repository that holds `dir`, found the way Git finds it with no `git` process: from `dir` up, the first folder whose `.git` is a Git directory or a file that names one is the top of a working tree, and the first folder that is itself a Git directory holds `dir` in its Git data. A `.git` file that names none ends the walk with nothing, as it stops Git. The walk never goes up into a ceiling folder of `discovery`, nor into a folder on another file system unless `discovery` allows it. Blocking.
pub(crate) fn enclosing(dir: &Path, discovery: &Discovery) -> Option<Enclosing> {
    let dir = fs::canonicalize(dir).ok()?;
    let ceilings = discovery.ceilings();
    let device = if discovery.crosses_filesystems() {
        None
    } else {
        fs::metadata(&dir).ok().map(|meta| meta.dev())
    };
    for folder in dir.ancestors() {
        if folder != dir {
            if ceilings
                .iter()
                .any(|ceiling| ceiling.as_os_str().as_bytes() == folder.as_os_str().as_bytes())
            {
                return None;
            }
            if device.is_some_and(|device| {
                fs::metadata(folder).ok().map(|meta| meta.dev()) != Some(device)
            }) {
                return None;
            }
        }
        let dot_git = folder.join(".git");
        match fs::metadata(&dot_git) {
            Ok(meta) if meta.is_dir() => {
                if is_git_directory(&dot_git) {
                    return Some(Enclosing::WorkingTree(folder.to_owned()));
                }
            }
            Ok(meta) if meta.is_file() => {
                return git_file_target(&dot_git)
                    .map(|_| Enclosing::WorkingTree(folder.to_owned()));
            }
            // Git passes by a `.git` that is neither a folder nor a file, such as a socket, and never opens it.
            _ => {}
        }
        if is_git_directory(folder) {
            return Some(Enclosing::GitDirectory(folder.to_owned()));
        }
    }
    None
}

/// The Git directory that the `.git` file at `file` names in its `gitdir: ` line, as a linked worktree or a submodule has it, or `None` when it names none.
fn git_file_target(file: &Path) -> Option<PathBuf> {
    let text = fs::read(file).ok()?;
    let named = text.strip_prefix(b"gitdir: ")?;
    let named = named.trim_ascii_end();
    if named.is_empty() {
        return None;
    }
    let named = Path::new(OsStr::from_bytes(named));
    let target = file.parent().unwrap_or(file).join(named);
    is_git_directory(&target).then_some(target)
}

/// Whether `dir` holds what Git takes for a Git directory: a `HEAD` that names a ref below `refs/` or a commit, and `objects/` and `refs/`, in `dir` or in the common directory that its `commondir` file names, as a linked worktree's has it.
fn is_git_directory(dir: &Path) -> bool {
    let Ok(head) = fs::read(dir.join("HEAD")) else {
        return false;
    };
    let head = head.trim_ascii_end();
    let names_ref = head
        .strip_prefix(b"ref:")
        .is_some_and(|name| name.trim_ascii_start().starts_with(b"refs/"));
    let names_commit = head.len() >= 40 && head.iter().all(u8::is_ascii_hexdigit);
    if !names_ref && !names_commit {
        return false;
    }
    let common = match fs::read(dir.join("commondir")) {
        Ok(named) => dir.join(OsStr::from_bytes(named.trim_ascii_end())),
        Err(_) => dir.to_owned(),
    };
    common.join("objects").is_dir() && common.join("refs").is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Git;

    fn git(dir: &Path, args: &[&str]) {
        let status = Git::default()
            .command(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// What `git` finds around `dir` and what the walk finds there, which must be the same.
    fn both(dir: &Path, discovery: &Discovery) -> Option<Enclosing> {
        let missing = tempfile::tempdir().unwrap();
        let by_git = Git::default()
            .with_discovery(discovery.clone())
            .git_enclosing(dir)
            .unwrap();
        let by_walk = Git::new(missing.path().join("no-git"))
            .with_discovery(discovery.clone())
            .enclosing_repository(dir, false)
            .unwrap();
        assert_eq!(by_git, by_walk, "{}", dir.display());
        by_walk
    }

    fn real(path: &Path) -> PathBuf {
        fs::canonicalize(path).unwrap()
    }

    fn tree(top: &Path) -> Option<Enclosing> {
        Some(Enclosing::WorkingTree(real(top)))
    }

    fn git_dir(dir: &Path) -> Option<Enclosing> {
        Some(Enclosing::GitDirectory(real(dir)))
    }

    /// No ceiling but the folder above `dir`, so a repository around the test's own folder never changes an answer.
    fn within(dir: &Path) -> Discovery {
        Discovery {
            ceiling_directories: real(dir)
                .parent()
                .map(|parent| parent.as_os_str().to_owned()),
            across_filesystem: None,
        }
    }

    #[test]
    fn the_walk_finds_the_working_tree_that_git_finds() {
        let dir = tempfile::tempdir().unwrap();
        let none = within(dir.path());
        let work = dir.path().join("work");
        fs::create_dir_all(work.join("sub/deeper")).unwrap();
        git(&work, &["init", "--quiet", "--initial-branch=main"]);
        for folder in [work.clone(), work.join("sub"), work.join("sub/deeper")] {
            assert_eq!(both(&folder, &none), tree(&work));
        }
        for folder in [work.join(".git"), work.join(".git/refs/heads")] {
            assert_eq!(both(&folder, &none), git_dir(&work.join(".git")));
        }
        assert_eq!(both(dir.path(), &none), None);
        assert_eq!(both(&dir.path().join("absent"), &none), None);

        // A `.git` file names the Git directory of a working tree kept elsewhere.
        let linked = dir.path().join("linked");
        git(
            dir.path(),
            &["init", "--quiet", "--separate-git-dir=store", "linked"],
        );
        fs::create_dir(linked.join("sub")).unwrap();
        assert!(linked.join(".git").is_file());
        for folder in [linked.clone(), linked.join("sub")] {
            assert_eq!(both(&folder, &none), tree(&linked));
        }

        // A linked worktree's Git directory keeps its objects and refs in the common directory of the main one.
        git(
            &work,
            &[
                "-c",
                "user.name=Ana",
                "-c",
                "user.email=ana@example.com",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "first",
            ],
        );
        let worktree = dir.path().join("worktree");
        git(&work, &["worktree", "add", "--quiet", "../worktree"]);
        fs::create_dir(worktree.join("sub")).unwrap();
        assert!(worktree.join(".git").is_file());
        for folder in [worktree.clone(), worktree.join("sub")] {
            assert_eq!(both(&folder, &none), tree(&worktree));
        }
        let worktree_git_dir = work.join(".git/worktrees/worktree");
        assert_eq!(both(&worktree_git_dir, &none), git_dir(&worktree_git_dir));

        // A bare repository is a Git directory with no working tree.
        let bare = dir.path().join("bare.git");
        git(dir.path(), &["init", "--quiet", "--bare", "bare.git"]);
        fs::create_dir(bare.join("notes")).unwrap();
        for folder in [bare.clone(), bare.join("notes"), bare.join("refs/heads")] {
            assert_eq!(both(&folder, &none), git_dir(&bare));
        }

        // A `.git` file that names no Git directory stops Git, and an empty `.git` directory is passed by.
        let broken = work.join("sub/broken");
        fs::create_dir_all(broken.join("in")).unwrap();
        fs::write(broken.join(".git"), "not a repository").unwrap();
        assert_eq!(both(&broken.join("in"), &none), None);
        fs::write(broken.join(".git"), "gitdir: ../absent\n").unwrap();
        assert_eq!(both(&broken, &none), None);
        let empty = work.join("sub/empty");
        fs::create_dir_all(empty.join(".git")).unwrap();
        assert_eq!(both(&empty, &none), tree(&work));

        // Git passes by a `.git` whose `HEAD` names no ref below `refs/`, and a `.git` that is neither a folder nor a file.
        let loose = work.join("sub/loose");
        fs::create_dir_all(loose.join(".git/objects")).unwrap();
        fs::create_dir_all(loose.join(".git/refs")).unwrap();
        fs::write(loose.join(".git/HEAD"), "ref: refs\n").unwrap();
        assert_eq!(both(&loose, &none), tree(&work));
        fs::write(loose.join(".git/HEAD"), "ref:  refs/heads/main\n").unwrap();
        assert_eq!(both(&loose, &none), tree(&loose));
        let socket = work.join("sub/socket");
        fs::create_dir(&socket).unwrap();
        let _listener = std::os::unix::net::UnixListener::bind(socket.join(".git")).unwrap();
        assert_eq!(both(&socket, &none), tree(&work));
    }

    #[test]
    fn a_repository_that_git_refuses_to_read_still_holds_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        fs::create_dir_all(work.join("sub")).unwrap();
        git(&work, &["init", "--quiet", "--initial-branch=main"]);
        let git = Git::default().with_discovery(Discovery {
            across_filesystem: Some(OsString::from("maybe")),
            ..within(dir.path())
        });
        assert_eq!(git.git_enclosing(&work.join("sub")).unwrap(), None);
        for needs_git in [false, true] {
            assert_eq!(
                git.enclosing_repository(&work.join("sub"), needs_git)
                    .unwrap(),
                tree(&work)
            );
        }
        let missing = Git::new(dir.path().join("no-git"));
        let error = missing.enclosing_repository(&work, true).unwrap_err();
        assert_eq!(error.code(), fold_core::codes::GIT_NOT_FOUND);
    }

    #[test]
    fn neither_way_goes_up_into_a_ceiling_folder() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        fs::create_dir_all(work.join("sub/deeper")).unwrap();
        git(&work, &["init", "--quiet", "--initial-branch=main"]);
        let ceiling = |value: &OsStr| Discovery {
            ceiling_directories: Some(value.to_owned()),
            across_filesystem: None,
        };
        let at_work = ceiling(work.as_os_str());
        assert_eq!(both(&work.join("sub"), &at_work), None);
        // The folder the walk starts from is looked at even when it is a ceiling.
        assert_eq!(both(&work, &at_work), tree(&work));
        let at_sub = ceiling(work.join("sub").as_os_str());
        assert_eq!(both(&work.join("sub/deeper"), &at_sub), None);
        assert_eq!(both(&work.join("sub"), &at_sub), tree(&work));
        // A relative entry is left out, and the entries after an empty one are taken as they are written.
        let mut list = OsString::from("sub:");
        list.push(":");
        list.push(real(&work).as_os_str());
        assert_eq!(both(&work.join("sub"), &ceiling(&list)), None);
        // Git compares such an entry with the folder byte by byte, one trailing slash left out.
        let mut slashed = OsString::from(":");
        slashed.push(real(&work).as_os_str());
        slashed.push("/");
        assert_eq!(both(&work.join("sub"), &ceiling(&slashed)), None);
        slashed.push("/");
        assert_eq!(both(&work.join("sub"), &ceiling(&slashed)), tree(&work));
        let mut dotted = OsString::from(":");
        dotted.push(real(dir.path()).as_os_str());
        dotted.push("/./work");
        assert_eq!(both(&work.join("sub"), &ceiling(&dotted)), tree(&work));
        assert_eq!(
            both(&work.join("sub"), &ceiling(OsStr::new("work:/absent"))),
            tree(&work)
        );
    }

    #[test]
    fn the_variables_read_as_git_reads_them() {
        let discovery = Discovery::from_vars(|name| {
            (name == CEILING_DIRECTORIES).then(|| OsString::from("/:relative::/a/../b/://"))
        });
        assert_eq!(discovery.across_filesystem, None);
        assert_eq!(
            discovery.ceilings(),
            [
                PathBuf::from("/"),
                PathBuf::from("/a/../b"),
                PathBuf::from("/")
            ]
        );
        assert!(Discovery::default().ceilings().is_empty());
        for (value, crosses) in [
            ("true", true),
            ("YES", true),
            ("on", true),
            ("2", true),
            ("0", false),
            ("off", false),
            ("", false),
            ("maybe", false),
        ] {
            let discovery = Discovery {
                ceiling_directories: None,
                across_filesystem: Some(OsString::from(value)),
            };
            assert_eq!(discovery.crosses_filesystems(), crosses, "{value}");
        }
        assert!(!Discovery::default().crosses_filesystems());
    }
}
