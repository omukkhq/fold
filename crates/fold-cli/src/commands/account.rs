use std::io::{self, Read, Write};
use std::path::PathBuf;

use clap::{Args, Subcommand};
use fold_core::accounts::{AccountName, check_token};
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
    /// Add a Claude subscription under a new name, with the token that `claude setup-token` printed, read from stdin
    Add(NameArgs),
    /// List the accounts, system-default (Claude Code's own login) first: name, when the token was added and which projects use each, never the token
    List,
    /// Remove an account and its token; refused while a project uses it
    Remove(NameArgs),
    /// Make the account the one of the project in this folder, system-default for Claude Code's own login: each worker continues in its own session with it when its turn ends
    Use(UseArgs),
    /// Give an account a new token from stdin, as for an expired or revoked one
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
            accounts::add(&dir, &name, &token, SystemClock.now())?;
            output::line(out, &added_line(&name))?;
        }
        AccountCommand::Update(args) => {
            let name = AccountName::parse(&args.name)?;
            name.check_not_reserved()?;
            let token = check_token(&read_token(input, terminal)?)?;
            let dir = host_paths(dirs)?.accounts_dir();
            accounts::update(&dir, &name, &token, SystemClock.now())?;
            output::line(out, &updated_line(&name))?;
        }
        AccountCommand::List => {
            let host = host_paths(dirs)?;
            let runtime = current_thread_runtime().map_err(AccountError::Runtime)?;
            let usage = runtime.block_on(accounts::usage_on_host(&host))?;
            let listed = accounts::list(&host.accounts_dir())?;
            let default = AccountName::system_default();
            let projects = usage.get(&default).map(Vec::as_slice).unwrap_or(&[]);
            output::line(out, &default_line(projects))?;
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
        "account {name} has the new token: each running worker of a project that uses it restarts in its own session with it when its next turn ends, and every worker that starts or resumes from now on starts with it"
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
        fold_core::accounts::SYSTEM_DEFAULT,
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
        assert!(updated_line(&name("work-a")).contains("when its next turn ends"));
        assert_eq!(
            default_line(&["app".to_owned()]),
            "system-default Claude Code's own login used by app"
        );
        assert!(use_line(&name("work-b")).contains("fld claude"));
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
