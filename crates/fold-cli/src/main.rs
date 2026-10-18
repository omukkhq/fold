mod claude;
mod claude_setup;
mod commands;
mod doctor;
mod fold_name;
mod git_identity;
mod hook;
mod init;
mod mcp;
mod news;
mod output;
mod platform;
#[cfg(feature = "dev-seed")]
mod seed;
mod serve;
mod service;
mod token;

use std::ffi::OsString;
use std::io::{self, Write};

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "fld", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Create a project in a directory and register it
    Init(init::InitArgs),
    /// Run the daemon in the foreground
    Serve(serve::ServeArgs),
    /// Connect a Claude Code session to the daemon over stdio, starting the daemon when needed
    Mcp(mcp::McpArgs),
    /// Check the host, the daemon and each project, and name the fix for every problem
    Doctor(doctor::DoctorArgs),
    /// Install the service that starts the daemon at login, then start it: a systemd user unit on Linux, a launchd agent on macOS. Then offer to add fold to Claude Code for your user
    Setup(service::SetupArgs),
    /// Delete unreferenced objects, prune checkpoints and remove closed workspace directories
    Gc(commands::gc::GcArgs),
    /// Start the daemon when needed, then continue the project's main agent: the Claude Code session that fld started last, or a new one
    Claude(claude::ClaudeArgs),
    /// Start the daemon when needed, then a new Claude Code session as the project's main agent; fld new claude is the same
    New(claude::NewArgs),
    /// Subscribe an email address to fold release news; the address goes once to Omukk and fld does not keep it
    Subscribe(news::SubscribeArgs),
    /// Manage the tokens that sign humans in to the web interface off localhost
    Token(token::TokenArgs),
    /// Add, remove and list the remote Git repositories that fld push and fld pull sync main with
    Remote(commands::remote::RemoteArgs),
    /// Push main's Git history, one commit per revision, to a Git remote, without force; the remote's name can be left out when the project has only one
    Push(commands::remote::PushArgs),
    /// Run a Claude Code hook; the settings that fld setup and fold's sessions get call these
    #[command(hide = true)]
    Hook(hook::HookArgs),
    /// Bring the commits of a Git remote into main; a conflict becomes a sync task. The remote's name can be left out when the project has only one
    Pull(commands::remote::PullArgs),
    /// Pause every agent of the project: fold stops each worker, keeps its session and starts none until fld resume
    Pause(commands::pause::PauseArgs),
    /// Resume the agents of a paused project: each stopped worker runs again in its own session
    Resume(commands::pause::PauseArgs),
    /// Record your changes in the project folder on main as one change of yours; a conflict with the agents' work waits for the main agent
    Record(commands::record::RecordArgs),
    /// Development helpers
    #[cfg(feature = "dev-seed")]
    #[command(hide = true)]
    Dev(seed::DevArgs),
}

/// clap prints a parse outcome, including `--help` and `--version`, and exits with its own code; a failed write is a runtime error with exit code 1.
#[derive(Debug)]
enum Failure {
    Parse(clap::Error),
    Write(io::Error),
    Serve(serve::ServeError),
    Init(init::InitError),
    Mcp(mcp::McpError),
    Doctor(doctor::DoctorError),
    Service(service::ServiceError),
    Gc(commands::gc::GcError),
    Claude(claude::ClaudeError),
    Token(token::TokenError),
    News(news::NewsError),
    Remote(commands::remote::RemoteError),
    #[cfg(feature = "dev-seed")]
    Seed(seed::SeedError),
}

fn run(args: impl IntoIterator<Item = OsString>, out: &mut impl Write) -> Result<(), Failure> {
    match Cli::try_parse_from(args).map_err(Failure::Parse)?.command {
        Some(Command::Init(args)) => {
            init::run(&args, &serve::Dirs::from_env(), out).map_err(Failure::Init)
        }
        Some(Command::Serve(args)) => serve::run(&args).map_err(Failure::Serve),
        Some(Command::Doctor(args)) => doctor::run(&args, out).map_err(Failure::Doctor),
        Some(Command::Setup(args)) => service::run(&args, out).map_err(Failure::Service),
        Some(Command::Mcp(args)) => mcp::run(&args).map_err(Failure::Mcp),
        Some(Command::Gc(args)) => commands::gc::run(
            &args,
            &serve::Dirs::from_env(),
            std::env::current_dir(),
            fold_name::current(),
            out,
        )
        .map_err(Failure::Gc),
        Some(Command::Claude(args)) => {
            claude::run(&args, claude::Session::Continue, &serve::Dirs::from_env())
                .map_err(Failure::Claude)
        }
        Some(Command::New(args)) => {
            claude::run_new(&args, &serve::Dirs::from_env()).map_err(Failure::Claude)
        }
        Some(Command::Subscribe(args)) => {
            news::run(&args, &serve::Dirs::from_env(), out).map_err(Failure::News)
        }
        Some(Command::Token(args)) => {
            token::run(&args, &serve::Dirs::from_env(), out).map_err(Failure::Token)
        }
        Some(Command::Remote(args)) => {
            commands::remote::run_remote(&args, out).map_err(Failure::Remote)
        }
        Some(Command::Push(args)) => {
            commands::remote::run_push(&args, out).map_err(Failure::Remote)
        }
        Some(Command::Hook(args)) => {
            hook::run(&args, out);
            Ok(())
        }
        Some(Command::Pull(args)) => {
            commands::remote::run_pull(&args, out).map_err(Failure::Remote)
        }
        Some(Command::Pause(args)) => {
            commands::pause::run_pause(&args, out).map_err(Failure::Remote)
        }
        Some(Command::Resume(args)) => {
            commands::pause::run_resume(&args, out).map_err(Failure::Remote)
        }
        Some(Command::Record(args)) => {
            commands::record::run_record(&args, out).map_err(Failure::Remote)
        }
        #[cfg(feature = "dev-seed")]
        Some(Command::Dev(args)) => {
            seed::run(&args, &serve::Dirs::from_env(), out).map_err(Failure::Seed)
        }
        None => output::line(out, &output::version_line()).map_err(Failure::Write),
    }
}

/// What the command line shows of a failure besides its message: the stable code and the hint that names the next step.
trait Coded: std::error::Error {
    fn code_and_hint(&self) -> (Option<String>, Option<String>);
}

/// Implements [`Coded`] for an error type from its `code` and, when it has one, `hint`.
macro_rules! coded {
    ($error:ty) => {
        impl Coded for $error {
            fn code_and_hint(&self) -> (Option<String>, Option<String>) {
                (self.code().map(str::to_owned), None)
            }
        }
    };
    ($error:ty, hint) => {
        impl Coded for $error {
            fn code_and_hint(&self) -> (Option<String>, Option<String>) {
                (
                    self.code().map(str::to_owned),
                    self.hint().map(str::to_owned),
                )
            }
        }
    };
}

coded!(serve::ServeError);
coded!(init::InitError, hint);
coded!(claude::ClaudeError, hint);
coded!(news::NewsError, hint);
coded!(token::TokenError, hint);
coded!(doctor::DoctorError, hint);
coded!(service::ServiceError, hint);
coded!(commands::gc::GcError, hint);
coded!(commands::remote::RemoteError, hint);
#[cfg(feature = "dev-seed")]
coded!(seed::SeedError, hint);

/// The error and every cause in its chain that its own message does not already hold, joined with `: `. Most of the CLI's errors name their cause in their message, which the chain would print a second time.
fn message_with_causes(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut cause = error.source();
    while let Some(next) = cause {
        let text = next.to_string();
        if !message.contains(&text) {
            message = format!("{message}: {text}");
        }
        cause = next.source();
    }
    message
}

/// `code: message`, then `hint: ...` on its own line, for the parts the error has.
fn failure_message(error: &(impl Coded + 'static)) -> anyhow::Error {
    let message = message_with_causes(error);
    let (code, hint) = error.code_and_hint();
    let mut text = match code {
        Some(code) => format!("{code}: {message}"),
        None => message,
    };
    if let Some(hint) = hint {
        text.push_str("\nhint: ");
        text.push_str(&hint);
    }
    anyhow::anyhow!(text)
}

fn main() -> anyhow::Result<()> {
    // Not locked here: `fld mcp` writes stdout from a tokio blocking thread, which would wait forever for a lock that this thread holds.
    match run(std::env::args_os(), &mut io::stdout()) {
        Ok(()) => Ok(()),
        Err(Failure::Parse(error)) => error.exit(),
        Err(Failure::Write(error)) => Err(error.into()),
        Err(Failure::Serve(error)) => Err(failure_message(&error)),
        Err(Failure::Init(error)) => Err(failure_message(&error)),
        Err(Failure::Claude(error)) => Err(failure_message(&error)),
        Err(Failure::Token(error)) => Err(failure_message(&error)),
        Err(Failure::News(error)) => Err(failure_message(&error)),
        Err(Failure::Mcp(error)) => {
            let _ = output::line(&mut io::stderr().lock(), &format!("fld mcp: {error}"));
            std::process::exit(1)
        }
        Err(Failure::Doctor(error)) => Err(failure_message(&error)),
        Err(Failure::Service(error)) => Err(failure_message(&error)),
        Err(Failure::Gc(error)) => Err(failure_message(&error)),
        Err(Failure::Remote(error)) => Err(failure_message(&error)),
        #[cfg(feature = "dev-seed")]
        Err(Failure::Seed(error)) => Err(failure_message(&error)),
    }
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;

    use super::*;

    fn run_with(args: &[&str]) -> (Result<(), Failure>, String) {
        let mut out = Vec::new();
        let result = run(args.iter().map(OsString::from), &mut out);
        (result, String::from_utf8(out).unwrap())
    }

    fn parse_kind(result: Result<(), Failure>) -> ErrorKind {
        match result {
            Err(Failure::Parse(error)) => error.kind(),
            other => panic!("expected a parse failure, got {other:?}"),
        }
    }

    #[test]
    fn no_subcommand_prints_the_version_line() {
        let (result, out) = run_with(&["fld"]);
        assert!(result.is_ok());
        assert_eq!(out, "fld 0.1.0\n");
    }

    #[test]
    fn version_flag_is_reported_as_display_version() {
        let (result, out) = run_with(&["fld", "--version"]);
        let Err(Failure::Parse(error)) = result else {
            panic!("expected a parse failure, got {result:?}");
        };
        assert_eq!(error.kind(), ErrorKind::DisplayVersion);
        assert_eq!(error.to_string().trim(), "fld 0.1.0");
        assert!(out.is_empty());
    }

    #[test]
    fn unknown_argument_is_rejected_without_output() {
        let (result, out) = run_with(&["fld", "--bogus"]);
        assert_eq!(parse_kind(result), ErrorKind::UnknownArgument);
        assert!(out.is_empty());
    }

    #[test]
    fn a_failure_shows_its_code_hint_and_each_cause_once() {
        let cause = io::Error::other("disk full");
        let error = service::ServiceError::WriteUnit {
            path: "/unit".into(),
            source: cause,
        };
        let shown = failure_message(&error).to_string();
        assert_eq!(
            shown,
            "service_install_failed: cannot write /unit: disk full"
        );
        let hinted = claude::ClaudeError::Refused {
            code: "main_tag_denied".to_owned(),
            message: "taken".to_owned(),
            hint: Some("run fld claude --name lead".to_owned()),
        };
        assert_eq!(
            failure_message(&hinted).to_string(),
            "main_tag_denied: taken\nhint: run fld claude --name lead"
        );
        let plain = doctor::DoctorError::Runtime(io::Error::other("x"));
        assert_eq!(
            failure_message(&plain).to_string(),
            "cannot start the async runtime: x"
        );
        let refused = commands::gc::GcError::Refused {
            code: "gc_in_progress".to_owned(),
            message: "a collection runs".to_owned(),
            hint: Some("try again later".to_owned()),
        };
        assert_eq!(
            failure_message(&refused).to_string(),
            "gc_in_progress: a collection runs\nhint: try again later"
        );
        let token = token::TokenError::Fold(
            fold_core::FoldError::new(fold_core::codes::TOKEN_NOT_FOUND, "no wt-9")
                .with_hint("run fld token list"),
        );
        assert_eq!(
            failure_message(&token).to_string(),
            "token_not_found: no wt-9\nhint: run fld token list"
        );
        let paths = doctor::DoctorError::HostPaths(
            fold_core::FoldError::new(fold_core::codes::INVALID_HOST_PATHS, "bad")
                .with_hint("set HOME"),
        );
        assert_eq!(
            failure_message(&paths).to_string(),
            "invalid_host_paths: bad\nhint: set HOME"
        );
    }

    #[test]
    fn a_cause_the_message_leaves_out_is_appended() {
        #[derive(Debug, thiserror::Error)]
        #[error("cannot read the file")]
        struct Outer(#[source] io::Error);
        let shown = message_with_causes(&Outer(io::Error::other("permission denied")));
        assert_eq!(shown, "cannot read the file: permission denied");
    }

    #[test]
    fn a_failed_write_is_not_a_parse_failure() {
        let mut full: &mut [u8] = &mut [];
        let result = run([OsString::from("fld")], &mut full);
        let Err(Failure::Write(error)) = result else {
            panic!("expected a write failure, got {result:?}");
        };
        assert_eq!(error.kind(), io::ErrorKind::WriteZero);
    }
}
