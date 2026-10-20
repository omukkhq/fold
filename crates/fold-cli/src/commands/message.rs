use std::io::Write;
use std::path::PathBuf;

use clap::Args;
use fold_cli::daemon_client::Response;
use fold_core::{AgentName, FoldError};
use hyper::Method;
use serde_json::{Value, json};

use super::remote::{RemoteError, refusal, send_to_project};
use crate::output;

#[derive(Args, Debug)]
pub struct MessageArgs {
    /// The agent to write to, such as web-lead or the main agent's name
    agent: String,
    /// The message; leave it out to show the chat with the agent
    text: Vec<String>,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

/// Writes the text to the agent in its direct chat, or shows the chat when there is no text, starting the daemon when none runs.
pub fn run_message(args: &MessageArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let agent = AgentName::parse(&args.agent).map_err(FoldError::from)?;
    let action = format!("agents/{agent}/chat");
    let text = args.text.join(" ");
    let lines = if text.trim().is_empty() {
        let response = send_to_project(
            args.socket.as_deref(),
            args.project.as_deref(),
            Method::GET,
            &action,
            None,
        )?;
        chat_lines(&agent, &response)?
    } else {
        let response = send_to_project(
            args.socket.as_deref(),
            args.project.as_deref(),
            Method::POST,
            &action,
            Some(&json!({"body": text})),
        )?;
        vec![sent_line(&agent, &response)?]
    };
    for line in lines {
        output::line(out, &line)?;
    }
    Ok(())
}

fn sent_line(agent: &AgentName, response: &Response) -> Result<String, RemoteError> {
    refusal(response)?;
    let id = response.body["id"].as_str().ok_or(RemoteError::Answer)?;
    Ok(format!(
        "sent {id} to {agent}; it answers in the chat: run fld message {agent} to read it"
    ))
}

/// One line per message, oldest first, with each further line of a body indented under it.
fn chat_lines(agent: &AgentName, response: &Response) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    let messages = response.body["messages"]
        .as_array()
        .ok_or(RemoteError::Answer)?;
    if messages.is_empty() {
        return Ok(vec![format!(
            "no messages with {agent} yet: run fld message {agent} <text> to write one"
        )]);
    }
    let mut lines = Vec::new();
    if !response.body["next"].is_null() {
        lines.push("older messages are in the chat of the web app".to_owned());
    }
    for message in messages {
        lines.extend(message_lines(message)?);
    }
    Ok(lines)
}

fn message_lines(message: &Value) -> Result<Vec<String>, RemoteError> {
    let text = |key: &str| message[key].as_str().ok_or(RemoteError::Answer);
    let sender = text("sender")?;
    let sender = if sender.starts_with("human:") {
        output::person(sender)
    } else {
        sender.to_owned()
    };
    let mut body = text("body")?.lines();
    let first = body.next().unwrap_or_default();
    let mut lines = vec![format!("{} {sender}: {first}", text("id")?)];
    lines.extend(body.map(|line| format!("    {line}")));
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, body: Value) -> Response {
        Response { status, body }
    }

    fn lead() -> AgentName {
        AgentName::parse("web-lead").unwrap()
    }

    #[test]
    fn a_sent_message_names_its_id_and_where_the_answer_comes() {
        let line = sent_line(&lead(), &response(201, json!({"id": "m-7"}))).unwrap();
        assert_eq!(
            line,
            "sent m-7 to web-lead; it answers in the chat: run fld message web-lead to read it"
        );
        assert!(matches!(
            sent_line(&lead(), &response(201, json!({}))),
            Err(RemoteError::Answer)
        ));
    }

    #[test]
    fn the_chat_prints_one_line_per_message_and_indents_further_lines() {
        let page = json!({
            "agent": "web-lead",
            "next": "m-3",
            "messages": [
                {"id": "m-3", "sender": "human:ana", "body": "Use the new tokens."},
                {"id": "m-4", "sender": "web-lead", "body": "Done.\nI told the workers."},
            ],
        });
        assert_eq!(
            chat_lines(&lead(), &response(200, page)).unwrap(),
            [
                "older messages are in the chat of the web app",
                "m-3 @ana: Use the new tokens.",
                "m-4 web-lead: Done.",
                "    I told the workers.",
            ]
        );
        assert_eq!(
            chat_lines(
                &lead(),
                &response(200, json!({"messages": [], "next": null}))
            )
            .unwrap(),
            ["no messages with web-lead yet: run fld message web-lead <text> to write one"]
        );
    }

    #[test]
    fn a_refusal_or_a_strange_answer_is_an_error() {
        let refused = response(
            404,
            json!({"error": {"code": "agent_not_found", "message": "agent web-lead does not exist"}}),
        );
        assert_eq!(
            chat_lines(&lead(), &refused).unwrap_err().code(),
            Some("agent_not_found")
        );
        assert!(matches!(
            chat_lines(&lead(), &response(200, json!({}))),
            Err(RemoteError::Answer)
        ));
        assert!(matches!(
            chat_lines(
                &lead(),
                &response(200, json!({"messages": [{"id": "m-1"}]}))
            ),
            Err(RemoteError::Answer)
        ));
    }
}
