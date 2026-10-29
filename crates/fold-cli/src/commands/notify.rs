use std::io::{BufRead, Write};
use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};
use fold_cli::daemon_client::{DaemonClient, Response};
use fold_core::{FoldError, NotifyTargetId, Timestamp, codes};
use hyper::Method;
use serde_json::{Map, Value, json};

use super::remote::{RemoteError, refusal, request_project_with};
use crate::output::{self, Item};
use crate::serve::{self, Dirs};

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
    /// List the targets that get the project's notices: its own and the host's; also what `fld notify` with no subcommand prints
    List,
    /// Add a target that gets the events it selects, grouped by its window
    Add(AddArgs),
    /// Change a target's name, URL, events, card-move columns, window or switch
    Edit(EditArgs),
    /// Remove a target
    Remove(TargetArg),
    /// Send a test notice through a target and wait for the answer; one that gets through turns a target fold turned off on again
    Test(TargetArg),
    /// Show a target's newest sends
    Log(TargetArg),
    /// Give a generic target a new signing secret; the old one signs too for a day
    Rotate(TargetArg),
    /// Show or set the web address that the links of every notice start with
    WebUrl(WebUrlArgs),
    /// Show or set whether the host's targets get the project's notices too
    HostTargets(HostTargetsArgs),
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Switch {
    On,
    Off,
}

#[derive(Args, Debug)]
struct HostTargetsArgs {
    state: Option<Switch>,
}

#[derive(Args, Debug)]
struct TargetArg {
    /// The target's id, such as nt-3, or its name
    target: String,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Kind {
    /// A receiver that gets a signed JSON body (Standard Webhooks)
    Webhook,
    /// A Slack incoming webhook
    Slack,
    /// A Discord channel webhook
    Discord,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Webhook => "webhook",
            Kind::Slack => "slack",
            Kind::Discord => "discord",
        }
    }
}

#[derive(Args, Debug)]
struct Selection {
    /// recommended, all, needs-you-only, or event names separated by commas, such as task.moved,checks.failed
    #[arg(long, value_name = "EVENTS")]
    events: Option<String>,
    /// The columns whose card moves the target gets, separated by commas: backlog, todo, in-progress, review, done
    #[arg(long, value_name = "COLUMNS")]
    moves_to: Option<String>,
    /// Seconds after the first notice that go in one send; 0 sends each notice alone
    #[arg(long, value_name = "SECONDS")]
    window: Option<u32>,
}

#[derive(Args, Debug)]
struct AddArgs {
    kind: Kind,
    /// The target's name, such as "Team Slack"
    name: String,
    /// The receiver's URL; without it fld reads the first line of standard input, so a URL that holds a token stays out of the shell history
    url: Option<String>,
    /// Add the target to the host, for every project that uses the host targets
    #[arg(long)]
    host: bool,
    #[command(flatten)]
    selection: Selection,
}

#[derive(Args, Debug)]
struct EditArgs {
    /// The target's id, such as nt-3, or its name
    target: String,
    #[arg(long)]
    name: Option<String>,
    /// Read a new URL from the first line of standard input
    #[arg(long)]
    url_stdin: bool,
    #[command(flatten)]
    selection: Selection,
    /// Turn the target on or off
    #[arg(long, value_enum)]
    switch: Option<Switch>,
}

#[derive(Args, Debug)]
struct WebUrlArgs {
    /// The address people open fold at
    url: Option<String>,
    /// Send notices without links
    #[arg(long, conflicts_with = "url")]
    clear: bool,
}

/// Runs `fld notify`, starting the daemon when none runs. A URL or a secret comes only from an argument a person chose to give, from standard input or from the daemon, which prints a new signing secret once.
pub fn run(
    args: &NotifyArgs,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<(), RemoteError> {
    let lines = match args.command.as_ref().unwrap_or(&NotifyCommand::List) {
        NotifyCommand::List => {
            let response = call(args, false, Method::GET, "notify", None)?;
            refusal(&response)?;
            output::items(list_items(&response.body)?)
        }
        NotifyCommand::Add(add) => {
            let url = match &add.url {
                Some(url) => url.clone(),
                None => read_line(input)?,
            };
            let mut body = selection(&add.selection)?;
            body.insert("kind".to_owned(), json!(add.kind.name()));
            body.insert("name".to_owned(), json!(add.name));
            body.insert("url".to_owned(), json!(url));
            let response = call(
                args,
                add.host,
                Method::POST,
                "notify/targets",
                Some(&Value::Object(body)),
            )?;
            refusal(&response)?;
            added_lines("Added", &response.body)?
        }
        NotifyCommand::Edit(edit) => {
            let (id, host) = resolve(args, &edit.target)?;
            let mut body = selection(&edit.selection)?;
            if let Some(name) = &edit.name {
                body.insert("name".to_owned(), json!(name));
            }
            if edit.url_stdin {
                body.insert("url".to_owned(), json!(read_line(input)?));
            }
            if let Some(switch) = edit.switch {
                body.insert("enabled".to_owned(), json!(matches!(switch, Switch::On)));
            }
            let response = call(
                args,
                host,
                Method::PUT,
                &format!("notify/targets/{id}"),
                Some(&Value::Object(body)),
            )?;
            refusal(&response)?;
            output::items(vec![target_item(&response.body)?])
        }
        NotifyCommand::Remove(target) => {
            let (id, host) = resolve(args, &target.target)?;
            let response = call(
                args,
                host,
                Method::DELETE,
                &format!("notify/targets/{id}"),
                None,
            )?;
            refusal(&response)?;
            vec![format!("Removed {id}")]
        }
        NotifyCommand::Test(target) => {
            let (id, _) = resolve(args, &target.target)?;
            let response = call(
                args,
                false,
                Method::POST,
                &format!("notify/targets/{id}/test"),
                None,
            )?;
            refusal(&response)?;
            vec![format!("{id} took the test notice")]
        }
        NotifyCommand::Log(target) => {
            let (id, _) = resolve(args, &target.target)?;
            let response = call(
                args,
                false,
                Method::GET,
                &format!("notify/targets/{id}/sends"),
                None,
            )?;
            refusal(&response)?;
            output::items(log_items(&response.body)?)
        }
        NotifyCommand::Rotate(target) => {
            let (id, host) = resolve(args, &target.target)?;
            let response = call(
                args,
                host,
                Method::POST,
                &format!("notify/targets/{id}/rotate"),
                None,
            )?;
            refusal(&response)?;
            added_lines("New secret for", &response.body)?
        }
        NotifyCommand::WebUrl(web) => {
            let response = if web.url.is_some() || web.clear {
                let body = json!({ "web_url": web.url });
                call(args, true, Method::PUT, "notify", Some(&body))?
            } else {
                call(args, true, Method::GET, "notify", None)?
            };
            refusal(&response)?;
            vec![web_url_line(&response.body)]
        }
        NotifyCommand::HostTargets(switch) => {
            let response = match switch.state {
                Some(state) => {
                    let body = json!({ "use_host_targets": matches!(state, Switch::On) });
                    call(args, false, Method::PUT, "notify", Some(&body))?
                }
                None => call(args, false, Method::GET, "notify", None)?,
            };
            refusal(&response)?;
            vec![host_targets_line(&response.body)?]
        }
    };
    for line in lines {
        output::line(out, &line)?;
    }
    Ok(())
}

fn invalid(message: String) -> RemoteError {
    FoldError::new(codes::INVALID_NOTIFY_TARGET, message).into()
}

/// The `events` or `preset`, `moves_to` and `window_seconds` of a request body.
fn selection(selection: &Selection) -> Result<Map<String, Value>, RemoteError> {
    let mut body = Map::new();
    if let Some(events) = &selection.events {
        match events
            .trim()
            .to_ascii_lowercase()
            .replace('-', "_")
            .as_str()
        {
            preset @ ("recommended" | "all" | "needs_you_only") => {
                body.insert("preset".to_owned(), json!(preset));
            }
            _ => {
                let names: Vec<&str> = events
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .collect();
                if names.is_empty() {
                    return Err(invalid("--events names no event".to_owned()));
                }
                body.insert("events".to_owned(), json!(names));
            }
        }
    }
    if let Some(columns) = &selection.moves_to {
        let columns = columns
            .split(',')
            .map(str::trim)
            .filter(|column| !column.is_empty())
            .map(
                |column| match column.to_ascii_lowercase().replace('-', "_").as_str() {
                    "to_do" | "todo" => Ok("todo".to_owned()),
                    known @ ("backlog" | "in_progress" | "review" | "done") => Ok(known.to_owned()),
                    _ => Err(invalid(format!(
                        "{} is not a column: name backlog, todo, in-progress, review or done",
                        output::escape_control(column)
                    ))),
                },
            )
            .collect::<Result<Vec<_>, _>>()?;
        body.insert("moves_to".to_owned(), json!(columns));
    }
    if let Some(window) = selection.window {
        body.insert("window_seconds".to_owned(), json!(window));
    }
    Ok(body)
}

/// The target a person named by its id or its name, with whether it is the host's.
fn resolve(args: &NotifyArgs, named: &str) -> Result<(NotifyTargetId, bool), RemoteError> {
    let response = call(args, false, Method::GET, "notify", None)?;
    refusal(&response)?;
    find_target(&response.body, named)
}

fn find_target(body: &Value, named: &str) -> Result<(NotifyTargetId, bool), RemoteError> {
    let all: Vec<&Value> = ["targets", "host_targets"]
        .iter()
        .filter_map(|key| body[*key].as_array())
        .flatten()
        .collect();
    let by_id = named.parse::<NotifyTargetId>().ok();
    let found: Vec<&&Value> = all
        .iter()
        .filter(|target| match by_id {
            Some(id) => target["id"].as_str() == Some(id.to_string().as_str()),
            None => target["name"].as_str() == Some(named),
        })
        .collect();
    match found.as_slice() {
        [target] => {
            let id = target["id"]
                .as_str()
                .and_then(|id| id.parse::<NotifyTargetId>().ok())
                .ok_or(RemoteError::Answer)?;
            Ok((id, target["host"].as_bool().unwrap_or(false)))
        }
        [] => Err(FoldError::new(
            codes::NOTIFY_TARGET_NOT_FOUND,
            format!("no notice target here is {}", output::escape_control(named)),
        )
        .with_hint("`fld notify list` lists the targets with their ids")
        .into()),
        _ => Err(invalid(format!(
            "more than one target is named {}: name it by its id",
            output::escape_control(named)
        ))),
    }
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
    let client = DaemonClient::new(config, crate::fold_name::current());
    let runtime = serve::current_thread_runtime().map_err(RemoteError::Runtime)?;
    let path = format!("/api/v1/{action}");
    Ok(runtime.block_on(client.request(method, &path, body))?)
}

fn read_line(input: &mut impl BufRead) -> Result<String, RemoteError> {
    let mut line = String::new();
    input.read_line(&mut line)?;
    let line = line.trim().to_owned();
    if line.is_empty() {
        return Err(FoldError::new(
            codes::INVALID_NOTIFY_TARGET,
            "standard input held no URL",
        )
        .with_hint("give the URL as an argument or pipe it in, such as `pass show slack-hook | fld notify add slack \"Team Slack\"`")
        .into());
    }
    Ok(line)
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, RemoteError> {
    value[key].as_str().ok_or(RemoteError::Answer)
}

fn time(value: &Value) -> Option<String> {
    value
        .as_i64()
        .and_then(|millis| Timestamp::from_millis(millis).ok())
        .map(output::local_time)
}

fn kind_label(kind: &str) -> &str {
    match kind {
        "webhook" => "Generic JSON",
        "slack" => "Slack",
        "discord" => "Discord",
        other => other,
    }
}

fn column_label(column: &str) -> &str {
    match column {
        "backlog" => "Backlog",
        "todo" => "To do",
        "in_progress" => "In progress",
        "review" => "Review",
        "done" => "Done",
        other => other,
    }
}

/// One target as `nt-3 Team Slack · Slack · on`, then its URL, events, columns and window, then its health.
fn target_item(target: &Value) -> Result<Item, RemoteError> {
    let state = text(target, "state")?;
    let state = match state {
        "failing" => "off: fold turned it off after its sends failed",
        "gone" => "off: the receiver answered 410 Gone",
        other => other,
    };
    let mut item = Item::new(output::escape_control(&format!(
        "{} {} · {} · {state}",
        text(target, "id")?,
        text(target, "name")?,
        kind_label(text(target, "kind")?),
    )));
    let events = target["events"].as_array().ok_or(RemoteError::Answer)?;
    let preset = match text(target, "preset")? {
        "recommended" => format!("Recommended ({} events)", events.len()),
        "all" => "All events".to_owned(),
        "needs_you_only" => "Needs you only".to_owned(),
        _ => events
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", "),
    };
    item.detail(output::escape_control(&format!(
        "{} · {preset}",
        text(target, "url")?
    )));
    let moves = events
        .iter()
        .any(|event| event.as_str() == Some("task.moved"));
    let columns: Vec<&str> = target["moves_to"]
        .as_array()
        .ok_or(RemoteError::Answer)?
        .iter()
        .filter_map(Value::as_str)
        .map(column_label)
        .collect();
    let window = target["window_seconds"]
        .as_u64()
        .ok_or(RemoteError::Answer)?;
    let window = if window == 0 {
        "each notice alone".to_owned()
    } else {
        format!("one send per {window} s")
    };
    if moves {
        item.detail(format!("card moves to {} · {window}", columns.join(", ")));
    } else {
        item.detail(window);
    }
    let scope = if target["host"].as_bool() == Some(true) {
        "host target"
    } else {
        "this project"
    };
    let mut health = vec![scope.to_owned()];
    if let Some(at) = time(&target["last_ok_at"]) {
        health.push(format!("last sent {at}"));
    }
    let failures = target["failures"].as_u64().unwrap_or(0);
    if failures > 0 {
        health.push(output::count(
            usize::try_from(failures).unwrap_or(usize::MAX),
            "failed try",
            "failed tries",
        ));
    }
    if let Some(at) = time(&target["next_try_at"]) {
        health.push(format!("next try {at}"));
    }
    item.detail(health.join(" · "));
    Ok(item)
}

/// The lines of an add or a rotation: the target, then a generic target's signing secret, which fold shows only now.
fn added_lines(verb: &str, body: &Value) -> Result<Vec<String>, RemoteError> {
    let target = &body["target"];
    let mut item = target_item(target)?;
    if let Some(secret) = body["secret"].as_str() {
        item.detail(format!("signing secret: {secret}"));
        item.deeper("fold shows it only now: give it to the receiver, which checks the webhook-signature header with it");
    }
    let mut lines = vec![format!("{verb} {}", text(target, "id")?)];
    lines.extend(output::items(vec![item]));
    Ok(lines)
}

/// The items of `fld notify list`: the project's targets, the host's, then the host targets switch and the web address.
fn list_items(body: &Value) -> Result<Vec<Item>, RemoteError> {
    let mut items = Vec::new();
    for key in ["targets", "host_targets"] {
        for target in body[key].as_array().ok_or(RemoteError::Answer)? {
            items.push(target_item(target)?);
        }
    }
    if items.is_empty() {
        let mut none = Item::new("No notice targets");
        none.detail("Next: `fld notify add slack|discord|webhook <name> [url]` adds one");
        items.push(none);
    }
    let mut settings = Item::new(host_targets_line(body)?);
    settings.detail(web_url_line(body));
    items.push(settings);
    Ok(items)
}

fn host_targets_line(body: &Value) -> Result<String, RemoteError> {
    Ok(if body["use_host_targets"]
        .as_bool()
        .ok_or(RemoteError::Answer)?
    {
        "Host targets: on, they get the project's notices too"
    } else {
        "Host targets: off, only the project's own targets get its notices"
    }
    .to_owned())
}

fn web_url_line(body: &Value) -> String {
    match body["web_url"].as_str() {
        Some(url) => output::escape_control(&format!("Links start with {url}")),
        None => "Notices carry no links: `fld notify web-url <url>` sets the address people open fold at".to_owned(),
    }
}

/// The items of `fld notify log`: one send each, newest first.
fn log_items(body: &Value) -> Result<Vec<Item>, RemoteError> {
    let sends = body["sends"].as_array().ok_or(RemoteError::Answer)?;
    if sends.is_empty() {
        return Ok(vec![Item::new("No sends yet")]);
    }
    sends
        .iter()
        .map(|send| {
            let answer = match (send["status"].as_u64(), send["error"].as_str()) {
                (Some(status), _) => status.to_string(),
                (None, Some(error)) => error.to_owned(),
                (None, None) => "no try yet".to_owned(),
            };
            let tries = send["tries"].as_u64().unwrap_or(0);
            let mut head = format!(
                "{} {} · {answer} · try {tries}",
                text(send, "id")?,
                text(send, "state")?.replace('_', " "),
            );
            if let Some(duration) = send["duration_ms"].as_u64() {
                head.push_str(&format!(" · {duration} ms"));
            }
            let mut item = Item::new(head);
            let notices: Vec<&str> = send["notices"]
                .as_array()
                .ok_or(RemoteError::Answer)?
                .iter()
                .filter_map(Value::as_str)
                .collect();
            let when = time(&send["created_at"]).unwrap_or_default();
            item.detail(output::escape_control(&format!(
                "{when} · {}",
                notices.join(", ")
            )));
            if let Some(next) =
                time(&send["next_try_at"]).filter(|_| text(send, "state").ok() == Some("pending"))
            {
                item.detail(format!("next try {next}"));
            }
            item.detail(format!("webhook-id {}", text(send, "webhook_id")?));
            Ok(item)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> Value {
        json!({
            "id": "nt-1", "name": "Team Slack", "kind": "slack", "url": "https://hooks.slack.com/…",
            "events": ["needs_you.opened", "task.moved", "task.blocked", "review.changes_requested", "proposal.integrated", "integration.failed", "checks.failed", "worker.failed", "git.failed"],
            "preset": "recommended", "moves_to": ["review", "done"], "window_seconds": 15,
            "enabled": true, "state": "on", "failures": 0, "last_ok_at": null, "last_failed_at": null,
            "next_try_at": null, "host": true, "created_at": 1, "created_by": "ana"
        })
    }

    fn lines(item: Item) -> Vec<String> {
        output::items(vec![item])
    }

    #[test]
    fn a_target_shows_its_place_selection_window_and_health() {
        assert_eq!(
            lines(target_item(&target()).unwrap()),
            [
                "nt-1 Team Slack · Slack · on",
                "  ↳ https://hooks.slack.com/… · Recommended (9 events)",
                "  ↳ card moves to Review, Done · one send per 15 s",
                "  ↳ host target",
            ]
        );
        let mut failing = target();
        failing["state"] = json!("failing");
        failing["failures"] = json!(3);
        failing["host"] = json!(false);
        failing["preset"] = json!("custom");
        failing["events"] = json!(["checks.failed"]);
        failing["window_seconds"] = json!(0);
        let shown = lines(target_item(&failing).unwrap());
        assert!(
            shown[0].ends_with("off: fold turned it off after its sends failed"),
            "{shown:?}"
        );
        assert_eq!(shown[1], "  ↳ https://hooks.slack.com/… · checks.failed");
        assert_eq!(shown[2], "  ↳ each notice alone");
        assert_eq!(shown[3], "  ↳ this project · 3 failed tries");
        failing["state"] = json!("gone");
        failing["preset"] = json!("all");
        assert!(lines(target_item(&failing).unwrap())[0].ends_with("410 Gone"));
        assert!(target_item(&json!({})).is_err());
    }

    #[test]
    fn the_list_ends_with_the_host_switch_and_the_web_address() {
        let none =
            json!({"targets": [], "host_targets": [], "use_host_targets": true, "web_url": null});
        let shown = output::items(list_items(&none).unwrap());
        assert_eq!(shown[0], "No notice targets");
        assert!(
            shown
                .iter()
                .any(|line| line == "Host targets: on, they get the project's notices too")
        );
        assert!(shown.last().unwrap().contains("Notices carry no links"));
        let some = json!({"targets": [], "host_targets": [target()], "use_host_targets": false, "web_url": "https://fold.example.org"});
        let shown = output::items(list_items(&some).unwrap());
        assert_eq!(shown[0], "nt-1 Team Slack · Slack · on");
        assert!(
            shown
                .iter()
                .any(|line| line.starts_with("Host targets: off"))
        );
        assert_eq!(
            shown.last().unwrap(),
            "  ↳ Links start with https://fold.example.org"
        );
        assert!(list_items(&json!({})).is_err());
    }

    #[test]
    fn a_target_is_named_by_its_id_or_its_name() {
        let mut own = target();
        own["id"] = json!("nt-2");
        own["name"] = json!("Release bot");
        own["host"] = json!(false);
        let body = json!({"targets": [own], "host_targets": [target()]});
        assert_eq!(
            find_target(&body, "nt-1").unwrap(),
            (NotifyTargetId::from_number(1).unwrap(), true)
        );
        assert_eq!(
            find_target(&body, "Release bot").unwrap(),
            (NotifyTargetId::from_number(2).unwrap(), false)
        );
        assert_eq!(
            find_target(&body, "nt-9").unwrap_err().code(),
            Some(codes::NOTIFY_TARGET_NOT_FOUND)
        );
        let twice = json!({"targets": [target()], "host_targets": [target()]});
        assert_eq!(
            find_target(&twice, "Team Slack").unwrap_err().code(),
            Some(codes::INVALID_NOTIFY_TARGET)
        );
    }

    #[test]
    fn a_selection_is_a_preset_or_named_events_with_columns_and_a_window() {
        let picked = |events: Option<&str>, moves: Option<&str>, window| {
            selection(&Selection {
                events: events.map(str::to_owned),
                moves_to: moves.map(str::to_owned),
                window,
            })
        };
        let body = picked(
            Some("needs-you-only"),
            Some("in-progress, To-do,done"),
            Some(0),
        )
        .unwrap();
        assert_eq!(body["preset"], "needs_you_only");
        assert_eq!(body["moves_to"], json!(["in_progress", "todo", "done"]));
        assert_eq!(body["window_seconds"], 0);
        let body = picked(Some("task.moved, checks.failed"), None, None).unwrap();
        assert_eq!(body["events"], json!(["task.moved", "checks.failed"]));
        assert!(picked(Some(" , "), None, None).is_err());
        assert!(picked(None, Some("sideways"), None).is_err());
        assert!(picked(None, None, None).unwrap().is_empty());
    }

    #[test]
    fn a_url_comes_from_one_line_of_input() {
        let mut input = "https://hooks.slack.com/services/T0/B0/x\nrest\n".as_bytes();
        assert_eq!(
            read_line(&mut input).unwrap(),
            "https://hooks.slack.com/services/T0/B0/x"
        );
        let mut empty = "\n".as_bytes();
        assert_eq!(
            read_line(&mut empty).unwrap_err().code(),
            Some(codes::INVALID_NOTIFY_TARGET)
        );
    }

    #[test]
    fn the_log_shows_each_send_with_its_answer() {
        let body = json!({"sends": [
            {"id": "ns-3", "state": "pending", "status": 503, "error": null, "tries": 3, "duration_ms": 12,
             "notices": ["task.moved T-18"], "created_at": 1_793_180_530_000_i64, "next_try_at": 1_793_180_830_000_i64, "webhook_id": "nobs-1a2b-3"},
            {"id": "ns-2", "state": "given_up", "status": null, "error": "timeout", "tries": 7, "duration_ms": null,
             "notices": ["needs_you.opened m-31"], "created_at": 1_793_180_000_000_i64, "next_try_at": null, "webhook_id": "nobs-1a2b-2"},
            {"id": "ns-1", "state": "pending", "status": null, "error": null, "tries": 0, "duration_ms": null,
             "notices": [], "created_at": 1, "next_try_at": null, "webhook_id": "w"}
        ]});
        let shown = output::items(log_items(&body).unwrap());
        assert_eq!(shown[0], "ns-3 pending · 503 · try 3 · 12 ms");
        assert!(shown[1].ends_with("task.moved T-18"), "{shown:?}");
        assert!(shown[2].starts_with("  ↳ next try "));
        assert_eq!(shown[3], "  ↳ webhook-id nobs-1a2b-3");
        assert!(shown.contains(&"ns-2 given up · timeout · try 7".to_owned()));
        assert!(shown.contains(&"ns-1 pending · no try yet · try 0".to_owned()));
        assert_eq!(
            output::items(log_items(&json!({"sends": []})).unwrap()),
            ["No sends yet"]
        );
        assert!(log_items(&json!({})).is_err());
    }

    #[test]
    fn an_add_prints_the_secret_once_with_its_use() {
        let shown = added_lines(
            "Added",
            &json!({"target": target(), "secret": "whsec_AAAA"}),
        )
        .unwrap();
        assert_eq!(shown[0], "Added nt-1");
        assert!(shown.contains(&"  ↳ signing secret: whsec_AAAA".to_owned()));
        assert!(
            !added_lines("Added", &json!({"target": target(), "secret": null}))
                .unwrap()
                .iter()
                .any(|line| line.contains("secret"))
        );
        assert_eq!(kind_label("webhook"), "Generic JSON");
        assert_eq!(kind_label("discord"), "Discord");
        assert_eq!(kind_label("fax"), "fax");
        assert_eq!(column_label("todo"), "To do");
        assert_eq!(column_label("backlog"), "Backlog");
        assert_eq!(column_label("in_progress"), "In progress");
        assert_eq!(column_label("x"), "x");
    }
}
