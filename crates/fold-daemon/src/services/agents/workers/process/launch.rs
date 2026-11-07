use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use fold_core::config::RoleSettings;
use fold_core::events::types;
use fold_core::layout::{
    AGENT_ENV, AGENT_TAGS_ENV, LOCAL_SETTINGS_FILE, MAINTAIN_DIR_ENV, MCP_COMMAND, MCP_SERVER_NAME,
    MCP_SERVERS_KEY, SKILLS_DIR, START_DIR_ENV, WS_DIR, fold_skills, session_settings,
    worker_settings,
};
use fold_core::roles::{
    CLAUDE_NOT_FOUND, FLD_MISSING, area_prompt, needs_restart, needs_update_note, with_update_note,
};
use fold_core::{
    AgentName, AreaName, EventActor, NewEvent, ProjectPaths, Role, WorkerState, codes,
};
use fold_store::StoreError;
use fold_store::agents::{self, ProcessEnd, WorkerProcessRow};
use fold_store::tasks::claims::{self, ClaimTarget, ReleaseReason};
use serde_json::{Map, Value, json};
use tokio::io::AsyncWriteExt;
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, watch};

use super::super::output::real_model;
use super::project_claude;
use super::stderr::StderrTail;
use super::{Input, Launcher, Process, Running, StartWatch, Supervisor, lock, own_binary};
use crate::push::StdinSink;
use crate::services::agents::AgentError;
use crate::services::agents::claude::{ClaudeInput, OutgoingRequest};
use crate::services::agents::workers::code_of;
use crate::services::events::EventBuffer;
use crate::services::projects::load_config;
use crate::support::SESSION_ENV;

impl Supervisor {
    /// Runs Claude Code for the row, which a committed command left in state `starting`: a new session, or `--resume` of the row's session. `turn` is the first user turn; without one a resumed worker waits idle for its next notice. A new session records the installed fld's fingerprint of the role's skill text, also when a pause ends its start; a resumed one whose recorded fingerprint differs gets the update note before its first turn, whichever turn that is, and records the new fingerprint once that turn is written. A process that cannot start, for an unreadable configuration, files that cannot be written, a failed spawn or a process that cannot be recorded, leaves the row `failed` with `worker.failed` and gives no [`StartWatch`], so no `starting` row holds a place under the limits.
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
        // Held until the process is in place, so no interactive main session binds meanwhile.
        let _main_start = if row.role == Role::Main {
            match self.claim_main_start(&config) {
                Ok(start) => Some(start),
                Err(refusal) => {
                    tracing::warn!(agent = %row.agent, code = code_of(&refusal), "the supervised main may not run now");
                    self.fail(row.id, &row.agent, code_of(&refusal)).await?;
                    return Ok(None);
                }
            }
        } else {
            None
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
        let fingerprint = launcher.fingerprints.get(&row.role).cloned();
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
            start,
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
        if start.is_some() {
            command
                .arg("--add-dir")
                .arg(self.inner.paths.root().join(WS_DIR));
        }
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
            .env_remove(AGENT_TAGS_ENV);
        match &start {
            Some(start) => command
                .env(START_DIR_ENV, start)
                .env(MAINTAIN_DIR_ENV, "1")
                .current_dir(start),
            None => command
                .env_remove(START_DIR_ENV)
                .current_dir(self.inner.paths.root()),
        };
        command
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
            Err(error) if claude_missing(&error, &program, &launcher) => {
                tracing::warn!(agent = %row.agent, "found no claude on the PATH the daemon gives workers");
                self.fail(row.id, &row.agent, CLAUDE_NOT_FOUND).await?;
                return Ok(None);
            }
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
        let open_turns = Arc::new(AtomicU64::new(0));
        let (session, sessions) = watch::channel(None);
        let process = Process {
            id: row.id,
            agent: row.agent.clone(),
            generation,
            pid,
            resumed: resume,
            session_id: row.session_id.clone(),
            session: Arc::new(session),
            stderr: StderrTail::read(stderr),
            open_turns: Arc::clone(&open_turns),
            confined: start.is_some(),
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
            open_turns,
        };
        lock(&self.inner.running).insert(row.agent.clone(), run.clone());
        // A stop or a pause that came after the start committed but before the process was in `running` found no process to end. The row is read only now that the process is there, so either that stop or pause sees the process or this check sees it.
        let idle = turn.is_none();
        let now = self.inner.clock.now();
        let current = self
            .inner
            .store
            .write(async |tx| {
                let found = agents::find_worker_process(tx.conn(), &row.agent)
                    .await?
                    .filter(|current| {
                        current.id == row.id && current.state == WorkerState::Starting
                    });
                let paused = agents::pause_state(tx.conn()).await?.is_paused();
                let mut update = None;
                // A new session whose start a pause ends records its fingerprint all the same, as the resume of the pause continues that session.
                if let (Some(_), Some(fingerprint)) = (&found, &fingerprint)
                    && !resume
                {
                    agents::set_fold_fingerprint(tx, row.id, fingerprint).await?;
                }
                if let Some(found) = found.as_ref().filter(|_| !paused) {
                    agents::set_worker_pid(tx, row.id, pid).await?;
                    agents::forget_made_up_model(tx, row.id).await?;
                    if idle {
                        agents::set_worker_state(tx, row.id, WorkerState::Idle, now).await?;
                    }
                    if let Some(fingerprint) = &fingerprint
                        && resume
                        && needs_update_note(found.fold_fingerprint.as_deref(), Some(fingerprint))
                    {
                        update = Some(Update {
                            role: row.role,
                            fingerprint: fingerprint.clone(),
                        });
                    }
                }
                Ok::<_, StoreError>((found.is_some(), paused, update))
            })
            .await;
        let (starting, paused, update) = match current {
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
            tokio::spawn(self.clone().feed(stdin, inputs, process.clone(), None));
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
        tokio::spawn(self.clone().feed(stdin, inputs, process.clone(), update));
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

    /// Writes the lines of the input channel to the process's stdin until the channel closes or a `Close` arrives, which closes stdin so Claude Code exits after its turn. Each user turn makes the worker `working`. With `update`, the first user turn starts with the update note, and the write that marks it working records the update's fingerprint.
    async fn feed(
        self,
        mut stdin: ChildStdin,
        mut inputs: mpsc::UnboundedReceiver<Input>,
        process: Process,
        mut update: Option<Update>,
    ) {
        while let Some(input) = inputs.recv().await {
            let (line, turn) = match input {
                Input::Line(line) => (line, None),
                Input::Turn(text) => {
                    let noted = update.take();
                    let text = match &noted {
                        Some(noted) => with_update_note(noted.role, &text),
                        None => text,
                    };
                    match ClaudeInput::user(text).to_line() {
                        Ok(line) => (line, Some(noted)),
                        Err(_) => continue,
                    }
                }
                Input::Close => break,
            };
            if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                break;
            }
            if let Some(noted) = turn {
                process.open_turns.fetch_add(1, Ordering::SeqCst);
                let marked = self
                    .inner
                    .store
                    .write(async |tx| {
                        if let Some(noted) = &noted {
                            agents::set_fold_fingerprint(tx, process.id, &noted.fingerprint)
                                .await?;
                        }
                        agents::mark_worker_working(tx, process.id).await
                    })
                    .await;
                if let Err(error) = marked {
                    tracing::warn!(agent = %process.agent, code = error.log_code(), "could not mark a worker working");
                }
            }
        }
    }
}

/// Whether a failed spawn of `program` means that Claude Code is not installed where the daemon looks: the program is the launcher's `claude`, the system found no such file, and no file is at that path either, which tells it apart from a script whose interpreter is missing. Blocking, for one `stat`.
fn claude_missing(error: &std::io::Error, program: &Path, launcher: &Launcher) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
        && program == launcher.claude
        && !(program.is_absolute() && program.is_file())
}

/// The fingerprint a resumed worker's first turn records, with the role whose update note starts that turn.
struct Update {
    role: Role,
    fingerprint: String,
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
    /// The worker's start folder, which is its working directory; `None` for the supervised main, which works in the project folder.
    start: Option<PathBuf>,
}

/// Whether a worker of `role` starts in its own start folder instead of the project folder. Every role but the main agent changes files only in a workspace, so none of its commands needs the project folder as its working directory.
pub(in crate::services::agents::workers) fn starts_apart(role: Role) -> bool {
    role != Role::Main
}

/// The project's Claude Code settings that a session reads from its working directory, which a worker's start folder holds copies of. The project's `.mcp.json` is never copied: Claude Code connects every server of the folder's `.mcp.json` in a `-p` session, approved or not, so a worker gets the servers the user approved through its MCP configuration instead.
const START_COPIES: [&str; 2] = [".claude/settings.json", LOCAL_SETTINGS_FILE];

/// Makes the start folder `start` again, a new empty folder, and fills it with copies of the project's Claude Code settings of [`START_COPIES`] and of fold's skills, so a worker there reads the same settings and skills as in the project folder. The hook commands of the settings name the project folder where they name `$CLAUDE_PROJECT_DIR`. Copies, never links, because a shell command's write to a link would change the project folder. The worker's shell can change its start folder, so fold removes whatever it left there, links included, without following any, and writes each copy as a new file: a link the worker planted never takes a write of fold's. The parents of `start` must be real folders (see [`fold_vcs::platform::real_dir_below`]). Blocking.
fn fill_start_dir(root: &Path, start: &Path) -> std::io::Result<()> {
    use fold_vcs::platform::{real_dir_below, remake_dir, write_new_file};
    remake_dir(start)?;
    let copy = |bytes: &[u8], to: &Path| -> std::io::Result<()> {
        if let Some(parent) = to.parent() {
            real_dir_below(start, parent)?;
        }
        write_new_file(to, bytes, PRIVATE_FILE_MODE)
    };
    for name in START_COPIES {
        let from = root.join(name);
        if !from.is_file() {
            continue;
        }
        let bytes = std::fs::read(&from)?;
        let bytes = match String::from_utf8(bytes) {
            Ok(text) => project_claude::with_project_dir(&text, root).into_bytes(),
            Err(error) => error.into_bytes(),
        };
        copy(&bytes, &start.join(name))?;
    }
    for skill in fold_skills() {
        let Ok(entries) = std::fs::read_dir(root.join(SKILLS_DIR).join(skill)) else {
            continue;
        };
        let to = start.join(SKILLS_DIR).join(skill);
        real_dir_below(start, &to)?;
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                copy(&std::fs::read(entry.path())?, &to.join(entry.file_name()))?;
            }
        }
    }
    Ok(())
}

/// The permission bits of the files fold writes for a worker: its MCP configuration can hold the credentials of the user's own servers, and the copies of the settings hold the user's local settings.
const PRIVATE_FILE_MODE: u32 = 0o600;

/// The permission rule that lets a worker in its start folder read the project folder without a question, as it could when it ran there: the shared paths it reads through the links in its workspace lead there. `None` for a folder whose path holds a character of Claude Code's path patterns.
fn read_rule(root: &Path) -> Option<String> {
    let root = root.canonicalize().ok()?;
    let text = root.to_str()?;
    if text.contains(['*', '?', '[', ']', '{', '}', '\\', '(', ')']) {
        return None;
    }
    Some(format!("Read(/{text}/**)"))
}

/// Writes the worker's MCP configuration, its settings and its prompt under the worker's folder, and fills its start folder when its role starts apart from the project folder. The settings allow the fold tools and run fold's hooks, with the hook of the shell tools for a worker in its start folder, so a worker never depends on the user's own Claude Code settings. The prompt is the role's text, shipped or the project's override, then the shared rules and the practices, then the area's instructions when there are any; the override file itself is never passed, because it lacks them. Returns the three paths Claude Code reads, with the program to run. An fld path that names no file writes nothing.
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
    let role = row.role;
    let project_mcp = paths.mcp_json();
    let fold = launcher.fold.clone();
    let agent = row.agent.clone();
    let prompt_text = row.role.prompt();
    let limit = settings.mcp.clone();
    let claude = launcher.claude.clone();
    let root = paths.root().to_owned();
    let start = starts_apart(role).then(|| paths.worker_start_dir(&row.agent));
    let workspaces = paths.root().join(WS_DIR);
    let user = project_claude::UserClaude::of(&launcher.env);
    tokio::task::spawn_blocking(move || {
        use fold_vcs::platform::write_new_file;
        let Some(fold) = fold.or_else(own_binary).filter(|fold| fold.is_file()) else {
            return Err(FilesError::FldMissing);
        };
        let (program, program_args) = worker_program(&fold_dir, claude, &fold);
        // The worker's shell reaches its own folder, so a link it planted there is replaced, never followed.
        fold_vcs::platform::real_dir_below(&fold_dir, &dir)?;
        // A worker in its start folder reads neither the project's `.mcp.json` nor the servers the user added for the project folder, so it gets the servers the user approved or added there; the supervised main reads them in the project folder.
        let user_project = start
            .as_ref()
            .and_then(|_| user.as_ref().map(|user| user.project(&root)));
        let approval = start.as_ref().map(|_| {
            user_project
                .as_ref()
                .map(project_claude::UserProject::approval)
                .unwrap_or_default()
        });
        let local = user_project
            .as_ref()
            .map(project_claude::UserProject::local_servers);
        let config = mcp_config(
            &fold,
            &agent,
            limit.as_deref(),
            &project_mcp,
            approval.as_ref(),
            local.as_ref(),
        );
        let mcp = dir.join("mcp.json");
        write_new_file(&mcp, config.to_string().as_bytes(), PRIVATE_FILE_MODE)?;
        let own_settings = dir.join("settings.json");
        let worker_settings = match &start {
            Some(start) => {
                fold_vcs::platform::create_dir_below(&fold_dir, &workspaces)?;
                fill_start_dir(&root, start)?;
                let mut settings = worker_settings();
                if let Some(allow) = settings["permissions"]["allow"].as_array_mut() {
                    allow.extend(read_rule(&root).map(Value::String));
                    if user_project
                        .as_ref()
                        .is_some_and(project_claude::UserProject::trusts)
                    {
                        allow.extend(project_claude::trusted_allow_rules(&root));
                    }
                }
                settings
            }
            None => session_settings(),
        };
        write_new_file(
            &own_settings,
            worker_settings.to_string().as_bytes(),
            PRIVATE_FILE_MODE,
        )?;
        let role_prompt = if override_prompt.is_file() {
            role.with_own_text(&std::fs::read_to_string(&override_prompt)?)
        } else {
            prompt_text.to_owned()
        };
        let prompt = match area_instructions {
            None => {
                let prompt = dir.join("prompt.md");
                write_new_file(&prompt, role_prompt.as_bytes(), PRIVATE_FILE_MODE)?;
                prompt
            }
            Some((area, instructions)) => {
                let prompt = dir.join("prompt.md");
                let text = area_prompt(&role_prompt, &area, &instructions);
                write_new_file(&prompt, text.as_bytes(), PRIVATE_FILE_MODE)?;
                prompt
            }
        };
        Ok(WorkerFiles {
            mcp,
            settings: own_settings,
            prompt,
            program,
            program_args,
            start,
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

/// The `fold` server with the worker's agent name, plus servers of the project's `.mcp.json` and the user's `local` servers of the project folder: when the role limits its servers, each one it names, which the user chose in fold's configuration; else, for a worker with the user's `approval`, each local server and each `.mcp.json` server the user approved for the project folder in Claude Code, so a worker never connects a server the user did not approve. A local server wins over a `.mcp.json` server of the same name, as in Claude Code. The user's servers of every folder are never copied, because their entries can hold credentials; without a limit Claude Code loads them itself. A local server's entry can hold credentials too, so fold writes the configuration readable by the user only.
fn mcp_config(
    fold: &Path,
    agent: &AgentName,
    limit: Option<&[String]>,
    project_mcp: &Path,
    approval: Option<&project_claude::Approval>,
    local: Option<&Map<String, Value>>,
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
    if limit.is_none() && approval.is_none() {
        return json!({ MCP_SERVERS_KEY: servers });
    }
    let project: Value = std::fs::read_to_string(project_mcp)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    let listed = project
        .get(MCP_SERVERS_KEY)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let local = local.cloned().unwrap_or_default();
    let names: Vec<String> = match (limit, approval) {
        (Some(names), _) => names.to_vec(),
        (None, Some(approval)) => local
            .keys()
            .chain(listed.keys().filter(|name| approval.allows(name)))
            .cloned()
            .collect(),
        (None, None) => Vec::new(),
    };
    for name in names {
        if servers.contains_key(&name) {
            continue;
        }
        if let Some(entry) = local.get(&name).or_else(|| listed.get(&name)) {
            servers.insert(name, entry.clone());
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

    /// A project folder with Claude Code settings, fold's index skill and a worker whose folder fold made.
    fn project_with_worker() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("app");
        let skill = root.join(SKILLS_DIR).join(fold_core::layout::INDEX_SKILL);
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "skill").unwrap();
        std::fs::write(
            root.join(".claude/settings.json"),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"$CLAUDE_PROJECT_DIR/x.sh"}]}]}}"#,
        )
        .unwrap();
        std::fs::write(root.join(".mcp.json"), "{}").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/settings.local.json"), "mine").unwrap();
        let start = root.join(".fold/workers/web-worker/start");
        std::fs::create_dir_all(&start).unwrap();
        (temp, root, start)
    }

    fn assert_project_untouched(root: &Path) {
        assert_eq!(
            std::fs::read_to_string(root.join("src/settings.local.json")).unwrap(),
            "mine"
        );
        assert_eq!(
            std::fs::read_to_string(root.join(".mcp.json")).unwrap(),
            "{}"
        );
        assert!(root.join(".claude/settings.json").metadata().unwrap().len() > 2);
        assert!(
            root.join(SKILLS_DIR)
                .join(fold_core::layout::INDEX_SKILL)
                .join("SKILL.md")
                .is_file()
        );
    }

    #[test]
    fn a_link_the_worker_planted_in_its_start_folder_never_takes_a_write() {
        let (_temp, root, start) = project_with_worker();
        // The worker made its `.claude` a link into the project folder.
        std::fs::create_dir_all(start.join("scratch")).unwrap();
        std::os::unix::fs::symlink(root.join("src"), start.join(".claude")).unwrap();
        fill_start_dir(&root, &start).unwrap();
        assert_project_untouched(&root);
        assert!(!root.join("src/settings.json").exists());
        assert!(!root.join("src/skills").exists());
        let copy = start.join(".claude");
        assert!(!std::fs::symlink_metadata(&copy).unwrap().is_symlink());
        let settings = std::fs::read_to_string(copy.join("settings.json")).unwrap();
        assert!(
            settings.contains(&format!("'{}'/x.sh", root.display())),
            "{settings}"
        );
        assert!(
            start
                .join(SKILLS_DIR)
                .join(fold_core::layout::INDEX_SKILL)
                .join("SKILL.md")
                .is_file()
        );
        assert!(!start.join("scratch").exists(), "the folder is made again");
        use std::os::unix::fs::PermissionsExt;
        let mode = copy
            .join("settings.json")
            .metadata()
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0);
    }

    #[test]
    fn a_start_folder_the_worker_swapped_for_a_link_to_the_project_folder_is_made_again() {
        let (_temp, root, start) = project_with_worker();
        std::fs::remove_dir(&start).unwrap();
        std::os::unix::fs::symlink(&root, &start).unwrap();
        fill_start_dir(&root, &start).unwrap();
        assert_project_untouched(&root);
        assert!(!std::fs::symlink_metadata(&start).unwrap().is_symlink());
        assert!(start.join(".claude/settings.json").is_file());
        assert!(!start.join(".mcp.json").exists());
        assert!(!start.join("src").exists());
    }

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
        let open = mcp_config(fold, &agent, None, &project, None, None);
        assert_eq!(
            open,
            json!({"mcpServers": {"fold": {"command": "/bin/fld", "args": ["mcp"], "env": {"FOLD_AGENT": "web-lead"}}}})
        );
        let names = ["fold".to_owned(), "pencil".to_owned(), "missing".to_owned()];
        let limited = mcp_config(fold, &agent, Some(&names), &project, None, None);
        let servers = limited["mcpServers"].as_object().unwrap();
        assert_eq!(servers.keys().collect::<Vec<_>>(), ["fold", "pencil"]);
        assert_eq!(servers["fold"]["command"], "/bin/fld");
        let none = mcp_config(
            fold,
            &agent,
            Some(&names),
            &dir.join("absent.json"),
            None,
            None,
        );
        assert_eq!(none["mcpServers"].as_object().unwrap().len(), 1);
    }

    #[test]
    fn a_worker_without_a_limit_gets_only_the_project_servers_the_user_approved_or_added() {
        let temp = tempfile::TempDir::new().unwrap();
        let project = temp.path().join(".mcp.json");
        std::fs::write(
            &project,
            r#"{"mcpServers":{"pencil":{"command":"pencil"},"db":{"command":"db"},"fold":{"command":"old"}}}"#,
        )
        .unwrap();
        let agent = AgentName::parse("web-worker").unwrap();
        let fold = Path::new("/bin/fld");
        let settings = temp.path().join("settings.json");
        std::fs::write(&settings, r#"{"enabledMcpjsonServers":["pencil","fold"]}"#).unwrap();
        let root = temp.path().join("app");
        let key = root.to_str().unwrap();
        std::fs::write(
            temp.path().join(".claude.json"),
            json!({"projects": {(key): {"mcpServers": {
                "docs": {"command": "docs", "env": {"TOKEN": "t"}},
                "pencil": {"command": "local-pencil"},
                "fold": {"command": "other"},
            }}}})
            .to_string(),
        )
        .unwrap();
        let env = vec![(
            "CLAUDE_CONFIG_DIR".to_owned(),
            temp.path().to_string_lossy().into_owned(),
        )];
        let user = project_claude::UserClaude::of(&env).unwrap().project(&root);
        let approval = user.approval();
        let local = user.local_servers();
        let config = mcp_config(fold, &agent, None, &project, Some(&approval), Some(&local));
        let servers = config["mcpServers"].as_object().unwrap();
        assert_eq!(
            servers.keys().collect::<Vec<_>>(),
            ["docs", "fold", "pencil"]
        );
        assert_eq!(servers["fold"]["command"], "/bin/fld");
        // The user's server of the project folder wins, as in Claude Code.
        assert_eq!(servers["pencil"]["command"], "local-pencil");
        assert_eq!(servers["docs"]["env"]["TOKEN"], "t");
        let without = mcp_config(fold, &agent, None, &project, Some(&approval), None);
        let names: Vec<_> = without["mcpServers"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(names, ["fold", "pencil"]);
        let nothing = mcp_config(
            fold,
            &agent,
            None,
            &project,
            Some(&Default::default()),
            None,
        );
        assert_eq!(nothing["mcpServers"].as_object().unwrap().len(), 1);
        let limited = mcp_config(
            fold,
            &agent,
            Some(&["docs".to_owned()]),
            &project,
            None,
            Some(&local),
        );
        let names: Vec<_> = limited["mcpServers"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(names, ["docs", "fold"]);
    }
}
