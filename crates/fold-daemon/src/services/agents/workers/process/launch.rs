use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fold_core::config::RoleSettings;
use fold_core::events::types;
use fold_core::layout::{
    AGENT_ENV, AGENT_TAGS_ENV, MCP_COMMAND, MCP_SERVER_NAME, MCP_SERVERS_KEY, session_settings,
};
use fold_core::roles::{FLD_MISSING, area_prompt, needs_restart};
use fold_core::{AgentName, AreaName, EventActor, NewEvent, ProjectPaths, WorkerState, codes};
use fold_store::StoreError;
use fold_store::agents::{self, ProcessEnd, WorkerProcessRow};
use fold_store::tasks::claims::{self, ClaimTarget, ReleaseReason};
use serde_json::{Map, Value, json};
use tokio::io::AsyncWriteExt;
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, watch};

use super::super::output::real_model;
use super::stderr::StderrTail;
use super::{Input, Launcher, Process, Running, StartWatch, Supervisor, lock, own_binary};
use crate::push::StdinSink;
use crate::services::agents::AgentError;
use crate::services::agents::claude::{ClaudeInput, OutgoingRequest};
use crate::services::events::EventBuffer;
use crate::services::projects::load_config;
use crate::support::SESSION_ENV;

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
        let WorkerFiles {
            mcp,
            settings: own_settings,
            prompt,
            program,
            program_args,
        } = match files {
            Ok(files) => files,
            Err(FilesError::FldMissing) => {
                tracing::warn!(agent = %row.agent, "the fld binary of the worker's MCP server is not there");
                self.fail(row.id, &row.agent, FLD_MISSING).await?;
                return Ok(None);
            }
            Err(FilesError::Io) => {
                tracing::warn!(agent = %row.agent, "could not write the worker's files");
                self.fail(row.id, &row.agent, "files").await?;
                return Ok(None);
            }
        };
        let mut command = Command::new(&program);
        command.args(program_args).args([
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
        // A row can hold a model Claude Code made up, which no start asked for; the area's or role's model stands in for it.
        let model = row
            .model
            .as_deref()
            .and_then(real_model)
            .or_else(|| row.model.as_ref().and(settings.model.as_deref()));
        if let Some(model) = model {
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
            .arg(&mcp)
            .arg("--settings")
            .arg(&own_settings);
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
            pid,
            resumed: resume,
            session_id: row.session_id.clone(),
            session: Arc::new(session),
            stderr: StderrTail::read(stderr),
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
                    agents::forget_made_up_model(tx, row.id).await?;
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

    /// Records a worker whose process could not start. A row that another start replaced, or that has ended already, is left as it is. A worker that failed before its first turn gives its claims back, unless it failed because it cannot reach fold: then it keeps them, as in a pause, for the daemon start that resumes it.
    pub(super) async fn fail(
        &self,
        id: i64,
        agent: &AgentName,
        reason: &str,
    ) -> Result<(), AgentError> {
        self.fail_when(id, agent, reason, |_| true)
            .await
            .map(|_| ())
    }

    /// [`Supervisor::fail`] of a row that `still` holds for, read in the transaction that ends it. Returns whether it ended the row.
    pub(super) async fn fail_when(
        &self,
        id: i64,
        agent: &AgentName,
        reason: &str,
        still: impl Fn(&WorkerProcessRow) -> bool,
    ) -> Result<bool, AgentError> {
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        let failed = self
            .inner
            .store
            .write(async |tx| {
                let current = agents::find_worker_process(tx.conn(), agent).await?;
                let Some(current) = current.filter(|current| {
                    current.id == id && current.state.is_running() && still(current)
                }) else {
                    return Ok(false);
                };
                agents::record_exit(tx, id, ProcessEnd::Failed(reason), None, now).await?;
                agents::close_open_questions(tx, id, now).await?;
                let event = NewEvent::new(types::WORKER_FAILED, EventActor::fold())?
                    .data(json!({"agent": agent.as_str(), "reason": reason}))?;
                buffer.append(tx, &event, now).await?;
                if current.turns == 0 && !needs_restart(reason) {
                    give_back_claims(tx, &mut buffer, &current, now).await?;
                }
                Ok::<_, AgentError>(true)
            })
            .await?;
        self.inner.bus.publish(buffer);
        Ok(failed)
    }

    /// Whether the fld binary that a worker's MCP configuration starts is there.
    pub(super) async fn fld_present(&self) -> bool {
        let fold = lock(&self.inner.launcher).fold.clone();
        tokio::task::spawn_blocking(move || {
            fold.or_else(own_binary).is_some_and(|fold| fold.is_file())
        })
        .await
        .unwrap_or(false)
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

/// Why a worker's files were not written.
enum FilesError {
    /// The fld binary that the MCP configuration would start is not a file, so the worker would run without the fold tools.
    FldMissing,
    /// A file could not be written; its error never reaches a log, because it can name a path of the user's.
    Io,
}

impl From<std::io::Error> for FilesError {
    fn from(_: std::io::Error) -> Self {
        Self::Io
    }
}

/// The files a worker process reads and the program that reads them.
struct WorkerFiles {
    mcp: PathBuf,
    settings: PathBuf,
    prompt: PathBuf,
    program: PathBuf,
    /// The arguments before Claude Code's own: the subcommand of fld's stand-in, else none.
    program_args: &'static [&'static str],
}

/// Writes the worker's MCP configuration, its settings and its prompt under the worker's folder. The settings allow the fold tools and run fold's hooks, so a worker never depends on the user's own Claude Code settings. The prompt is the role's text, shipped or the project's override, then the shared rules, then the area's instructions when there are any; the override file itself is never passed, because it lacks the shared rules. Returns the three paths Claude Code reads, with the program to run. An fld path that names no file writes nothing.
async fn write_files(
    paths: &ProjectPaths,
    launcher: &Launcher,
    row: &WorkerProcessRow,
    settings: &RoleSettings,
    area_instructions: Option<(AreaName, String)>,
) -> Result<WorkerFiles, FilesError> {
    let dir = paths.worker_dir(&row.agent);
    let fold_dir = paths.fold_dir();
    let override_prompt = paths.role_prompt(row.role);
    let project_mcp = paths.mcp_json();
    let fold = launcher.fold.clone();
    let agent = row.agent.clone();
    let prompt_text = row.role.prompt();
    let limit = settings.mcp.clone();
    let claude = launcher.claude.clone();
    tokio::task::spawn_blocking(move || {
        let Some(fold) = fold.or_else(own_binary).filter(|fold| fold.is_file()) else {
            return Err(FilesError::FldMissing);
        };
        let (program, program_args) = worker_program(&fold_dir, claude, &fold);
        fold_vcs::platform::create_dir_below(&fold_dir, &dir)?;
        let config = mcp_config(&fold, &agent, limit.as_deref(), &project_mcp);
        let mcp = dir.join("mcp.json");
        std::fs::write(&mcp, config.to_string())?;
        let own_settings = dir.join("settings.json");
        std::fs::write(&own_settings, session_settings().to_string())?;
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
        Ok(WorkerFiles {
            mcp,
            settings: own_settings,
            prompt,
            program,
            program_args,
        })
    })
    .await
    .map_err(|_| FilesError::Io)?
}

/// Claude Code, or, in a build for development whose project holds [`super::DEV_STUB_DIR`] as a real folder, never a symlink, the fld binary of the worker's MCP server as `fld dev stub`, which runs the agent's script from that folder through the same tools. Blocking.
#[cfg(feature = "dev-launcher")]
fn worker_program(
    fold_dir: &Path,
    claude: PathBuf,
    fold: &Path,
) -> (PathBuf, &'static [&'static str]) {
    let stub_dir = std::fs::symlink_metadata(fold_dir.join(super::DEV_STUB_DIR));
    if stub_dir.is_ok_and(|meta| meta.is_dir()) {
        (fold.to_owned(), &super::DEV_STUB_COMMAND)
    } else {
        (claude, &[])
    }
}

#[cfg(not(feature = "dev-launcher"))]
fn worker_program(
    _fold_dir: &Path,
    claude: PathBuf,
    _fold: &Path,
) -> (PathBuf, &'static [&'static str]) {
    (claude, &[])
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

/// Releases every claim of a worker that failed before its first turn, which its lead gave it at its start, and closes its open batches inside the caller's write, so the lead can give the work to another worker at once.
async fn give_back_claims(
    tx: &mut fold_store::WriteTx,
    buffer: &mut EventBuffer,
    row: &WorkerProcessRow,
    now: fold_core::Timestamp,
) -> Result<(), AgentError> {
    let by = EventActor::fold();
    let reason = ReleaseReason::WorkerFailed;
    crate::services::reviews::close_batches_of(tx, buffer, &by, &row.agent, now).await?;
    let mut tasks = Vec::new();
    for held in claims::unreleased_of_agent(tx.conn(), row.agent_id).await? {
        let event = match held.target {
            ClaimTarget::Task(task) => {
                claims::release(tx, task, held.worker_id, reason, now).await?;
                tasks.push(task);
                NewEvent::new(types::CLAIM_RELEASED, by.clone())?
                    .task(task)
                    .data(json!({
                        "kind": "implementation",
                        "worker": held.worker.as_str(),
                        "reason": reason.as_str(),
                    }))?
            }
            ClaimTarget::Review { proposal, task } => {
                fold_store::reviews::release_review_claim(tx, proposal, reason.as_str(), now)
                    .await?;
                NewEvent::new(types::CLAIM_RELEASED, by.clone())?
                    .task(task)
                    .proposal(proposal)
                    .data(json!({
                        "kind": "review",
                        "proposal": proposal.to_string(),
                        "worker": held.worker.as_str(),
                        "reason": reason.as_str(),
                    }))?
            }
        };
        buffer.append(tx, &event, now).await?;
    }
    buffer.extend(
        crate::services::tasks::recompute(tx, &tasks, now)
            .await?
            .events,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "dev-launcher")]
    #[test]
    fn a_project_with_the_stub_folder_runs_the_stub_of_its_fld() {
        let temp = tempfile::TempDir::new().unwrap();
        let fold_dir = temp.path().join(".fold");
        std::fs::create_dir(&fold_dir).unwrap();
        let fold = Path::new("/bin/fld");
        let claude = PathBuf::from("claude");
        assert_eq!(
            worker_program(&fold_dir, claude.clone(), fold),
            (claude.clone(), &[][..])
        );
        let elsewhere = temp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let stub_dir = fold_dir.join(super::super::DEV_STUB_DIR);
        std::os::unix::fs::symlink(&elsewhere, &stub_dir).unwrap();
        assert_eq!(
            worker_program(&fold_dir, claude.clone(), fold),
            (claude.clone(), &[][..])
        );
        std::fs::remove_file(&stub_dir).unwrap();
        std::fs::create_dir(&stub_dir).unwrap();
        assert_eq!(
            worker_program(&fold_dir, claude, fold),
            (PathBuf::from("/bin/fld"), &["dev", "stub"][..])
        );
    }

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
