use std::ffi::OsStr;
use std::fmt;
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::str::FromStr;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use fold_core::{FoldError, codes};
use serde_json::json;

/// Variables through which a parent Git process, such as a hook that runs fold, would point every command at its own repository instead of the one `-C` names.
const REPOSITORY_VARIABLES: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_DIR",
    "GIT_INDEX_FILE",
    "GIT_NAMESPACE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_PREFIX",
    "GIT_WORK_TREE",
];

/// The `git` program, found on `PATH` unless a path is given. fold needs it for the project's own Git history, a Git import, `fld remote` and push and pull. Arguments go to the process directly, never through a shell.
#[derive(Debug, Clone)]
pub struct Git {
    program: PathBuf,
    network_timeout: Duration,
}

/// How long a Git command that talks to a remote may run before fold kills it.
pub const NETWORK_TIMEOUT: Duration = Duration::from_secs(120);

impl Default for Git {
    fn default() -> Self {
        Self::new("git")
    }
}

impl Git {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            network_timeout: NETWORK_TIMEOUT,
        }
    }

    /// The same program with another deadline for the commands that talk to a remote.
    pub fn with_network_timeout(mut self, timeout: Duration) -> Self {
        self.network_timeout = timeout;
        self
    }

    /// Runs a command that talks to a remote and kills it, with every process it started, when it outlives the network deadline, so a remote that never answers cannot hold the caller's locks. The standard output is returned when `command` pipes it. The group is only ever signalled while the child is not yet reaped, so its id cannot have passed to another process.
    pub(crate) fn network_run(
        &self,
        mut command: Command,
        what: &'static str,
    ) -> Result<(ExitStatus, Vec<u8>), FoldError> {
        command.process_group(0);
        let mut child = Self::spawn(&mut command, what)?;
        let deadline = Instant::now() + self.network_timeout;
        let stdout = match child.stdout.take() {
            None => Vec::new(),
            Some(mut stdout) => {
                let (sender, receiver) = mpsc::channel();
                std::thread::spawn(move || {
                    let mut bytes = Vec::new();
                    let read = stdout.read_to_end(&mut bytes).map(|_| bytes);
                    let _ = sender.send(read);
                });
                // A helper that outlives Git can keep the pipe open, so the output too must arrive before the deadline.
                match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(Ok(bytes)) => bytes,
                    Ok(Err(error)) => {
                        kill_group(&mut child);
                        return Err(wait_error(&error, what));
                    }
                    Err(_) => {
                        kill_group(&mut child);
                        return Err(timed_out(what, self.network_timeout));
                    }
                }
            }
        };
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    kill_group(&mut child);
                    return Err(timed_out(what, self.network_timeout));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(error) => {
                    kill_group(&mut child);
                    return Err(wait_error(&error, what));
                }
            }
        };
        Ok((status, stdout))
    }

    /// The standard output of a network command that must succeed.
    pub(crate) fn network_output<S: AsRef<OsStr>>(
        &self,
        repo: &Path,
        args: &[S],
        what: &'static str,
    ) -> Result<Vec<u8>, FoldError> {
        let mut command = self.command(repo);
        command.args(args).stdout(Stdio::piped());
        let (status, stdout) = self.network_run(command, what)?;
        if status.success() {
            Ok(stdout)
        } else {
            Err(failed(what, status))
        }
    }

    /// The exit status of a network command whose non-zero codes mean different answers.
    pub(crate) fn network_status<S: AsRef<OsStr>>(
        &self,
        repo: &Path,
        args: &[S],
        what: &'static str,
    ) -> Result<ExitStatus, FoldError> {
        let mut command = self.command(repo);
        command.args(args);
        Ok(self.network_run(command, what)?.0)
    }

    /// A command that runs in `repo` with no input and its output discarded; a caller that reads the output pipes it.
    pub(crate) fn command(&self, repo: &Path) -> Command {
        let mut command = Command::new(&self.program);
        // `ext::` runs any command it names and `fd::` talks over inherited descriptors. Command-line config outranks every config file and an `insteadOf` rewrite, and `GIT_ALLOW_PROTOCOL` would outrank it, so the variable goes.
        command
            .args([
                "-c",
                "protocol.ext.allow=never",
                "-c",
                "protocol.fd.allow=never",
            ])
            .arg("-C")
            .arg(repo);
        for name in REPOSITORY_VARIABLES {
            command.env_remove(name);
        }
        command.env_remove("GIT_ALLOW_PROTOCOL");
        // A command the daemon runs has nobody to answer a credential prompt, so Git fails instead of waiting.
        command.env("GIT_TERMINAL_PROMPT", "0");
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    pub(crate) fn spawn(command: &mut Command, what: &'static str) -> Result<Child, FoldError> {
        command.spawn().map_err(|error| spawn_error(&error, what))
    }

    /// The standard output of a command that must succeed.
    pub(crate) fn output<S: AsRef<OsStr>>(
        &self,
        repo: &Path,
        args: &[S],
        what: &'static str,
    ) -> Result<Vec<u8>, FoldError> {
        let mut command = self.command(repo);
        command.args(args).stdout(Stdio::piped());
        let output = Self::spawn(&mut command, what)?
            .wait_with_output()
            .map_err(|error| wait_error(&error, what))?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(failed(what, output.status))
        }
    }

    /// The exit status of a command whose non-zero codes mean different answers.
    pub(crate) fn status<S: AsRef<OsStr>>(
        &self,
        repo: &Path,
        args: &[S],
        what: &'static str,
    ) -> Result<ExitStatus, FoldError> {
        let mut command = self.command(repo);
        command.args(args);
        Self::spawn(&mut command, what)?
            .wait()
            .map_err(|error| wait_error(&error, what))
    }

    /// The standard output of a command that exits 0, or `None` when it exits 1, which the queries that use it give for an answer of "none"; any other exit is a failure.
    pub(crate) fn output_or_none<S: AsRef<OsStr>>(
        &self,
        repo: &Path,
        args: &[S],
        what: &'static str,
    ) -> Result<Option<Vec<u8>>, FoldError> {
        let mut command = self.command(repo);
        command.args(args).stdout(Stdio::piped());
        let output = Self::spawn(&mut command, what)?
            .wait_with_output()
            .map_err(|error| wait_error(&error, what))?;
        match output.status.code() {
            Some(0) => Ok(Some(output.stdout)),
            Some(1) => Ok(None),
            _ => Err(failed(what, output.status)),
        }
    }

    /// True when a check exits 0 and false when it exits 1; any other exit is a failure.
    pub(crate) fn check<S: AsRef<OsStr>>(
        &self,
        repo: &Path,
        args: &[S],
        what: &'static str,
    ) -> Result<bool, FoldError> {
        let status = self.status(repo, args, what)?;
        match status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(failed(what, status)),
        }
    }

    /// Whether a command exits zero, for the queries whose failure is the answer.
    pub(crate) fn succeeds<S: AsRef<OsStr>>(
        &self,
        repo: &Path,
        args: &[S],
        what: &'static str,
    ) -> Result<bool, FoldError> {
        Ok(self.status(repo, args, what)?.success())
    }

    /// The commit `rev` names in `repo`.
    pub fn resolve_commit(&self, repo: &Path, rev: &str) -> Result<CommitId, FoldError> {
        check_rev(rev)?;
        let spec = format!("{rev}^{{commit}}");
        let out = self.output(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &spec,
            ],
            "rev-parse",
        )?;
        parse_commit_line(&out, "rev-parse")
    }

    /// The commit `refname` points to, or `None` when the ref does not exist.
    pub fn ref_commit(&self, repo: &Path, refname: &str) -> Result<Option<CommitId>, FoldError> {
        check_rev(refname)?;
        self.output_or_none(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                refname,
            ],
            "rev-parse",
        )?
        .map(|out| parse_commit_line(&out, "rev-parse"))
        .transpose()
    }

    /// Whether `repo` holds `commit` as a commit object.
    pub fn has_commit(&self, repo: &Path, commit: &CommitId) -> Result<bool, FoldError> {
        let spec = format!("{commit}^{{commit}}");
        self.succeeds(repo, &["cat-file", "-e", &spec], "cat-file")
    }

    /// The repository whose top `dir` is, the working tree of one or the directory of a bare one, or `None` when `dir` is not such a top. fold reads and writes SHA-1 repositories only, so any other object format is refused with `invalid_request`.
    pub fn repository(&self, dir: &Path) -> Result<Option<Repository>, FoldError> {
        let Ok(top) = std::fs::canonicalize(dir) else {
            return Ok(None);
        };
        let Some(out) = self.query(
            dir,
            &[
                "rev-parse",
                "--is-bare-repository",
                "--show-object-format",
                "--absolute-git-dir",
            ],
        )?
        else {
            return Ok(None);
        };
        let mut lines = out.splitn(3, '\n');
        let (Some(bare), Some(format), Some(git_dir)) = (lines.next(), lines.next(), lines.next())
        else {
            return Err(unexpected_output("rev-parse"));
        };
        let bare = bare == "true";
        let found = if bare {
            Some(git_dir.trim_end_matches('\n').to_owned())
        } else {
            self.query(dir, &["rev-parse", "--show-toplevel"])?
        };
        let is_top = found.is_some_and(|found| {
            std::fs::canonicalize(found.trim_end_matches('\n')).is_ok_and(|found| found == top)
        });
        if !is_top {
            return Ok(None);
        }
        if format != "sha1" {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!(
                    "{} uses {format} object ids, and fold reads and writes SHA-1 Git repositories only",
                    dir.display()
                ),
            )
            .with_hint("use a repository created with --object-format=sha1")
            .with_details([("object_format", json!(format))]));
        }
        Ok(Some(Repository { bare }))
    }

    /// Whether a working tree of `repo`, the main one or a linked one, has `refname` checked out.
    pub fn checked_out(&self, repo: &Path, refname: &str) -> Result<bool, FoldError> {
        let out = self.output(repo, &["worktree", "list", "--porcelain", "-z"], "worktree")?;
        let wanted = format!("branch {refname}");
        Ok(out
            .split(|&byte| byte == 0)
            .any(|field| field == wanted.as_bytes()))
    }

    /// Creates a bare SHA-1 repository in `dir` whose unborn `HEAD` names `branch`, whatever the user's defaults are. Bare, because a branch that is checked out would keep its old files in the working tree when fold moves it.
    pub fn init_bare(&self, dir: &Path, branch: &str) -> Result<(), FoldError> {
        check_rev(branch)?;
        let initial = format!("--initial-branch={branch}");
        self.output(
            dir,
            &[
                "init",
                "--quiet",
                "--bare",
                "--object-format=sha1",
                &initial,
            ],
            "init",
        )?;
        Ok(())
    }

    /// Creates a SHA-1 repository whose working tree is `dir`, with an unborn `HEAD` on `branch`, whatever the user's defaults are.
    pub fn init_checkout(&self, dir: &Path, branch: &str) -> Result<(), FoldError> {
        check_rev(branch)?;
        let initial = format!("--initial-branch={branch}");
        self.output(
            dir,
            &["init", "--quiet", "--object-format=sha1", &initial],
            "init",
        )?;
        Ok(())
    }

    /// The top of the working tree that holds `dir`, `dir` itself or a folder above it, or `None` when no repository's working tree holds it.
    pub fn working_tree_top(&self, dir: &Path) -> Result<Option<PathBuf>, FoldError> {
        Ok(self
            .query(dir, &["rev-parse", "--show-toplevel"])?
            .map(|top| PathBuf::from(top.trim_end_matches('\n')))
            .filter(|top| !top.as_os_str().is_empty()))
    }

    /// The UTF-8 standard output of a query, or `None` when it exits non-zero, for the questions a failure answers.
    fn query(&self, dir: &Path, args: &[&str]) -> Result<Option<String>, FoldError> {
        let mut command = self.command(dir);
        command.args(args).stdout(Stdio::piped());
        let output = Self::spawn(&mut command, "rev-parse")?
            .wait_with_output()
            .map_err(|error| wait_error(&error, "rev-parse"))?;
        if !output.status.success() {
            return Ok(None);
        }
        String::from_utf8(output.stdout)
            .map(Some)
            .map_err(|_| unexpected_output("rev-parse"))
    }
}

/// A repository whose top a path is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Repository {
    pub bare: bool,
}

/// A Git commit id as 40 lowercase hex digits.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommitId(String);

impl CommitId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for CommitId {
    type Err = FoldError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.len() == 40
            && text
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            Ok(Self(text.to_owned()))
        } else {
            Err(FoldError::new(
                codes::INVALID_REQUEST,
                "a Git commit id is 40 lowercase hex digits",
            ))
        }
    }
}

impl fmt::Display for CommitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A revision or ref name that Git could read as an option is refused before any command runs.
pub(crate) fn check_rev(rev: &str) -> Result<(), FoldError> {
    if rev.is_empty() || rev.starts_with('-') || rev.contains(['\0', '\n']) {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            "a Git revision must not be empty, start with `-` or hold a line break",
        )
        .with_details([("rev", json!(rev))]));
    }
    Ok(())
}

fn parse_commit_line(out: &[u8], what: &'static str) -> Result<CommitId, FoldError> {
    std::str::from_utf8(out)
        .ok()
        .and_then(|text| text.strip_suffix('\n'))
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| unexpected_output(what))
}

pub(crate) fn unexpected_output(what: &'static str) -> FoldError {
    FoldError::new(
        codes::GIT_FAILED,
        format!("git {what} gave output fold cannot read"),
    )
    .with_details([("command", json!(what))])
}

fn kill_group(child: &mut Child) {
    if let Ok(pid) = i32::try_from(child.id())
        && pid > 1
    {
        // SAFETY: `kill` takes plain integers and touches no memory. The child leads its own process group (`process_group(0)`), and every caller signals before it reaps the child, so the group id is still the child's own and `-pid` reaches it and the helpers it started and nothing else. `pid > 1`, so `-pid` never means every process.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn timed_out(what: &'static str, limit: Duration) -> FoldError {
    FoldError::new(
        codes::GIT_FAILED,
        format!(
            "git {what} did not finish within {} seconds",
            limit.as_secs()
        ),
    )
    .with_hint("check that the remote is reachable, then try again")
    .with_details([("command", json!(what)), ("timed_out", json!(true))])
}

/// The exit status only: Git's own message can quote file content, so it is never kept.
pub(crate) fn failed(what: &'static str, status: ExitStatus) -> FoldError {
    FoldError::new(codes::GIT_FAILED, format!("git {what} failed ({status})"))
        .with_hint("run the same git command in the repository to see why")
        .with_details([("command", json!(what)), ("status", json!(status.code()))])
}

fn spawn_error(error: &io::Error, what: &'static str) -> FoldError {
    if error.kind() == io::ErrorKind::NotFound {
        FoldError::new(codes::GIT_NOT_FOUND, "the git program is not on PATH")
            .with_hint("install Git or add it to PATH")
    } else {
        FoldError::new(
            codes::GIT_FAILED,
            format!("cannot start git {what}: {}", error.kind()),
        )
        .with_details([("command", json!(what))])
    }
}

pub(crate) fn wait_error(error: &io::Error, what: &'static str) -> FoldError {
    FoldError::new(
        codes::GIT_FAILED,
        format!("lost git {what}: {}", error.kind()),
    )
    .with_details([("command", json!(what))])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_commit_id_is_forty_lowercase_hex_digits() {
        let id: CommitId = "0123456789abcdef0123456789abcdef01234567".parse().unwrap();
        assert_eq!(id.to_string(), "0123456789abcdef0123456789abcdef01234567");
        assert_eq!(id.as_str().len(), 40);
        for bad in [
            "",
            "0123",
            &"A".repeat(40),
            &"g".repeat(40),
            &"a".repeat(41),
        ] {
            assert_eq!(
                bad.parse::<CommitId>().unwrap_err().code(),
                codes::INVALID_REQUEST
            );
        }
    }

    #[test]
    fn revisions_that_read_as_options_are_refused() {
        for bad in ["", "-x", "--upload-pack=x", "a\nb", "a\0b"] {
            assert_eq!(check_rev(bad).unwrap_err().code(), codes::INVALID_REQUEST);
        }
        assert!(check_rev("HEAD~1").is_ok());
    }

    #[test]
    fn output_lines_must_hold_one_commit() {
        let line = b"0123456789abcdef0123456789abcdef01234567\n";
        assert!(parse_commit_line(line, "x").is_ok());
        for bad in [
            &b"0123456789abcdef0123456789abcdef01234567"[..],
            b"\xff\n",
            b"x\n",
        ] {
            assert_eq!(
                parse_commit_line(bad, "x").unwrap_err().code(),
                codes::GIT_FAILED
            );
        }
    }

    #[test]
    fn a_missing_program_is_git_not_found_and_other_spawn_errors_are_git_failed() {
        let dir = tempfile::tempdir().unwrap();
        let missing = Git::new(dir.path().join("no-git"));
        let error = missing.resolve_commit(dir.path(), "HEAD").unwrap_err();
        assert_eq!(error.code(), codes::GIT_NOT_FOUND);
        // A directory cannot be executed.
        let denied = Git::new(dir.path());
        let error = denied.resolve_commit(dir.path(), "HEAD").unwrap_err();
        assert_eq!(error.code(), codes::GIT_FAILED);
        let lost = wait_error(&io::Error::other("x"), "archive");
        assert_eq!(lost.code(), codes::GIT_FAILED);
    }

    #[test]
    fn commands_never_wait_for_a_credential_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let command = Git::default().command(dir.path());
        let prompt = command
            .get_envs()
            .find(|(name, _)| *name == "GIT_TERMINAL_PROMPT");
        assert_eq!(
            prompt,
            Some((OsStr::new("GIT_TERMINAL_PROMPT"), Some(OsStr::new("0"))))
        );
    }

    #[test]
    fn commands_refuse_the_transports_that_run_commands() {
        let dir = tempfile::tempdir().unwrap();
        let command = Git::default().command(dir.path());
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            args[..4],
            [
                "-c",
                "protocol.ext.allow=never",
                "-c",
                "protocol.fd.allow=never"
            ]
        );
        let allow = command
            .get_envs()
            .find(|(name, _)| *name == "GIT_ALLOW_PROTOCOL");
        assert_eq!(allow, Some((OsStr::new("GIT_ALLOW_PROTOCOL"), None)));
    }

    #[test]
    fn queries_outside_a_repository_answer_no() {
        let dir = tempfile::tempdir().unwrap();
        let git = Git::default();
        assert_eq!(git.repository(dir.path()).unwrap(), None);
        assert_eq!(git.repository(&dir.path().join("absent")).unwrap(), None);
        let error = git.resolve_commit(dir.path(), "HEAD").unwrap_err();
        assert_eq!(error.code(), codes::GIT_FAILED);
        assert_eq!(error.details().unwrap()["command"], "rev-parse");
        assert!(git.ref_commit(dir.path(), "refs/heads/main").is_err());
    }

    fn run(dir: &Path, args: &[&str]) {
        let status = Git::default()
            .command(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn only_the_top_of_a_sha1_repository_is_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        let git = Git::default();
        let work = dir.path().join("work");
        std::fs::create_dir_all(work.join("sub")).unwrap();
        run(&work, &["init", "--quiet", "--initial-branch=main"]);
        assert_eq!(
            git.repository(&work).unwrap(),
            Some(Repository { bare: false })
        );
        assert_eq!(git.repository(&work.join("sub")).unwrap(), None);
        assert_eq!(git.repository(&work.join(".git")).unwrap(), None);
        let bare = dir.path().join("bare.git");
        std::fs::create_dir(&bare).unwrap();
        git.init_bare(&bare, "main").unwrap();
        assert_eq!(
            git.repository(&bare).unwrap(),
            Some(Repository { bare: true })
        );
        assert!(!git.checked_out(&bare, "refs/heads/main").unwrap());
        assert!(git.checked_out(&work, "refs/heads/main").unwrap());
        assert!(!git.checked_out(&work, "refs/heads/other").unwrap());
        let sha256 = dir.path().join("sha256");
        std::fs::create_dir(&sha256).unwrap();
        run(&sha256, &["init", "--quiet", "--object-format=sha256"]);
        let error = git.repository(&sha256).unwrap_err();
        assert_eq!(error.code(), codes::INVALID_REQUEST);
        assert_eq!(error.details().unwrap()["object_format"], "sha256");
    }
}
