use std::io;
use std::path::{Path, PathBuf};

use fold_core::layout::MCP_COMMAND;
use fold_core::{FoldError, codes};
use serde_json::{Map, Value, json};
use thiserror::Error;

pub const MCP_FILE: &str = ".mcp.json";
pub use fold_core::layout::{AGENT_ENV, AGENT_TAGS_ENV, MCP_SERVER_NAME, MCP_SERVERS_KEY};

/// The folders of Claude Code files that fold's files sit in, each one inside the one before.
pub const FOLDERS: [&str; 3] = [".claude", ".claude/skills", ".claude/skills/fold"];

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
pub const CLAUDE_FILE: &str = "CLAUDE.md";
/// fold's own text in the file from [`instructions_file`] sits between these lines.
pub const AGENTS_BEGIN: &str = "<!-- fold:begin -->";
pub const AGENTS_END: &str = "<!-- fold:end -->";

pub const SKILL_FILE: &str = ".claude/skills/fold/SKILL.md";

/// Not named `AGENTS.md`, so an agent working in the fold repository never loads it as rules.
const AGENTS_SECTION: &str = include_str!("../templates/agents_section.md");
const SKILL: &str = include_str!("../templates/SKILL.md");

/// `details.reason` of a Claude Code file that fold cannot merge into.
pub const MALFORMED_REASON: &str = "malformed_claude_file";

/// Each of these in the project root or a parent directory makes Claude Code skip `AGENTS.md`. The root `CLAUDE.md` is the exception when it gets fold's section or links to the root `AGENTS.md`.
pub const SHADOWING_FILES: [&str; 3] = [CLAUDE_FILE, ".claude/CLAUDE.md", "CLAUDE.local.md"];
/// The user's own instructions below `HOME`, which never shadow `AGENTS.md`.
const USER_FILE: &str = ".claude/CLAUDE.md";

#[derive(Debug, Error)]
pub enum ClaudeFileError {
    /// `invalid_request` with `details.reason` [`MALFORMED_REASON`]; the file is left as it is.
    #[error(transparent)]
    Malformed(FoldError),
    /// `invalid_path`: the instructions file is a symbolic link that does not lead to a regular file inside the project folder; every file is left as it is.
    #[error(transparent)]
    Link(FoldError),
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
            Self::Malformed(error) | Self::Link(error) => Some(error.code()),
            Self::Io { .. } => None,
        }
    }

    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::Malformed(error) | Self::Link(error) => error.hint(),
            Self::Io { .. } => None,
        }
    }
}

/// The root file that gets fold's marked section, which is the one Claude Code reads: `CLAUDE.md` when it is there and is no symbolic link, else `AGENTS.md`. Claude Code reads a `CLAUDE.md` file in place of `AGENTS.md`, and a `CLAUDE.md` link usually leads to `AGENTS.md`.
pub fn instructions_file(root: &Path) -> &'static str {
    match std::fs::symlink_metadata(root.join(CLAUDE_FILE)) {
        Ok(meta) if !meta.file_type().is_symlink() => CLAUDE_FILE,
        _ => AGENTS_FILE,
    }
}

/// The root instructions file that does not get the section.
fn other_instructions_file(name: &str) -> &'static str {
    if name == CLAUDE_FILE {
        AGENTS_FILE
    } else {
        CLAUDE_FILE
    }
}

/// Whether both paths lead to one existing file.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The other root instructions file when a refresh would remove a fold section from it, such as `AGENTS.md` after a `CLAUDE.md` file was added. A file whose fold markers a refresh refuses counts too.
pub fn stale_section(root: &Path) -> Option<&'static str> {
    let project = std::fs::canonicalize(root).ok()?;
    let name = instructions_file(root);
    match without_stale_section(&project, root, name) {
        Ok(Some(_)) | Err(ClaudeFileError::Malformed(_)) => Some(other_instructions_file(name)),
        Ok(None) | Err(_) => None,
    }
}

/// Writes or refreshes the Claude Code files in the project root, keeping everything in them that fold does not own. The marked section goes into [`instructions_file`], and the other root instructions file loses a section an earlier write left there, so the project holds one. Every file is checked and every symbolic link followed before the first file is written, so a refusal leaves all of them untouched. A file whose content would not change is not written, so its modification time stays.
pub fn write(root: &Path) -> Result<(), ClaudeFileError> {
    let project = std::fs::canonicalize(root).map_err(|source| ClaudeFileError::Io {
        path: root.to_owned(),
        source,
    })?;
    let mcp_path = root.join(MCP_FILE);
    let settings_path = root.join(SETTINGS_FILE);
    let mcp = merge_json(&mcp_path, read(&mcp_path)?, merge_mcp)?;
    let settings = merge_json(&settings_path, read(&settings_path)?, merge_settings)?;
    let name = instructions_file(root);
    let instructions_path = root.join(name);
    let instructions_end = in_project(&project, &instructions_path)?;
    let instructions = agents_md(
        read_text(&instructions_path, &instructions_end)?.as_deref(),
        AGENTS_SECTION,
    )
    .map_err(|problem| malformed(&instructions_path, problem))?;
    let mut writes = vec![
        (link_end_of(&mcp_path)?, mcp),
        (link_end_of(&settings_path)?, settings),
        (instructions_end, instructions.into_bytes()),
    ];
    writes.extend(without_stale_section(&project, root, name)?);
    let skill_path = root.join(SKILL_FILE);
    writes.push((link_end_of(&skill_path)?, SKILL.as_bytes().to_vec()));
    for (path, content) in &writes {
        replace(path, content)?;
    }
    Ok(())
}

/// The other root instructions file without fold's section, when it holds one and is a regular file inside the project folder other than the one that gets the section. A link that leads elsewhere is left as it is.
fn without_stale_section(
    project: &Path,
    root: &Path,
    name: &str,
) -> Result<Option<(PathBuf, Vec<u8>)>, ClaudeFileError> {
    let path = root.join(other_instructions_file(name));
    let Ok(end) = std::fs::canonicalize(&path) else {
        return Ok(None);
    };
    if !end.starts_with(project) || !end.is_file() || same_file(&end, &root.join(name)) {
        return Ok(None);
    }
    let Some(text) = read_text(&path, &end)? else {
        return Ok(None);
    };
    let stripped = without_section(&text).map_err(|problem| malformed(&path, problem))?;
    Ok(stripped.map(|text| (end, text.into_bytes())))
}

/// The file fold writes for the instructions file at `path`: `path` itself when it is no symbolic link, else the regular file inside the project folder (`project`, canonical) that the link leads to. A link that leads elsewhere is refused, so the section is part of the project's revisions and reaches every workspace.
fn in_project(project: &Path, path: &Path) -> Result<PathBuf, ClaudeFileError> {
    let io_error = |source| ClaudeFileError::Io {
        path: path.to_owned(),
        source,
    };
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {}
        Ok(_) => return Ok(path.to_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(path.to_owned()),
        Err(source) => return Err(io_error(source)),
    }
    match std::fs::canonicalize(path) {
        Ok(end) if !end.starts_with(project) => {
            Err(link_refused(path, "leads outside the project folder"))
        }
        Ok(end) if end.is_file() => Ok(end),
        Ok(_) => Err(link_refused(path, "leads to no regular file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(link_refused(path, "leads to no file"))
        }
        Err(source) => Err(io_error(source)),
    }
}

fn link_refused(path: &Path, problem: &str) -> ClaudeFileError {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    ClaudeFileError::Link(
        FoldError::new(
            codes::INVALID_PATH,
            format!("{} is a symbolic link that {problem}", path.display()),
        )
        .with_details([("path", json!(path.display().to_string()))])
        .with_hint(format!(
            "point {name} at a file inside the project folder or replace the link with a file, then run the command again"
        )),
    )
}

/// The end of the links that start at `path`, which may lead anywhere and to a file that does not exist yet.
fn link_end_of(path: &Path) -> Result<PathBuf, ClaudeFileError> {
    link_end(path).map_err(|source| ClaudeFileError::Io {
        path: path.to_owned(),
        source,
    })
}

/// The text at `end`, the file `path` leads to; an error names `path`.
fn read_text(path: &Path, end: &Path) -> Result<Option<String>, ClaudeFileError> {
    match read(end)? {
        Some(bytes) => Ok(Some(
            String::from_utf8(bytes).map_err(|_| malformed(path, "is not UTF-8"))?,
        )),
        None => Ok(None),
    }
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
            "args": [MCP_COMMAND],
            "env": {(AGENT_ENV): format!("${{{AGENT_ENV}:-}}")},
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

/// Byte offsets of the one fold section in a text: the begin marker line starts at `begin` and ends at `inner_start`, the end marker line starts at `inner_end` and ends at `end`.
struct Section {
    begin: usize,
    inner_start: usize,
    inner_end: usize,
    end: usize,
}

/// The fold section of `text`, if it has one. Markers count only as whole lines. The error says what is wrong with the markers.
fn section(text: &str) -> Result<Option<Section>, &'static str> {
    let mut begins = Vec::new();
    let mut ends = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches('\n').trim_end_matches('\r');
        if content == AGENTS_BEGIN {
            begins.push((offset, offset + line.len()));
        } else if content == AGENTS_END {
            ends.push((offset, offset + line.len()));
        }
        offset += line.len();
    }
    match (begins.as_slice(), ends.as_slice()) {
        ([], []) => Ok(None),
        ([(begin, inner_start)], [(inner_end, end)]) if inner_start <= inner_end => {
            Ok(Some(Section {
                begin: *begin,
                inner_start: *inner_start,
                inner_end: *inner_end,
                end: *end,
            }))
        }
        ([_], [_]) => Err("has the fold end marker before its begin marker"),
        ([], _) => Err("has a fold end marker without a begin marker"),
        (_, []) => Err("has a fold begin marker without an end marker"),
        _ => Err("has more than one fold section"),
    }
}

/// The text of the instructions file with `section` between the markers. A file without markers keeps its bytes as the prefix and gains the section after one blank line; a file with one pair keeps every byte outside it. Markers count only as whole lines. The error says what is wrong with the markers.
pub fn agents_md(existing: Option<&str>, section_text: &str) -> Result<String, &'static str> {
    let body = section_text.trim_end_matches('\n');
    let block = format!("{AGENTS_BEGIN}\n{body}\n{AGENTS_END}\n");
    let existing = match existing {
        None | Some("") => return Ok(block),
        Some(text) => text,
    };
    match section(existing)? {
        None => {
            let mut text = existing.to_owned();
            if !text.ends_with('\n') {
                text.push('\n');
            }
            text.push('\n');
            text.push_str(&block);
            Ok(text)
        }
        Some(found) => {
            let before = existing.get(..found.inner_start).unwrap_or_default();
            let after = existing.get(found.inner_end..).unwrap_or_default();
            Ok(format!("{before}{body}\n{after}"))
        }
    }
}

/// The text without its fold section, both marker lines included, keeping every other byte; `None` when it has no section.
pub fn without_section(text: &str) -> Result<Option<String>, &'static str> {
    Ok(section(text)?.map(|found| {
        let before = text.get(..found.begin).unwrap_or_default();
        let after = text.get(found.end..).unwrap_or_default();
        format!("{before}{after}")
    }))
}

/// Atomic through `write_atomic`; an existing file keeps its permissions. `path` is the end of any symbolic link, so a link stays a link, also one whose file does not exist yet: the file it points to is the one that changes, because the user put the link there to keep the content elsewhere.
fn replace(path: &Path, content: &[u8]) -> Result<(), ClaudeFileError> {
    if read(path)?.as_deref() == Some(content) {
        return Ok(());
    }
    fold_vcs::atomic::write_atomic(path, content).map_err(|source| ClaudeFileError::Io {
        path: path.to_owned(),
        source,
    })
}

/// How many symbolic links [`link_end`] follows, as Linux does for one path.
const MAX_LINKS: usize = 40;

/// The path at the end of the chain of symbolic links that starts at `path`, or `path` itself when it is no link. The end need not exist.
fn link_end(path: &Path) -> io::Result<PathBuf> {
    let mut end = path.to_owned();
    for _ in 0..MAX_LINKS {
        match std::fs::symlink_metadata(&end) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let target = std::fs::read_link(&end)?;
                end = match end.parent() {
                    Some(parent) => parent.join(target),
                    None => target,
                };
            }
            Ok(_) => return Ok(end),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(end),
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other("too many levels of symbolic links"))
}

/// The files that make Claude Code skip `AGENTS.md`, from the project root upwards. `root` must be absolute, or the walk stops at the current directory. `home` is the canonical `HOME`, whose `.claude/CLAUDE.md` is the user's own and is skipped.
pub fn shadowing_files(root: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for dir in root.ancestors() {
        for relative in SHADOWING_FILES {
            let user_file = relative == USER_FILE && home == Some(dir);
            let path = dir.join(relative);
            let read_for_fold = dir == root && relative == CLAUDE_FILE && claude_md_is_read(root);
            if !user_file && !read_for_fold && path.is_file() {
                found.push(path);
            }
        }
    }
    found
}

/// Whether the root `CLAUDE.md` is the file that holds fold's section or a link to the root `AGENTS.md`, so the section is what Claude Code reads.
fn claude_md_is_read(root: &Path) -> bool {
    let claude = root.join(CLAUDE_FILE);
    match std::fs::symlink_metadata(&claude) {
        Ok(meta) if meta.file_type().is_symlink() => same_file(&claude, &root.join(AGENTS_FILE)),
        Ok(_) => true,
        Err(_) => false,
    }
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
    fn a_symlinked_file_stays_a_link_and_its_target_changes() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = root.path().join("shared");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("rules.md"), "# mine\n").unwrap();
        let link = root.path().join(AGENTS_FILE);
        std::os::unix::fs::symlink(elsewhere.join("rules.md"), &link).unwrap();
        write(root.path()).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let text = std::fs::read_to_string(elsewhere.join("rules.md")).unwrap();
        assert!(text.starts_with("# mine\n") && text.contains(AGENTS_BEGIN));
    }

    #[test]
    fn a_dangling_link_stays_a_link_and_its_file_is_created() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("shared")).unwrap();
        let link = root.path().join(MCP_FILE);
        std::os::unix::fs::symlink("shared/mcp.json", &link).unwrap();
        write(root.path()).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let text = std::fs::read_to_string(root.path().join("shared/mcp.json")).unwrap();
        assert!(text.contains(MCP_SERVER_NAME));
    }

    #[test]
    fn a_link_loop_is_an_error_and_both_links_stay() {
        let root = tempfile::tempdir().unwrap();
        let link = root.path().join(AGENTS_FILE);
        std::os::unix::fs::symlink("other.md", &link).unwrap();
        std::os::unix::fs::symlink(AGENTS_FILE, root.path().join("other.md")).unwrap();
        assert!(link_end(&link).is_err());
        assert!(write(root.path()).is_err());
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn the_files_sit_in_the_folders() {
        let parent = |file: &str| Path::new(file).parent().map(Path::to_owned);
        assert_eq!(parent(SETTINGS_FILE), Some(PathBuf::from(FOLDERS[0])));
        assert_eq!(parent(SKILL_FILE), Some(PathBuf::from(FOLDERS[2])));
        assert_eq!(parent(FOLDERS[2]), Some(PathBuf::from(FOLDERS[1])));
        assert_eq!(parent(FOLDERS[1]), Some(PathBuf::from(FOLDERS[0])));
    }

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
