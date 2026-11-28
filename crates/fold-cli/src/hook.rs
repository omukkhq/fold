use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use clap::{Args, Subcommand};
use fold_cli::claude_files;
use fold_cli::daemon_client::{self, ClientConfig, ClientError, DaemonClient};
use fold_cli::main_reload::{self, Current, Loaded, Turn, prompt_turn};
use fold_core::layout::{SHELL_TOOLS, START_DIR_ENV};
use fold_core::roles::{HandoffAt, MAIN_HANDOFF_NOTE, Role, handoff_due};
use fold_core::{AgentName, ProjectConfig, ProjectPaths, codes};
use hyper::Method;
use serde_json::{Value, json};

use crate::init::project_root_at_or_above;
use crate::serve::{self, Dirs};
use crate::{claude, output};

/// How long `fld hook session-start` waits for a daemon it starts.
const START_TIMEOUT: Duration = Duration::from_secs(5);
/// How long either hook waits for the daemon's answer, and `fld hook stop` for the socket.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(2);
/// Claude Code's hook input is a small JSON object; anything longer is refused unread.
const MAX_INPUT: u64 = 1 << 20;
/// The edit guard reads a `Write` of any size, because the content is part of the input; past this many bytes it denies.
const MAX_EDIT_INPUT: u64 = 256 << 20;

#[derive(Args, Debug)]
pub struct HookArgs {
    #[command(subcommand)]
    command: HookCommand,
}

#[derive(Subcommand, Debug)]
enum HookCommand {
    /// Claude Code SessionStart hook: give the session its fold context, starting the daemon when needed
    SessionStart(HookOptions),
    /// Claude Code Stop hook: keep the turn going once while the agent has unread fold messages
    Stop(HookOptions),
    /// Claude Code PreToolUse hook for the file tools: refuse an edit of the project folder outside the workspaces in .fold/ws/, and an edit of a review checkout other than the session's own; for a worker in its start folder, also a shell command that would run in the project folder outside the workspaces
    PreToolUse(HookOptions),
    /// Claude Code UserPromptSubmit hook of the main agent's session that fld claude starts: repeat the reload note at each turn until the session loads its skill after an update
    UserPromptSubmit,
    /// Claude Code PostToolUse hook of the main agent's session for the Skill tool: record that the session loaded its skill
    PostToolUse,
}

#[derive(Args, Debug)]
struct HookOptions {
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long, env = serve::SOCKET_ENV)]
    socket: Option<PathBuf>,
}

/// A hook never fails the session: every failure writes one line naming its code to stderr and nothing to stdout, and the exit code stays 0.
pub fn run(args: &HookArgs, out: &mut impl Write) {
    let Some(agent) = std::env::var_os(claude_files::AGENT_ENV)
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return;
    };
    let mut carried = None;
    let (name, outcome) = match &args.command {
        HookCommand::SessionStart(options) => ("session-start", session_start(options, &agent)),
        HookCommand::Stop(options) => ("stop", stop(options, &agent, &mut carried)),
        HookCommand::PreToolUse(options) => ("pre-tool-use", pre_tool_use(options, &agent)),
        HookCommand::UserPromptSubmit => ("user-prompt-submit", user_prompt_submit(&agent)),
        HookCommand::PostToolUse => ("post-tool-use", post_tool_use(&agent)),
    };
    let failed = |code: String| {
        output::line(
            &mut io::stderr().lock(),
            &format!("fld hook {name}: {code}"),
        )
    };
    let written = match outcome {
        Ok(Some(answer)) => output::line(out, &answer.to_string()).and_then(|()| out.flush()),
        Ok(None) => Ok(()),
        Err(code) => failed(code),
    };
    // Claude Code may close the pipe first; the session goes on either way.
    if written.is_ok()
        && let Some(carried) = carried
        && let Err(code) = carried.delivered()
    {
        let _ = failed(code);
    }
}

/// The messages a Stop hook's reason carried, which become delivered only once the reason is written for the session: a hook that gave up on a slow answer leaves them for the next wake.
struct Carried {
    config: ClientConfig,
    body: Value,
}

impl Carried {
    fn delivered(self) -> Result<(), String> {
        exchange(
            self.config,
            ANSWER_TIMEOUT,
            "/api/v1/hooks/stop/delivered",
            &self.body,
        )
        .map(|_| ())
    }
}

/// Claude Code's JSON from stdin. The text is never echoed, because it can hold the session's paths and messages.
fn input() -> Result<Value, &'static str> {
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .take(MAX_INPUT)
        .read_to_end(&mut bytes)
        .map_err(|_| codes::INVALID_REQUEST)?;
    match serde_json::from_slice(&bytes) {
        Ok(value @ Value::Object(_)) => Ok(value),
        _ => Err(codes::INVALID_REQUEST),
    }
}

/// The daemon resolves the project from the canonical path, as the adapter handshake does.
fn cwd(input: &Value) -> Result<String, &'static str> {
    let cwd = input["cwd"].as_str().ok_or(codes::INVALID_REQUEST)?;
    let cwd = std::fs::canonicalize(cwd).map_err(|_| codes::INVALID_REQUEST)?;
    cwd.into_os_string()
        .into_string()
        .map_err(|_| codes::INVALID_REQUEST)
}

fn session_start(options: &HookOptions, agent: &str) -> Result<Option<Value>, String> {
    let input = input()?;
    follow_session(&input, agent);
    let mut body = json!({"cwd": cwd(&input)?, "agent": agent});
    let messaging = |name| std::env::var(name).ok().filter(|value| !value.is_empty());
    if let (Some(socket), Some(token)) = (
        messaging("CLAUDE_CODE_MESSAGING_SOCKET"),
        messaging("CLAUDE_CODE_MESSAGING_TOKEN"),
    ) {
        body["socket"] = json!(socket);
        body["token"] = json!(token);
    }
    let dirs = Dirs::from_env();
    let starting = ClientConfig {
        socket: socket(options, &dirs)?,
        runtime_dir: serve::runtime_dir(&dirs).ok(),
        fold_exe: std::env::current_exe().ok(),
    };
    let answer = exchange(
        starting,
        START_TIMEOUT,
        "/api/v1/hooks/session-start",
        &body,
    )?;
    let context = answer["context"].as_str().ok_or(codes::INTERNAL_ERROR)?;
    Ok(Some(json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": context,
        }
    })))
}

/// Claude Code starts a new session in the same process on `/clear` and opens another one on its own `/resume`, and each start runs this hook with the session's id, so the record that `fld claude` resumes follows the conversation the user has open. It writes the id only, and only for an agent that already has a record; after a `/clear` or a compaction the session's fingerprint record follows too. A failure leaves the old record, which `fld claude` resumes or replaces, so it never holds the session's context back.
fn follow_session(input: &Value, agent: &str) {
    let (Some(id), Some(cwd), Ok(agent)) = (
        input["session_id"].as_str(),
        input["cwd"].as_str(),
        AgentName::parse(agent),
    ) else {
        return;
    };
    let Ok(cwd) = std::fs::canonicalize(cwd) else {
        return;
    };
    if let Some(root) = project_root_at_or_above(&cwd) {
        let continues = matches!(input["source"].as_str(), Some("clear" | "compact"));
        let _ = claude::follow_record(&ProjectPaths::new(&root), &agent, id, continues);
    }
}

fn stop(
    options: &HookOptions,
    agent: &str,
    carried: &mut Option<Carried>,
) -> Result<Option<Value>, String> {
    let input = input()?;
    if input["stop_hook_active"].as_bool() == Some(true) {
        return Ok(None);
    }
    let cwd = cwd(&input)?;
    let body = json!({"cwd": cwd, "agent": agent});
    let config = ClientConfig::running(socket(options, &Dirs::from_env())?);
    let answer = exchange(config.clone(), ANSWER_TIMEOUT, "/api/v1/hooks/stop", &body)?;
    let unread = answer["unread"].as_u64().ok_or(codes::INTERNAL_ERROR)?;
    if unread == 0 {
        return Ok(None);
    }
    let reason = match answer["reason"].as_str() {
        Some(reason) => {
            *carried = Some(Carried {
                config,
                body: json!({"cwd": cwd, "agent": agent, "messages": answer["messages"]}),
            });
            reason.to_owned()
        }
        None => format!("{unread} unread fold messages. Call get_inbox."),
    };
    // A Stop hook keeps the turn going only through a block decision; Claude Code then sets `stop_hook_active` on the next stop.
    Ok(Some(json!({"decision": "block", "reason": reason})))
}

/// The main agent's session that a hook of [`main_reload::main_hooks`] runs in: the fingerprint record of `agent` and what it says the session last loaded. `None` for an agent that `fld claude` never started, which has no session record.
struct MainSession {
    root: PathBuf,
    record: PathBuf,
    session: String,
    loaded: Option<Loaded>,
}

impl MainSession {
    fn of(input: &Value, agent: &str) -> Option<Self> {
        let session = input["session_id"].as_str()?;
        let cwd = std::fs::canonicalize(input["cwd"].as_str()?).ok()?;
        let agent = AgentName::parse(agent).ok()?;
        let root = project_root_at_or_above(&cwd)?;
        let paths = ProjectPaths::new(&root);
        if !paths.main_session(&agent).is_file() {
            return None;
        }
        let record = paths.main_session_fingerprint(&agent);
        let loaded = std::fs::read_to_string(&record)
            .ok()
            .and_then(|text| Loaded::parse(&text));
        Some(Self {
            root,
            record,
            session: session.to_owned(),
            loaded,
        })
    }

    /// Records that the session loaded what this fld gives, so that later turns get no note.
    fn loads(&self, current: Current<'_>) {
        claude::record_loaded(&self.record, &current.loaded_by(&self.session));
    }

    /// The marker of the session that got the hand-off note: the session id in a file next to the fingerprint record. A new session after `/clear` has another id, so it gets the note again once its own context grows large.
    fn handoff_marker(&self) -> PathBuf {
        self.record.with_extension("handoff")
    }

    /// Whether this turn gets the hand-off note: the session's context in `transcript` has reached the main role's `handoff_at`, and the session has not had the note. A due note is marked first, and a marker that cannot be written gives no note, so the note never comes on every turn. Any failure to read gives no note.
    fn handoff_due(&self, transcript: Option<&Path>) -> bool {
        let marker = self.handoff_marker();
        if std::fs::read_to_string(&marker).is_ok_and(|text| text.trim() == self.session) {
            return false;
        }
        let Some(context) = transcript.and_then(transcript_context) else {
            return false;
        };
        let Some(at) = main_handoff_at(&self.root) else {
            return false;
        };
        handoff_due(context, assumed_window(context), at)
            && std::fs::write(&marker, &self.session).is_ok()
    }
}

/// Runs only on this machine, without the daemon, so a turn never waits for it.
fn user_prompt_submit(agent: &str) -> Result<Option<Value>, String> {
    let input = input()?;
    let Some(main) = MainSession::of(&input, agent) else {
        return Ok(None);
    };
    let fingerprint = claude_files::role_fingerprint(Role::Main);
    let tools = fold_daemon::mcp::tool_schema_hashes();
    let current = Current {
        fingerprint: &fingerprint,
        tools: &tools,
    };
    let prompt = input["prompt"].as_str().unwrap_or_default();
    let turn = prompt_turn(main.loaded.as_ref(), &main.session, prompt, current);
    // A newer fld may be installed while neither the daemon nor `fld claude` has run it, so the skill the session is told to load is written first, when it can be, by the fld whose fingerprint the load records.
    if turn != Turn::Quiet {
        let _ = claude_files::write_skills(&main.root, Dirs::from_env().home.as_deref());
    }
    let reload = match turn {
        Turn::Loads => {
            main.loads(current);
            None
        }
        Turn::Remind(note) => Some(note),
        Turn::Quiet => None,
    };
    let transcript = input["transcript_path"].as_str().map(Path::new);
    Ok(prompt_answer(
        reload.as_deref(),
        main.handoff_due(transcript),
    ))
}

/// The prompt hook's answer: the reload note and the hand-off note that are due, in one additional context, or nothing.
fn prompt_answer(reload: Option<&str>, handoff: bool) -> Option<Value> {
    let notes: Vec<&str> = reload
        .into_iter()
        .chain(handoff.then_some(MAIN_HANDOFF_NOTE))
        .collect();
    (!notes.is_empty()).then(|| {
        json!({
            "hookSpecificOutput": {
                "hookEventName": main_reload::PROMPT_HOOK_EVENT,
                "additionalContext": notes.join("\n\n"),
            }
        })
    })
}

/// The most bytes of the transcript's end that the hand-off check reads: the newest assistant entry lies near the end, and a long transcript is never read whole on each turn.
const TRANSCRIPT_TAIL: u64 = 1 << 20;

/// The windows Claude Code's models have. The transcript does not record the window of a session, so the check takes the smallest one that holds the context.
const SMALL_WINDOW: u64 = 200_000;
const LARGE_WINDOW: u64 = 1_000_000;

/// The window of a session whose context is `context` tokens: [`SMALL_WINDOW`] while the context fits in it, else [`LARGE_WINDOW`].
fn assumed_window(context: u64) -> u64 {
    if context <= SMALL_WINDOW {
        SMALL_WINDOW
    } else {
        LARGE_WINDOW
    }
}

/// One transcript line, read only for its type and the token counts of its model call; serde skips every other field, the content too.
#[derive(serde::Deserialize)]
struct TranscriptEntry {
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "isSidechain", default)]
    sidechain: bool,
    message: Option<EntryMessage>,
}

#[derive(serde::Deserialize)]
struct EntryMessage {
    usage: Option<EntryUsage>,
}

#[derive(serde::Deserialize)]
struct EntryUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

/// The context size of the newest model call of the session's own thread in the transcript at `path`, counted as the daemon counts a worker's: input, cache writes and cache reads. `None` when the file cannot be read or holds no such entry.
fn transcript_context(path: &Path) -> Option<u64> {
    use std::io::{Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(TRANSCRIPT_TAIL);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = Vec::new();
    file.take(TRANSCRIPT_TAIL).read_to_end(&mut tail).ok()?;
    let text = String::from_utf8_lossy(&tail);
    // A tail that starts inside a line loses that line's start, which then does not parse and is skipped.
    text.lines().rev().find_map(|line| {
        let entry: TranscriptEntry = serde_json::from_str(line).ok()?;
        if entry.kind != "assistant" || entry.sidechain {
            return None;
        }
        let usage = entry.message?.usage?;
        let context = usage
            .input_tokens
            .saturating_add(usage.cache_creation_input_tokens)
            .saturating_add(usage.cache_read_input_tokens);
        (context > 0).then_some(context)
    })
}

/// The main role's `handoff_at` from the project's config, the defaults when the project has no config file. `None` when the file cannot be read or parsed, or the role has no `handoff_at`.
fn main_handoff_at(root: &Path) -> Option<HandoffAt> {
    let text = match std::fs::read_to_string(ProjectPaths::new(root).config()) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(_) => return None,
    };
    ProjectConfig::from_toml(&text)
        .ok()?
        .role(Role::Main)
        .handoff_at
}

fn post_tool_use(agent: &str) -> Result<Option<Value>, String> {
    let input = input()?;
    let tool = input["tool_name"].as_str().unwrap_or_default();
    if !main_reload::loads_main_skill(tool, &input["tool_input"]) {
        return Ok(None);
    }
    if let Some(main) = MainSession::of(&input, agent) {
        let fingerprint = claude_files::role_fingerprint(Role::Main);
        let tools = fold_daemon::mcp::tool_schema_hashes();
        main.loads(Current {
            fingerprint: &fingerprint,
            tools: &tools,
        });
    }
    Ok(None)
}

/// Claude Code reads a deny decision from this hook even where no permission rule could express it: its path rules cannot deny a folder but one folder below it, and they would bind the human's own sessions too, while this hook runs for agents only.
fn pre_tool_use(options: &HookOptions, agent: &str) -> Result<Option<Value>, String> {
    let ask = |root: &Path, workspace: &str| -> Result<Option<String>, String> {
        let body = json!({
            "cwd": root.to_str().ok_or(codes::INVALID_REQUEST)?,
            "agent": agent,
            "workspace": workspace,
        });
        let config = ClientConfig::running(socket(options, &Dirs::from_env())?);
        let answer = exchange(config, ANSWER_TIMEOUT, "/api/v1/hooks/edit", &body)?;
        match answer["allowed"].as_bool() {
            Some(true) => Ok(None),
            Some(false) => Ok(Some(
                answer["reason"]
                    .as_str()
                    .unwrap_or("fold refused the edit")
                    .to_owned(),
            )),
            None => Err(codes::INTERNAL_ERROR.to_owned()),
        }
    };
    let start = std::env::var_os(START_DIR_ENV)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from);
    edit_decision(io::stdin().lock(), MAX_EDIT_INPUT, start.as_deref(), ask)
}

/// What the edit guard needs from the tool input. The content of a large `Write` is skipped while reading, so the input's size never hides the path from the guard.
#[derive(Default)]
struct EditTarget {
    cwd: Option<String>,
    tool: Option<String>,
    file: Option<String>,
    command: Option<String>,
}

impl<'de> serde::Deserialize<'de> for EditTarget {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'de> serde::de::Visitor<'de> for Fields {
            type Value = EditTarget;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<EditTarget, A::Error> {
                let mut target = EditTarget::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "cwd" => target.cwd = map.next_value::<Option<String>>()?,
                        "tool_name" => target.tool = map.next_value::<Option<String>>()?,
                        "tool_input" => {
                            let input = map.next_value::<ToolInput>()?;
                            target.file = input.file;
                            target.command = input.command;
                        }
                        _ => {
                            map.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(target)
            }
        }
        deserializer.deserialize_map(Fields)
    }
}

#[derive(Default)]
struct ToolInput {
    file: Option<String>,
    command: Option<String>,
}

impl<'de> serde::Deserialize<'de> for ToolInput {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'de> serde::de::Visitor<'de> for Fields {
            type Value = ToolInput;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<ToolInput, A::Error> {
                let (mut file, mut notebook, mut command) = (None, None, None);
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "file_path" => file = map.next_value::<Option<String>>()?,
                        "notebook_path" => notebook = map.next_value::<Option<String>>()?,
                        "command" => command = map.next_value::<Option<String>>()?,
                        _ => {
                            map.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(ToolInput {
                    file: file.or(notebook),
                    command,
                })
            }
            fn visit_unit<E>(self) -> Result<ToolInput, E> {
                Ok(ToolInput::default())
            }
        }
        deserializer.deserialize_any(Fields)
    }
}

/// The guard's answer for one tool input. Input past `limit` bytes cannot be read to its end, so it is denied rather than left without a decision. `ask` gives the daemon's answer for a review checkout: `None` when the session may edit there, else the reason it may not. A daemon that cannot answer denies the edit too, since an error here would leave the decision to Claude Code's own rules. A shell tool's command is judged only for a worker with a start folder `start`.
fn edit_decision(
    input: impl Read,
    limit: u64,
    start: Option<&Path>,
    ask: impl FnOnce(&Path, &str) -> Result<Option<String>, String>,
) -> Result<Option<Value>, String> {
    let mut input = input.take(limit);
    let target = match serde_json::from_reader::<_, EditTarget>(&mut input) {
        Ok(target) => target,
        Err(_) if input.limit() == 0 => {
            return Ok(Some(deny(
                "the tool input is too large for fold to check; make the edit in smaller parts inside your workspace under .fold/ws/",
            )));
        }
        Err(_) => return Err(codes::INVALID_REQUEST.to_owned()),
    };
    let cwd = target.cwd.ok_or(codes::INVALID_REQUEST)?;
    if let (Some(start), Some(tool)) = (start, target.tool.as_deref())
        && SHELL_TOOLS.split('|').any(|shell| shell == tool)
    {
        let command = target.command.unwrap_or_default();
        return Ok(shell_guard(Path::new(&cwd), &command, start).map(|reason| deny(&reason)));
    }
    let Some(file) = target.file else {
        return Ok(None);
    };
    let reason = match edit_guard(Path::new(&cwd), Path::new(&file)) {
        EditGuard::Allow => return Ok(None),
        EditGuard::Deny(reason) => reason,
        EditGuard::AskDaemon { root, workspace } => match ask(&root, &workspace) {
            Ok(None) => return Ok(None),
            Ok(Some(reason)) => reason,
            Err(code) => format!(
                "fold could not check that this review checkout is yours ({code}); try the edit again once the fold daemon runs"
            ),
        },
    };
    Ok(Some(deny(&reason)))
}

fn deny(reason: &str) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    })
}

/// What the edit guard decides for one file before it asks the daemon anything.
#[derive(Debug, PartialEq, Eq)]
enum EditGuard {
    Allow,
    Deny(String),
    /// The file is in the review checkout `workspace`, a path relative to the project folder `root`; only the daemon knows whose it is.
    AskDaemon {
        root: PathBuf,
        workspace: String,
    },
}

/// The batch checkouts' directories start with this below `.fold/ws/`; every other `review-` directory is the checkout of one proposal.
const BATCH_CHECKOUT_PREFIX: &str = "review-rb-";

/// What the guard decides for `file`. Inside the fold project that holds `cwd`, an agent changes files only inside one workspace below `.fold/ws/`: the project folder is main, which changes only through integration, and the rest of `.fold/` is fold's data. A review checkout of one proposal is edited only by the reviewer it belongs to, which the daemon decides, and a batch checkout never, because its files mix every member's work and never become a version. A path outside the project is left to Claude Code's own rules. `..` and the symlinks of the folders that exist are resolved first, so neither leads into or out of a workspace unseen.
fn edit_guard(cwd: &Path, file: &Path) -> EditGuard {
    let Some((root, inside)) = std::fs::canonicalize(cwd).ok().and_then(|cwd| {
        let root = project_root_at_or_above(&cwd)?;
        let target = resolve(&cwd.join(file));
        let inside = target.strip_prefix(&root).ok()?.to_path_buf();
        Some((root, inside))
    }) else {
        return EditGuard::Allow;
    };
    let below = inside
        .strip_prefix(fold_core::layout::WS_DIR)
        .ok()
        .filter(|below| below.components().count() >= 2);
    let Some(dir) = below
        .and_then(|below| below.components().next())
        .and_then(|first| first.as_os_str().to_str())
    else {
        return EditGuard::Deny(
            "the project folder is fold main and changes only through integration; edit the file in your own workspace under .fold/ws/".to_owned(),
        );
    };
    if dir.starts_with(BATCH_CHECKOUT_PREFIX) {
        return EditGuard::Deny(
            "a batch checkout mixes every member and never becomes a version; call create_review_checkout for the member you fix and edit there".to_owned(),
        );
    }
    if dir.starts_with("review-") {
        return EditGuard::AskDaemon {
            root,
            workspace: format!("{}/{dir}", fold_core::layout::WS_DIR),
        };
    }
    EditGuard::Allow
}

/// The reason to refuse a shell command that would run in `cwd`, for a worker whose start folder is `start`; `None` lets it run. In its own project a worker's shell runs only in its start folder or in a directory below `.fold/ws/<workspace>/`, so a relative path of the command without `..` never lands in the project folder or in fold's data; the start folder lies inside the project folder, so a path with `..`, or a command that changes into the project folder from the start folder, still reaches it. A command that starts with `cd` into such a directory followed by `&&` runs there, which lets a shell that ended up elsewhere move back. Both paths are followed to their real paths first, so a symbolic link into the project folder counts as the project folder. A `cwd` outside the project is left to Claude Code's own rules. Where a command may run, it is refused still when it names a shared path as a write target in a common form (see [`shared_write`]); every other path it names is left to the worker, so only Claude Code's sandbox can refuse every write.
fn shell_guard(cwd: &Path, command: &str, start: &Path) -> Option<String> {
    let start = std::fs::canonicalize(start).ok()?;
    let root = project_root_at_or_above(&start)?;
    let cwd = std::fs::canonicalize(cwd).ok()?;
    let allowed = |dir: &Path| {
        !dir.starts_with(&root)
            || dir.starts_with(&start)
            || dir
                .strip_prefix(&root)
                .ok()
                .and_then(|inside| inside.strip_prefix(fold_core::layout::WS_DIR).ok())
                .is_some_and(|below| below.components().next().is_some())
    };
    let moves_back = || {
        leading_cd(command)
            .and_then(|target| std::fs::canonicalize(cwd.join(target)).ok())
            .is_some_and(|target| target.starts_with(&root) && allowed(&target))
    };
    if allowed(&cwd) || moves_back() {
        return shared_write(&root, &cwd, command).map(|path| {
            format!(
                "this command writes to {path}, a shared file of the project folder that your workspace links to; workers only read shared files, and a change there cannot be undone. Ask the main agent with send_message to change it."
            )
        });
    }
    Some(format!(
        "fold runs your shell commands only in your workspace under .fold/ws/ or in your start folder {}: the project folder is fold main and changes only through integration. Run the command again as cd '<absolute path of your workspace>' && <command>.",
        start.display()
    ))
}

/// The shared path of the project at `root` that `command`, run in `cwd`, names as a write target in one of the forms [`crate::shell_targets::write_targets`] reads, through a workspace's link or by its path in the project folder. A net for the common mistakes, not a boundary.
fn shared_write(root: &Path, cwd: &Path, command: &str) -> Option<String> {
    let shared = fold_vcs::shared::read_shared(&ProjectPaths::new(root).fold_dir()).ok()?;
    if shared.is_empty() {
        return None;
    }
    crate::shell_targets::write_targets(command)
        .into_iter()
        .find_map(|(dir, target)| {
            let base = dir.map_or_else(|| cwd.to_path_buf(), |dir| cwd.join(dir));
            let real = resolve(&base.join(target));
            shared
                .iter()
                .find(|path| real.starts_with(root.join(path.to_string())))
                .map(ToString::to_string)
        })
}

/// The folder of a command that starts with `cd <folder>`, alone or followed by `&&`: one word, bare or in quotes, that holds no expansion, so the folder is the one the shell goes to. `None` for every other command.
fn leading_cd(command: &str) -> Option<&str> {
    let rest = command.trim_start().strip_prefix("cd")?;
    let rest = rest.strip_prefix([' ', '\t'])?.trim_start();
    let (folder, after) = match rest.chars().next()? {
        quote @ ('\'' | '"') => {
            let end = rest[1..].find(quote)? + 1;
            let folder = &rest[1..end];
            if quote == '"' && folder.contains(['$', '`', '\\']) {
                return None;
            }
            (folder, &rest[end + 1..])
        }
        _ => {
            let end = rest
                .find(|c: char| c.is_whitespace() || "&;|<>()".contains(c))
                .unwrap_or(rest.len());
            let folder = &rest[..end];
            if folder.contains(['$', '`', '\\', '\'', '"', '*', '?', '[', '{', '~']) {
                return None;
            }
            (folder, &rest[end..])
        }
    };
    let after = after.trim_start();
    (!folder.is_empty() && (after.is_empty() || after.starts_with("&&"))).then_some(folder)
}

/// `path` with `.` and `..` removed and its longest existing ancestor replaced by its canonical form. A `..` follows the symlinks before it, as the operating system does, so `link/..` is the parent of the link's target.
fn resolve(path: &Path) -> PathBuf {
    let mut lexical = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if let Ok(canonical) = std::fs::canonicalize(&lexical) {
                    lexical = canonical;
                }
                lexical.pop();
            }
            Component::CurDir => {}
            other => lexical.push(other),
        }
    }
    let mut rest = Vec::new();
    let mut existing = lexical.as_path();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(existing) {
            return rest
                .iter()
                .rev()
                .fold(canonical, |path, name| path.join(name));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                existing = parent;
            }
            _ => return lexical,
        }
    }
}

fn socket(options: &HookOptions, dirs: &Dirs) -> Result<PathBuf, String> {
    let socket = serve::socket_path(options.socket.as_deref(), dirs)
        .map_err(|error| error.code().unwrap_or(codes::INVALID_CONFIG).to_owned())?
        .ok_or(codes::RUNTIME_DIR_MISSING)?;
    std::path::absolute(&socket).map_err(|_| codes::INVALID_REQUEST.to_owned())
}

/// Connects first within `connect_timeout`, starting the daemon when `config` allows it, then posts `body` over a connection of its own. The runtime is dropped without waiting, so a daemon start that outlives its timeout never holds the hook.
fn exchange(
    config: ClientConfig,
    connect_timeout: Duration,
    path: &str,
    body: &Value,
) -> Result<Value, String> {
    let runtime = serve::current_thread_runtime().map_err(|_| codes::INTERNAL_ERROR)?;
    let outcome = runtime.block_on(async {
        match tokio::time::timeout(connect_timeout, daemon_client::connect(&config)).await {
            Ok(Ok(_stream)) => {}
            Ok(Err(error)) => return Err(client_code(&error)),
            Err(_) => return Err(codes::DAEMON_UNREACHABLE),
        }
        let running = ClientConfig {
            fold_exe: None,
            ..config
        };
        // A hook has no person to answer, so it never asks.
        let human = crate::fold_name::known(&serve::Dirs::from_env())
            .ok()
            .map(|found| found.name);
        let client = DaemonClient::new(running, human);
        match tokio::time::timeout(
            ANSWER_TIMEOUT,
            client.request(Method::POST, path, Some(body)),
        )
        .await
        {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => Err(client_code(&error)),
            Err(_) => Err(codes::DAEMON_UNREACHABLE),
        }
    });
    runtime.shutdown_background();
    let response = outcome?;
    if response.is_success() {
        return Ok(response.body);
    }
    Err(response
        .refusal()
        .code
        .unwrap_or_else(|| codes::INTERNAL_ERROR.to_owned()))
}

fn client_code(error: &ClientError) -> &'static str {
    match error {
        ClientError::HumanUnknown => codes::HUMAN_NAME_REQUIRED,
        ClientError::HumanInvalid => codes::INVALID_NAME,
        ClientError::RuntimeDirMissing => codes::RUNTIME_DIR_MISSING,
        ClientError::NoDaemon { .. }
        | ClientError::StartTimeout { .. }
        | ClientError::StartFailed
        | ClientError::Io { .. }
        | ClientError::Http(_)
        | ClientError::Body => codes::DAEMON_UNREACHABLE,
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    /// The local refusal alone: a review checkout that the daemon would decide counts as not refused here.
    fn edit_refusal(cwd: &Path, file: &Path) -> Option<String> {
        match edit_guard(cwd, file) {
            EditGuard::Deny(reason) => Some(reason),
            EditGuard::Allow | EditGuard::AskDaemon { .. } => None,
        }
    }

    fn no_daemon(_: &Path, _: &str) -> Result<Option<String>, String> {
        Err(codes::DAEMON_UNREACHABLE.to_owned())
    }

    #[test]
    fn an_agent_edits_only_inside_a_workspace_of_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir_all(root.join(".fold/ws/t-1/src")).unwrap();
        std::fs::write(root.join(".fold/fold.db"), "").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        symlink(root.join("src"), root.join(".fold/ws/t-1/escape")).unwrap();
        let refused = |cwd: &Path, file: &str| edit_refusal(cwd, Path::new(file)).is_some();

        for allowed in [
            ".fold/ws/t-1/src/lib.rs",
            ".fold/ws/t-1/new/deep/file.rs",
            "./.fold/ws/t-1/x",
        ] {
            assert!(!refused(&root, allowed), "{allowed}");
        }
        for denied in [
            "src/lib.rs",
            "README.md",
            ".gitignore",
            ".git/config",
            ".claude/settings.json",
            ".fold/config.toml",
            ".fold/ws/loose.txt",
            ".fold/ws/t-1/../../config.toml",
            ".fold/ws/t-1/escape/lib.rs",
        ] {
            assert!(refused(&root, denied), "{denied}");
        }
        let absolute = root.join("src/lib.rs");
        assert!(refused(&root, absolute.to_str().unwrap()));
        // From inside a workspace the project is the same, and a path outside it is Claude Code's to decide.
        let workspace = root.join(".fold/ws/t-1");
        assert!(!refused(&workspace, "src/lib.rs"));
        assert!(refused(&workspace, "../../../src/lib.rs"));
        assert!(!refused(
            &root,
            dir.path().join("elsewhere.txt").to_str().unwrap()
        ));
        assert!(!refused(dir.path(), "app/src/lib.rs"));
        assert!(!refused(&dir.path().join("missing"), "x"));
    }

    #[test]
    fn dot_dot_follows_the_symlink_before_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir_all(root.join(".fold/ws/t-1")).unwrap();
        std::fs::write(root.join(".fold/fold.db"), "").unwrap();
        std::fs::write(root.join("README.md"), "").unwrap();
        // `link` leads to a folder below the project folder, so `link/..` is the project folder.
        let below = root.join("sub/deeper");
        std::fs::create_dir_all(&below).unwrap();
        symlink(&below, root.join(".fold/ws/t-1/link")).unwrap();
        let outside = dir.path().join("outside/deep");
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, root.join(".fold/ws/t-1/away")).unwrap();
        let workspace = root.join(".fold/ws/t-1");

        // Lexically `link/../README.md` is inside the workspace; the operating system writes to `sub/README.md` of main.
        assert!(edit_refusal(&workspace, Path::new("link/../README.md")).is_some());
        assert!(edit_refusal(&workspace, Path::new("link/../../README.md")).is_some());
        // Lexically `inner/../../x` leaves the workspace; really `inner` is two levels down and the file stays in it.
        std::fs::create_dir_all(workspace.join("a/b")).unwrap();
        symlink(workspace.join("a/b"), workspace.join("inner")).unwrap();
        assert!(edit_refusal(&workspace, Path::new("inner/../../x")).is_none());
        assert!(edit_refusal(&root, Path::new(".fold/ws/t-1/link/../../README.md")).is_some());
        // `away/..` leaves the project; Claude Code's own rules decide there.
        assert!(edit_refusal(&workspace, Path::new("away/../x")).is_none());
        // `sub/..` is a plain folder inside main.
        assert!(edit_refusal(&root, Path::new("sub/../README.md")).is_some());
    }

    fn project_with_input(content_len: usize, file: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir_all(root.join(".fold/ws/t-1")).unwrap();
        std::fs::write(root.join(".fold/fold.db"), "").unwrap();
        let input = json!({
            "tool_input": {"content": "x".repeat(content_len), "file_path": file},
            "cwd": root.to_str().unwrap(),
        });
        (dir, input.to_string())
    }

    fn denied(answer: Result<Option<Value>, String>) -> bool {
        answer
            .unwrap()
            .is_some_and(|value| value["hookSpecificOutput"]["permissionDecision"] == "deny")
    }

    #[test]
    fn a_large_write_is_judged_by_its_path() {
        let (_dir, into_main) = project_with_input(2 << 20, "README.md");
        assert!(into_main.len() > MAX_INPUT as usize);
        assert!(denied(edit_decision(
            into_main.as_bytes(),
            MAX_EDIT_INPUT,
            None,
            no_daemon
        )));

        let (_dir, into_workspace) = project_with_input(2 << 20, ".fold/ws/t-1/big.txt");
        assert!(!denied(edit_decision(
            into_workspace.as_bytes(),
            MAX_EDIT_INPUT,
            None,
            no_daemon
        )));
    }

    #[test]
    fn input_past_the_limit_is_denied_and_bad_input_is_an_error() {
        let (_dir, input) = project_with_input(1000, ".fold/ws/t-1/a.txt");
        assert!(denied(edit_decision(
            input.as_bytes(),
            100,
            None,
            no_daemon
        )));
        assert!(edit_decision(&b"not json"[..], MAX_EDIT_INPUT, None, no_daemon).is_err());
        assert!(
            edit_decision(
                &br#"{"tool_input":{}}"#[..],
                MAX_EDIT_INPUT,
                None,
                no_daemon
            )
            .is_err()
        );
        assert!(
            edit_decision(
                &br#"{"cwd":"/","tool_input":{}}"#[..],
                MAX_EDIT_INPUT,
                None,
                no_daemon
            )
            .unwrap()
            .is_none()
        );
        assert!(
            edit_decision(
                &br#"{"cwd":"/","tool_input":null}"#[..],
                MAX_EDIT_INPUT,
                None,
                no_daemon
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn a_review_checkout_is_the_daemons_to_decide_and_a_batch_checkout_is_never_edited() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        for ws in ["review-p-1", "review-p-1-2", "review-rb-2", "t-1"] {
            std::fs::create_dir_all(root.join(".fold/ws").join(ws)).unwrap();
        }
        std::fs::write(root.join(".fold/fold.db"), "").unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let checkout = root.join(".fold/ws/review-p-1");

        for (cwd, file, workspace) in [
            (
                &root,
                ".fold/ws/review-p-1/src/lib.rs",
                ".fold/ws/review-p-1",
            ),
            (
                &root,
                ".fold/ws/review-p-1-2/new/file.rs",
                ".fold/ws/review-p-1-2",
            ),
            (&checkout, "src/lib.rs", ".fold/ws/review-p-1"),
        ] {
            assert_eq!(
                edit_guard(cwd, Path::new(file)),
                EditGuard::AskDaemon {
                    root: root.clone(),
                    workspace: workspace.to_owned()
                },
                "{file}"
            );
        }
        assert!(matches!(
            edit_guard(&root, Path::new(".fold/ws/review-rb-2/src/lib.rs")),
            EditGuard::Deny(reason) if reason.contains("create_review_checkout")
        ));
        assert_eq!(
            edit_guard(&root, Path::new(".fold/ws/t-1/src/lib.rs")),
            EditGuard::Allow
        );
        assert!(matches!(
            edit_guard(&checkout, Path::new("../../../src/lib.rs")),
            EditGuard::Deny(_)
        ));

        let input = json!({
            "tool_input": {"file_path": ".fold/ws/review-p-1/src/lib.rs"},
            "cwd": root.to_str().unwrap(),
        })
        .to_string();
        let mut asked = None;
        let answer = edit_decision(input.as_bytes(), MAX_EDIT_INPUT, None, |root, workspace| {
            asked = Some((root.to_path_buf(), workspace.to_owned()));
            Ok(None)
        });
        assert!(!denied(answer));
        assert_eq!(
            asked,
            Some((root.clone(), ".fold/ws/review-p-1".to_owned()))
        );
        let refused = edit_decision(input.as_bytes(), MAX_EDIT_INPUT, None, |_, _| {
            Ok(Some(
                "this review checkout belongs to another reviewer".to_owned(),
            ))
        })
        .unwrap()
        .unwrap();
        assert_eq!(
            refused["hookSpecificOutput"]["permissionDecisionReason"],
            "this review checkout belongs to another reviewer"
        );
        // A daemon that cannot answer denies the edit; an error would leave it to Claude Code.
        let unreachable = edit_decision(input.as_bytes(), MAX_EDIT_INPUT, None, no_daemon)
            .unwrap()
            .unwrap();
        assert_eq!(
            unreachable["hookSpecificOutput"]["permissionDecision"],
            "deny"
        );
        assert!(
            unreachable["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .contains(codes::DAEMON_UNREACHABLE)
        );
    }

    fn shell_input(cwd: &Path, tool: &str, command: &str) -> String {
        json!({
            "tool_name": tool,
            "tool_input": {"command": command, "description": "x"},
            "cwd": cwd.to_str().unwrap(),
        })
        .to_string()
    }

    #[test]
    fn a_confined_worker_runs_shell_commands_only_in_a_workspace_or_its_start_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        let start = root.join(".fold/workers/web-worker/start");
        for folder in [".fold/ws/t-1/src", ".fold/ws/review-rb-2", "src", "design"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        std::fs::create_dir_all(&start).unwrap();
        std::fs::write(root.join(".fold/fold.db"), "").unwrap();
        // A link in a workspace that leads into the project folder, as a shared reference folder is.
        symlink(root.join("design"), root.join(".fold/ws/t-1/design")).unwrap();
        let workspace = root.join(".fold/ws/t-1");
        let refused = |cwd: &Path, command: &str| shell_guard(cwd, command, &start).is_some();

        for cwd in [
            start.clone(),
            workspace.clone(),
            workspace.join("src"),
            root.join(".fold/ws/review-rb-2"),
            dir.path().to_path_buf(),
        ] {
            assert!(!refused(&cwd, "touch x"), "{}", cwd.display());
        }
        for cwd in [
            root.clone(),
            root.join("src"),
            root.join(".fold"),
            root.join(".fold/ws"),
            root.join(".fold/workers/web-worker"),
            workspace.join("design"),
        ] {
            assert!(refused(&cwd, "touch x"), "{}", cwd.display());
        }
        let back = format!("cd '{}' && touch x", workspace.display());
        assert!(!refused(&root, &back));
        assert!(!refused(&root, &format!("cd {}", workspace.display())));
        assert!(!refused(&root, "cd .fold/ws/t-1 && cargo test"));
        assert!(!refused(&root, "cd \".fold/ws/t-1/src\" && ls"));
        for command in [
            format!("cd '{}'; touch x", workspace.display()),
            format!("cd '{}' || touch x", workspace.display()),
            "cd src && touch x".to_owned(),
            "cd $HOME && touch x".to_owned(),
            "cd \"$WS\" && touch x".to_owned(),
            "cd .fold/ws/t-1/design && touch x".to_owned(),
            "cdx .fold/ws/t-1".to_owned(),
            "cd".to_owned(),
            "touch x".to_owned(),
        ] {
            assert!(refused(&root, &command), "{command}");
        }
        let reason = shell_guard(&root, "touch x", &start).unwrap();
        assert!(reason.contains(".fold/workers/web-worker/start"));
    }

    #[test]
    fn only_a_worker_with_a_start_folder_has_its_shell_commands_judged() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        let start = root.join(".fold/workers/web-worker/start");
        std::fs::create_dir_all(&start).unwrap();
        std::fs::create_dir_all(root.join(".fold/ws/t-1")).unwrap();
        std::fs::write(root.join(".fold/fold.db"), "").unwrap();
        let in_root = shell_input(&root, "Bash", "echo > plan.md");
        assert!(denied(edit_decision(
            in_root.as_bytes(),
            MAX_EDIT_INPUT,
            Some(&start),
            no_daemon
        )));
        let monitor = shell_input(&root, "Monitor", "tail -f log");
        assert!(denied(edit_decision(
            monitor.as_bytes(),
            MAX_EDIT_INPUT,
            Some(&start),
            no_daemon
        )));
        // The main agent and a session opened by hand work in the project folder.
        assert!(!denied(edit_decision(
            in_root.as_bytes(),
            MAX_EDIT_INPUT,
            None,
            no_daemon
        )));
        let in_workspace = shell_input(&root.join(".fold/ws/t-1"), "Bash", "echo > plan.md");
        assert!(!denied(edit_decision(
            in_workspace.as_bytes(),
            MAX_EDIT_INPUT,
            Some(&start),
            no_daemon
        )));
        // A file tool is still judged by its path.
        let write = json!({
            "tool_name": "Write",
            "tool_input": {"file_path": "plan.md", "content": ""},
            "cwd": start.to_str().unwrap(),
        })
        .to_string();
        assert!(denied(edit_decision(
            write.as_bytes(),
            MAX_EDIT_INPUT,
            Some(&start),
            no_daemon
        )));
    }

    #[test]
    fn an_edit_through_a_shared_link_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir_all(root.join(".fold/ws/t-1")).unwrap();
        std::fs::create_dir_all(root.join("design")).unwrap();
        std::fs::write(root.join("design/home.pen"), "pen").unwrap();
        std::fs::write(root.join(".fold/fold.db"), "").unwrap();
        // The link fold makes for a shared folder: its absolute target in the project folder.
        symlink(root.join("design"), root.join(".fold/ws/t-1/design")).unwrap();
        let workspace = root.join(".fold/ws/t-1");
        for file in ["design/home.pen", "design/new.pen"] {
            assert!(
                edit_refusal(&workspace, Path::new(file)).is_some(),
                "{file}"
            );
        }
        // A shell command started inside the linked folder runs in the project folder.
        let start = root.join(".fold/workers/web-worker/start");
        std::fs::create_dir_all(&start).unwrap();
        assert!(shell_guard(&workspace.join("design"), "touch x", &start).is_some());
        assert!(shell_guard(&workspace, "cat design/home.pen", &start).is_none());
    }

    #[test]
    fn a_shell_command_that_writes_to_a_shared_path_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap().join("app");
        let workspace = root.join(".fold/ws/t-1");
        let start = root.join(".fold/workers/web-worker/start");
        for folder in [&workspace, &start, &root.join("design")] {
            std::fs::create_dir_all(folder).unwrap();
        }
        std::fs::write(root.join(".fold/fold.db"), "").unwrap();
        std::fs::write(root.join(".fold/shared"), "design\n").unwrap();
        symlink(root.join("design"), workspace.join("design")).unwrap();
        let ws = workspace.display().to_string();
        for command in [
            "echo x > design/home.pen".to_owned(),
            "rm -rf design".to_owned(),
            "sed -i s/a/b/ design/notes.md".to_owned(),
            "mv design/home.pen src/home.pen".to_owned(),
            format!("echo x >> {}/design/a.md", root.display()),
            format!("cd '{ws}' && touch design/new.pen"),
        ] {
            let cwd = if command.starts_with("cd") {
                &start
            } else {
                &workspace
            };
            let reason = shell_guard(cwd, &command, &start);
            assert!(
                reason
                    .as_deref()
                    .is_some_and(|text| text.contains("design")),
                "{command}"
            );
        }
        for command in [
            "cat design/home.pen",
            "cp design/home.pen src/home.pen",
            "echo x > src/a.md",
        ] {
            assert_eq!(shell_guard(&workspace, command, &start), None, "{command}");
        }
    }

    const MAIN_SESSION: &str = "5d1c7e2a-0b3f-4a6d-9e8c-7f6a5b4c3d2e";

    /// A project folder with its sessions folder, and the main session of `session` in it.
    fn main_session(session: &str) -> (tempfile::TempDir, MainSession) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_owned();
        std::fs::create_dir_all(root.join(".fold/sessions")).unwrap();
        let main = MainSession {
            record: root.join(".fold/sessions/main.fingerprint"),
            root,
            session: session.to_owned(),
            loaded: None,
        };
        (dir, main)
    }

    fn assistant(context: u64) -> String {
        json!({
            "type": "assistant",
            "message": {"role": "assistant", "content": [{"type": "text", "text": "secret words"}], "usage": {
                "input_tokens": 10,
                "cache_creation_input_tokens": 990,
                "cache_read_input_tokens": context - 1000,
                "output_tokens": 500,
            }},
        })
        .to_string()
    }

    /// Writes `lines` as a transcript in `dir`, in a file of its own for each set of lines.
    fn write_transcript(dir: &Path, lines: &[String]) -> PathBuf {
        let name = transcript_name(lines);
        let path = dir.join(name);
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        path
    }

    fn transcript_name(lines: &[String]) -> String {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        lines.hash(&mut hasher);
        format!("{:x}.jsonl", hasher.finish())
    }

    #[test]
    fn the_context_is_the_newest_call_of_the_sessions_own_thread() {
        let dir = tempfile::tempdir().unwrap();
        let sidechain = json!({"type": "assistant", "isSidechain": true, "message": {"usage": {"input_tokens": 900_000}}}).to_string();
        let user =
            json!({"type": "user", "message": {"role": "user", "content": "hi"}}).to_string();
        let empty =
            json!({"type": "assistant", "message": {"usage": {"input_tokens": 0}}}).to_string();
        let path = write_transcript(
            dir.path(),
            &[
                assistant(50_000),
                assistant(120_000),
                sidechain,
                user,
                empty,
                "not json".to_owned(),
            ],
        );
        assert_eq!(transcript_context(&path), Some(120_000));
        assert_eq!(transcript_context(&dir.path().join("missing.jsonl")), None);
        let none = write_transcript(dir.path(), &[json!({"type": "user"}).to_string()]);
        assert_eq!(transcript_context(&none), None);
    }

    #[test]
    fn a_long_transcript_is_read_from_its_end() {
        let dir = tempfile::tempdir().unwrap();
        let filler = "x".repeat(usize::try_from(TRANSCRIPT_TAIL).unwrap());
        let path = write_transcript(dir.path(), &[assistant(30_000), filler, assistant(40_000)]);
        assert_eq!(transcript_context(&path), Some(40_000));
    }

    #[test]
    fn the_window_is_the_smallest_that_holds_the_context() {
        assert_eq!(assumed_window(1), SMALL_WINDOW);
        assert_eq!(assumed_window(SMALL_WINDOW), SMALL_WINDOW);
        assert_eq!(assumed_window(SMALL_WINDOW + 1), LARGE_WINDOW);
    }

    #[test]
    fn a_large_context_gets_the_note_once_per_session() {
        let (dir, main) = main_session(MAIN_SESSION);
        let small = write_transcript(dir.path(), &[assistant(169_999)]);
        assert!(!main.handoff_due(Some(&small)), "below 85% of 200k");
        assert!(!main.handoff_marker().exists());
        let large = write_transcript(dir.path(), &[assistant(170_000)]);
        assert!(main.handoff_due(Some(&large)));
        assert_eq!(
            std::fs::read_to_string(main.handoff_marker()).unwrap(),
            MAIN_SESSION
        );
        assert!(!main.handoff_due(Some(&large)), "the note comes once");
        let larger = write_transcript(dir.path(), &[assistant(400_000)]);
        assert!(!main.handoff_due(Some(&larger)));

        let cleared = MainSession {
            session: "a-new-session".to_owned(),
            ..main
        };
        assert!(!cleared.handoff_due(Some(&small)));
        assert!(
            cleared.handoff_due(Some(&large)),
            "a new session can get it again"
        );
    }

    #[test]
    fn the_main_roles_handoff_at_sets_the_share() {
        let (dir, main) = main_session(MAIN_SESSION);
        std::fs::write(
            main.root.join(".fold/config.toml"),
            "[roles.main]\nhandoff_at = 20\n",
        )
        .unwrap();
        let path = write_transcript(dir.path(), &[assistant(40_000)]);
        assert!(main.handoff_due(Some(&path)));
    }

    #[test]
    fn a_missing_or_unreadable_input_gives_no_note() {
        let (dir, main) = main_session(MAIN_SESSION);
        let large = write_transcript(dir.path(), &[assistant(190_000)]);
        assert!(!main.handoff_due(None));
        assert!(!main.handoff_due(Some(&dir.path().join("missing.jsonl"))));
        let config = main.root.join(".fold/config.toml");
        std::fs::write(&config, "not = [valid").unwrap();
        assert!(
            !main.handoff_due(Some(&large)),
            "a config that does not parse"
        );
        std::fs::remove_file(&config).unwrap();
        std::fs::create_dir(&config).unwrap();
        assert!(
            !main.handoff_due(Some(&large)),
            "a config that cannot be read"
        );
        std::fs::remove_dir(&config).unwrap();
        std::fs::remove_dir_all(main.root.join(".fold/sessions")).unwrap();
        assert!(
            !main.handoff_due(Some(&large)),
            "a marker that cannot be written"
        );
        assert!(!main.handoff_marker().exists());
    }

    #[test]
    fn the_answer_holds_each_due_note() {
        assert_eq!(prompt_answer(None, false), None);
        let context = |answer: Option<Value>| {
            answer.unwrap()["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert_eq!(context(prompt_answer(Some("reload"), false)), "reload");
        assert_eq!(context(prompt_answer(None, true)), MAIN_HANDOFF_NOTE);
        assert_eq!(
            context(prompt_answer(Some("reload"), true)),
            format!("reload\n\n{MAIN_HANDOFF_NOTE}")
        );
    }
}
