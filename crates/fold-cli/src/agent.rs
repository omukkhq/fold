use std::io::{self, Write};
use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};
use fold_cli::daemon_client::{self, ClientConfig, ClientError, DaemonClient, Response};
use fold_core::{AgentName, FoldError};
use fold_daemon::services::projects::{ProjectError, slug_for_dir_on_host};
use hyper::Method;
use thiserror::Error;

use crate::init::host_paths;
use crate::output;
use crate::serve::{self, Dirs};

#[derive(Args, Debug)]
pub struct AgentArgs {
    #[command(subcommand)]
    command: AgentCommand,
}

#[derive(Subcommand, Debug)]
enum AgentCommand {
    /// Grant a tag to an agent of the project in the working directory; only a human grants integrator
    Tag(TagArgs),
    /// Remove a tag from an agent of the project in the working directory
    Untag(TagArgs),
}

#[derive(Args, Debug)]
struct TagArgs {
    /// The agent name
    agent: String,
    /// The tag; agents set their other tags themselves when they register
    #[arg(value_enum)]
    tag: HumanTag,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
}

/// The tags a human sets from the CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum HumanTag {
    Integrator,
}

#[derive(Debug, Error)]
pub enum AgentError {
    #[error(transparent)]
    Fold(#[from] FoldError),
    #[error(transparent)]
    Config(#[from] serve::ServeError),
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error("cannot read the working directory: {0}")]
    Cwd(io::Error),
    #[error("cannot find the fld binary: {0}")]
    Exe(io::Error),
    #[error("cannot start the async runtime: {0}")]
    Runtime(io::Error),
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("{message}")]
    Refused { code: String, message: String },
    #[error("cannot write the output: {0}")]
    Write(io::Error),
}

impl AgentError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Fold(error) => Some(error.code()),
            Self::Config(error) => error.code(),
            Self::Project(error) => error.code(),
            Self::Refused { code, .. } => Some(code),
            Self::Cwd(_) | Self::Exe(_) | Self::Runtime(_) | Self::Client(_) | Self::Write(_) => {
                None
            }
        }
    }
}

/// Sends the tag change to the daemon over the socket as the human from `FOLD_HUMAN`, starting the daemon when it is not running. The project is the registered one whose root holds the working directory.
pub fn run(args: &AgentArgs, dirs: &Dirs, out: &mut impl Write) -> Result<(), AgentError> {
    let (tag_args, method) = match &args.command {
        AgentCommand::Tag(tag_args) => (tag_args, Method::PUT),
        AgentCommand::Untag(tag_args) => (tag_args, Method::DELETE),
    };
    let HumanTag::Integrator = tag_args.tag;
    let agent = AgentName::parse(&tag_args.agent).map_err(FoldError::from)?;
    let host = host_paths(dirs)?;
    let cwd = std::env::current_dir()
        .and_then(|dir| dir.canonicalize())
        .map_err(AgentError::Cwd)?;
    let config = client_config(tag_args.socket.as_deref(), dirs)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(AgentError::Runtime)?;
    let response = runtime.block_on(async {
        let slug = slug_for_dir_on_host(&host, &cwd).await?;
        let client = DaemonClient::new(config, daemon_client::human_from_env().ok());
        let path = format!("/api/v1/projects/{slug}/agents/{agent}/tags/integrator");
        Ok::<_, AgentError>(client.request(method.clone(), &path, None).await?)
    })?;
    let verb = if method == Method::PUT {
        "granted"
    } else {
        "removed"
    };
    output::line(out, &summary(&response, &agent, verb)?).map_err(AgentError::Write)
}

fn client_config(
    socket: Option<&std::path::Path>,
    dirs: &Dirs,
) -> Result<ClientConfig, AgentError> {
    let socket = serve::socket_path(socket, dirs)?.ok_or(ClientError::RuntimeDirMissing)?;
    Ok(ClientConfig {
        socket: std::path::absolute(&socket).map_err(AgentError::Cwd)?,
        runtime_dir: serve::runtime_dir(dirs).ok(),
        fold_exe: Some(std::env::current_exe().map_err(AgentError::Exe)?),
    })
}

/// `integrator granted to review; tags: integrator, review`, or the daemon's refusal.
fn summary(response: &Response, agent: &AgentName, verb: &str) -> Result<String, AgentError> {
    if !(200..300).contains(&response.status) {
        let error = &response.body["error"];
        return Err(AgentError::Refused {
            code: error["code"]
                .as_str()
                .unwrap_or("internal_error")
                .to_owned(),
            message: error["message"]
                .as_str()
                .unwrap_or("the daemon refused the request")
                .to_owned(),
        });
    }
    let tags: Vec<&str> = response.body["tags"]
        .as_array()
        .map(|tags| tags.iter().filter_map(|tag| tag.as_str()).collect())
        .unwrap_or_default();
    let preposition = if verb == "granted" { "to" } else { "from" };
    Ok(format!(
        "integrator {verb} {preposition} {agent}; tags: {}",
        if tags.is_empty() {
            "none".to_owned()
        } else {
            tags.join(", ")
        }
    ))
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use clap::error::ErrorKind;
    use serde_json::json;

    use super::*;

    #[derive(Parser, Debug)]
    struct Wrapper {
        #[command(flatten)]
        args: AgentArgs,
    }

    fn parse(args: &[&str]) -> Result<Wrapper, clap::Error> {
        Wrapper::try_parse_from(std::iter::once("agent").chain(args.iter().copied()))
    }

    #[test]
    fn tag_and_untag_take_an_agent_the_integrator_tag_and_a_socket() {
        let parsed = parse(&["tag", "review", "integrator", "--socket", "/s.sock"]).unwrap();
        let AgentCommand::Tag(args) = parsed.args.command else {
            panic!("expected tag");
        };
        assert_eq!(args.agent, "review");
        assert_eq!(args.tag, HumanTag::Integrator);
        assert_eq!(
            args.socket.as_deref(),
            Some(std::path::Path::new("/s.sock"))
        );
        assert!(matches!(
            parse(&["untag", "review", "integrator"])
                .unwrap()
                .args
                .command,
            AgentCommand::Untag(_)
        ));
    }

    #[test]
    fn any_other_tag_is_a_usage_error() {
        for tag in ["ui", "review", "Integrator"] {
            let error = parse(&["tag", "review", tag]).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::InvalidValue, "{tag}");
        }
        assert_eq!(
            parse(&["tag", "review"]).unwrap_err().kind(),
            ErrorKind::MissingRequiredArgument
        );
    }

    #[test]
    fn a_success_lists_the_tags_and_a_refusal_keeps_its_code() {
        let agent = AgentName::parse("review").unwrap();
        let ok = Response {
            status: 200,
            body: json!({"agent": "review", "tags": ["integrator", "review"]}),
        };
        assert_eq!(
            summary(&ok, &agent, "granted").unwrap(),
            "integrator granted to review; tags: integrator, review"
        );
        let none = Response {
            status: 200,
            body: json!({"agent": "review", "tags": []}),
        };
        assert_eq!(
            summary(&none, &agent, "removed").unwrap(),
            "integrator removed from review; tags: none"
        );
        let refused = Response {
            status: 404,
            body: json!({"error": {"code": "agent_not_found", "message": "agent review does not exist", "details": {}}}),
        };
        let error = summary(&refused, &agent, "granted").unwrap_err();
        assert_eq!(error.code(), Some("agent_not_found"));
        assert_eq!(error.to_string(), "agent review does not exist");
        let bare = Response {
            status: 500,
            body: serde_json::Value::Null,
        };
        let error = summary(&bare, &agent, "granted").unwrap_err();
        assert_eq!(error.code(), Some("internal_error"));
    }

    #[test]
    fn errors_without_a_code_say_so() {
        assert_eq!(AgentError::Runtime(io::Error::other("x")).code(), None);
        assert_eq!(
            AgentError::Fold(FoldError::new(fold_core::codes::INVALID_NAME, "bad")).code(),
            Some("invalid_name")
        );
    }
}
