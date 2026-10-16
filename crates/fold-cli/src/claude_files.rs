use std::io;
use std::path::{Path, PathBuf};

use fold_core::{FoldError, codes};
use serde_json::{Map, Value, json};
use thiserror::Error;

pub const MCP_FILE: &str = ".mcp.json";
pub const MCP_SERVERS_KEY: &str = "mcpServers";
/// The fold server's key below `mcpServers`.
pub const MCP_SERVER_NAME: &str = "fold";

pub const SETTINGS_FILE: &str = ".claude/settings.json";
/// The `permissions.allow` entry that allows every fold MCP tool; `mcp__fold__*` does the same.
pub const ALLOW_FOLD_TOOLS: &str = "mcp__fold";
/// The `permissions.deny` rules an earlier fold wrote, which a refresh removes. An agent's edits are checked by the `PreToolUse` hook instead: Claude Code reads an `Edit` path as a gitignore pattern, a deny rule always wins over an allow rule, and a `!` carve-out never reopens a path below a folder a deny rule matches, so no rule denies the project folder but `.fold/ws/` (<https://code.claude.com/docs/en/permissions#read-and-edit>).
const OLD_DENY: [&str; 2] = ["Edit(/main/**)", "Write(/main/**)"];
/// The Claude Code hook events and the fold command each one runs.
pub const HOOKS: [(&str, &str); 3] = [
    ("SessionStart", "fld hook session-start"),
    ("Stop", "fld hook stop"),
    ("PreToolUse", "fld hook pre-tool-use"),
];
/// The tools whose calls the `PreToolUse` hook sees: those that change a file named by its path.
const FILE_TOOLS: &str = "Edit|Write|MultiEdit|NotebookEdit";
/// A hook command that starts with this belongs to fold, and a refresh replaces it.
/// `fold hook` is what files written before the binary became `fld` hold; a refresh replaces those too.
const HOOK_PREFIXES: [&str; 2] = ["fld hook", "fold hook"];

pub const AGENTS_FILE: &str = "AGENTS.md";
/// fold's own text in `AGENTS.md` sits between these lines.
pub const AGENTS_BEGIN: &str = "<!-- fold:begin -->";
pub const AGENTS_END: &str = "<!-- fold:end -->";

pub const SKILL_FILE: &str = ".claude/skills/fold/SKILL.md";

/// Not named `AGENTS.md`, so an agent working in the fold repository never loads it as rules.
const AGENTS_SECTION: &str = include_str!("../templates/agents_section.md");
const SKILL: &str = include_str!("../templates/SKILL.md");

/// `details.reason` of a Claude Code file that fold cannot merge into.
pub const MALFORMED_REASON: &str = "malformed_claude_file";

/// Each of these in the project root or a parent directory makes Claude Code skip `AGENTS.md`.
pub const SHADOWING_FILES: [&str; 3] = ["CLAUDE.md", ".claude/CLAUDE.md", "CLAUDE.local.md"];
/// The user's own instructions below `HOME`, which never shadow `AGENTS.md`.
const USER_FILE: &str = ".claude/CLAUDE.md";

#[derive(Debug, Error)]
pub enum ClaudeFileError {
    /// `invalid_request` with `details.reason` [`MALFORMED_REASON`]; the file is left as it is.
    #[error(transparent)]
    Malformed(FoldError),
    #[error("cannot write {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl ClaudeFileError {
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Malformed(error) => Some(error.code()),
            Self::Io { .. } => None,
        }
    }
}

/// Writes or refreshes the four Claude Code files in the project root, keeping everything in them that fold does not own. Every file is checked before the first one is written, so a malformed file leaves all four untouched. A file whose content would not change is not written, so its modification time stays.
pub fn write(root: &Path) -> Result<(), ClaudeFileError> {
    let mcp_path = root.join(MCP_FILE);
    let settings_path = root.join(SETTINGS_FILE);
    let agents_path = root.join(AGENTS_FILE);
    let mcp = merge_json(&mcp_path, read(&mcp_path)?, merge_mcp)?;
    let settings = merge_json(&settings_path, read(&settings_path)?, merge_settings)?;
    let agents_text = match read(&agents_path)? {
        Some(bytes) => {
            Some(String::from_utf8(bytes).map_err(|_| malformed(&agents_path, "is not UTF-8"))?)
        }
        None => None,
    };
    let agents = agents_md(agents_text.as_deref(), AGENTS_SECTION)
        .map_err(|problem| malformed(&agents_path, problem))?;
    replace(&mcp_path, &mcp)?;
    replace(&settings_path, &settings)?;
    replace(&agents_path, agents.as_bytes())?;
    replace(&root.join(SKILL_FILE), SKILL.as_bytes())?;
    Ok(())
}

fn read(path: &Path) -> Result<Option<Vec<u8>>, ClaudeFileError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ClaudeFileError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

fn malformed(path: &Path, problem: &str) -> ClaudeFileError {
    ClaudeFileError::Malformed(
        FoldError::new(
            codes::INVALID_REQUEST,
            format!("{} {problem}", path.display()),
        )
        .with_details([
            ("reason", json!(MALFORMED_REASON)),
            ("path", json!(path.display().to_string())),
        ])
        .with_hint("correct or move the file, then run fld init --refresh"),
    )
}

/// The file's JSON object with `merge` applied, pretty-printed with a final newline.
fn merge_json(
    path: &Path,
    existing: Option<Vec<u8>>,
    merge: fn(&mut Map<String, Value>) -> Result<(), &'static str>,
) -> Result<Vec<u8>, ClaudeFileError> {
    let mut object = match existing {
        None => Map::new(),
        Some(bytes) => match serde_json::from_slice(&bytes) {
            Ok(Value::Object(object)) => object,
            Ok(_) => return Err(malformed(path, "does not hold a JSON object")),
            Err(_) => return Err(malformed(path, "is not valid JSON")),
        },
    };
    merge(&mut object).map_err(|problem| malformed(path, problem))?;
    let mut bytes = serde_json::to_vec_pretty(&Value::Object(object))
        .map_err(|_| malformed(path, "cannot be written as JSON"))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// The member `key` of `object` as an object, inserted empty when absent.
fn object_at<'a>(
    object: &'a mut Map<String, Value>,
    key: &str,
    problem: &'static str,
) -> Result<&'a mut Map<String, Value>, &'static str> {
    object
        .entry(key)
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or(problem)
}

/// The member `key` of `object` as an array, inserted empty when absent.
fn array_at<'a>(
    object: &'a mut Map<String, Value>,
    key: &str,
    problem: &'static str,
) -> Result<&'a mut Vec<Value>, &'static str> {
    object
        .entry(key)
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or(problem)
}

/// The fold server entry replaces any earlier one; other servers stay.
fn merge_mcp(object: &mut Map<String, Value>) -> Result<(), &'static str> {
    let servers = object_at(
        object,
        MCP_SERVERS_KEY,
        "has an mcpServers that is not an object",
    )?;
    servers.insert(
        MCP_SERVER_NAME.to_owned(),
        json!({
            "command": "fld",
            "args": ["mcp"],
            "env": {"FOLD_AGENT": "${FOLD_AGENT:-}"},
        }),
    );
    Ok(())
}

/// fold's permission rules and hook commands are removed wherever they are and appended again, so a refresh gives the same bytes and keeps every foreign entry in its place.
fn merge_settings(object: &mut Map<String, Value>) -> Result<(), &'static str> {
    let permissions = object_at(
        object,
        "permissions",
        "has a permissions that is not an object",
    )?;
    let allow = array_at(
        permissions,
        "allow",
        "has a permissions.allow that is not an array",
    )?;
    allow.retain(|rule| rule.as_str() != Some(ALLOW_FOLD_TOOLS));
    allow.push(json!(ALLOW_FOLD_TOOLS));
    if let Some(deny) = permissions.get_mut("deny") {
        deny.as_array_mut()
            .ok_or("has a permissions.deny that is not an array")?
            .retain(|rule| !rule.as_str().is_some_and(|rule| OLD_DENY.contains(&rule)));
    }

    let hooks = object_at(object, "hooks", "has a hooks that is not an object")?;
    for groups in hooks.values_mut() {
        let groups = groups
            .as_array_mut()
            .ok_or("has a hook event that is not an array")?;
        // A group that held only fold commands goes with them; a group that was empty already stays.
        groups.retain_mut(|group| {
            let Some(commands) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let before = commands.len();
            commands.retain(|hook| !is_fold_hook(hook));
            before == commands.len() || !commands.is_empty()
        });
    }
    for (event, command) in HOOKS {
        let groups = array_at(hooks, event, "has a hook event that is not an array")?;
        let hook = json!([{"type": "command", "command": command}]);
        groups.push(match event {
            "PreToolUse" => json!({"matcher": FILE_TOOLS, "hooks": hook}),
            _ => json!({"hooks": hook}),
        });
    }
    Ok(())
}

fn is_fold_hook(hook: &Value) -> bool {
    hook.get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| {
            let command = command.trim_start();
            HOOK_PREFIXES
                .iter()
                .any(|prefix| command.starts_with(prefix))
        })
}

/// The text of `AGENTS.md` with `section` between the markers. A file without markers keeps its bytes as the prefix and gains the section after one blank line; a file with one pair keeps every byte outside it. Markers count only as whole lines. The error says what is wrong with the markers.
pub fn agents_md(existing: Option<&str>, section: &str) -> Result<String, &'static str> {
    let body = section.trim_end_matches('\n');
    let block = format!("{AGENTS_BEGIN}\n{body}\n{AGENTS_END}\n");
    let existing = match existing {
        None | Some("") => return Ok(block),
        Some(text) => text,
    };
    let mut begins = Vec::new();
    let mut ends = Vec::new();
    let mut offset = 0;
    for line in existing.split_inclusive('\n') {
        let content = line.trim_end_matches('\n').trim_end_matches('\r');
        if content == AGENTS_BEGIN {
            begins.push(offset + line.len());
        } else if content == AGENTS_END {
            ends.push(offset);
        }
        offset += line.len();
    }
    match (begins.as_slice(), ends.as_slice()) {
        ([], []) => {
            let mut text = existing.to_owned();
            if !text.ends_with('\n') {
                text.push('\n');
            }
            text.push('\n');
            text.push_str(&block);
            Ok(text)
        }
        ([inner_start], [inner_end]) if inner_start <= inner_end => {
            let before = existing.get(..*inner_start).unwrap_or_default();
            let after = existing.get(*inner_end..).unwrap_or_default();
            Ok(format!("{before}{body}\n{after}"))
        }
        ([_], [_]) => Err("has the fold end marker before its begin marker"),
        ([], _) => Err("has a fold end marker without a begin marker"),
        (_, []) => Err("has a fold begin marker without an end marker"),
        _ => Err("has more than one fold section"),
    }
}

/// Atomic through `write_atomic`; an existing file keeps its permissions.
fn replace(path: &Path, content: &[u8]) -> Result<(), ClaudeFileError> {
    let io_error = |source| ClaudeFileError::Io {
        path: path.to_owned(),
        source,
    };
    if read(path)?.as_deref() == Some(content) {
        return Ok(());
    }
    fold_vcs::atomic::write_atomic(path, content).map_err(io_error)
}

/// The files that make Claude Code skip `AGENTS.md`, from the project root upwards. `root` must be absolute, or the walk stops at the current directory. `home` is the canonical `HOME`, whose `.claude/CLAUDE.md` is the user's own and is skipped.
pub fn shadowing_files(root: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for dir in root.ancestors() {
        for relative in SHADOWING_FILES {
            let user_file = relative == USER_FILE && home == Some(dir);
            let path = dir.join(relative);
            if !user_file && path.is_file() {
                found.push(path);
            }
        }
    }
    found
}

/// The line `fld init` prints for one file from [`shadowing_files`].
pub fn shadowing_warning(path: &Path) -> String {
    format!(
        "warning: {} makes Claude Code skip AGENTS.md; move its text into AGENTS.md and delete it",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refresh_drops_the_old_deny_rules_and_hooks_the_file_tools() {
        let mut settings =
            json!({"permissions": {"deny": ["Read(.env)", "Edit(/main/**)", "Write(/main/**)"]}});
        let object = settings.as_object_mut().unwrap();
        merge_settings(object).unwrap();
        merge_settings(object).unwrap();
        assert_eq!(settings["permissions"]["deny"], json!(["Read(.env)"]));
        assert_eq!(
            settings["hooks"]["PreToolUse"],
            json!([{"matcher": "Edit|Write|MultiEdit|NotebookEdit", "hooks": [{"type": "command", "command": "fld hook pre-tool-use"}]}])
        );
    }
}
