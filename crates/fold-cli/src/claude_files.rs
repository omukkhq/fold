use std::path::{Path, PathBuf};

pub(crate) const MCP_FILE: &str = ".mcp.json";
pub(crate) const MCP_SERVERS_KEY: &str = "mcpServers";
/// The fold server's key below `mcpServers`.
pub(crate) const MCP_SERVER_NAME: &str = "fold";

pub(crate) const SETTINGS_FILE: &str = ".claude/settings.json";
/// The `permissions.allow` entry that allows every fold MCP tool; `mcp__fold__*` does the same.
pub(crate) const ALLOW_FOLD_TOOLS: &str = "mcp__fold";
/// The Claude Code hook events and the fold command each one runs.
pub(crate) const HOOKS: [(&str, &str); 2] = [
    ("SessionStart", "fold hook session-start"),
    ("Stop", "fold hook stop"),
];

pub(crate) const AGENTS_FILE: &str = "AGENTS.md";
/// fold's own text in `AGENTS.md` sits between these lines.
pub(crate) const AGENTS_BEGIN: &str = "<!-- fold:begin -->";
pub(crate) const AGENTS_END: &str = "<!-- fold:end -->";

/// Each of these in the project root or a parent directory makes Claude Code skip `AGENTS.md`.
pub(crate) const SHADOWING_FILES: [&str; 3] = ["CLAUDE.md", ".claude/CLAUDE.md", "CLAUDE.local.md"];
/// The user's own instructions below `HOME`, which never shadow `AGENTS.md`.
const USER_FILE: &str = ".claude/CLAUDE.md";

/// The files that make Claude Code skip `AGENTS.md`, from the project root upwards. `root` must be absolute, or the walk stops at the current directory.
pub(crate) fn shadowing_files(root: &Path, home: Option<&Path>) -> Vec<PathBuf> {
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
