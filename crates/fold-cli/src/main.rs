mod agent;
mod commands;
mod doctor;
mod hook;
mod init;
mod mcp;
mod output;
mod platform;
mod serve;
mod service;
mod token;

use std::ffi::OsString;
use std::io::{self, Write};

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "fold", version)]
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
    /// Print the OpenAPI document of the HTTP API
    Openapi,
    /// Connect a Claude Code session to the daemon over stdio, starting the daemon when needed
    Mcp(mcp::McpArgs),
    /// Append the main revisions not yet exported to a Git repository as commits
    ExportGit(commands::export_git::ExportGitArgs),
    /// Check the host, the daemon and each project, and name the fix for every problem
    Doctor(doctor::DoctorArgs),
    /// Manage the service that starts the daemon at login
    Service(service::ServiceArgs),
    /// Delete unreferenced objects, prune checkpoints and remove closed workspace directories
    Gc(commands::gc::GcArgs),
    /// Manage the agents of the project in the working directory
    Agent(agent::AgentArgs),
    /// Manage the tokens that sign humans in to the web interface off localhost
    Token(token::TokenArgs),
    /// Link the project to a remote Git repository
    Remote(commands::remote::RemoteArgs),
    /// Push the main revisions not yet on the Git remote as commits, without force
    Push(commands::remote::PushArgs),
    /// Run a Claude Code hook; the generated .claude/settings.json calls these
    Hook(hook::HookArgs),
    /// Bring the commits of the Git remote into main; a conflict becomes a sync task
    Pull(commands::remote::PullArgs),
}

/// clap prints a parse outcome, including `--help` and `--version`, and exits with its own code; a failed write is a runtime error with exit code 1.
#[derive(Debug)]
enum Failure {
    Parse(clap::Error),
    Write(io::Error),
    Serve(serve::ServeError),
    Init(init::InitError),
    Mcp(mcp::McpError),
    ExportGit(commands::export_git::ExportGitError),
    Doctor(doctor::DoctorError),
    Service(service::ServiceError),
    Gc(commands::gc::GcError),
    Agent(agent::AgentError),
    Token(token::TokenError),
    Remote(commands::remote::RemoteError),
}

fn run(args: impl IntoIterator<Item = OsString>, out: &mut impl Write) -> Result<(), Failure> {
    match Cli::try_parse_from(args).map_err(Failure::Parse)?.command {
        Some(Command::Init(args)) => {
            init::run(&args, &serve::Dirs::from_env(), out).map_err(Failure::Init)
        }
        Some(Command::Serve(args)) => serve::run(&args).map_err(Failure::Serve),
        Some(Command::Doctor(args)) => doctor::run(&args, out).map_err(Failure::Doctor),
        Some(Command::Service(args)) => service::run(&args, out).map_err(Failure::Service),
        Some(Command::Openapi) => commands::openapi::run(out).map_err(Failure::Write),
        Some(Command::Mcp(args)) => mcp::run(&args).map_err(Failure::Mcp),
        Some(Command::ExportGit(args)) => {
            commands::export_git::run(&args, out).map_err(Failure::ExportGit)
        }
        Some(Command::Gc(args)) => commands::gc::run(
            &args,
            &serve::Dirs::from_env(),
            std::env::current_dir(),
            fold_cli::daemon_client::human_from_env().ok(),
            out,
        )
        .map_err(Failure::Gc),
        Some(Command::Agent(args)) => {
            agent::run(&args, &serve::Dirs::from_env(), out).map_err(Failure::Agent)
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
        None => output::line(out, &output::version_line()).map_err(Failure::Write),
    }
}

fn main() -> anyhow::Result<()> {
    // Not locked here: `fold mcp` writes stdout from a tokio blocking thread, which would wait forever for a lock that this thread holds.
    match run(std::env::args_os(), &mut io::stdout()) {
        Ok(()) => Ok(()),
        Err(Failure::Parse(error)) => error.exit(),
        Err(Failure::Write(error)) => Err(error.into()),
        Err(Failure::Serve(error)) => Err(match error.code() {
            Some(code) => anyhow::anyhow!("{code}: {error}"),
            None => error.into(),
        }),
        Err(Failure::Init(error)) => Err(match error.code() {
            Some(code) => anyhow::anyhow!("{code}: {error}"),
            None => error.into(),
        }),
        Err(Failure::Agent(error)) => Err(match error.code() {
            Some(code) => anyhow::anyhow!("{code}: {error}"),
            None => error.into(),
        }),
        Err(Failure::Token(error)) => Err(match error.code() {
            Some(code) => anyhow::anyhow!("{code}: {error}"),
            None => error.into(),
        }),
        Err(Failure::Mcp(error)) => {
            let _ = output::line(&mut io::stderr().lock(), &format!("fold mcp: {error}"));
            std::process::exit(1)
        }
        Err(Failure::ExportGit(error)) => Err(match error.code() {
            Some(code) => anyhow::anyhow!("{code}: {error}"),
            None => error.into(),
        }),
        Err(Failure::Doctor(error)) => Err(match error.code() {
            Some(code) => anyhow::anyhow!("{code}: {error}"),
            None => error.into(),
        }),
        Err(Failure::Service(error)) => Err(match (error.code(), error.hint()) {
            (Some(code), Some(hint)) => anyhow::anyhow!("{code}: {error}\nhint: {hint}"),
            (Some(code), None) => anyhow::anyhow!("{code}: {error}"),
            _ => error.into(),
        }),
        Err(Failure::Gc(error)) => Err(match error.code() {
            Some(code) => anyhow::anyhow!("{code}: {error}"),
            None => error.into(),
        }),
        Err(Failure::Remote(error)) => Err(match error.code() {
            Some(code) => anyhow::anyhow!("{code}: {error}"),
            None => anyhow::anyhow!("{error}"),
        }),
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
        let (result, out) = run_with(&["fold"]);
        assert!(result.is_ok());
        assert_eq!(out, "fold 0.1.0\n");
    }

    #[test]
    fn version_flag_is_reported_as_display_version() {
        let (result, out) = run_with(&["fold", "--version"]);
        let Err(Failure::Parse(error)) = result else {
            panic!("expected a parse failure, got {result:?}");
        };
        assert_eq!(error.kind(), ErrorKind::DisplayVersion);
        assert_eq!(error.to_string().trim(), "fold 0.1.0");
        assert!(out.is_empty());
    }

    #[test]
    fn unknown_argument_is_rejected_without_output() {
        let (result, out) = run_with(&["fold", "--bogus"]);
        assert_eq!(parse_kind(result), ErrorKind::UnknownArgument);
        assert!(out.is_empty());
    }

    #[test]
    fn openapi_subcommand_prints_valid_json() {
        let (result, out) = run_with(&["fold", "openapi"]);
        assert!(result.is_ok());
        assert!(out.ends_with("}\n"));
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["openapi"], "3.1.0");
        assert_eq!(doc["info"]["title"], "fold");
        assert_eq!(doc["info"]["version"], env!("CARGO_PKG_VERSION"));
        assert!(doc["paths"]["/api/v1/projects"]["get"].is_object());
    }

    #[test]
    fn openapi_reports_a_failed_write() {
        let mut full: &mut [u8] = &mut [];
        let result = run(["fold", "openapi"].map(OsString::from), &mut full);
        assert!(matches!(result, Err(Failure::Write(_))));
    }

    #[test]
    fn a_failed_write_is_not_a_parse_failure() {
        let mut full: &mut [u8] = &mut [];
        let result = run([OsString::from("fold")], &mut full);
        let Err(Failure::Write(error)) = result else {
            panic!("expected a write failure, got {result:?}");
        };
        assert_eq!(error.kind(), io::ErrorKind::WriteZero);
    }
}
