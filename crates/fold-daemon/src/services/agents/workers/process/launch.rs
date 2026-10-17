use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fold_core::config::RoleSettings;
use fold_core::events::types;
use fold_core::layout::{AGENT_ENV, AGENT_TAGS_ENV, MCP_COMMAND, MCP_SERVER_NAME, MCP_SERVERS_KEY};
use fold_core::roles::area_prompt;
use fold_core::{AgentName, AreaName, EventActor, NewEvent, ProjectPaths, WorkerState, codes};
use fold_store::StoreError;
use fold_store::agents::{self, WorkerProcessRow};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{ChildStderr, ChildStdin, Command};
use tokio::sync::{mpsc, watch};

use super::{Input, Launcher, Process, Running, StartWatch, Supervisor, lock};
use crate::push::StdinSink;
use crate::services::agents::AgentError;
use crate::services::agents::claude::{ClaudeInput, OutgoingRequest};
use crate::services::events::EventBuffer;
use crate::services::projects::load_config;

/// Session-specific variables of a Claude Code session that started the daemon; a worker is a session of its own.
const SESSION_ENV: [&str; 5] = [
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SSE_PORT",
];

impl Supervisor {
    /// Runs Claude Code for the row, which a committed command left in state `starting`: a new session, or `--resume` of the row's session. `turn` is the first user turn; without one a resumed worker waits idle for its next notice. A process that cannot start, for an unreadable configuration, files that cannot be written, a failed spawn or a process that cannot be recorded, leaves the row `failed` with `worker.failed` and gives no [`StartWatch`], so no `starting` row holds a place under the limits.
    pub(in crate::services::agents::workers) async fn launch(
        &self,
        row: &WorkerProcessRow,
        resume: bool,
        turn: Option<String>,
    ) -> Result<Option<StartWatch>, AgentError> {
        let config = match load_config(&self.inner.paths).await {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!(agent = %row.agent, code = error.code().unwrap_or(codes::INTERNAL_ERROR), "could not read the configuration for a worker");
                self.fail(row.id, &row.agent, "config").await?;
                return Ok(None);
            }
        };
        let mut settings = config.role(row.role);
        let mut area_instructions = None;
        if let Some(area) = &row.area {
            match agents::find_area(self.inner.store.read(), area).await {
                Ok(Some(area)) => {
                    settings = settings.with_area(&area.profile);
                    area_instructions = area.profile.instructions.map(|text| (area.name, text));
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(agent = %row.agent, code = error.log_code(), "could not read the area profile for a worker");
                    self.fail(row.id, &row.agent, "config").await?;
                    return Ok(None);
                }
            }
        }
        let launcher = lock(&self.inner.launcher).clone();
        let files = write_files(
            &self.inner.paths,
            &launcher,
            row,
            &settings,
            area_instructions,
        )
        .await;
        let (mcp, prompt) = match files {
            Ok(files) => files,
            Err(_) => {
                tracing::warn!(agent = %row.agent, "could not write the worker's files");
                self.fail(row.id, &row.agent, "files").await?;
                return Ok(None);
            }
        };
        let mut command = Command::new(&launcher.claude);
        command.args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
        ]);
        if resume {
            command.arg("--resume").arg(&row.session_id);
        } else {
            command.arg("--session-id").arg(&row.session_id);
        }
        if let Some(model) = &row.model {
            command.arg("--model").arg(model);
        }
        command
            .args([
                "--permission-mode",
                "auto",
                "--permission-prompt-tool",
                "stdio",
            ])
            .arg("--mcp-config")
            .arg(&mcp);
        if settings.mcp.is_some() {
            command.arg("--strict-mcp-config");
        }
        command.arg("--append-system-prompt-file").arg(&prompt);
        for name in SESSION_ENV {
            command.env_remove(name);
        }
        let ttl = settings.cache_ttl.as_str();
        command
            .env(AGENT_ENV, row.agent.as_str())
            .env("CLAUDE_CODE_PROMPT_CACHE_TTL", ttl)
            .env("CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL", ttl)
            .envs(launcher.env.iter().map(|(key, value)| (key, value)))
            // A daemon that `fld claude`'s session started inherits its `main` tag request, which a worker never makes.
            .env_remove(AGENT_TAGS_ENV)
            .current_dir(self.inner.paths.root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // The worker leads a process group of its own, so the signals of a stop reach the commands its tools started too.
            .process_group(0)
            .kill_on_drop(true);
        #[cfg(feature = "failpoints")]
        fail::fail_point!("worker_launch_before_spawn");
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => {
                tracing::warn!(agent = %row.agent, "could not start Claude Code for a worker");
                self.fail(row.id, &row.agent, "spawn").await?;
                return Ok(None);
            }
        };
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            self.fail(row.id, &row.agent, "spawn").await?;
            return Ok(None);
        };
        let pid = child.id();
        let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (input, inputs) = mpsc::unbounded_channel();
        let (exit, exited) = watch::channel(false);
        let detached = Arc::new(AtomicBool::new(false));
        let reaped = Arc::new(AtomicBool::new(false));
        let (session, sessions) = watch::channel(None);
        let process = Process {
            id: row.id,
            agent: row.agent.clone(),
            base_cost: row.cost_micro_usd,
            generation,
            resumed: resume,
            session_id: row.session_id.clone(),
            session: Arc::new(session),
        };
        let start = StartWatch {
            session: sessions,
            exited: exited.clone(),
        };
        let run = Running {
            generation,
            input: input.clone(),
            exited,
            pid,
            detached: Arc::clone(&detached),
            reaped: Arc::clone(&reaped),
        };
        lock(&self.inner.running).insert(row.agent.clone(), run.clone());
        // A stop or a pause that came after the start committed but before the process was in `running` found no process to end. The row is read only now that the process is there, so either that stop or pause sees the process or this check sees it.
        let idle = turn.is_none();
        let now = self.inner.clock.now();
        let current = self
            .inner
            .store
            .write(async |tx| {
                let starting = agents::find_worker_process(tx.conn(), &row.agent)
                    .await?
                    .is_some_and(|current| {
                        current.id == row.id && current.state == WorkerState::Starting
                    });
                let paused = agents::pause_state(tx.conn()).await?.is_paused();
                if starting && !paused {
                    agents::set_worker_pid(tx, row.id, pid).await?;
                    if idle {
                        agents::set_worker_state(tx, row.id, WorkerState::Idle, now).await?;
                    }
                }
                Ok::<_, StoreError>((starting, paused))
            })
            .await;
        let (starting, paused) = match current {
            Ok(found) => found,
            Err(error) => {
                self.forget(&process);
                tracing::warn!(agent = %row.agent, code = error.log_code(), "could not record a worker's process");
                // Dropping the child on return kills the process.
                self.fail(row.id, &row.agent, "record").await?;
                return Ok(None);
            }
        };
        tokio::spawn(drain(stderr));
        // A paused project keeps the row `starting` for the resume, as a daemon stop keeps a running row.
        if paused {
            detached.store(true, Ordering::SeqCst);
        }
        if !starting || paused {
            tokio::spawn(self.clone().feed(stdin, inputs, process.clone()));
            tokio::spawn(
                self.clone()
                    .drive(child, stdout, process, exit, detached, reaped),
            );
            let supervisor = self.clone();
            tokio::spawn(async move { supervisor.terminate(&run, false).await });
            return Ok(None);
        }
        let initialize = ClaudeInput::ControlRequest {
            request_id: self.request_id(),
            request: OutgoingRequest::Initialize { hooks: None },
        };
        if let Ok(line) = initialize.to_line() {
            let _ = input.send(Input::Line(line));
        }
        if let Some(turn) = turn {
            let _ = input.send(Input::Turn(turn));
        }
        let sink_input = input.clone();
        let sink: StdinSink =
            Arc::new(move |text: &str| sink_input.send(Input::Turn(text.to_owned())).is_ok());
        self.inner.push.register_stdin(&row.agent, sink);
        tokio::spawn(self.clone().feed(stdin, inputs, process.clone()));
        tokio::spawn(
            self.clone()
                .drive(child, stdout, process, exit, detached, reaped),
        );
        Ok(Some(start))
    }

    /// Records a worker whose process could not start. A row that another start replaced, or that has ended already, is left as it is.
    pub(super) async fn fail(
        &self,
        id: i64,
        agent: &AgentName,
        reason: &str,
    ) -> Result<(), AgentError> {
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        self.inner
            .store
            .write(async |tx| {
                let current = agents::find_worker_process(tx.conn(), agent).await?;
                if !current.is_some_and(|current| current.id == id && current.state.is_running()) {
                    return Ok(());
                }
                agents::record_exit(tx, id, WorkerState::Failed, None, now).await?;
                agents::close_open_questions(tx, id, now).await?;
                let event = NewEvent::new(types::WORKER_FAILED, EventActor::fold())?
                    .data(json!({"agent": agent.as_str(), "reason": reason}))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, AgentError>(())
            })
            .await?;
        self.inner.bus.publish(buffer);
        Ok(())
    }

    /// Writes the lines of the input channel to the process's stdin until the channel closes or a `Close` arrives, which closes stdin so Claude Code exits after its turn. Each user turn makes the worker `working`.
    async fn feed(
        self,
        mut stdin: ChildStdin,
        mut inputs: mpsc::UnboundedReceiver<Input>,
        process: Process,
    ) {
        while let Some(input) = inputs.recv().await {
            let (line, turn) = match input {
                Input::Line(line) => (line, false),
                Input::Turn(text) => match ClaudeInput::user(text).to_line() {
                    Ok(line) => (line, true),
                    Err(_) => continue,
                },
                Input::Close => break,
            };
            if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                break;
            }
            if turn {
                let marked = self
                    .inner
                    .store
                    .write(async |tx| agents::mark_worker_working(tx, process.id).await)
                    .await;
                if let Err(error) = marked {
                    tracing::warn!(agent = %process.agent, code = error.log_code(), "could not mark a worker working");
                }
            }
        }
    }
}

/// Reads and drops the process's stderr, so a full pipe never blocks it. Its text never reaches a log.
async fn drain(mut stderr: ChildStderr) {
    let mut buffer = [0_u8; 4096];
    while let Ok(read) = stderr.read(&mut buffer).await {
        if read == 0 {
            break;
        }
    }
}

/// Writes the worker's MCP configuration and its prompt under the worker's folder. The prompt is the role's text, shipped or the project's override, then the shared rules, then the area's instructions when there are any; the override file itself is never passed, because it lacks the shared rules. Returns the two paths Claude Code reads.
async fn write_files(
    paths: &ProjectPaths,
    launcher: &Launcher,
    row: &WorkerProcessRow,
    settings: &RoleSettings,
    area_instructions: Option<(AreaName, String)>,
) -> std::io::Result<(PathBuf, PathBuf)> {
    let dir = paths.worker_dir(&row.agent);
    let fold_dir = paths.fold_dir();
    let override_prompt = paths.role_prompt(row.role);
    let project_mcp = paths.mcp_json();
    let fold = match &launcher.fold {
        Some(fold) => fold.clone(),
        None => std::env::current_exe().unwrap_or_else(|_| PathBuf::from("fld")),
    };
    let agent = row.agent.clone();
    let prompt_text = row.role.prompt();
    let limit = settings.mcp.clone();
    tokio::task::spawn_blocking(move || {
        fold_vcs::platform::create_dir_below(&fold_dir, &dir)?;
        let config = mcp_config(&fold, &agent, limit.as_deref(), &project_mcp);
        let mcp = dir.join("mcp.json");
        std::fs::write(&mcp, config.to_string())?;
        let role_prompt = if override_prompt.is_file() {
            fold_core::with_shared_rules(&std::fs::read_to_string(&override_prompt)?)
        } else {
            prompt_text.to_owned()
        };
        let prompt = match area_instructions {
            None => {
                let prompt = dir.join("prompt.md");
                std::fs::write(&prompt, role_prompt)?;
                prompt
            }
            Some((area, instructions)) => {
                let prompt = dir.join("prompt.md");
                std::fs::write(&prompt, area_prompt(&role_prompt, &area, &instructions))?;
                prompt
            }
        };
        Ok((mcp, prompt))
    })
    .await
    .map_err(std::io::Error::other)?
}

/// The `fold` server with the worker's agent name, plus, when the role limits its servers, each named server of the project's `.mcp.json`. Servers of the user's own configuration are never copied, because their entries can hold credentials; without a limit Claude Code loads them itself.
fn mcp_config(
    fold: &Path,
    agent: &AgentName,
    limit: Option<&[String]>,
    project_mcp: &Path,
) -> Value {
    let mut servers = Map::new();
    servers.insert(
        MCP_SERVER_NAME.to_owned(),
        json!({
            "command": fold,
            "args": [MCP_COMMAND],
            "env": {(AGENT_ENV): agent.as_str()},
        }),
    );
    if let Some(names) = limit {
        let project: Value = std::fs::read_to_string(project_mcp)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null);
        for name in names {
            if servers.contains_key(name) {
                continue;
            }
            if let Some(entry) = project
                .get(MCP_SERVERS_KEY)
                .and_then(|servers| servers.get(name))
            {
                servers.insert(name.clone(), entry.clone());
            }
        }
    }
    json!({ MCP_SERVERS_KEY: servers })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_limited_config_copies_only_named_project_servers() {
        let temp = tempfile::TempDir::new().unwrap();
        let dir = temp.path();
        let project = dir.join(".mcp.json");
        std::fs::write(
            &project,
            r#"{"mcpServers":{"pencil":{"command":"pencil"},"other":{"command":"x"},"fold":{"command":"old"}}}"#,
        )
        .unwrap();
        let agent = AgentName::parse("web-lead").unwrap();
        let fold = Path::new("/bin/fld");
        let open = mcp_config(fold, &agent, None, &project);
        assert_eq!(
            open,
            json!({"mcpServers": {"fold": {"command": "/bin/fld", "args": ["mcp"], "env": {"FOLD_AGENT": "web-lead"}}}})
        );
        let names = ["fold".to_owned(), "pencil".to_owned(), "missing".to_owned()];
        let limited = mcp_config(fold, &agent, Some(&names), &project);
        let servers = limited["mcpServers"].as_object().unwrap();
        assert_eq!(servers.keys().collect::<Vec<_>>(), ["fold", "pencil"]);
        assert_eq!(servers["fold"]["command"], "/bin/fld");
        let none = mcp_config(fold, &agent, Some(&names), &dir.join("absent.json"));
        assert_eq!(none["mcpServers"].as_object().unwrap().len(), 1);
    }
}
