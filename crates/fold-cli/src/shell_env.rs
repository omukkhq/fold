use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use fold_daemon::services::agents::environment::{
    EnvSource, ShellProblem, WorkerEnvironment, sign_in_names, worker_variables,
};

use fold_daemon::services::agents::Launcher;

use crate::platform::on_path;
use crate::service::SERVICE_ENV;

/// The hidden subcommand that the login shell runs to print its environment.
pub(crate) const SUBCOMMAND: &str = "shell-env";

/// What `fld shell-env` prints before and after the environment. The shell's startup files can print text before it; `exec` leaves nothing to print after it.
const BEGIN: &[u8] = b"\0fold-shell-env-begin\0";
const END: &[u8] = b"fold-shell-env-end";

/// How long the login shell may take to print its environment before the daemon stops it and starts without it.
pub(crate) const SHELL_TIMEOUT: Duration = Duration::from_secs(10);

/// How often the daemon looks whether the shell has exited.
const POLL: Duration = Duration::from_millis(20);

/// The most output of the shell that is read; an environment is far smaller.
const MAX_OUTPUT: u64 = 8 * 1024 * 1024;

/// `fld shell-env`: every variable as `name=value` followed by a NUL byte, between the markers.
pub(crate) fn print(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
    out: &mut impl Write,
) -> io::Result<()> {
    out.write_all(BEGIN)?;
    for (name, value) in vars {
        out.write_all(name.as_bytes())?;
        out.write_all(b"=")?;
        out.write_all(value.as_bytes())?;
        out.write_all(b"\0")?;
    }
    out.write_all(END)?;
    out.write_all(b"\0")?;
    out.flush()
}

/// The variables that [`print`] wrote into `output`, after any text before them; `None` when the markers or an entry are missing.
pub(crate) fn parse(output: &[u8]) -> Option<Vec<(OsString, OsString)>> {
    let start = output
        .windows(BEGIN.len())
        .position(|window| window == BEGIN)?
        + BEGIN.len();
    let mut vars = Vec::new();
    for entry in output[start..].split(|byte| *byte == 0) {
        if entry == END {
            return Some(vars);
        }
        let at = entry.iter().position(|byte| *byte == b'=')?;
        vars.push((
            OsString::from_vec(entry[..at].to_vec()),
            OsString::from_vec(entry[at + 1..].to_vec()),
        ));
    }
    None
}

/// The user's login shell: `SHELL` when it names an absolute path, else the shell of `uid`'s line in the password file.
pub(crate) fn login_shell(
    shell: Option<OsString>,
    passwd: Option<&str>,
    uid: u32,
) -> Option<PathBuf> {
    shell
        .map(PathBuf::from)
        .filter(|shell| shell.is_absolute())
        .or_else(|| passwd.and_then(|text| passwd_shell(text, uid)))
}

fn passwd_shell(text: &str, uid: u32) -> Option<PathBuf> {
    text.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        let [_, _, id, _, _, _, shell] = fields.as_slice() else {
            return None;
        };
        (id.parse::<u32>().ok()? == uid)
            .then(|| PathBuf::from(shell))
            .filter(|shell| shell.is_absolute())
    })
}

/// Runs `shell` once in `dir` as a login and interactive shell that replaces itself with `fld shell-env`, and reads the environment it prints. Its input is empty and its error output goes nowhere, so a startup file that asks or prints cannot reach the user. It runs in a session of its own, without the daemon's controlling terminal: an interactive shell in a background process group of that terminal stops when it tries to take the terminal. A shell that runs longer than `timeout` is killed with its process group. Nothing the shell printed reaches an error or a log.
pub(crate) fn read_shell(
    shell: &Path,
    fld: &Path,
    dir: &Path,
    timeout: Duration,
) -> Result<Vec<(OsString, OsString)>, ShellProblem> {
    // A single quote has no escape that every shell reads the same way.
    let fld = fld
        .to_str()
        .filter(|text| !text.contains('\''))
        .ok_or(ShellProblem::Failed)?;
    let mut output = tempfile::tempfile().map_err(|_| ShellProblem::Failed)?;
    let stdout = output.try_clone().map_err(|_| ShellProblem::Failed)?;
    let mut command = Command::new(shell);
    command
        .args(["-l", "-i", "-c", &format!("exec '{fld}' {SUBCOMMAND}")])
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::null());
    // SAFETY: the closure runs in the forked child before `exec` and calls only `setsid`, which is async-signal-safe and touches no memory of the parent. A forked child leads no process group, so `setsid` succeeds, and the new session's group id is the child's pid, which `kill` signals.
    unsafe {
        command.pre_exec(|| rustix::process::setsid().map(drop).map_err(io::Error::from));
    }
    let mut child = command.spawn().map_err(|_| ShellProblem::Failed)?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            Ok(None) => {
                kill(&mut child);
                return Err(ShellProblem::TimedOut);
            }
            Err(_) => {
                kill(&mut child);
                return Err(ShellProblem::Failed);
            }
        }
    };
    if !status.success() {
        return Err(ShellProblem::Failed);
    }
    let mut bytes = Vec::new();
    output
        .seek(SeekFrom::Start(0))
        .and_then(|_| output.take(MAX_OUTPUT).read_to_end(&mut bytes))
        .map_err(|_| ShellProblem::Failed)?;
    parse(&bytes).ok_or(ShellProblem::Failed)
}

/// Kills the shell and every process of its group, such as a command a startup file left waiting, and reaps the shell.
fn kill(child: &mut Child) {
    if let Some(group) = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// The environment the daemon gives its workers, with what it found about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chosen {
    pub(crate) environment: WorkerEnvironment,
    /// The shell's variables that every worker takes over the daemon's own; empty when the daemon's own environment was chosen.
    pub(crate) workers: Vec<(String, String)>,
    /// The `PATH` the daemon itself takes, so the `git` of its commands and the commands of its checks are found where the shell finds them; `None` keeps its own.
    pub(crate) path: Option<OsString>,
}

/// The login shell's environment when it read and its `PATH` holds `claude`; otherwise the daemon's own, `own`, which counts as the environment of `fld setup` when `service` says the service that `fld setup` installed started the daemon.
pub(crate) fn choose(
    shell: Result<Vec<(OsString, OsString)>, ShellProblem>,
    own: &[(OsString, OsString)],
    service: bool,
) -> Chosen {
    let shell_problem = match shell {
        Ok(vars) => {
            let path = value(&vars, "PATH").map(OsStr::to_owned);
            let path = path.as_deref();
            match program(path, "claude") {
                Some(claude) => {
                    let workers = worker_variables(vars);
                    let names = own
                        .iter()
                        .filter_map(|(name, _)| name.to_str())
                        .chain(workers.iter().map(|(name, _)| name.as_str()));
                    return Chosen {
                        environment: WorkerEnvironment {
                            source: EnvSource::Shell,
                            shell_problem: None,
                            claude: Some(claude),
                            git: program(path, "git"),
                            sign_in: sign_in_names(names),
                        },
                        path: path.map(OsStr::to_owned),
                        workers,
                    };
                }
                None => ShellProblem::NoClaude,
            }
        }
        Err(problem) => problem,
    };
    let path = value(own, "PATH");
    Chosen {
        environment: WorkerEnvironment {
            source: if service {
                EnvSource::Setup
            } else {
                EnvSource::Daemon
            },
            shell_problem: Some(shell_problem),
            claude: program(path, "claude"),
            git: program(path, "git"),
            sign_in: sign_in_names(own.iter().filter_map(|(name, _)| name.to_str())),
        },
        workers: Vec::new(),
        path: None,
    }
}

fn value<'a>(vars: &'a [(OsString, OsString)], name: &str) -> Option<&'a OsStr> {
    vars.iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_os_str())
}

/// `name` on `path`, when that names an absolute file a worker can run from any folder.
fn program(path: Option<&OsStr>, name: &str) -> Option<PathBuf> {
    on_path(name, path).filter(|found| found.is_absolute())
}

/// The folder the login shell runs in: the home folder when `home` names an existing one, else `/`. Startup files that load the tools of the folder they start in then never give every worker the tools of the folder the daemon was started from.
pub(crate) fn shell_dir(home: Option<OsString>) -> PathBuf {
    home.map(PathBuf::from)
        .filter(|home| home.is_absolute() && home.is_dir())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Reads the login shell of this process's user once, with the running fld as the program that prints its environment, and chooses the workers' environment as [`choose`] does.
pub(crate) fn from_host() -> Chosen {
    let own: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    let service = value(&own, SERVICE_ENV).is_some();
    let passwd = std::fs::read_to_string("/etc/passwd").ok();
    let uid = rustix::process::getuid().as_raw();
    let shell = match (
        login_shell(std::env::var_os("SHELL"), passwd.as_deref(), uid),
        Launcher::default().with_own_binary().fold,
    ) {
        (Some(shell), Some(fld)) => read_shell(
            &shell,
            &fld,
            &shell_dir(std::env::var_os("HOME")),
            SHELL_TIMEOUT,
        ),
        (None, _) => Err(ShellProblem::NoShell),
        (Some(_), None) => Err(ShellProblem::Failed),
    };
    choose(shell, &own, service)
}

/// Logs which environment the workers get and why, with source and problem names only, never a path or a value.
pub(crate) fn log(environment: &WorkerEnvironment) {
    let source = environment.source.as_str();
    match environment.shell_problem {
        None => tracing::info!(source, "workers run with the login shell's environment"),
        Some(problem) => tracing::warn!(
            source,
            shell = problem.as_str(),
            "the login shell gave no environment with claude, so workers run with the daemon's own environment"
        ),
    }
    if environment.claude.is_none() {
        tracing::warn!(
            source,
            "claude is not on the PATH of the workers' environment, so no worker can start; install Claude Code or add its folder to PATH in your shell's startup files, then run `fld restart`"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    }

    fn script(path: &Path, body: &str) {
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A `bin` folder in `dir` with an executable stand-in for each name.
    fn bin(dir: &Path, names: &[&str]) -> PathBuf {
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for name in names {
            script(&bin.join(name), "exit 0");
        }
        bin
    }

    #[test]
    fn the_printed_environment_reads_back_after_the_noise_of_a_shell() {
        use std::os::unix::ffi::OsStringExt;
        let printed = vec![
            (OsString::from("PATH"), OsString::from("/a:/b")),
            (OsString::from("EQ"), OsString::from("x=y")),
            (
                OsString::from("LINES"),
                OsString::from("one\ntwo fold-shell-env-end"),
            ),
            (OsString::from("RAW"), OsString::from_vec(vec![0xff, b'z'])),
            (OsString::from("EMPTY"), OsString::new()),
        ];
        let mut out = b"Welcome!\n\0stray\0".to_vec();
        print(printed.clone(), &mut out).unwrap();
        out.extend_from_slice(b"bye\n");
        assert_eq!(parse(&out), Some(printed));
        assert_eq!(parse(b"no markers at all"), None);
        let mut cut = Vec::new();
        print(vars(&[("A", "1")]), &mut cut).unwrap();
        assert_eq!(parse(&cut[..cut.len() - 6]), None, "no end marker");
        let mut broken = BEGIN.to_vec();
        broken.extend_from_slice(b"no-equals\0fold-shell-env-end\0");
        assert_eq!(parse(&broken), None);
    }

    #[test]
    fn the_login_shell_is_shell_when_absolute_else_the_password_files() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\nbroken line\nana:x:1000:1000:Ana,,,:/home/ana:/usr/bin/fish\nbob:x:1001:1001::/home/bob:nologin\n";
        assert_eq!(
            login_shell(Some("/bin/zsh".into()), Some(passwd), 1000),
            Some(PathBuf::from("/bin/zsh"))
        );
        for shell in [None, Some(OsString::from("zsh")), Some(OsString::new())] {
            assert_eq!(
                login_shell(shell, Some(passwd), 1000),
                Some(PathBuf::from("/usr/bin/fish"))
            );
        }
        assert_eq!(login_shell(None, Some(passwd), 1001), None);
        assert_eq!(login_shell(None, Some(passwd), 4242), None);
        assert_eq!(login_shell(None, None, 0), None);
    }

    #[test]
    fn a_shell_that_prints_noise_gives_the_environment_of_its_startup_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let shell = dir.path().join("fake-shell");
        // The stand-in checks its flags, prints what a startup file might, exports a folder of the user's and runs the command it was given.
        script(
            &shell,
            "[ \"$1 $2 $3\" = '-l -i -c' ] || exit 9\necho 'Welcome to the fake shell'\necho 'noise' >&2\nexport PATH=\"/home/ana/.local/bin:$PATH\" FROM_RC=yes\nexec /bin/sh -c \"$4\"",
        );
        let fld = dir.path().join("fld");
        script(
            &fld,
            "[ \"$1\" = shell-env ] || exit 8\nprintf '\\000fold-shell-env-begin\\000'\nprintf 'PATH=%s\\000FROM_RC=%s\\000' \"$PATH\" \"$FROM_RC\"\nprintf 'fold-shell-env-end\\000'",
        );
        let read = read_shell(&shell, &fld, dir.path(), Duration::from_secs(10)).unwrap();
        assert_eq!(value(&read, "FROM_RC"), Some(OsStr::new("yes")));
        assert!(
            value(&read, "PATH")
                .unwrap()
                .to_string_lossy()
                .starts_with("/home/ana/.local/bin:")
        );
    }

    #[test]
    fn a_shell_that_hangs_past_the_limit_is_killed_with_its_group() {
        let dir = tempfile::TempDir::new().unwrap();
        let shell = dir.path().join("fake-shell");
        let pid_file = dir.path().join("child.pid");
        script(
            &shell,
            &format!("sleep 300 &\necho $! > '{}'\nwait", pid_file.display()),
        );
        let started = Instant::now();
        let read = read_shell(
            &shell,
            Path::new("/bin/true"),
            dir.path(),
            Duration::from_millis(300),
        );
        assert_eq!(read, Err(ShellProblem::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(5));
        let child: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let pid = rustix::process::Pid::from_raw(child).unwrap();
        while rustix::process::test_kill_process(pid).is_ok() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            rustix::process::test_kill_process(pid).is_err(),
            "the shell's background command is gone"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_shell_runs_in_a_session_of_its_own_and_in_the_given_folder() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let shell = dir.path().join("fake-shell");
        let seen = dir.path().join("seen");
        // Field 6 of /proc/<pid>/stat is the session id; the stand-in's name holds no space, so the fields split on spaces.
        script(
            &shell,
            &format!(
                "cut -d' ' -f6 /proc/$$/stat > '{seen}'\npwd -P >> '{seen}'\nexec /bin/sh -c \"$4\"",
                seen = seen.display()
            ),
        );
        let fld = dir.path().join("fld");
        script(
            &fld,
            "printf '\\000fold-shell-env-begin\\000fold-shell-env-end\\000'",
        );
        assert_eq!(
            read_shell(&shell, &fld, &home, SHELL_TIMEOUT),
            Ok(Vec::new())
        );
        let text = std::fs::read_to_string(&seen).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let own = rustix::process::getsid(None)
            .unwrap()
            .as_raw_nonzero()
            .get();
        assert_ne!(lines[0].parse::<i32>().unwrap(), own, "{text}");
        assert_eq!(Path::new(lines[1]), home.canonicalize().unwrap());
    }

    #[test]
    fn the_shell_starts_in_the_home_folder_or_else_at_the_root() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(shell_dir(Some(dir.path().into())), dir.path());
        for home in [
            None,
            Some(OsString::new()),
            Some(OsString::from("relative/home")),
            Some(dir.path().join("missing").into()),
        ] {
            assert_eq!(shell_dir(home), Path::new("/"));
        }
    }

    #[test]
    fn a_shell_that_fails_or_prints_no_environment_is_a_failure() {
        let dir = tempfile::TempDir::new().unwrap();
        let failing = dir.path().join("failing");
        script(&failing, "exit 1");
        assert_eq!(
            read_shell(&failing, Path::new("/bin/true"), dir.path(), SHELL_TIMEOUT),
            Err(ShellProblem::Failed)
        );
        let silent = dir.path().join("silent");
        script(&silent, "echo only noise");
        assert_eq!(
            read_shell(&silent, Path::new("/bin/true"), dir.path(), SHELL_TIMEOUT),
            Err(ShellProblem::Failed)
        );
        assert_eq!(
            read_shell(
                &dir.path().join("missing"),
                Path::new("/bin/true"),
                dir.path(),
                SHELL_TIMEOUT
            ),
            Err(ShellProblem::Failed)
        );
        assert_eq!(
            read_shell(
                &silent,
                Path::new("/opt/it's/fld"),
                dir.path(),
                SHELL_TIMEOUT
            ),
            Err(ShellProblem::Failed)
        );
    }

    #[test]
    fn the_shell_wins_when_its_path_holds_claude() {
        let dir = tempfile::TempDir::new().unwrap();
        let shell_bin = bin(&dir.path().join("shell"), &["claude", "git"]);
        let own_bin = bin(&dir.path().join("own"), &["git"]);
        let path = shell_bin.to_string_lossy().into_owned();
        let shell = vars(&[
            ("PATH", &path),
            ("ANTHROPIC_API_KEY", "secret"),
            ("FOLD_AGENT", "main"),
        ]);
        let own = vars(&[("PATH", &own_bin.to_string_lossy()), ("HOME", "/home/ana")]);
        let chosen = choose(Ok(shell), &own, true);
        assert_eq!(chosen.environment.source, EnvSource::Shell);
        assert_eq!(chosen.environment.shell_problem, None);
        assert_eq!(chosen.environment.claude, Some(shell_bin.join("claude")));
        assert_eq!(chosen.environment.git, Some(shell_bin.join("git")));
        assert_eq!(chosen.environment.sign_in, ["ANTHROPIC_API_KEY", "HOME"]);
        assert_eq!(chosen.path, Some(OsString::from(&path)));
        let names: Vec<&str> = chosen
            .workers
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(names, ["ANTHROPIC_API_KEY", "PATH"]);
    }

    #[test]
    fn without_claude_on_the_shells_path_the_daemons_own_environment_is_used() {
        let dir = tempfile::TempDir::new().unwrap();
        let shell_bin = bin(&dir.path().join("shell"), &["git"]);
        let own_bin = bin(&dir.path().join("own"), &["claude"]);
        let shell = vars(&[("PATH", &shell_bin.to_string_lossy())]);
        let own = vars(&[("PATH", &own_bin.to_string_lossy())]);
        let chosen = choose(Ok(shell), &own, true);
        assert_eq!(chosen.environment.source, EnvSource::Setup);
        assert_eq!(
            chosen.environment.shell_problem,
            Some(ShellProblem::NoClaude)
        );
        assert_eq!(chosen.environment.claude, Some(own_bin.join("claude")));
        assert_eq!(chosen.environment.git, None);
        assert!(chosen.workers.is_empty());
        assert_eq!(chosen.path, None);

        let chosen = choose(Err(ShellProblem::TimedOut), &own, false);
        assert_eq!(chosen.environment.source, EnvSource::Daemon);
        assert_eq!(
            chosen.environment.shell_problem,
            Some(ShellProblem::TimedOut)
        );

        let relative = vars(&[("PATH", "relative/bin")]);
        let chosen = choose(Err(ShellProblem::NoShell), &relative, false);
        assert_eq!(chosen.environment.claude, None);
        assert_eq!(
            choose(Err(ShellProblem::Failed), &[], false)
                .environment
                .claude,
            None
        );
    }
}
