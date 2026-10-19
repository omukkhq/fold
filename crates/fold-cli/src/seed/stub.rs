mod script;

use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use clap::Args;
use fold_core::AgentName;
use fold_core::layout::{AGENT_ENV, MCP_SERVER_NAME, MCP_SERVERS_KEY};
use fold_daemon::services::agents::DEV_STUB_DIR;
use fold_daemon::services::agents::claude::{ClaudeInput, OutgoingRequest};
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use script::{Call, Script, ScriptError, Step, Turn, expand, expand_value, inside, saved};

use crate::serve;

/// The stub's own record in the worker's folder: the sessions it ran, the next turn of its script and the values its calls saved. It belongs to the agent, so a resumed or freshly started process goes on where the last one stopped.
const STATE_FILE: &str = "stub-state.json";

/// One line per step in the worker's folder: the turn, the step and how it ended, never a file's content.
const LOG_FILE: &str = "stub.log";

/// How long a call with `until` keeps polling, and the pause between two of its calls.
const UNTIL_LIMIT: Duration = Duration::from_secs(120);
const UNTIL_PAUSE: Duration = Duration::from_millis(100);

/// The model the stub names in its output.
const MODEL: &str = "stub";

#[derive(Args, Debug)]
pub struct StubArgs {
    /// Claude Code's arguments, as fold's supervisor passes them to `claude`
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Debug, Error)]
pub enum StubError {
    #[error("the arguments have no {0}")]
    MissingFlag(&'static str),
    #[error("{AGENT_ENV} names no agent")]
    Agent,
    #[error("the MCP configuration has no {MCP_SERVER_NAME} server with a command")]
    McpConfig,
    #[error("cannot start the {MCP_SERVER_NAME} MCP server: {0}")]
    Spawn(io::Error),
    #[error("the {MCP_SERVER_NAME} MCP server did not open a session")]
    Connect,
    #[error("cannot use the stub's files: {0}")]
    Files(#[from] io::Error),
    #[error("cannot read the script {}: {message}", path.display())]
    Script { path: PathBuf, message: String },
    #[error("cannot write a line: {0}")]
    Json(#[from] serde_json::Error),
    #[error("turn {turn}, step {step}: {reason}")]
    Step {
        turn: usize,
        step: usize,
        reason: String,
    },
    #[error("no session {0} to resume")]
    Lost(String),
    #[error("cannot start the async runtime: {0}")]
    Runtime(io::Error),
}

/// What the stub reads of Claude Code's arguments.
#[derive(Debug, PartialEq, Eq)]
struct Options {
    session: String,
    resume: bool,
    mcp_config: PathBuf,
    model: Option<String>,
}

impl Options {
    fn parse(args: &[String]) -> Result<Self, StubError> {
        let value = |flag: &str| {
            args.iter()
                .position(|arg| arg == flag)
                .and_then(|at| args.get(at + 1))
                .cloned()
        };
        let (session, resume) = match (value("--resume"), value("--session-id")) {
            (Some(session), _) => (session, true),
            (None, Some(session)) => (session, false),
            (None, None) => return Err(StubError::MissingFlag("--session-id or --resume")),
        };
        Ok(Self {
            session,
            resume,
            mcp_config: value("--mcp-config")
                .map(PathBuf::from)
                .ok_or(StubError::MissingFlag("--mcp-config"))?,
            model: value("--model"),
        })
    }
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
struct State {
    sessions: Vec<String>,
    turn: usize,
    vars: BTreeMap<String, String>,
}

/// Plays Claude Code for fold's supervisor: answers the control requests and runs the agent's script, one script turn per user turn, through the `fold` server of `--mcp-config`. It exits when stdin closes, and with an error once a step fails, so the worker ends `failed`.
pub fn run(args: &StubArgs) -> Result<(), StubError> {
    let options = Options::parse(&args.args)?;
    let runtime = serve::current_thread_runtime().map_err(StubError::Runtime)?;
    let outcome = runtime.block_on(serve_stdio(options));
    // The stdin reader blocks a runtime thread until the next line, which may never come.
    runtime.shutdown_background();
    outcome
}

async fn serve_stdio(options: Options) -> Result<(), StubError> {
    let root = std::env::current_dir()?;
    let agent = std::env::var(AGENT_ENV)
        .ok()
        .and_then(|name| AgentName::parse(&name).ok())
        .ok_or(StubError::Agent)?;
    let folder = options
        .mcp_config
        .parent()
        .map_or_else(|| root.clone(), Path::to_path_buf);
    let script_path = root
        .join(".fold")
        .join(DEV_STUB_DIR)
        .join(format!("{agent}.json"));
    let mut state = read_state(&folder.join(STATE_FILE)).await?;
    let lost = options.resume && !state.sessions.contains(&options.session);
    if !options.resume && !state.sessions.contains(&options.session) {
        state.sessions.push(options.session.clone());
    }
    let (sender, lines) = mpsc::unbounded_channel();
    tokio::spawn(read_stdin(sender));
    let mut stub = Stub {
        root,
        folder,
        script_path,
        options,
        state,
        lines,
        queued: VecDeque::new(),
        tools: None,
        turns: 0,
        calls: 0,
        out: tokio::io::stdout(),
    };
    stub.save().await?;
    let outcome = stub.serve(lost).await;
    if let Some(tools) = stub.tools.take() {
        tools.close().await;
    }
    outcome
}

async fn read_stdin(sender: mpsc::UnboundedSender<String>) {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if sender.send(line).is_err() {
            break;
        }
    }
}

async fn read_state(path: &Path) -> Result<State, StubError> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) => Ok(serde_json::from_str(&text)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(State::default()),
        Err(error) => Err(error.into()),
    }
}

/// One line of fold's stdin, as the stub needs it.
#[derive(Debug, PartialEq, Eq)]
enum Incoming {
    User(String),
    Interrupt(String),
    /// A control request of any other subtype, `initialize` included, which the stub accepts.
    Control(String),
    Other,
}

fn incoming(line: &str) -> Incoming {
    match serde_json::from_str::<ClaudeInput>(line) {
        Ok(ClaudeInput::User { message, .. }) => Incoming::User(message.content),
        Ok(ClaudeInput::ControlRequest {
            request_id,
            request: OutgoingRequest::Interrupt,
        }) => Incoming::Interrupt(request_id),
        Ok(ClaudeInput::ControlRequest { request_id, .. }) => Incoming::Control(request_id),
        Ok(ClaudeInput::ControlResponse { .. }) => Incoming::Other,
        Err(_) => serde_json::from_str::<Value>(line)
            .ok()
            .filter(|value| value["type"] == "control_request")
            .and_then(|value| value["request_id"].as_str().map(str::to_owned))
            .map_or(Incoming::Other, Incoming::Control),
    }
}

/// How a turn ended.
enum Ended {
    Done(String),
    Interrupted,
}

struct Stub<W> {
    root: PathBuf,
    folder: PathBuf,
    script_path: PathBuf,
    options: Options,
    state: State,
    lines: mpsc::UnboundedReceiver<String>,
    /// User turns that came while a turn ran; Claude Code queues them the same way.
    queued: VecDeque<String>,
    tools: Option<Tools>,
    /// The turns this process ran.
    turns: u32,
    /// The tool calls this process made, which number their ids.
    calls: u32,
    out: W,
}

impl<W: AsyncWrite + Unpin> Stub<W> {
    async fn serve(&mut self, lost: bool) -> Result<(), StubError> {
        // Claude Code answers a `--resume` of an unknown session at once, before it reads a line.
        if lost {
            let error = StubError::Lost(self.options.session.clone());
            self.log(&error.to_string()).await?;
            self.result(true, "error_during_execution", None).await?;
            return Err(error);
        }
        loop {
            let text = match self.queued.pop_front() {
                Some(text) => text,
                None => {
                    let Some(line) = self.lines.recv().await else {
                        return Ok(());
                    };
                    match incoming(&line) {
                        Incoming::User(text) => text,
                        Incoming::Interrupt(id) => {
                            self.control_ok(&id, json!({"still_queued": []})).await?;
                            continue;
                        }
                        Incoming::Control(id) => {
                            self.control_ok(&id, json!({})).await?;
                            continue;
                        }
                        Incoming::Other => continue,
                    }
                }
            };
            self.turns += 1;
            self.emit(&json!({
                "type": "system",
                "subtype": "init",
                "session_id": self.options.session,
                "model": self.model(),
                "cwd": self.root,
                "tools": [],
                "mcp_servers": [{"name": MCP_SERVER_NAME, "status": "connected"}],
                "permissionMode": "auto",
            }))
            .await?;
            match self.turn(&text).await {
                Ok(Ended::Done(reply)) => {
                    self.assistant(json!({"type": "text", "text": reply}))
                        .await?;
                    self.result(false, "success", Some(&reply)).await?;
                }
                Ok(Ended::Interrupted) => {
                    self.result(true, "error_during_execution", None).await?;
                }
                Err(error) => {
                    self.log(&error.to_string()).await?;
                    self.assistant(json!({"type": "text", "text": error.to_string()}))
                        .await?;
                    self.result(true, "error_during_execution", None).await?;
                    return Err(error);
                }
            }
        }
    }

    /// Runs the script's next turn when `text` is the user turn it waits for.
    async fn turn(&mut self, text: &str) -> Result<Ended, StubError> {
        let script = self.script().await?;
        let index = self.state.turn;
        let Some(turn) = script.turns.get(index).filter(|turn| {
            turn.when
                .as_deref()
                .is_none_or(|wanted| text.contains(wanted))
        }) else {
            return Ok(Ended::Done("Nothing to do.".to_owned()));
        };
        let Turn { steps, reply, .. } = turn.clone();
        self.state.turn += 1;
        self.save().await?;
        for (number, step) in steps.iter().enumerate() {
            if self.interrupted().await? {
                self.log(&format!("turn {index}: interrupted before step {number}"))
                    .await?;
                return Ok(Ended::Interrupted);
            }
            let fail = |reason: String| StubError::Step {
                turn: index,
                step: number,
                reason,
            };
            match self.step(step).await {
                Ok(Some(())) => {}
                Ok(None) => return Ok(Ended::Interrupted),
                Err(StepError::Stub(error)) => return Err(error),
                Err(StepError::Failed(reason)) => return Err(fail(reason)),
            }
            self.save().await?;
            self.log(&format!("turn {index}, step {number}: done"))
                .await?;
        }
        Ok(Ended::Done(reply.unwrap_or_else(|| "Done.".to_owned())))
    }

    async fn script(&self) -> Result<Script, StubError> {
        let error = |message: String| StubError::Script {
            path: self.script_path.clone(),
            message,
        };
        let text = tokio::fs::read_to_string(&self.script_path)
            .await
            .map_err(|source| error(source.to_string()))?;
        serde_json::from_str(&text).map_err(|source| error(source.to_string()))
    }

    /// Runs one step; `None` when an interrupt came while a call with `until` polled.
    async fn step(&mut self, step: &Step) -> Result<Option<()>, StepError> {
        let vars = &self.state.vars;
        let path = |path: &str| -> Result<PathBuf, StepError> {
            Ok(inside(&self.root, &expand(path, vars)?)?)
        };
        match step {
            Step::Call(call) => return self.call(call).await,
            Step::Write { path: file, text } => {
                let file = path(file)?;
                let text = expand(text, vars)?;
                self.tool_use("Write", json!({"file_path": file})).await?;
                write_file(&self.root, &file, |_| text).await?;
            }
            Step::Insert {
                path: file,
                after,
                text,
            } => {
                let file = path(file)?;
                let text = expand(text, vars)?;
                let after = *after;
                self.tool_use("Edit", json!({"file_path": file})).await?;
                write_file(&self.root, &file, move |current| {
                    super::insert_after(&current, after, &text)
                })
                .await?;
            }
            Step::Replace {
                path: file,
                line,
                text,
            } => {
                let file = path(file)?;
                let text = expand(text, vars)?;
                let line = *line;
                self.tool_use("Edit", json!({"file_path": file})).await?;
                write_file(&self.root, &file, move |current| {
                    super::replace_line(&current, line, &text)
                })
                .await?;
            }
            Step::Remove { path: file } => {
                let file = path(file)?;
                self.tool_use("Bash", json!({"command": "rm"})).await?;
                let root = self.root.clone();
                tokio::task::spawn_blocking(move || {
                    no_symlink_below(&root, &file)?;
                    std::fs::remove_file(&file)
                })
                .await
                .map_err(io::Error::other)
                .flatten()
                .map_err(|error| StepError::Failed(format!("cannot remove a file: {error}")))?;
            }
        }
        self.tool_result(&json!({"ok": true}), false).await?;
        Ok(Some(()))
    }

    async fn call(&mut self, call: &Call) -> Result<Option<()>, StepError> {
        let arguments =
            match expand_value(&Value::Object(call.arguments.clone()), &self.state.vars)? {
                Value::Object(arguments) => arguments,
                _ => serde_json::Map::new(),
            };
        let name = format!("mcp__{MCP_SERVER_NAME}__{}", call.tool);
        self.tool_use(&name, Value::Object(arguments.clone()))
            .await?;
        let started = Instant::now();
        let (failed, body) = loop {
            let (failed, body) = self
                .tools()
                .await?
                .call(&call.tool, arguments.clone())
                .await?;
            let pending = call
                .until
                .as_deref()
                .is_some_and(|pointer| !failed && body.pointer(pointer).is_none_or(Value::is_null));
            if !pending {
                break (failed, body);
            }
            if started.elapsed() > UNTIL_LIMIT {
                self.tool_result(&body, true).await?;
                return Err(StepError::Failed(format!(
                    "{} never gave {} in {} s",
                    call.tool,
                    call.until.as_deref().unwrap_or_default(),
                    UNTIL_LIMIT.as_secs()
                )));
            }
            if self.interrupted().await? {
                self.tool_result(&body, true).await?;
                return Ok(None);
            }
            tokio::time::sleep(UNTIL_PAUSE).await;
        };
        self.tool_result(&body, failed).await?;
        let code = body["error"]["code"].as_str().unwrap_or("unknown");
        match (&call.refused, failed) {
            (None, true) => {
                return Err(StepError::Failed(format!("{} failed: {code}", call.tool)));
            }
            (Some(wanted), true) if wanted != code => {
                return Err(StepError::Failed(format!(
                    "{} failed with {code}, not {wanted}",
                    call.tool
                )));
            }
            (Some(wanted), false) => {
                return Err(StepError::Failed(format!(
                    "{} succeeded, not refused with {wanted}",
                    call.tool
                )));
            }
            _ => {}
        }
        for (name, pointer) in &call.save {
            let value = saved(&body, &call.tool, pointer)?;
            self.state.vars.insert(name.clone(), value);
        }
        Ok(Some(()))
    }

    /// The `fold` server, started at the first call.
    async fn tools(&mut self) -> Result<&Tools, StubError> {
        if self.tools.is_none() {
            self.tools = Some(Tools::start(&self.options.mcp_config).await?);
        }
        self.tools.as_ref().ok_or(StubError::Connect)
    }

    /// Reads what came on stdin meanwhile: answers each control request, queues each user turn, and tells whether an interrupt came.
    async fn interrupted(&mut self) -> Result<bool, StubError> {
        let mut interrupted = false;
        while let Ok(line) = self.lines.try_recv() {
            match incoming(&line) {
                Incoming::User(text) => self.queued.push_back(text),
                Incoming::Interrupt(id) => {
                    self.control_ok(&id, json!({"still_queued": []})).await?;
                    interrupted = true;
                }
                Incoming::Control(id) => self.control_ok(&id, json!({})).await?,
                Incoming::Other => {}
            }
        }
        Ok(interrupted)
    }

    fn model(&self) -> &str {
        self.options.model.as_deref().unwrap_or(MODEL)
    }

    async fn emit(&mut self, value: &Value) -> Result<(), StubError> {
        let mut line = serde_json::to_string(value)?;
        line.push('\n');
        self.out.write_all(line.as_bytes()).await?;
        self.out.flush().await?;
        Ok(())
    }

    async fn control_ok(&mut self, id: &str, response: Value) -> Result<(), StubError> {
        self.emit(&json!({
            "type": "control_response",
            "response": {"subtype": "success", "request_id": id, "response": response},
        }))
        .await
    }

    async fn assistant(&mut self, block: Value) -> Result<(), StubError> {
        let session = self.options.session.clone();
        let model = self.model().to_owned();
        self.emit(&json!({
            "type": "assistant",
            "message": {
                "model": model,
                "type": "message",
                "role": "assistant",
                "content": [block],
                "usage": usage(),
            },
            "parent_tool_use_id": null,
            "session_id": session,
        }))
        .await
    }

    async fn tool_use(&mut self, name: &str, input: Value) -> Result<(), StubError> {
        self.calls += 1;
        let id = format!("stub-tool-{}", self.calls);
        self.assistant(json!({"type": "tool_use", "id": id, "name": name, "input": input}))
            .await
    }

    async fn tool_result(&mut self, body: &Value, failed: bool) -> Result<(), StubError> {
        let id = format!("stub-tool-{}", self.calls);
        let session = self.options.session.clone();
        self.emit(&json!({
            "type": "user",
            "message": {"role": "user", "content": [{
                "type": "tool_result",
                "tool_use_id": id,
                "content": body.to_string(),
                "is_error": failed,
            }]},
            "parent_tool_use_id": null,
            "session_id": session,
        }))
        .await
    }

    async fn result(
        &mut self,
        is_error: bool,
        subtype: &str,
        reply: Option<&str>,
    ) -> Result<(), StubError> {
        let session = self.options.session.clone();
        let turns = self.turns;
        self.emit(&json!({
            "type": "result",
            "subtype": subtype,
            "is_error": is_error,
            "num_turns": turns,
            "session_id": session,
            "total_cost_usd": 0,
            "duration_ms": 0,
            "usage": usage(),
            "result": reply,
        }))
        .await
    }

    async fn save(&self) -> Result<(), StubError> {
        let path = self.folder.join(STATE_FILE);
        let partial = self.folder.join(format!("{STATE_FILE}.partial"));
        tokio::fs::write(&partial, serde_json::to_vec(&self.state)?).await?;
        tokio::fs::rename(&partial, &path).await?;
        Ok(())
    }

    async fn log(&self, line: &str) -> Result<(), StubError> {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.folder.join(LOG_FILE))
            .await?;
        file.write_all(format!("{line}\n").as_bytes()).await?;
        // A tokio file hands the write to a blocking thread and returns; only `flush` waits until the line is in the file, before a reader or the exit of the process.
        file.flush().await?;
        Ok(())
    }
}

fn usage() -> Value {
    json!({
        "input_tokens": 0,
        "output_tokens": 0,
        "cache_read_input_tokens": 0,
        "cache_creation_input_tokens": 0,
    })
}

/// Why a step stopped: a failure of the stub itself, or a step the script got wrong.
enum StepError {
    Stub(StubError),
    Failed(String),
}

impl From<StubError> for StepError {
    fn from(error: StubError) -> Self {
        Self::Stub(error)
    }
}

impl From<ScriptError> for StepError {
    fn from(error: ScriptError) -> Self {
        Self::Failed(error.to_string())
    }
}

impl From<io::Error> for StepError {
    fn from(error: io::Error) -> Self {
        Self::Failed(format!("cannot change a file: {error}"))
    }
}

/// Rewrites `file` below `root` the way an agent's editor would, creating it and its folders when they are new.
async fn write_file(
    root: &Path,
    file: &Path,
    change: impl FnOnce(String) -> String + Send + 'static,
) -> io::Result<()> {
    let root = root.to_owned();
    let file = file.to_owned();
    tokio::task::spawn_blocking(move || {
        no_symlink_below(&root, &file)?;
        let current = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error),
        };
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&file, change(current))
    })
    .await
    .map_err(io::Error::other)?
}

/// Refuses `file` when a part of it below `root` that exists is a symlink, so a step never reaches a place outside the project through one. Blocking.
fn no_symlink_below(root: &Path, file: &Path) -> io::Result<()> {
    let relative = file.strip_prefix(root).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the path is outside the project",
        )
    })?;
    let mut at = root.to_path_buf();
    for part in relative.components() {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is a symlink", at.display()),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// The `fold` MCP server of the worker's configuration, started as Claude Code starts it: its command, arguments and environment, in the worker's folder.
struct Tools {
    client: RunningService<RoleClient, ()>,
    child: Child,
}

impl Tools {
    async fn start(config: &Path) -> Result<Self, StubError> {
        let text = tokio::fs::read_to_string(config).await?;
        let config: Value = serde_json::from_str(&text)?;
        let server = &config[MCP_SERVERS_KEY][MCP_SERVER_NAME];
        let command = server["command"].as_str().ok_or(StubError::McpConfig)?;
        let mut child = Command::new(command);
        if let Some(args) = server["args"].as_array() {
            child.args(args.iter().filter_map(Value::as_str));
        }
        if let Some(env) = server["env"].as_object() {
            child.envs(
                env.iter()
                    .filter_map(|(key, value)| Some((key, value.as_str()?))),
            );
        }
        let mut child = child
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(StubError::Spawn)?;
        let (Some(stdout), Some(stdin)) = (child.stdout.take(), child.stdin.take()) else {
            return Err(StubError::Connect);
        };
        let client = ().serve((stdout, stdin)).await.map_err(|_| StubError::Connect)?;
        Ok(Self { client, child })
    }

    /// Whether the call failed, and its JSON.
    async fn call(
        &self,
        tool: &str,
        arguments: serde_json::Map<String, Value>,
    ) -> Result<(bool, Value), StubError> {
        let result = self
            .client
            .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments))
            .await
            .map_err(|_| StubError::Connect)?;
        let body = result
            .content
            .first()
            .and_then(|block| block.as_text())
            .and_then(|text| serde_json::from_str(&text.text).ok())
            .unwrap_or(Value::Null);
        Ok((result.is_error == Some(true), body))
    }

    async fn close(mut self) {
        let _ = self.client.cancel().await;
        let _ = self.child.wait().await;
    }
}

#[cfg(test)]
mod tests {
    use fold_daemon::services::agents::claude::{ClaudeMessage, ContentBlock, SystemMessage};

    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split(' ').map(str::to_owned).collect()
    }

    #[test]
    fn the_options_come_from_claude_codes_flags() {
        let fresh = Options::parse(&args(
            "-p --session-id s-1 --model sonnet --mcp-config /w/mcp.json --settings /w/s.json",
        ))
        .unwrap();
        assert_eq!(
            fresh,
            Options {
                session: "s-1".to_owned(),
                resume: false,
                mcp_config: PathBuf::from("/w/mcp.json"),
                model: Some("sonnet".to_owned()),
            }
        );
        let resumed = Options::parse(&args("-p --resume s-2 --mcp-config /w/mcp.json")).unwrap();
        assert!(resumed.resume && resumed.session == "s-2" && resumed.model.is_none());
        assert!(Options::parse(&args("-p --mcp-config /w/mcp.json")).is_err());
        assert!(Options::parse(&args("-p --session-id s")).is_err());
    }

    #[test]
    fn fold_s_own_input_lines_are_read_as_fold_writes_them() {
        let user = ClaudeInput::user("work on T-1").to_line().unwrap();
        assert_eq!(incoming(&user), Incoming::User("work on T-1".to_owned()));
        let interrupt = ClaudeInput::ControlRequest {
            request_id: "fold-2".to_owned(),
            request: OutgoingRequest::Interrupt,
        };
        assert_eq!(
            incoming(&interrupt.to_line().unwrap()),
            Incoming::Interrupt("fold-2".to_owned())
        );
        let initialize = ClaudeInput::ControlRequest {
            request_id: "fold-1".to_owned(),
            request: OutgoingRequest::Initialize { hooks: None },
        };
        assert_eq!(
            incoming(&initialize.to_line().unwrap()),
            Incoming::Control("fold-1".to_owned())
        );
        assert_eq!(
            incoming(
                r#"{"type":"control_request","request_id":"x","request":{"subtype":"set_model"}}"#
            ),
            Incoming::Control("x".to_owned())
        );
        assert_eq!(incoming("not json"), Incoming::Other);
    }

    /// A stub in `dir` with `script` and the stdin `lines`, which writes into a buffer instead of stdout.
    fn stub(dir: &Path, script: &Value, resume: bool, lines: &[String]) -> Stub<Vec<u8>> {
        let script_path = dir.join("script.json");
        std::fs::write(&script_path, script.to_string()).unwrap();
        let (sender, receiver) = mpsc::unbounded_channel();
        for line in lines {
            sender.send(line.trim_end().to_owned()).unwrap();
        }
        Stub {
            root: dir.to_owned(),
            folder: dir.to_owned(),
            script_path,
            options: Options {
                session: "s-1".to_owned(),
                resume,
                mcp_config: dir.join("mcp.json"),
                model: None,
            },
            state: State {
                sessions: vec!["s-1".to_owned()],
                ..State::default()
            },
            lines: receiver,
            queued: VecDeque::new(),
            tools: None,
            turns: 0,
            calls: 0,
            out: Vec::new(),
        }
    }

    fn output(stub: &Stub<Vec<u8>>) -> Vec<ClaudeMessage> {
        String::from_utf8(stub.out.clone())
            .unwrap()
            .lines()
            .map(|line| ClaudeMessage::parse(line).unwrap())
            .collect()
    }

    fn user(text: &str) -> String {
        ClaudeInput::user(text).to_line().unwrap()
    }

    fn control(id: &str, request: OutgoingRequest) -> String {
        ClaudeInput::ControlRequest {
            request_id: id.to_owned(),
            request,
        }
        .to_line()
        .unwrap()
    }

    #[tokio::test]
    async fn a_turn_edits_files_and_every_line_is_one_the_supervisor_reads() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("old.txt"), "x").unwrap();
        let script = json!({"turns": [
            {"steps": [
                {"write": {"path": "src/a.txt", "text": "one\nthree\n"}},
                {"insert": {"path": "src/a.txt", "after": 1, "text": "two"}},
                {"replace": {"path": "src/a.txt", "line": 3, "text": "3"}},
                {"remove": {"path": "old.txt"}}
            ], "reply": "Edited."},
            {"when": "changes requested", "steps": [], "reply": "Fixed."}
        ]});
        let lines = [
            control("fold-1", OutgoingRequest::Initialize { hooks: None }),
            user("Work on T-1."),
            user("A note."),
            user("Review: changes requested."),
            user("Anything else?"),
        ];
        let mut stub = stub(dir.path(), &script, false, &lines);
        stub.serve(false).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/a.txt")).unwrap(),
            "one\ntwo\n3\n"
        );
        assert!(!dir.path().join("old.txt").exists());
        assert_eq!(stub.state.turn, 2);

        let messages = output(&stub);
        assert!(matches!(&messages[0], ClaudeMessage::ControlResponse(_)));
        let results: Vec<(bool, Option<String>)> = messages
            .iter()
            .filter_map(|message| match message {
                ClaudeMessage::Result(result) => Some((result.is_error, result.result.clone())),
                _ => None,
            })
            .collect();
        let replies = ["Edited.", "Nothing to do.", "Fixed.", "Nothing to do."];
        assert_eq!(
            results,
            replies
                .map(|reply| (false, Some(reply.to_owned())))
                .to_vec()
        );
        let inits = messages
            .iter()
            .filter(|message| matches!(message, ClaudeMessage::System(SystemMessage::Init(init)) if init.session_id == "s-1"))
            .count();
        assert_eq!(inits, 4);
        let tools: Vec<String> = messages
            .iter()
            .filter_map(|message| match message {
                ClaudeMessage::Assistant(assistant) => match assistant.message.content.first() {
                    Some(ContentBlock::ToolUse { name, .. }) => Some(name.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(tools, ["Write", "Edit", "Edit", "Bash"]);
    }

    #[tokio::test]
    async fn a_failed_step_ends_the_turn_with_an_error_and_stops_the_stub() {
        let dir = tempfile::tempdir().unwrap();
        let script = json!({"turns": [{"steps": [{"write": {"path": "../out.txt", "text": ""}}]}]});
        let mut stub = stub(dir.path(), &script, false, &[user("Go.")]);
        let error = stub.serve(false).await.unwrap_err();
        assert!(
            matches!(
                error,
                StubError::Step {
                    turn: 0,
                    step: 0,
                    ..
                }
            ),
            "{error}"
        );
        let messages = output(&stub);
        assert!(matches!(messages.last(), Some(ClaudeMessage::Result(result)) if result.is_error));
        let log = std::fs::read_to_string(dir.path().join(LOG_FILE)).unwrap();
        assert!(log.contains("turn 0, step 0"), "{log}");
        assert!(!dir.path().parent().unwrap().join("out.txt").exists());
    }

    #[tokio::test]
    async fn a_resume_of_an_unknown_session_runs_no_turn() {
        let dir = tempfile::tempdir().unwrap();
        let script = json!({"turns": [{"steps": [{"write": {"path": "a.txt", "text": ""}}]}]});
        let mut stub = stub(dir.path(), &script, true, &[]);
        let error = stub.serve(true).await.unwrap_err();
        assert!(matches!(error, StubError::Lost(_)));
        assert!(!dir.path().join("a.txt").exists());
        let messages = output(&stub);
        assert!(
            matches!(&messages[..], [ClaudeMessage::Result(result)] if result.is_error && result.num_turns == 0)
        );
        let log = std::fs::read_to_string(dir.path().join(LOG_FILE)).unwrap();
        assert!(log.contains("no session s-1 to resume"), "{log}");
    }

    #[tokio::test]
    async fn an_interrupt_ends_the_turn_before_its_next_step_and_a_missing_script_fails() {
        let dir = tempfile::tempdir().unwrap();
        let script = json!({"turns": [{"steps": [{"write": {"path": "a.txt", "text": ""}}]}]});
        let lines = [user("Go."), control("fold-3", OutgoingRequest::Interrupt)];
        let mut stub = stub(dir.path(), &script, false, &lines);
        stub.serve(false).await.unwrap();
        assert!(!dir.path().join("a.txt").exists());
        assert_eq!(stub.state.turn, 1);
        let messages = output(&stub);
        assert!(matches!(messages.last(), Some(ClaudeMessage::Result(result)) if result.is_error));

        let mut again = self::stub(dir.path(), &script, false, &[user("Go.")]);
        std::fs::remove_file(&again.script_path).unwrap();
        let error = again.serve(false).await.unwrap_err();
        assert!(matches!(error, StubError::Script { .. }), "{error}");
    }

    #[tokio::test]
    async fn a_step_never_goes_through_a_symlink() {
        let outer = tempfile::tempdir().unwrap();
        let dir = outer.path().join("project");
        let elsewhere = outer.path().join("elsewhere");
        std::fs::create_dir(&dir).unwrap();
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("kept.txt"), "kept\n").unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir.join("link")).unwrap();
        for step in [
            json!({"write": {"path": "link/new.txt", "text": "x\n"}}),
            json!({"insert": {"path": "link/kept.txt", "after": 1, "text": "x"}}),
            json!({"replace": {"path": "link/kept.txt", "line": 1, "text": "x"}}),
            json!({"remove": {"path": "link/kept.txt"}}),
        ] {
            let script = json!({"turns": [{"steps": [step]}]});
            let mut stub = stub(&dir, &script, false, &[user("Go.")]);
            let error = stub.serve(false).await.unwrap_err();
            assert!(
                matches!(&error, StubError::Step { reason, .. } if reason.contains("symlink")),
                "{error}"
            );
        }
        assert!(!elsewhere.join("new.txt").exists());
        assert_eq!(
            std::fs::read_to_string(elsewhere.join("kept.txt")).unwrap(),
            "kept\n"
        );
    }
}
