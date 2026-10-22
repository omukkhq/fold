use std::io;
use std::path::{Path, PathBuf};

use fold_core::layout::{HOOKS, hook_group};
use fold_core::roles::Role;
use fold_core::{FoldError, codes};
use serde_json::{Map, Value, json};
use thiserror::Error;

/// The project's MCP servers. fold keeps its own server in the user's Claude Code configuration, so a refresh removes the entry an earlier fold wrote here.
pub const MCP_FILE: &str = ".mcp.json";
pub use fold_core::layout::{
    AGENT_ENV, AGENT_TAGS_ENV, ALLOW_FOLD_TOOLS, MCP_SERVER_NAME, MCP_SERVERS_KEY,
};

/// The folders of Claude Code files that fold's files sit in. Each folder after `.claude/skills` is the folder of one skill: the `fold` skill and the skill of each role.
pub const FOLDERS: [&str; 8] = [
    ".claude",
    ".claude/skills",
    ".claude/skills/fold",
    ".claude/skills/fold-main",
    ".claude/skills/fold-area-lead",
    ".claude/skills/fold-review-lead",
    ".claude/skills/fold-task-worker",
    ".claude/skills/fold-reviewer",
];

/// The project's Claude Code settings. A folder whose settings allow tools makes Claude Code warn when the folder is first trusted, so fold keeps its permission and hooks in the user's settings and a refresh removes the ones an earlier fold wrote here.
pub const SETTINGS_FILE: &str = ".claude/settings.json";
/// The `permissions.deny` rules an earlier fold wrote, which a refresh removes. An agent's edits are checked by the `PreToolUse` hook instead: Claude Code reads an `Edit` path as a gitignore pattern, a deny rule always wins over an allow rule, and a `!` carve-out never reopens a path below a folder a deny rule matches, so no rule denies the project folder but `.fold/ws/` (<https://code.claude.com/docs/en/permissions#read-and-edit>).
const OLD_DENY: [&str; 2] = ["Edit(/main/**)", "Write(/main/**)"];
/// A hook command that starts with this belongs to fold, and a refresh replaces it.
/// `fold hook` is what files written before the binary became `fld` hold; a refresh replaces those too.
const HOOK_PREFIXES: [&str; 2] = ["fld hook", "fold hook"];

pub const AGENTS_FILE: &str = "AGENTS.md";
pub const CLAUDE_FILE: &str = "CLAUDE.md";
/// fold's own text in the file from [`instructions_file`] sits between these lines.
pub const AGENTS_BEGIN: &str = "<!-- fold:begin -->";
pub const AGENTS_END: &str = "<!-- fold:end -->";

/// The `fold` skill: the index of the role skills, and the skill of a session opened by hand, which has no role.
pub const SKILL_FILE: &str = ".claude/skills/fold/SKILL.md";
/// The reference beside the skill: fold's file formats, the human's commands and the web app's screens, which a session reads when its task needs them instead of holding them in every context.
pub const REFERENCE_FILE: &str = ".claude/skills/fold/REFERENCE.md";
/// Each role has a skill of its own, so that one Skill call loads the steps of the role and nothing else.
pub const MAIN_SKILL_FILE: &str = ".claude/skills/fold-main/SKILL.md";
/// See [`MAIN_SKILL_FILE`].
pub const AREA_LEAD_SKILL_FILE: &str = ".claude/skills/fold-area-lead/SKILL.md";
/// See [`MAIN_SKILL_FILE`].
pub const REVIEW_LEAD_SKILL_FILE: &str = ".claude/skills/fold-review-lead/SKILL.md";
/// See [`MAIN_SKILL_FILE`].
pub const TASK_WORKER_SKILL_FILE: &str = ".claude/skills/fold-task-worker/SKILL.md";
/// See [`MAIN_SKILL_FILE`].
pub const REVIEWER_SKILL_FILE: &str = ".claude/skills/fold-reviewer/SKILL.md";
/// Every error code with what to do, which an agent reads only when a refusal's hint does not settle its next step.
pub const ERRORS_FILE: &str = ".claude/skills/fold/ERRORS.md";
/// The files of fold's skills, which fold owns whole.
pub const SKILL_FILES: [&str; 8] = [
    SKILL_FILE,
    REFERENCE_FILE,
    MAIN_SKILL_FILE,
    AREA_LEAD_SKILL_FILE,
    REVIEW_LEAD_SKILL_FILE,
    TASK_WORKER_SKILL_FILE,
    REVIEWER_SKILL_FILE,
    ERRORS_FILE,
];

/// Not named `AGENTS.md`, so an agent working in the fold repository never loads it as rules.
const AGENTS_SECTION: &str = include_str!("../templates/agents_section.md");
const REFERENCE: &str = include_str!("../templates/REFERENCE.md");
const ERRORS: &str = include_str!("../templates/ERRORS.md");

/// Joins the sections of one skill, so that an agent loads one skill for its role while each section has one source that several skills share. The first section holds the skill's frontmatter, and the shared rules always come last.
macro_rules! skill {
    ($($section:literal),+) => {
        concat!(
            $(include_str!(concat!("../templates/parts/", $section, ".md")), "\n",)+
            include_str!("../templates/parts/shared.md")
        )
    };
}

const SKILL: &str = skill!("index", "identity");
/// The skill of each role: every step of that role and the shared rules, so that a worker loads only what its role needs.
const MAIN_SKILL: &str = skill!(
    "main",
    "identity",
    "planning",
    "kinds",
    "workers",
    "building",
    "reviewing",
    "batch",
    "scope"
);
const AREA_LEAD_SKILL: &str = skill!("area_lead", "planning", "kinds", "workers");
const REVIEW_LEAD_SKILL: &str = skill!("review_lead", "scope", "workers");
const TASK_WORKER_SKILL: &str = skill!("task_worker", "building", "kinds");
const REVIEWER_SKILL: &str = skill!("reviewer", "batch", "reviewing", "scope");

/// The text fold writes into each of [`SKILL_FILES`], in the same order.
pub const SKILL_TEXTS: [&str; 8] = [
    SKILL,
    REFERENCE,
    MAIN_SKILL,
    AREA_LEAD_SKILL,
    REVIEW_LEAD_SKILL,
    TASK_WORKER_SKILL,
    REVIEWER_SKILL,
    ERRORS,
];

/// The skill file that holds the steps of `role`. The supervised main loads the interactive main agent's skill.
pub fn role_skill_file(role: Role) -> &'static str {
    match role {
        Role::Main => MAIN_SKILL_FILE,
        Role::AreaLead => AREA_LEAD_SKILL_FILE,
        Role::ReviewLead => REVIEW_LEAD_SKILL_FILE,
        Role::TaskWorker => TASK_WORKER_SKILL_FILE,
        Role::Reviewer => REVIEWER_SKILL_FILE,
    }
}

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

/// Writes or refreshes fold's Claude Code files in the project root, keeping everything in them that fold does not own: the marked section in [`instructions_file`], the fold skill with its reference and the skill of each role. The other root instructions file loses a section an earlier write left there, so the project holds one, and [`MCP_FILE`] and [`SETTINGS_FILE`] lose the entries an earlier fold wrote; a file that held nothing else goes. Every file is checked and every symbolic link followed before the first file changes, so a refusal leaves all of them untouched. A file whose content would not change is not written, so its modification time stays.
pub fn write(root: &Path) -> Result<(), ClaudeFileError> {
    let project = std::fs::canonicalize(root).map_err(|source| ClaudeFileError::Io {
        path: root.to_owned(),
        source,
    })?;
    let mut changes = old_entries_removed(root)?;
    let name = instructions_file(root);
    let instructions_path = root.join(name);
    let instructions_end = in_project(&project, &instructions_path)?;
    let instructions = agents_md(
        read_text(&instructions_path, &instructions_end)?.as_deref(),
        AGENTS_SECTION,
    )
    .map_err(|problem| malformed(&instructions_path, problem))?;
    changes.push(Change::Write(instructions_end, instructions.into_bytes()));
    if let Some((end, text)) = without_stale_section(&project, root, name)? {
        changes.push(Change::Write(end, text));
    }
    for (relative, text) in SKILL_FILES.into_iter().zip(SKILL_TEXTS) {
        changes.push(Change::Write(
            link_end_of(&root.join(relative))?,
            text.as_bytes().to_vec(),
        ));
    }
    for change in &changes {
        change.apply()?;
    }
    Ok(())
}

/// One file change of [`write`], decided before any file changes.
enum Change {
    /// The file at the end of any link gets these bytes.
    Write(PathBuf, Vec<u8>),
    /// The regular file goes.
    Remove(PathBuf),
}

impl Change {
    fn apply(&self) -> Result<(), ClaudeFileError> {
        match self {
            Self::Write(path, content) => replace(path, content),
            Self::Remove(path) => match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(source) => Err(ClaudeFileError::Io {
                    path: path.clone(),
                    source,
                }),
            },
        }
    }
}

/// The project files that still hold entries an earlier fold wrote there: its server in [`MCP_FILE`], or its permission, hooks or deny rules in [`SETTINGS_FILE`]. A file fold cannot read counts as holding none.
pub fn old_entries(root: &Path) -> Vec<&'static str> {
    [
        (MCP_FILE, remove_fold_server as Removal),
        (SETTINGS_FILE, remove_fold_settings),
    ]
    .into_iter()
    .filter(|(relative, remove)| {
        let path = root.join(relative);
        matches!(without_fold(&path, *remove, PROJECT_HINT), Ok(Some(_)))
    })
    .map(|(relative, _)| relative)
    .collect()
}

/// The changes that take fold's old entries out of the project's MCP and settings files. A file that is absent stays absent.
fn old_entries_removed(root: &Path) -> Result<Vec<Change>, ClaudeFileError> {
    let mut changes = Vec::new();
    for (relative, remove) in [
        (MCP_FILE, remove_fold_server as Removal),
        (SETTINGS_FILE, remove_fold_settings),
    ] {
        let path = root.join(relative);
        let Some(object) = without_fold(&path, remove, PROJECT_HINT)? else {
            continue;
        };
        let regular =
            std::fs::symlink_metadata(&path).is_ok_and(|meta| !meta.file_type().is_symlink());
        changes.push(if object.is_empty() && regular {
            Change::Remove(path)
        } else {
            Change::Write(
                link_end_of(&path)?,
                json_bytes(&path, object, PROJECT_HINT)?,
            )
        });
    }
    Ok(changes)
}

/// Takes fold's entries out of a JSON object; `prune` also takes out each container that held only fold's entries.
type Removal = fn(&mut Map<String, Value>, bool) -> Result<(), &'static str>;

/// The JSON object of the file at `path` without fold's entries, or `None` when the file is absent or holds none of them.
fn without_fold(
    path: &Path,
    remove: Removal,
    hint: &str,
) -> Result<Option<Map<String, Value>>, ClaudeFileError> {
    let Some(mut object) = read_object(path, hint)? else {
        return Ok(None);
    };
    let before = object.clone();
    remove(&mut object, true).map_err(|problem| malformed_with(path, problem, hint))?;
    Ok((object != before).then_some(object))
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
pub(crate) fn link_end_of(path: &Path) -> Result<PathBuf, ClaudeFileError> {
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

pub(crate) fn read(path: &Path) -> Result<Option<Vec<u8>>, ClaudeFileError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ClaudeFileError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

/// The hint of a malformed project file.
const PROJECT_HINT: &str = "correct or move the file, then run `fld init --refresh`";

fn malformed(path: &Path, problem: &str) -> ClaudeFileError {
    malformed_with(path, problem, PROJECT_HINT)
}

pub(crate) fn malformed_with(path: &Path, problem: &str, hint: &str) -> ClaudeFileError {
    ClaudeFileError::Malformed(
        FoldError::new(
            codes::INVALID_REQUEST,
            format!("{} {problem}", path.display()),
        )
        .with_details([
            ("reason", json!(MALFORMED_REASON)),
            ("path", json!(path.display().to_string())),
        ])
        .with_hint(hint),
    )
}

/// The JSON object the file at `path` holds, `None` when the file is absent.
pub(crate) fn read_object(
    path: &Path,
    hint: &str,
) -> Result<Option<Map<String, Value>>, ClaudeFileError> {
    read(path)?
        .map(|bytes| parse_object(path, &bytes, hint))
        .transpose()
}

/// The JSON object in `bytes`, the content of the file at `path`.
pub(crate) fn parse_object(
    path: &Path,
    bytes: &[u8],
    hint: &str,
) -> Result<Map<String, Value>, ClaudeFileError> {
    match serde_json::from_slice(bytes) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(_) => Err(malformed_with(path, "does not hold a JSON object", hint)),
        Err(_) => Err(malformed_with(path, "is not valid JSON", hint)),
    }
}

/// The object pretty-printed with a final newline, as Claude Code writes its own files.
pub(crate) fn json_bytes(
    path: &Path,
    object: Map<String, Value>,
    hint: &str,
) -> Result<Vec<u8>, ClaudeFileError> {
    let mut bytes = serde_json::to_vec_pretty(&Value::Object(object))
        .map_err(|_| malformed_with(path, "cannot be written as JSON", hint))?;
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

/// The fold server entry of the user's configuration, as `claude mcp add --scope user` writes an entry for `fld mcp`. It sets no `FOLD_AGENT`: Claude Code starts the server with its own environment, which holds the agent name in a session that fold started, and an unset name is a session that registers by hand.
pub(crate) fn add_fold_server(object: &mut Map<String, Value>) -> Result<(), &'static str> {
    let servers = object_at(
        object,
        MCP_SERVERS_KEY,
        "has an mcpServers that is not an object",
    )?;
    servers.insert(
        MCP_SERVER_NAME.to_owned(),
        json!({"type": "stdio", "command": "fld", "args": [fold_core::layout::MCP_COMMAND], "env": {}}),
    );
    Ok(())
}

pub(crate) fn remove_fold_server(
    object: &mut Map<String, Value>,
    prune: bool,
) -> Result<(), &'static str> {
    let Some(servers) = object.get_mut(MCP_SERVERS_KEY) else {
        return Ok(());
    };
    let servers = servers
        .as_object_mut()
        .ok_or("has an mcpServers that is not an object")?;
    if servers.remove(MCP_SERVER_NAME).is_some() && prune && servers.is_empty() {
        object.remove(MCP_SERVERS_KEY);
    }
    Ok(())
}

/// fold's permission rule and hook commands are removed wherever they are and appended again, so a second add gives the same bytes and keeps every foreign entry in its place.
pub(crate) fn add_fold_settings(object: &mut Map<String, Value>) -> Result<(), &'static str> {
    remove_fold_settings(object, false)?;
    let permissions = object_at(
        object,
        "permissions",
        "has a permissions that is not an object",
    )?;
    array_at(
        permissions,
        "allow",
        "has a permissions.allow that is not an array",
    )?
    .push(json!(ALLOW_FOLD_TOOLS));
    let hooks = object_at(object, "hooks", "has a hooks that is not an object")?;
    for (event, command) in HOOKS {
        array_at(hooks, event, "has a hook event that is not an array")?
            .push(hook_group(event, command));
    }
    Ok(())
}

/// A list of `permissions`, the problem when it is no array, and whether a rule of it is fold's.
type PermissionRule = (&'static str, &'static str, fn(&str) -> bool);

/// Removes fold's permission rule, the deny rules an earlier fold wrote and fold's hook commands. A hook group that held only fold commands goes with them; a group that was empty already stays.
pub(crate) fn remove_fold_settings(
    object: &mut Map<String, Value>,
    prune: bool,
) -> Result<(), &'static str> {
    if let Some(permissions) = object.get_mut("permissions") {
        let permissions = permissions
            .as_object_mut()
            .ok_or("has a permissions that is not an object")?;
        let before = permissions.len();
        let rules: [PermissionRule; 2] = [
            (
                "allow",
                "has a permissions.allow that is not an array",
                |rule| rule == ALLOW_FOLD_TOOLS,
            ),
            (
                "deny",
                "has a permissions.deny that is not an array",
                |rule| OLD_DENY.contains(&rule),
            ),
        ];
        for (key, problem, is_fold) in rules {
            let Some(list) = permissions.get_mut(key) else {
                continue;
            };
            let list = list.as_array_mut().ok_or(problem)?;
            let count = list.len();
            list.retain(|rule| !rule.as_str().is_some_and(is_fold));
            if prune && list.is_empty() && count > 0 {
                permissions.remove(key);
            }
        }
        if prune && permissions.is_empty() && before > 0 {
            object.remove("permissions");
        }
    }
    if let Some(hooks) = object.get_mut("hooks") {
        let hooks = hooks
            .as_object_mut()
            .ok_or("has a hooks that is not an object")?;
        let before = hooks.len();
        let mut emptied = Vec::new();
        for (event, groups) in hooks.iter_mut() {
            let groups = groups
                .as_array_mut()
                .ok_or("has a hook event that is not an array")?;
            let count = groups.len();
            groups.retain_mut(|group| {
                let Some(commands) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                    return true;
                };
                let before = commands.len();
                commands.retain(|hook| !is_fold_hook(hook));
                before == commands.len() || !commands.is_empty()
            });
            if groups.is_empty() && count > 0 {
                emptied.push(event.clone());
            }
        }
        if prune {
            for event in emptied {
                hooks.remove(&event);
            }
            if hooks.is_empty() && before > 0 {
                object.remove("hooks");
            }
        }
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
pub(crate) fn replace(path: &Path, content: &[u8]) -> Result<(), ClaudeFileError> {
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
        std::fs::create_dir_all(root.path().join(FOLDERS[2])).unwrap();
        let link = root.path().join(SKILL_FILE);
        std::os::unix::fs::symlink("../../../skill.md", &link).unwrap();
        write(root.path()).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let text = std::fs::read_to_string(root.path().join("skill.md")).unwrap();
        assert_eq!(text, SKILL);
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
        for file in SKILL_FILES {
            let folder = parent(file).unwrap();
            assert!(FOLDERS[2..].contains(&folder.to_str().unwrap()), "{file}");
        }
        for folder in &FOLDERS[2..] {
            assert_eq!(parent(folder), Some(PathBuf::from(FOLDERS[1])), "{folder}");
        }
        assert_eq!(parent(FOLDERS[1]), Some(PathBuf::from(FOLDERS[0])));
    }

    #[test]
    fn an_add_drops_the_old_deny_rules_and_hooks_the_file_tools() {
        let mut settings =
            json!({"permissions": {"deny": ["Read(.env)", "Edit(/main/**)", "Write(/main/**)"]}});
        let object = settings.as_object_mut().unwrap();
        add_fold_settings(object).unwrap();
        add_fold_settings(object).unwrap();
        assert_eq!(settings["permissions"]["deny"], json!(["Read(.env)"]));
        assert_eq!(settings["permissions"]["allow"], json!([ALLOW_FOLD_TOOLS]));
        assert_eq!(
            settings["hooks"]["PreToolUse"],
            json!([{"matcher": "Edit|Write|MultiEdit|NotebookEdit", "hooks": [{"type": "command", "command": "fld hook pre-tool-use"}]}])
        );
    }

    #[test]
    fn a_removal_prunes_only_the_containers_fold_emptied() {
        let mut settings = json!({
            "permissions": {"allow": ["mcp__fold"], "ask": []},
            "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "fld hook stop"}]}, {"matcher": "kept"}], "Notification": []},
        });
        let object = settings.as_object_mut().unwrap();
        remove_fold_settings(object, true).unwrap();
        assert_eq!(
            settings,
            json!({"permissions": {"ask": []}, "hooks": {"Stop": [{"matcher": "kept"}], "Notification": []}})
        );
        let mut only_fold = json!({"mcpServers": {"fold": {}}});
        remove_fold_server(only_fold.as_object_mut().unwrap(), true).unwrap();
        assert_eq!(only_fold, json!({}));
        let mut kept = json!({"mcpServers": {"fold": {}}});
        remove_fold_server(kept.as_object_mut().unwrap(), false).unwrap();
        assert_eq!(kept, json!({"mcpServers": {}}));
    }
}
