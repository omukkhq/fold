use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use clap::{Args, Subcommand};
use fold_core::accounts::{
    ACCOUNT_ENV, AccountName, AccountToken, OAUTH_TOKEN_ENV, OVERRIDING_ENV, TOKEN_CHECK_PROMPT,
    TOKEN_CHECK_TIMEOUT, TokenCheck, TokenCheckRun, Unchecked, check_token, token_check,
    token_rejected,
};
use fold_core::{Clock, FoldError, HostPaths};
use fold_daemon::SystemClock;
use fold_daemon::services::agents::accounts;
use hyper::Method;
use serde_json::json;
use thiserror::Error;

use super::remote::{RemoteError, refusal, request_project_with};
use crate::init::host_paths;
use crate::output;
use crate::serve::{Dirs, current_thread_runtime};

/// The most bytes `fld account add` reads from a pipe: far more than a token, so a file given by mistake is refused as no token.
const MAX_INPUT: u64 = 64 * 1024;

#[derive(Args, Debug)]
pub struct AccountArgs {
    #[command(subcommand)]
    command: AccountCommand,
}

#[derive(Subcommand, Debug)]
enum AccountCommand {
    /// Add a Claude subscription under a new name, with the token that `claude setup-token` printed, read from stdin; one small Claude Code turn checks the token first
    Add(NameArgs),
    /// List the accounts, default (Claude Code's own login, shown while `claude login` stored one on this host) first: name, when the token was added and which projects use each, never the token
    List,
    /// Remove an account and its token; refused while a project uses it
    Remove(NameArgs),
    /// Make the account the one of the project in this folder, default for Claude Code's own login: each worker continues in its own session with it when its turn ends
    Use(UseArgs),
    /// Give an account a new token from stdin, as for an expired or revoked one; one small Claude Code turn checks the token first
    Update(NameArgs),
}

#[derive(Args, Debug)]
struct NameArgs {
    /// The account's name, such as work-a
    name: String,
}

#[derive(Args, Debug)]
struct UseArgs {
    /// The account's name, such as work-a
    name: String,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum AccountError {
    #[error(transparent)]
    Fold(#[from] FoldError),
    #[error(transparent)]
    Daemon(#[from] RemoteError),
    #[error("cannot read the token from stdin: {0}")]
    Read(io::Error),
    #[error("cannot start the async runtime: {0}")]
    Runtime(io::Error),
    #[error("cannot write the output: {0}")]
    Write(#[from] io::Error),
}

impl AccountError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Fold(error) => Some(error.code()),
            Self::Daemon(error) => error.code(),
            Self::Read(_) | Self::Runtime(_) | Self::Write(_) => None,
        }
    }

    /// The hint that names the next step, when a refusal carries one.
    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::Fold(error) => error.hint(),
            Self::Daemon(error) => error.hint(),
            _ => None,
        }
    }
}

/// `add`, `list` and `remove` read and write the host's accounts directly, the way `fld token list` reads the registry, so they work while the daemon is down; a worker reads its account's token at its start. `use` goes through the daemon, which changes the project's config and lets each worker continue with the new account.
pub fn run(
    args: &AccountArgs,
    dirs: &Dirs,
    input: &mut impl Read,
    terminal: bool,
    out: &mut impl Write,
) -> Result<(), AccountError> {
    match &args.command {
        AccountCommand::Add(args) => {
            let name = AccountName::parse(&args.name)?;
            name.check_not_reserved()?;
            let token = check_token(&read_token(input, terminal)?)?;
            let dir = host_paths(dirs)?.accounts_dir();
            accounts::check_change(&dir, &name, false)?;
            let checked = checked_token(&name, &token, false, terminal)?;
            accounts::add(&dir, &name, &token, SystemClock.now())?;
            output::line(out, &added_line(&name))?;
            warn_unchecked(out, &name, checked)?;
        }
        AccountCommand::Update(args) => {
            let name = AccountName::parse(&args.name)?;
            name.check_not_reserved()?;
            let token = check_token(&read_token(input, terminal)?)?;
            let dir = host_paths(dirs)?.accounts_dir();
            accounts::check_change(&dir, &name, true)?;
            let checked = checked_token(&name, &token, true, terminal)?;
            accounts::update(&dir, &name, &token, SystemClock.now())?;
            output::line(out, &updated_line(&name))?;
            warn_unchecked(out, &name, checked)?;
        }
        AccountCommand::List => {
            let host = host_paths(dirs)?;
            let runtime = current_thread_runtime().map_err(AccountError::Runtime)?;
            let usage = runtime.block_on(accounts::usage_on_host(&host))?;
            let listed = accounts::list(&host.accounts_dir())?;
            let default = AccountName::default_account();
            if accounts::claude_login(|name| std::env::var_os(name)) {
                let projects = usage.get(&default).map(Vec::as_slice).unwrap_or(&[]);
                output::line(out, &default_line(projects))?;
            }
            for account in listed {
                let projects = usage.get(&account.name).map(Vec::as_slice).unwrap_or(&[]);
                output::line(
                    out,
                    &list_line(
                        &account.name,
                        &output::local_time(account.added_at),
                        projects,
                    ),
                )?;
            }
        }
        AccountCommand::Remove(args) => {
            let name = AccountName::parse(&args.name)?;
            name.check_not_reserved()?;
            remove(&host_paths(dirs)?, &name)?;
            output::line(out, &format!("removed account {name} and its token"))?;
        }
        AccountCommand::Use(args) => {
            let name = AccountName::parse(&args.name)?;
            let body = json!({ "account": name.as_str() });
            let response = request_project_with(
                args.socket.as_deref(),
                args.project.as_deref(),
                Method::PUT,
                "account",
                Some(&body),
            )?;
            refusal(&response)?;
            output::line(out, &use_line(&name))?;
        }
    }
    Ok(())
}

/// The variables of a Claude Code session that a check's turn never takes, besides the ones that rank above the token ([`OVERRIDING_ENV`]): the check is no session of the caller's, and a token of an account fold gave the caller is not the new one.
const CHECK_DROPPED_ENV: [&str; 8] = [
    ACCOUNT_ENV,
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_CONFIG_DIR",
];

/// Refuses `token` for the account `name` when the check's turn shows that the API refuses it; otherwise gives what the check found, which the caller stores the token after. `update` names the command in the refusal.
fn checked_token(
    name: &AccountName,
    token: &AccountToken,
    update: bool,
    terminal: bool,
) -> Result<TokenCheck, AccountError> {
    if terminal {
        let _ = output::line(
            &mut io::stderr().lock(),
            "fold checks the token with one small Claude Code turn…",
        );
    }
    match check_with_claude(Path::new("claude"), token) {
        TokenCheck::Rejected => Err(token_rejected(name, update).into()),
        checked => Ok(checked),
    }
}

/// The lines after a change whose token fold could not check: why, and what the person can do.
fn warn_unchecked(
    out: &mut impl Write,
    name: &AccountName,
    checked: TokenCheck,
) -> Result<(), AccountError> {
    if let TokenCheck::Unchecked(why) = checked {
        output::line(out, &unchecked_line(why))?;
        output::line(out, &format!("hint: {}", why.hint(name)))?;
    }
    Ok(())
}

fn unchecked_line(why: Unchecked) -> String {
    format!(
        "warning: fold could not check the token now: {}. fold stored it; a worker whose login the API refuses waits for a working login, and Needs you names the account",
        why.words()
    )
}

/// Runs one small turn of `claude` with `token` as the only login and reads its result (see [`token_check`]): the smallest model, no tools, no MCP server, no saved session, a temporary folder as the working folder and Claude Code's folder, and [`TOKEN_CHECK_TIMEOUT`]. The token goes only into the process's environment, never into an argument, a file or an output. A `claude` that cannot run, or that runs too long, leaves the token unchecked.
fn check_with_claude(claude: &Path, token: &AccountToken) -> TokenCheck {
    let Ok(dir) = tempfile::Builder::new()
        .prefix("fold-token-check-")
        .tempdir()
    else {
        return TokenCheck::Unchecked(Unchecked::NoAnswer);
    };
    let output = dir.path().join("result.json");
    let mcp = dir.path().join("mcp.json");
    if std::fs::write(&mcp, b"{\"mcpServers\":{}}\n").is_err() {
        return TokenCheck::Unchecked(Unchecked::NoAnswer);
    }
    let config = dir.path().join("claude");
    if std::fs::create_dir(&config).is_err() {
        return TokenCheck::Unchecked(Unchecked::NoAnswer);
    }
    let mut command = Command::new(claude);
    command
        .args([
            "-p",
            "--output-format",
            "json",
            "--model",
            "haiku",
            "--tools",
            "",
            "--strict-mcp-config",
            "--mcp-config",
        ])
        .arg(&mcp)
        .args(["--no-session-persistence", "--max-turns", "1"])
        .arg(TOKEN_CHECK_PROMPT)
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    for name in OVERRIDING_ENV.iter().chain(CHECK_DROPPED_ENV.iter()) {
        command.env_remove(name);
    }
    command
        .env("CLAUDE_CONFIG_DIR", &config)
        .env(OAUTH_TOKEN_ENV, token.expose());
    match run_bounded(&mut command, &output, TOKEN_CHECK_TIMEOUT) {
        Ok(run) => token_check(run.as_ref()),
        Err(why) => TokenCheck::Unchecked(why),
    }
}

/// The most bytes fold reads of the check's output: its result is one short JSON object.
const MAX_CHECK_OUTPUT: u64 = 1024 * 1024;

/// The result of `command` as `--output-format json` writes it into the file `output`, `None` when it wrote no result; an error when it cannot start or runs past `timeout`, which kills its process group. The output goes into a file, not a pipe, so a process that the turn left running cannot hold the read open. The exit status is not read: Claude Code exits with 1 after a result that is an error, and the result says why.
fn run_bounded(
    command: &mut Command,
    output: &Path,
    timeout: Duration,
) -> Result<Option<TokenCheckRun>, Unchecked> {
    use std::os::unix::process::CommandExt as _;
    let file = std::fs::File::create(output).map_err(|_| Unchecked::NoAnswer)?;
    let mut child = command
        .stdout(file)
        .process_group(0)
        .spawn()
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => Unchecked::NotFound,
            _ => Unchecked::NoAnswer,
        })?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                kill_group(&mut child);
                return Err(Unchecked::Timeout);
            }
            Err(_) => {
                kill_group(&mut child);
                return Err(Unchecked::NoAnswer);
            }
        }
    }
    let mut bytes = Vec::new();
    std::fs::File::open(output)
        .and_then(|file| file.take(MAX_CHECK_OUTPUT).read_to_end(&mut bytes))
        .map_err(|_| Unchecked::NoAnswer)?;
    Ok(check_result(&String::from_utf8_lossy(&bytes)))
}

/// Kills the check's process and every process of its group, and reaps it. The group is the child's own (`process_group(0)`) and the child is not reaped yet, so its id still names that group.
fn kill_group(child: &mut std::process::Child) {
    if let Some(group) = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// The `result` object in Claude Code's output, the last one when it wrote several lines.
fn check_result(output: &str) -> Option<TokenCheckRun> {
    output.lines().rev().find_map(|line| {
        let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        if value["type"] != "result" {
            return None;
        }
        let usage = &value["usage"];
        let tokens = [
            "input_tokens",
            "output_tokens",
            "cache_creation_input_tokens",
            "cache_read_input_tokens",
        ]
        .iter()
        .map(|field| usage[field].as_u64().unwrap_or(0))
        .fold(0_u64, u64::saturating_add);
        Some(TokenCheckRun {
            is_error: value["is_error"].as_bool().unwrap_or(true),
            text: value["result"].as_str().map(str::to_owned),
            reached_model: tokens > 0,
            refused_login: fold_core::turn_waits::is_refused_login(
                value["api_error_status"].as_u64(),
                value["api_error_type"].as_str(),
            ),
        })
    })
}

fn remove(host: &HostPaths, name: &AccountName) -> Result<(), AccountError> {
    let runtime = current_thread_runtime().map_err(AccountError::Runtime)?;
    runtime.block_on(accounts::remove_on_host(host, name))?;
    Ok(())
}

fn added_line(name: &AccountName) -> String {
    format!(
        "added account {name}; run `fld account use {name}` in a project folder to give its workers this subscription"
    )
}

fn updated_line(name: &AccountName) -> String {
    format!(
        "account {name} has the new token: each running worker of a project that uses it restarts in its own session with it before its next turn, a working one when its current turn ends, and every worker that starts or resumes from now on starts with it"
    )
}

fn used_by(projects: &[String]) -> String {
    if projects.is_empty() {
        "no project".to_owned()
    } else {
        projects.join(", ")
    }
}

fn default_line(projects: &[String]) -> String {
    format!(
        "{} Claude Code's own login used by {}",
        fold_core::accounts::DEFAULT_ACCOUNT,
        used_by(projects)
    )
}

fn list_line(name: &AccountName, added: &str, projects: &[String]) -> String {
    format!("{name} added {added} used by {}", used_by(projects))
}

/// The token: one hidden line on a terminal, else everything the pipe gives.
fn read_token(input: &mut impl Read, terminal: bool) -> Result<String, AccountError> {
    if terminal {
        read_hidden_line()
    } else {
        read_piped(input)
    }
}

fn use_line(name: &AccountName) -> String {
    format!(
        "the project uses account {name}: each worker continues in its own session with it when its turn ends, an idle one at once; a running `fld claude` session keeps its account until you start it again"
    )
}

/// Everything a pipe or a file gives, up to [`MAX_INPUT`] bytes.
fn read_piped(input: &mut impl Read) -> Result<String, AccountError> {
    let mut bytes = Vec::new();
    input
        .take(MAX_INPUT + 1)
        .read_to_end(&mut bytes)
        .map_err(AccountError::Read)?;
    if bytes.len() as u64 > MAX_INPUT {
        bytes.clear();
        bytes.extend_from_slice(b"\n\n");
    }
    String::from_utf8(bytes).map_err(|_| AccountError::Read(io::ErrorKind::InvalidData.into()))
}

/// One line from the terminal, which does not show what the person pastes, after a prompt on stderr. The terminal reads byte by byte without echo and without signals, so Ctrl-C ends the read here and the terminal's own settings come back before the line is checked; Ctrl-C and Ctrl-D on an empty line end it without a token.
fn read_hidden_line() -> Result<String, AccountError> {
    use rustix::termios::{LocalModes, OptionalActions, SpecialCodeIndex, tcgetattr, tcsetattr};
    let stdin = io::stdin();
    let mut stderr = io::stderr().lock();
    output::line(
        &mut stderr,
        "paste the token that `claude setup-token` printed, then press Enter:",
    )
    .map_err(AccountError::Write)?;
    let saved = tcgetattr(&stdin).ok();
    if let Some(saved) = &saved {
        let mut quiet = saved.clone();
        quiet
            .local_modes
            .remove(LocalModes::ECHO | LocalModes::ICANON | LocalModes::ISIG);
        quiet.special_codes[SpecialCodeIndex::VMIN] = 1;
        quiet.special_codes[SpecialCodeIndex::VTIME] = 0;
        let _ = tcsetattr(&stdin, OptionalActions::Now, &quiet);
    }
    let read = hidden_line(&mut stdin.lock());
    if let Some(saved) = &saved {
        let _ = tcsetattr(&stdin, OptionalActions::Now, saved);
    }
    let _ = output::line(&mut stderr, "");
    read
}

const CTRL_C: u8 = 0x03;
const CTRL_D: u8 = 0x04;
const BACKSPACE: u8 = 0x08;
const DELETE: u8 = 0x7f;

/// The line the person types or pastes into a terminal without line editing: Enter ends it, Backspace and Delete take back a character, Ctrl-C cancels, and Ctrl-D or the end of input end it.
fn hidden_line(input: &mut impl Read) -> Result<String, AccountError> {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        if input.read(&mut byte).map_err(AccountError::Read)? == 0 {
            break;
        }
        match byte[0] {
            b'\n' | b'\r' | CTRL_D => break,
            CTRL_C => return Err(AccountError::Read(io::ErrorKind::Interrupted.into())),
            BACKSPACE | DELETE => {
                line.pop();
            }
            other => line.push(other),
        }
        if line.len() as u64 > MAX_INPUT {
            return Ok("\n\n".to_owned());
        }
    }
    String::from_utf8(line).map_err(|_| AccountError::Read(io::ErrorKind::InvalidData.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> AccountName {
        AccountName::parse(text).unwrap()
    }

    #[test]
    fn the_lines_name_accounts_and_projects_and_never_a_token() {
        assert_eq!(
            list_line(&name("work-a"), "2026-11-14 10:00", &[]),
            "work-a added 2026-11-14 10:00 used by no project"
        );
        assert_eq!(
            list_line(
                &name("work-a"),
                "2026-11-14 10:00",
                &["demo".to_owned(), "shop".to_owned()]
            ),
            "work-a added 2026-11-14 10:00 used by demo, shop"
        );
        assert!(added_line(&name("work-a")).contains("fld account use work-a"));
        assert!(updated_line(&name("work-a")).contains("before its next turn"));
        assert_eq!(
            default_line(&["app".to_owned()]),
            "default Claude Code's own login used by app"
        );
        assert!(use_line(&name("work-b")).contains("fld claude"));
    }

    #[test]
    fn the_check_reads_claude_codes_result_and_its_usage() {
        let refused = r#"{"type":"result","subtype":"success","is_error":true,"result":"Invalid API key · Please run /login","usage":{"input_tokens":0,"output_tokens":0}}"#;
        assert_eq!(
            check_result(&format!("noise\n{refused}\n")),
            Some(TokenCheckRun {
                is_error: true,
                text: Some("Invalid API key · Please run /login".to_owned()),
                reached_model: false,
                refused_login: false,
            })
        );
        let recorded = check_result(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../fold-daemon/tests/fixtures/claude/login_refused_result.json"
        )))
        .unwrap();
        assert!(recorded.refused_login && recorded.is_error && !recorded.reached_model);
        assert_eq!(token_check(Some(&recorded)), TokenCheck::Rejected);
        let unknown = check_result(
            r#"{"type":"result","is_error":true,"result":"Some new wording","api_error_status":401,"usage":{"input_tokens":0}}"#,
        );
        assert_eq!(token_check(unknown.as_ref()), TokenCheck::Rejected);
        let answered = r#"{"type":"result","is_error":false,"result":"ok","usage":{"input_tokens":3,"output_tokens":1}}"#;
        assert_eq!(
            token_check(check_result(answered).as_ref()),
            TokenCheck::Works
        );
        assert_eq!(check_result("{\"type\":\"system\"}\nnot json"), None);
        assert!(unchecked_line(Unchecked::Network).starts_with(
            "warning: fold could not check the token now: Claude Code could not reach the model."
        ));
        let mut out = Vec::new();
        warn_unchecked(
            &mut out,
            &AccountName::parse("work-a").unwrap(),
            TokenCheck::Unchecked(Unchecked::Timeout),
        )
        .unwrap();
        let shown = String::from_utf8(out).unwrap();
        assert!(
            shown.contains("the turn did not end within 60 seconds")
                && shown.contains("\nhint: `fld account update work-a`"),
            "{shown}"
        );
    }

    #[test]
    fn a_claude_that_cannot_run_or_runs_too_long_leaves_the_token_unchecked() {
        let token = check_token("sk-ant-oat01-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        assert_eq!(
            check_with_claude(Path::new("/nonexistent/claude"), &token),
            TokenCheck::Unchecked(Unchecked::NotFound)
        );
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("result.json");
        let started = Instant::now();
        let mut slow = Command::new("sh");
        slow.args(["-c", "sleep 30 & sleep 30"]);
        assert_eq!(
            run_bounded(&mut slow, &output, Duration::from_millis(200)),
            Err(Unchecked::Timeout)
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the turn is killed at the timeout"
        );

        let started = Instant::now();
        let mut left = Command::new("sh");
        left.stderr(Stdio::null()).args([
            "-c",
            r#"sleep 15 </dev/null 2>/dev/null & echo '{"type":"result","is_error":false,"result":"ok","usage":{"input_tokens":3}}'"#,
        ]);
        let run = run_bounded(&mut left, &output, Duration::from_secs(20))
            .unwrap()
            .unwrap();
        assert!(run.reached_model && !run.is_error, "{run:?}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "a process the turn left running does not hold the read"
        );
    }

    #[test]
    fn a_hidden_line_ends_at_enter_takes_back_a_character_and_cancels_at_ctrl_c() {
        let mut typed: &[u8] = b"sk-ant-oat01-aaaaaaaaaaaaaaaaaaaaaaaX\x7f\rrest";
        assert_eq!(
            hidden_line(&mut typed).unwrap(),
            "sk-ant-oat01-aaaaaaaaaaaaaaaaaaaaaaa"
        );
        let mut cancelled: &[u8] = b"sk-ant\x03";
        assert!(matches!(
            hidden_line(&mut cancelled),
            Err(AccountError::Read(error)) if error.kind() == io::ErrorKind::Interrupted
        ));
        let mut ended: &[u8] = b"abc\x04def";
        assert_eq!(hidden_line(&mut ended).unwrap(), "abc");
        let mut empty: &[u8] = b"";
        assert_eq!(hidden_line(&mut empty).unwrap(), "");
        let huge = vec![b'a'; 70_000];
        assert_eq!(
            check_token(&hidden_line(&mut huge.as_slice()).unwrap())
                .unwrap_err()
                .code(),
            fold_core::codes::INVALID_ACCOUNT_TOKEN
        );
    }

    #[test]
    fn a_pipe_gives_its_text_and_a_huge_one_gives_no_token() {
        let mut small: &[u8] = b"sk-ant-oat01-aaaaaaaaaaaaaaaaaaaaaaaa\n";
        assert_eq!(
            read_piped(&mut small).unwrap(),
            "sk-ant-oat01-aaaaaaaaaaaaaaaaaaaaaaaa\n"
        );
        let big = vec![b'a'; 70_000];
        let text = read_piped(&mut big.as_slice()).unwrap();
        assert_eq!(
            check_token(&text).unwrap_err().code(),
            fold_core::codes::INVALID_ACCOUNT_TOKEN
        );
        let mut broken: &[u8] = &[0xff, 0xfe];
        assert!(matches!(
            read_piped(&mut broken),
            Err(AccountError::Read(_))
        ));
    }
}
