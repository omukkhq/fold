use std::io::Write;
use std::path::PathBuf;

use clap::Args;
use fold_cli::daemon_client::Response;
use fold_core::{AreaName, FoldError, codes};
use hyper::Method;
use serde_json::{Value, json};

use super::remote::{RemoteError, refusal, send_to_project};
use crate::output;

#[derive(Args, Debug)]
pub struct AreaArgs {
    /// The area, such as web
    name: String,
    /// Add an instruction after the area's current instructions
    #[arg(long, value_name = "TEXT", conflicts_with_all = ["set", "clear"])]
    add: Option<String>,
    /// Replace the area's instructions
    #[arg(long, value_name = "TEXT", conflicts_with = "clear")]
    set: Option<String>,
    /// Remove the area's instructions
    #[arg(long)]
    clear: bool,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

/// Shows the area's instructions, or adds to, replaces or clears them, starting the daemon when none runs.
pub fn run_area(args: &AreaArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let name = AreaName::parse(&args.name).map_err(FoldError::from)?;
    let change = match (&args.add, &args.set, args.clear) {
        (Some(text), _, _) => Some(json!({"add_instructions": text})),
        (None, Some(text), _) => Some(json!({"instructions": text})),
        (None, None, true) => Some(json!({"clear_instructions": true})),
        (None, None, false) => None,
    };
    let lines = match change {
        Some(body) => {
            let response = send_to_project(
                args.socket.as_deref(),
                args.project.as_deref(),
                Method::PATCH,
                &format!("areas/{name}"),
                Some(&body),
            )?;
            refusal(&response)?;
            changed_lines(&name, &response.body)?
        }
        None => {
            let response = send_to_project(
                args.socket.as_deref(),
                args.project.as_deref(),
                Method::GET,
                "workers",
                None,
            )?;
            shown_lines(&name, &response)?
        }
    };
    for line in lines {
        output::line(out, &line)?;
    }
    Ok(())
}

fn instructions(area: &Value) -> Result<Option<&str>, RemoteError> {
    match &area["instructions"] {
        Value::Null => Ok(None),
        Value::String(text) => Ok(Some(text)),
        _ => Err(RemoteError::Answer),
    }
}

fn changed_lines(name: &AreaName, area: &Value) -> Result<Vec<String>, RemoteError> {
    Ok(match instructions(area)? {
        Some(text) => {
            let mut lines = vec![format!(
                "area {name} has these instructions now; its running lead and task workers get them on their next turn, the others on their next start:"
            )];
            lines.extend(text.lines().map(|line| format!("    {line}")));
            lines
        }
        None => vec![format!(
            "area {name} has no instructions now; its running lead and task workers hear of it on their next turn"
        )],
    })
}

fn shown_lines(name: &AreaName, response: &Response) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    let area = response.body["areas"]
        .as_array()
        .ok_or(RemoteError::Answer)?
        .iter()
        .find(|area| area["name"].as_str() == Some(name.as_str()))
        .ok_or_else(|| {
            FoldError::new(codes::AREA_NOT_FOUND, format!("area {name} does not exist"))
                .with_hint("ask the main agent to define the area")
        })?;
    Ok(match instructions(area)? {
        Some(text) => {
            let mut lines = vec![format!("the instructions of area {name}:")];
            lines.extend(text.lines().map(|line| format!("    {line}")));
            lines
        }
        None => vec![format!(
            "area {name} has no instructions: run fld area {name} --add <text> to add one"
        )],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn web() -> AreaName {
        AreaName::parse("web").unwrap()
    }

    #[test]
    fn a_change_prints_the_whole_new_text_or_that_none_is_left() {
        let area = json!({"name": "web", "instructions": "Draw first.\nUse the tokens."});
        assert_eq!(
            changed_lines(&web(), &area).unwrap(),
            [
                "area web has these instructions now; its running lead and task workers get them on their next turn, the others on their next start:",
                "    Draw first.",
                "    Use the tokens.",
            ]
        );
        assert_eq!(
            changed_lines(&web(), &json!({"name": "web", "instructions": null})).unwrap(),
            [
                "area web has no instructions now; its running lead and task workers hear of it on their next turn"
            ]
        );
        assert!(matches!(
            changed_lines(&web(), &json!({"instructions": 3})),
            Err(RemoteError::Answer)
        ));
    }

    #[test]
    fn the_area_is_found_in_the_worker_list() {
        let list = |areas: Value| Response {
            status: 200,
            body: json!({"areas": areas}),
        };
        assert_eq!(
            shown_lines(
                &web(),
                &list(json!([{"name": "api", "instructions": "x"}, {"name": "web", "instructions": "Draw first."}]))
            )
            .unwrap(),
            ["the instructions of area web:", "    Draw first."]
        );
        assert_eq!(
            shown_lines(
                &web(),
                &list(json!([{"name": "web", "instructions": null}]))
            )
            .unwrap(),
            ["area web has no instructions: run fld area web --add <text> to add one"]
        );
        assert_eq!(
            shown_lines(&web(), &list(json!([]))).unwrap_err().code(),
            Some(codes::AREA_NOT_FOUND)
        );
        assert!(matches!(
            shown_lines(
                &web(),
                &Response {
                    status: 200,
                    body: json!({})
                }
            ),
            Err(RemoteError::Answer)
        ));
    }
}
