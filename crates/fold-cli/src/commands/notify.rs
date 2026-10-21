use std::io::{BufRead, Write};
use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};
use fold_cli::daemon_client::{DaemonClient, Response};
use fold_core::{FoldError, NotifyTargetId, codes};
use hyper::Method;
use serde_json::{Value, json};

use super::remote::{RemoteError, refusal, request_project_with};
use crate::serve::{self, Dirs};
use crate::{fold_name, output};

#[derive(Args, Debug)]
pub struct NotifyArgs {
    #[command(subcommand)]
    command: Option<NotifyCommand>,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR", global = true)]
    project: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum NotifyCommand {
    /// List the targets that get the project's notices, or the host's with --host; also what `fld notify` with no subcommand prints
    List(ScopeArgs),
    /// Add a target that gets a notice when an item comes into Needs you, and at most one digest in each interval
    Add(AddArgs),
    /// Remove a target
    Remove(IdArgs),
    /// Send a test notice through a target and wait until it is taken
    Test(IdArgs),
    /// Show or set the web address that the links of every notice start with
    WebUrl(WebUrlArgs),
}

#[derive(Args, Debug)]
struct ScopeArgs {
    /// The host's targets, which every project without targets of its own uses
    #[arg(long)]
    host: bool,
}

#[derive(Args, Debug)]
struct IdArgs {
    /// The target, such as nt-3
    id: String,
    /// A target of the host instead of the project
    #[arg(long)]
    host: bool,
}

#[derive(Args, Debug)]
struct AddArgs {
    #[command(subcommand)]
    kind: AddKind,
}

#[derive(Subcommand, Debug)]
enum AddKind {
    /// A URL that gets each notice as a JSON post, signed with HMAC-SHA256 in the x-fold-signature-256 header
    Webhook(WebhookArgs),
    /// An address that gets each notice as an email through an SMTP server
    Email(EmailArgs),
}

#[derive(Args, Debug)]
struct Common {
    /// The shortest time in minutes between two sends to the target; 10 by default
    #[arg(long, value_name = "MINUTES")]
    interval: Option<u32>,
    /// Add the target to the host, for every project without targets of its own
    #[arg(long)]
    host: bool,
}

#[derive(Args, Debug)]
struct WebhookArgs {
    /// The http or https URL that receives the posts
    url: String,
    /// Read the signing secret from the first line of standard input; without it fld makes one and prints it once
    #[arg(long)]
    secret_stdin: bool,
    #[command(flatten)]
    common: Common,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Security {
    /// TLS from the first byte, port 465 by default
    Tls,
    /// A plain connection that STARTTLS upgrades first, port 587 by default
    Starttls,
    /// No TLS, for a relay on a trusted network, port 25 by default; it takes no password
    None,
}

impl Security {
    fn name(self) -> &'static str {
        match self {
            Security::Tls => "tls",
            Security::Starttls => "starttls",
            Security::None => "none",
        }
    }
}

#[derive(Args, Debug)]
struct EmailArgs {
    /// The address that gets the notices
    #[arg(long)]
    to: String,
    /// The sender address
    #[arg(long)]
    from: String,
    /// The SMTP server's host name
    #[arg(long)]
    smtp_host: String,
    /// The SMTP server's port; it follows --security by default
    #[arg(long)]
    smtp_port: Option<u16>,
    #[arg(long, value_enum, default_value = "starttls")]
    security: Security,
    /// The SMTP user name
    #[arg(long)]
    username: Option<String>,
    /// Read the SMTP password from the first line of standard input
    #[arg(long)]
    password_stdin: bool,
    #[command(flatten)]
    common: Common,
}

#[derive(Args, Debug)]
struct WebUrlArgs {
    /// The address people open fold at, such as https://fold.example.com
    url: Option<String>,
    /// Let links follow the daemon's listen address again
    #[arg(long, conflicts_with = "url")]
    clear: bool,
}

/// Runs `fld notify`, starting the daemon when none runs. A secret comes only from standard input or from fld itself, never from an argument, which other users of the machine can read.
pub fn run(
    args: &NotifyArgs,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<(), RemoteError> {
    let list = NotifyCommand::List(ScopeArgs { host: false });
    match args.command.as_ref().unwrap_or(&list) {
        NotifyCommand::List(scope) => {
            let response = call(args, scope.host, Method::GET, "notify", None)?;
            refusal(&response)?;
            for line in list_lines(&response.body, scope.host)? {
                output::line(out, &line)?;
            }
        }
        NotifyCommand::Add(add) => {
            let (body, common, made) = match &add.kind {
                AddKind::Webhook(webhook) => {
                    let (secret, made) = if webhook.secret_stdin {
                        (read_line(input)?, false)
                    } else {
                        (random_secret()?, true)
                    };
                    let body = json!({
                        "kind": "webhook",
                        "url": webhook.url,
                        "secret": secret,
                        "interval_minutes": webhook.common.interval,
                    });
                    (body, &webhook.common, made.then_some(secret))
                }
                AddKind::Email(email) => {
                    let password = if email.password_stdin {
                        Some(read_line(input)?)
                    } else {
                        None
                    };
                    let body = json!({
                        "kind": "email",
                        "to": email.to,
                        "from": email.from,
                        "smtp_host": email.smtp_host,
                        "smtp_port": email.smtp_port,
                        "security": email.security.name(),
                        "username": email.username,
                        "password": password,
                        "interval_minutes": email.common.interval,
                    });
                    (body, &email.common, None)
                }
            };
            let response = call(
                args,
                common.host,
                Method::POST,
                "notify/targets",
                Some(&body),
            )?;
            refusal(&response)?;
            output::line(out, &format!("added {}", target_line(&response.body)?))?;
            if let Some(secret) = made {
                output::line(
                    out,
                    &format!(
                        "signing secret: {secret}\nfold shows it only now: give it to the receiver, which checks the x-fold-signature-256 header with it"
                    ),
                )?;
            }
        }
        NotifyCommand::Remove(target) => {
            let id = parse_id(&target.id)?;
            let response = call(
                args,
                target.host,
                Method::DELETE,
                &format!("notify/targets/{id}"),
                None,
            )?;
            refusal(&response)?;
            output::line(out, &format!("removed {id}"))?;
        }
        NotifyCommand::Test(target) => {
            let id = parse_id(&target.id)?;
            let response = call(
                args,
                target.host,
                Method::POST,
                &format!("notify/targets/{id}/test"),
                None,
            )?;
            refusal(&response)?;
            output::line(out, &format!("{id} took the test notice"))?;
        }
        NotifyCommand::WebUrl(web) => {
            let response = if web.url.is_some() || web.clear {
                let body = json!({ "web_url": web.url });
                call(args, true, Method::PUT, "notify", Some(&body))?
            } else {
                call(args, true, Method::GET, "notify", None)?
            };
            refusal(&response)?;
            let base = response.body["link_base"]
                .as_str()
                .ok_or(RemoteError::Answer)?;
            let line = if response.body["web_url"].is_null() {
                format!("links start with {base}, the daemon's listen address")
            } else {
                format!("links start with {base}")
            };
            output::line(out, &line)?;
        }
    }
    Ok(())
}

fn parse_id(text: &str) -> Result<NotifyTargetId, RemoteError> {
    Ok(text.parse::<NotifyTargetId>().map_err(FoldError::from)?)
}

/// The daemon's answer for `action` below the project's routes, or below the host's with `host`.
fn call(
    args: &NotifyArgs,
    host: bool,
    method: Method,
    action: &str,
    body: Option<&Value>,
) -> Result<Response, RemoteError> {
    if !host {
        return request_project_with(
            args.socket.as_deref(),
            args.project.as_deref(),
            method,
            action,
            body,
        );
    }
    let dirs = Dirs::from_env();
    let config = serve::client_config(args.socket.as_deref(), &dirs)?;
    let client = DaemonClient::new(config, fold_name::current());
    let runtime = serve::current_thread_runtime().map_err(RemoteError::Runtime)?;
    let path = format!("/api/v1/{action}");
    Ok(runtime.block_on(client.request(method, &path, body))?)
}

fn read_line(input: &mut impl BufRead) -> Result<String, RemoteError> {
    let mut line = String::new();
    input.read_line(&mut line)?;
    let line = line.trim_end_matches(['\r', '\n']).to_owned();
    if line.is_empty() {
        return Err(FoldError::new(
            codes::INVALID_NOTIFY_TARGET,
            "standard input held no secret",
        )
        .with_hint("pipe the secret in, such as `pass show fold-hook | fld notify add ...`")
        .into());
    }
    Ok(line)
}

/// 24 bytes from the OS random source as 48 hex digits.
fn random_secret() -> Result<String, RemoteError> {
    let mut bytes = [0_u8; 24];
    getrandom::fill(&mut bytes).map_err(|error| {
        FoldError::new(
            codes::INTERNAL_ERROR,
            format!("the OS random source failed: {error}"),
        )
    })?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// One target as `nt-3 webhook https://hooks.example.com/… every 10 min`.
fn target_line(target: &Value) -> Result<String, RemoteError> {
    let text = |key: &str| target[key].as_str();
    let id = text("id").ok_or(RemoteError::Answer)?;
    let interval = target["interval_minutes"]
        .as_u64()
        .ok_or(RemoteError::Answer)?;
    let place = match text("kind") {
        Some("webhook") => format!("webhook {}", text("url").ok_or(RemoteError::Answer)?),
        Some("email") => format!(
            "email to {} through {}:{}",
            text("to").ok_or(RemoteError::Answer)?,
            text("smtp_host").ok_or(RemoteError::Answer)?,
            target["smtp_port"].as_u64().ok_or(RemoteError::Answer)?
        ),
        _ => return Err(RemoteError::Answer),
    };
    Ok(output::escape_control(&format!(
        "{id} {place} every {}",
        output::count(
            usize::try_from(interval).unwrap_or(usize::MAX),
            "minute",
            "minutes"
        )
    )))
}

/// The lines of `fld notify list`: the targets, then which ones get the project's notices.
fn list_lines(body: &Value, host: bool) -> Result<Vec<String>, RemoteError> {
    let targets = |key: &str| -> Result<Vec<String>, RemoteError> {
        body[key]
            .as_array()
            .ok_or(RemoteError::Answer)?
            .iter()
            .map(target_line)
            .collect()
    };
    let mut lines = Vec::new();
    if host {
        lines.extend(targets("targets")?);
        if lines.is_empty() {
            lines.push(
                "no host targets: add one with fld notify add webhook <url> --host".to_owned(),
            );
        }
        return Ok(lines);
    }
    let own = targets("targets")?;
    let host_targets = targets("host_targets")?;
    if !own.is_empty() {
        lines.extend(own);
    } else if host_targets.is_empty() {
        lines.push(
            "no targets: add one with fld notify add webhook <url>, or fld notify add email"
                .to_owned(),
        );
    } else {
        lines.push("the project has no targets of its own and uses the host's:".to_owned());
        lines.extend(host_targets);
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn webhook() -> Value {
        json!({"id": "nt-1", "kind": "webhook", "url": "https://h/…", "interval_minutes": 10})
    }

    fn email() -> Value {
        json!({"id": "nt-2", "kind": "email", "to": "team@example.com", "smtp_host": "smtp.example.com", "smtp_port": 587, "interval_minutes": 1})
    }

    #[test]
    fn a_target_is_one_line_of_its_id_place_and_interval() {
        assert_eq!(
            target_line(&webhook()).unwrap(),
            "nt-1 webhook https://h/… every 10 minutes"
        );
        assert_eq!(
            target_line(&email()).unwrap(),
            "nt-2 email to team@example.com through smtp.example.com:587 every 1 minute"
        );
        assert!(target_line(&json!({"id": "nt-3", "kind": "fax", "interval_minutes": 1})).is_err());
    }

    #[test]
    fn the_list_says_whose_targets_get_the_notices() {
        let none = json!({"targets": [], "host_targets": []});
        assert_eq!(
            list_lines(&none, false).unwrap()[0],
            "no targets: add one with fld notify add webhook <url>, or fld notify add email"
        );
        let host = json!({"targets": [], "host_targets": [webhook()]});
        assert_eq!(
            list_lines(&host, false).unwrap(),
            [
                "the project has no targets of its own and uses the host's:",
                "nt-1 webhook https://h/… every 10 minutes"
            ]
        );
        let own = json!({"targets": [email()], "host_targets": [webhook()]});
        assert_eq!(list_lines(&own, false).unwrap().len(), 1);
        assert_eq!(
            list_lines(&json!({"targets": []}), true).unwrap()[0],
            "no host targets: add one with fld notify add webhook <url> --host"
        );
        assert!(list_lines(&json!({}), true).is_err());
    }

    #[test]
    fn a_secret_comes_from_one_line_of_input_or_from_the_random_source() {
        let mut input = "from-stdin-secret\nrest\n".as_bytes();
        assert_eq!(read_line(&mut input).unwrap(), "from-stdin-secret");
        let mut empty = "\n".as_bytes();
        assert_eq!(
            read_line(&mut empty).unwrap_err().code(),
            Some(codes::INVALID_NOTIFY_TARGET)
        );
        let made = random_secret().unwrap();
        assert_eq!(made.len(), 48);
        assert!(made.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(made, random_secret().unwrap());
        assert!(parse_id("nt-3").is_ok());
        assert!(parse_id("T-3").is_err());
    }
}
