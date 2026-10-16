use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use clap::{Args, Subcommand};
use fold_cli::claude_files;
use fold_cli::daemon_client::{self, ClientConfig, ClientError, DaemonClient};
use fold_core::{AgentName, ProjectPaths, codes};
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
    /// Claude Code PreToolUse hook for the file tools: refuse an edit of the project folder outside the workspaces in .fold/ws/
    PreToolUse,
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
    let (name, outcome) = match &args.command {
        HookCommand::SessionStart(options) => ("session-start", session_start(options, &agent)),
        HookCommand::Stop(options) => ("stop", stop(options, &agent)),
        HookCommand::PreToolUse => ("pre-tool-use", pre_tool_use()),
    };
    let written = match outcome {
        Ok(Some(answer)) => output::line(out, &answer.to_string()),
        Ok(None) => Ok(()),
        Err(code) => output::line(
            &mut io::stderr().lock(),
            &format!("fld hook {name}: {code}"),
        ),
    };
    // Claude Code may close the pipe first; the session goes on either way.
    let _ = written;
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

/// Claude Code starts a new session in the same process on `/clear` and opens another one on its own `/resume`, and each start runs this hook with the session's id, so the record that `fld claude` resumes follows the conversation the user has open. It writes the id only, and only for an agent that already has a record. A failure leaves the old record, which `fld claude` resumes or replaces, so it never holds the session's context back.
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
        let _ = claude::follow_record(&ProjectPaths::new(&root), &agent, id);
    }
}

fn stop(options: &HookOptions, agent: &str) -> Result<Option<Value>, String> {
    let input = input()?;
    if input["stop_hook_active"].as_bool() == Some(true) {
        return Ok(None);
    }
    let body = json!({"cwd": cwd(&input)?, "agent": agent});
    let config = ClientConfig::running(socket(options, &Dirs::from_env())?);
    let answer = exchange(config, ANSWER_TIMEOUT, "/api/v1/hooks/stop", &body)?;
    let unread = answer["unread"].as_u64().ok_or(codes::INTERNAL_ERROR)?;
    if unread == 0 {
        return Ok(None);
    }
    // A Stop hook keeps the turn going only through a block decision; Claude Code then sets `stop_hook_active` on the next stop.
    Ok(Some(json!({
        "decision": "block",
        "reason": format!("{unread} unread fold messages. Call get_inbox."),
    })))
}

/// Claude Code reads a deny decision from this hook even where no permission rule could express it: its path rules cannot deny a folder but one folder below it, and they would bind the human's own sessions too, while this hook runs for agents only.
fn pre_tool_use() -> Result<Option<Value>, String> {
    edit_decision(io::stdin().lock(), MAX_EDIT_INPUT)
}

/// What the edit guard needs from the tool input. The content of a large `Write` is skipped while reading, so the input's size never hides the path from the guard.
#[derive(Default)]
struct EditTarget {
    cwd: Option<String>,
    file: Option<String>,
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
                        "tool_input" => target.file = map.next_value::<ToolInput>()?.0,
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

struct ToolInput(Option<String>);

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
                let (mut file, mut notebook) = (None, None);
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "file_path" => file = map.next_value::<Option<String>>()?,
                        "notebook_path" => notebook = map.next_value::<Option<String>>()?,
                        _ => {
                            map.next_value::<serde::de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(ToolInput(file.or(notebook)))
            }
            fn visit_unit<E>(self) -> Result<ToolInput, E> {
                Ok(ToolInput(None))
            }
        }
        deserializer.deserialize_any(Fields)
    }
}

/// The guard's answer for one tool input. Input past `limit` bytes cannot be read to its end, so it is denied rather than left without a decision.
fn edit_decision(input: impl Read, limit: u64) -> Result<Option<Value>, String> {
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
    let Some(file) = target.file else {
        return Ok(None);
    };
    Ok(edit_refusal(Path::new(&cwd), Path::new(&file)).map(|reason| deny(&reason)))
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

/// Why an agent may not change `file`, or `None` when it may. Inside the fold project that holds `cwd`, an agent changes files only inside one workspace below `.fold/ws/`: the project folder is main, which changes only through integration, and the rest of `.fold/` is fold's data. A path outside the project is left to Claude Code's own rules. `..` and the symlinks of the folders that exist are resolved first, so neither leads into or out of a workspace unseen.
fn edit_refusal(cwd: &Path, file: &Path) -> Option<String> {
    let cwd = std::fs::canonicalize(cwd).ok()?;
    let root = project_root_at_or_above(&cwd)?;
    let target = resolve(&cwd.join(file));
    let inside = target.strip_prefix(&root).ok()?;
    let in_workspace = inside
        .strip_prefix(fold_core::layout::WS_DIR)
        .is_ok_and(|below| below.components().count() >= 2);
    (!in_workspace).then(|| {
        "the project folder is fold main and changes only through integration; edit the file in your own workspace under .fold/ws/".to_owned()
    })
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
        let client = DaemonClient::new(running, daemon_client::human_from_env().ok());
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
        assert!(denied(edit_decision(into_main.as_bytes(), MAX_EDIT_INPUT)));

        let (_dir, into_workspace) = project_with_input(2 << 20, ".fold/ws/t-1/big.txt");
        assert!(!denied(edit_decision(
            into_workspace.as_bytes(),
            MAX_EDIT_INPUT
        )));
    }

    #[test]
    fn input_past_the_limit_is_denied_and_bad_input_is_an_error() {
        let (_dir, input) = project_with_input(1000, ".fold/ws/t-1/a.txt");
        assert!(denied(edit_decision(input.as_bytes(), 100)));
        assert!(edit_decision(&b"not json"[..], MAX_EDIT_INPUT).is_err());
        assert!(edit_decision(&br#"{"tool_input":{}}"#[..], MAX_EDIT_INPUT).is_err());
        assert!(
            edit_decision(&br#"{"cwd":"/","tool_input":{}}"#[..], MAX_EDIT_INPUT)
                .unwrap()
                .is_none()
        );
        assert!(
            edit_decision(&br#"{"cwd":"/","tool_input":null}"#[..], MAX_EDIT_INPUT)
                .unwrap()
                .is_none()
        );
    }
}
