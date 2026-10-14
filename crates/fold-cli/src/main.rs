mod commands;
mod init;
mod mcp;
mod output;
mod serve;

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
}

/// clap prints a parse outcome, including `--help` and `--version`, and exits with its own code; a failed write is a runtime error with exit code 1.
#[derive(Debug)]
enum Failure {
    Parse(clap::Error),
    Write(io::Error),
    Serve(serve::ServeError),
    Init(init::InitError),
    Mcp(mcp::McpError),
}

fn run(args: impl IntoIterator<Item = OsString>, out: &mut impl Write) -> Result<(), Failure> {
    match Cli::try_parse_from(args).map_err(Failure::Parse)?.command {
        Some(Command::Init(args)) => {
            init::run(&args, &serve::Dirs::from_env(), out).map_err(Failure::Init)
        }
        Some(Command::Serve(args)) => serve::run(&args).map_err(Failure::Serve),
        Some(Command::Openapi) => commands::openapi::run(out).map_err(Failure::Write),
        Some(Command::Mcp(args)) => mcp::run(&args).map_err(Failure::Mcp),
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
        Err(Failure::Mcp(error)) => {
            let _ = output::line(&mut io::stderr().lock(), &format!("fold mcp: {error}"));
            std::process::exit(1)
        }
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
