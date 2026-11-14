use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use fold_core::ProjectConfig;
use tempfile::TempDir;

const TOKEN_A: &str = "sk-ant-oat01-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TOKEN_B: &str = "sk-ant-oat01-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// HOME and the XDG directories in one temporary directory.
#[cfg(test)]
struct Sandbox {
    dir: TempDir,
}

#[cfg(test)]
impl Sandbox {
    fn new() -> Self {
        let sandbox = Self {
            dir: TempDir::new().unwrap(),
        };
        fs::create_dir_all(sandbox.path("config/fold")).unwrap();
        fs::write(
            sandbox.path("config/fold/daemon.toml"),
            "listen = \"127.0.0.1:0\"\n",
        )
        .unwrap();
        sandbox
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().canonicalize().unwrap().join(name)
    }

    fn accounts(&self) -> PathBuf {
        self.path("data/fold/accounts")
    }

    fn socket(&self) -> PathBuf {
        self.path("fold.sock")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fld"));
        command
            .args(args)
            .current_dir(self.dir.path())
            .env_clear()
            .env("HOME", self.path("home"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("FOLD_HUMAN", "ana")
            .env("PATH", system_path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        command
    }

    fn fold(&self, args: &[&str]) -> Output {
        self.command(args).stdin(Stdio::null()).output().unwrap()
    }

    /// `fld <args>` with `input` on its stdin, a pipe.
    fn fold_with_input(&self, args: &[&str], input: &str) -> Output {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    /// A plain project `app`, registered in the sandbox.
    fn project(&self) -> PathBuf {
        fs::create_dir(self.path("app")).unwrap();
        let init = self
            .command(&["init"])
            .current_dir(self.path("app"))
            .output()
            .unwrap();
        assert!(init.status.success(), "{init:?}");
        self.path("app")
    }

    /// `fld serve` on the sandbox socket, its log in `daemon.log`, stopped when the guard drops.
    fn serve(&self) -> Daemon {
        let socket = self.socket();
        let log = fs::File::create(self.path("daemon.log")).unwrap();
        let child = self
            .command(&["serve", "--socket", socket.to_str().unwrap()])
            .env("XDG_RUNTIME_DIR", self.path("run"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap();
        let daemon = Daemon(child);
        let deadline = Instant::now() + Duration::from_secs(20);
        while std::os::unix::net::UnixStream::connect(&socket).is_err() {
            assert!(
                Instant::now() < deadline,
                "the daemon did not open its socket"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        daemon
    }

    fn use_account(&self, project: &Path, name: &str) -> Output {
        let socket = self.socket();
        self.fold(&[
            "account",
            "use",
            name,
            "--socket",
            socket.to_str().unwrap(),
            "--project",
            project.to_str().unwrap(),
        ])
    }
}

#[cfg(test)]
struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(test)]
fn system_path() -> String {
    std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_owned())
}

#[cfg(test)]
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[cfg(test)]
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[cfg(test)]
fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Neither output of `output` holds a token; the check names no token when it fails.
#[cfg(test)]
fn holds_no_token(output: &Output) {
    for token in [TOKEN_A, TOKEN_B] {
        assert!(
            !stdout(output).contains(token) && !stderr(output).contains(token),
            "an output holds a token"
        );
    }
}

#[test]
fn add_reads_the_token_from_stdin_into_a_private_file_and_list_never_shows_it() {
    let sandbox = Sandbox::new();
    let empty = sandbox.fold(&["account", "list"]);
    assert!(empty.status.success(), "{empty:?}");
    assert_eq!(
        stdout(&empty),
        "system-default Claude Code's own login used by no project\n"
    );
    let added = sandbox.fold_with_input(&["account", "add", "work-a"], &format!("{TOKEN_A}\n"));
    assert!(added.status.success(), "{}", stderr(&added));
    holds_no_token(&added);
    assert!(stdout(&added).starts_with("added account work-a;"));
    let dir = sandbox.accounts();
    assert_eq!(mode(&dir), 0o700);
    let mut holding = 0;
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        assert_eq!(mode(&path), 0o600, "{}", path.display());
        if fs::read_to_string(&path).unwrap().contains(TOKEN_A) {
            holding += 1;
        }
    }
    assert_eq!(holding, 1, "one file holds the token");
    let listed = sandbox.fold(&["account", "list"]);
    assert!(listed.status.success(), "{listed:?}");
    holds_no_token(&listed);
    let text = stdout(&listed);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].starts_with("system-default "),
        "system-default comes first"
    );
    assert!(lines[1].starts_with("work-a added "), "{lines:?}");
    assert!(lines[1].ends_with(" used by no project"));

    let again = sandbox.fold_with_input(&["account", "add", "work-a"], TOKEN_B);
    assert!(!again.status.success());
    assert!(stderr(&again).contains("account_exists: "), "{again:?}");
    assert!(stderr(&again).contains("fld account update work-a"));
    holds_no_token(&again);
    let missing = sandbox.fold_with_input(&["account", "update", "work-b"], TOKEN_B);
    assert!(
        stderr(&missing).contains("account_not_found: "),
        "{missing:?}"
    );
    let renewed = sandbox.fold_with_input(&["account", "update", "work-a"], TOKEN_B);
    assert!(renewed.status.success(), "{}", stderr(&renewed));
    holds_no_token(&renewed);
    assert!(stdout(&renewed).starts_with("account work-a has the new token"));
    assert!(stdout(&renewed).contains("before its next turn"));
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        assert_eq!(mode(&path), 0o600, "{}", path.display());
    }
    let files: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap_or_default())
        .collect();
    assert!(
        !files.iter().any(|text| text.contains(TOKEN_A)),
        "the old token is gone"
    );
    assert!(files.iter().any(|text| text.contains(TOKEN_B)));

    let removed = sandbox.fold(&["account", "remove", "work-a"]);
    assert!(removed.status.success(), "{removed:?}");
    assert_eq!(stdout(&removed), "removed account work-a and its token\n");
    for entry in fs::read_dir(&dir).unwrap() {
        let text = fs::read_to_string(entry.unwrap().path()).unwrap_or_default();
        assert!(!text.contains(TOKEN_B), "a file still holds the token");
    }
}

#[test]
fn a_token_comes_only_from_stdin_and_a_bad_one_is_refused_with_its_reason() {
    let sandbox = Sandbox::new();
    let argument = sandbox.fold(&["account", "add", "work-a", TOKEN_A]);
    assert!(!argument.status.success());
    assert!(
        stderr(&argument).contains("unexpected argument"),
        "{argument:?}"
    );
    assert!(!sandbox.accounts().exists());
    for (input, reason) in [
        ("", "fld account add read no token"),
        ("hello\nworld\n", "more than one line"),
        ("short", "not a token"),
    ] {
        let refused = sandbox.fold_with_input(&["account", "add", "work-a"], input);
        assert!(!refused.status.success(), "{input:?}");
        let text = stderr(&refused);
        assert!(text.contains("invalid_account_token: "), "{text}");
        assert!(text.contains(reason), "{text}");
        assert!(text.contains("hint: run `claude setup-token`"), "{text}");
    }
    let bad_name = sandbox.fold_with_input(&["account", "add", "Work A"], TOKEN_A);
    assert!(
        stderr(&bad_name).contains("invalid_account_name: "),
        "{bad_name:?}"
    );
    holds_no_token(&bad_name);
    let unknown = sandbox.fold(&["account", "remove", "work-a"]);
    assert!(
        stderr(&unknown).contains("account_not_found: "),
        "{unknown:?}"
    );
    assert!(stderr(&unknown).contains("hint: run `fld account list`"));
    for args in [
        &["account", "add", "system-default"][..],
        &["account", "update", "system-default"][..],
        &["account", "remove", "system-default"][..],
    ] {
        let reserved = sandbox.fold_with_input(args, TOKEN_A);
        assert!(!reserved.status.success(), "{args:?}");
        let text = stderr(&reserved);
        assert!(text.contains("account_reserved: "), "{text}");
        assert!(text.contains("hint: "), "{text}");
        holds_no_token(&reserved);
    }
    let written = fs::read_dir(sandbox.accounts())
        .map(|entries| {
            entries.flatten().any(|entry| {
                fs::read_to_string(entry.path()).is_ok_and(|text| text.contains(TOKEN_A))
            })
        })
        .unwrap_or(false);
    assert!(!written, "no token was written");
}

#[test]
fn use_goes_through_the_daemon_and_remove_is_refused_while_a_project_uses_the_account() {
    let sandbox = Sandbox::new();
    let project = sandbox.project();
    let _daemon = sandbox.serve();
    let unknown = sandbox.use_account(&project, "work-a");
    assert!(!unknown.status.success());
    assert!(
        stderr(&unknown).contains("account_not_found: "),
        "{unknown:?}"
    );
    let added = sandbox.fold_with_input(&["account", "add", "work-a"], TOKEN_A);
    assert!(added.status.success(), "{added:?}");
    let used = sandbox.use_account(&project, "work-a");
    assert!(used.status.success(), "{}", stderr(&used));
    holds_no_token(&used);
    assert!(stdout(&used).starts_with("the project uses account work-a: "));
    let config = fs::read_to_string(project.join(".fold/config.toml")).unwrap();
    assert!(
        !config.contains(TOKEN_A),
        "the project folder holds the name only"
    );
    assert_eq!(
        ProjectConfig::from_toml(&config)
            .unwrap()
            .account
            .map(|name| name.to_string()),
        Some("work-a".to_owned())
    );
    let listed = sandbox.fold(&["account", "list"]);
    assert!(stdout(&listed).ends_with(" used by app\n"), "{listed:?}");
    let refused = sandbox.fold(&["account", "remove", "work-a"]);
    assert!(!refused.status.success());
    let text = stderr(&refused);
    assert!(text.contains("account_in_use: "), "{text}");
    assert!(text.contains("project app"), "{text}");
    assert!(text.contains("hint: choose another account"), "{text}");
    assert!(sandbox.fold(&["account", "list"]).status.success());
    let back = sandbox.use_account(&project, "system-default");
    assert!(back.status.success(), "{}", stderr(&back));
    let config = fs::read_to_string(project.join(".fold/config.toml")).unwrap();
    assert_eq!(ProjectConfig::from_toml(&config).unwrap().account, None);
    assert!(
        !config.contains("account ="),
        "system-default removes the key"
    );
    let listed = sandbox.fold(&["account", "list"]);
    assert!(
        stdout(&listed).starts_with("system-default Claude Code's own login used by app\n"),
        "{listed:?}"
    );
    let reserved = sandbox.fold(&["account", "remove", "system-default"]);
    assert!(
        stderr(&reserved).contains("account_reserved: "),
        "the reserved name is refused before its use: {reserved:?}"
    );
    let log = fs::read_to_string(sandbox.path("daemon.log")).unwrap();
    assert!(!log.contains(TOKEN_A), "the daemon's log holds the token");
}
