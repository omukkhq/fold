use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::claude_files::{
    self, ClaudeFileError, add_fold_server, add_fold_settings, json_bytes, link_end_of,
    parse_object, read_object, remove_fold_server, remove_fold_settings, replace,
};
use fold_core::layout::{ALLOW_FOLD_TOOLS, HOOKS, MCP_SERVER_NAME, MCP_SERVERS_KEY};

/// The hint of a malformed user file.
const USER_HINT: &str = "correct or move the file, then run `fld setup --claude`";

/// Claude Code's own files of the user, which fold's user-scope setup changes: the configuration that holds the user-scope MCP servers, and the user settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserFiles {
    /// `~/.claude.json`, whose top-level `mcpServers` are the user-scope servers.
    pub config: PathBuf,
    /// `~/.claude/settings.json`.
    pub settings: PathBuf,
}

impl UserFiles {
    /// The files Claude Code reads for the environment `var`: below `CLAUDE_CONFIG_DIR` when it is an absolute path, else below `HOME`. `None` without an absolute `HOME` or `CLAUDE_CONFIG_DIR`.
    pub fn from_vars(var: impl Fn(&str) -> Option<OsString>) -> Option<Self> {
        let absolute = |name: &str| {
            var(name)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        if let Some(dir) = absolute("CLAUDE_CONFIG_DIR") {
            return Some(Self {
                config: dir.join(".claude.json"),
                settings: dir.join("settings.json"),
            });
        }
        let home = absolute("HOME")?;
        Some(Self {
            config: home.join(".claude.json"),
            settings: home.join(".claude").join("settings.json"),
        })
    }

    pub fn from_env() -> Option<Self> {
        Self::from_vars(|name| std::env::var_os(name))
    }
}

/// One change to the JSON object of a user file; the error says what is wrong with the file.
type FileEdit = fn(&mut serde_json::Map<String, Value>) -> Result<(), &'static str>;

/// Whether the user files hold fold's entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserSetup {
    Complete,
    /// The problems with the entries, such as a missing server.
    Incomplete(Vec<String>),
}

impl UserSetup {
    pub fn is_complete(&self) -> bool {
        *self == Self::Complete
    }
}

/// Claude Code rewrites `~/.claude.json` while it runs, so a file that changed between fold's read and its write is read and merged again, up to this many times.
const ATTEMPTS: usize = 3;

/// Adds the fold server, the fold tools' permission and fold's hooks to the user files, keeping every entry that is not fold's. Returns whether a file changed.
pub fn add(files: &UserFiles) -> Result<bool, ClaudeFileError> {
    change(files, [add_fold_server, add_fold_settings])
}

/// Removes fold's entries from the user files and every container that held only them, keeping everything else; a file stays even when it is left empty. Returns whether a file changed.
pub fn remove(files: &UserFiles) -> Result<bool, ClaudeFileError> {
    change(
        files,
        [
            |object| remove_fold_server(object, true),
            |object| remove_fold_settings(object, true),
        ],
    )
}

/// Both files are read and checked before either is written, so a malformed one leaves both as they are. Each file is read again right before its write, and one that changed since the first read starts the change again from the new content: a running Claude Code can then lose a write only in the moment between that read and the rename. A new file is readable by its owner only, as Claude Code keeps its own.
fn change(files: &UserFiles, edits: [FileEdit; 2]) -> Result<bool, ClaudeFileError> {
    let mut changed = false;
    let mut raced = files.config.clone();
    'attempt: for _ in 0..ATTEMPTS {
        let mut writes = Vec::new();
        for (path, edit) in [&files.config, &files.settings].into_iter().zip(edits) {
            let end = link_end_of(path)?;
            let before = claude_files::read(&end)?;
            let existing = before
                .as_deref()
                .map(|bytes| parse_object(path, bytes, USER_HINT))
                .transpose()?;
            let mut object = existing.clone().unwrap_or_default();
            edit(&mut object)
                .map_err(|problem| claude_files::malformed_with(path, problem, USER_HINT))?;
            let same = existing
                .as_ref()
                .map_or(object.is_empty(), |existing| *existing == object);
            if !same {
                let bytes = json_bytes(path, object, USER_HINT)?;
                writes.push((end, before, bytes));
            }
        }
        for (end, before, bytes) in writes {
            if claude_files::read(&end)? != before {
                raced = end;
                continue 'attempt;
            }
            replace(&end, &bytes)?;
            changed = true;
            if before.is_none() {
                std::fs::set_permissions(&end, std::fs::Permissions::from_mode(0o600)).map_err(
                    |source| ClaudeFileError::Io {
                        path: end.clone(),
                        source,
                    },
                )?;
            }
        }
        return Ok(changed);
    }
    Err(ClaudeFileError::Io {
        path: raced,
        source: std::io::Error::other("it kept changing while fld wrote it; run the command again"),
    })
}

/// Whether the user's configuration holds the fold server.
pub fn holds_server(files: &UserFiles) -> bool {
    read_object(&files.config, USER_HINT)
        .ok()
        .flatten()
        .and_then(|config| config.get(MCP_SERVERS_KEY)?.get(MCP_SERVER_NAME).cloned())
        .is_some_and(|server| server.is_object())
}

/// Whether the user's settings allow the fold tools and run every `fld hook` command.
pub fn holds_settings(files: &UserFiles) -> bool {
    read_object(&files.settings, USER_HINT)
        .ok()
        .flatten()
        .map(Value::Object)
        .is_some_and(|settings| {
            allows_fold_tools(&settings)
                && HOOKS
                    .iter()
                    .all(|(event, command)| runs_hook(&settings, event, command))
        })
}

/// What the user files hold of fold's setup. A file that is absent or not valid JSON holds none of it.
pub fn state(files: &UserFiles) -> UserSetup {
    let read = |path: &Path| read_object(path, USER_HINT).ok().flatten();
    let name = |path: &Path| path.display().to_string();
    let mut problems = Vec::new();
    if !holds_server(files) {
        problems.push(format!("{} lacks the fold server", name(&files.config)));
    }
    match read(&files.settings).map(Value::Object) {
        None => problems.push(format!(
            "{} does not allow the fold tools and lacks the fold hooks",
            name(&files.settings)
        )),
        Some(settings) => {
            if !allows_fold_tools(&settings) {
                problems.push(format!(
                    "{} does not allow the fold tools",
                    name(&files.settings)
                ));
            }
            if !HOOKS
                .iter()
                .all(|(event, command)| runs_hook(&settings, event, command))
            {
                problems.push(format!("{} lacks the fold hooks", name(&files.settings)));
            }
        }
    }
    if problems.is_empty() {
        UserSetup::Complete
    } else {
        UserSetup::Incomplete(problems)
    }
}

/// Whether the settings allow every fold tool, by `mcp__fold` or `mcp__fold__*`.
pub fn allows_fold_tools(settings: &Value) -> bool {
    let wildcard = format!("{ALLOW_FOLD_TOOLS}__*");
    settings["permissions"]["allow"]
        .as_array()
        .is_some_and(|rules| {
            rules
                .iter()
                .filter_map(Value::as_str)
                .any(|rule| rule == ALLOW_FOLD_TOOLS || rule == wildcard)
        })
}

/// A hook command may name the binary by its full path, so only its end is compared.
pub fn runs_hook(settings: &Value, event: &str, command: &str) -> bool {
    let groups = settings["hooks"][event].as_array().into_iter().flatten();
    groups
        .flat_map(|group| group["hooks"].as_array().into_iter().flatten())
        .filter_map(|hook| hook["command"].as_str())
        .any(|run| run.trim().ends_with(command))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    fn files(home: &Path) -> UserFiles {
        UserFiles::from_vars(|name| (name == "HOME").then(|| home.as_os_str().to_owned())).unwrap()
    }

    #[test]
    fn claude_config_dir_moves_both_files_and_a_relative_one_counts_as_unset() {
        let vars = |config: &'static str| {
            move |name: &str| match name {
                "HOME" => Some(OsStr::new("/home/ana").to_owned()),
                "CLAUDE_CONFIG_DIR" => Some(OsStr::new(config).to_owned()),
                _ => None,
            }
        };
        let moved = UserFiles::from_vars(vars("/srv/claude")).unwrap();
        assert_eq!(moved.config, Path::new("/srv/claude/.claude.json"));
        assert_eq!(moved.settings, Path::new("/srv/claude/settings.json"));
        let home = UserFiles::from_vars(vars("claude")).unwrap();
        assert_eq!(home.config, Path::new("/home/ana/.claude.json"));
        assert_eq!(home.settings, Path::new("/home/ana/.claude/settings.json"));
        assert_eq!(UserFiles::from_vars(|_| None), None);
    }

    #[test]
    fn a_new_file_is_private_and_a_linked_file_stays_a_link() {
        let home = tempfile::tempdir().unwrap();
        let files = files(home.path());
        let dotfiles = home.path().join("dotfiles");
        std::fs::create_dir(&dotfiles).unwrap();
        std::fs::write(dotfiles.join("settings.json"), r#"{"theme": "dark"}"#).unwrap();
        std::fs::create_dir(home.path().join(".claude")).unwrap();
        std::os::unix::fs::symlink(dotfiles.join("settings.json"), &files.settings).unwrap();
        assert!(add(&files).unwrap());
        let mode = std::fs::metadata(&files.config)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(
            std::fs::symlink_metadata(&files.settings)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let linked: Value =
            serde_json::from_str(&std::fs::read_to_string(dotfiles.join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(linked["theme"], "dark");
        assert!(allows_fold_tools(&linked));
        assert!(state(&files).is_complete());
        assert!(!add(&files).unwrap(), "a second add changes nothing");
    }

    #[test]
    fn a_malformed_settings_file_leaves_the_config_unwritten() {
        let home = tempfile::tempdir().unwrap();
        let files = files(home.path());
        std::fs::create_dir(home.path().join(".claude")).unwrap();
        std::fs::write(&files.settings, r#"{"hooks": []}"#).unwrap();
        let error = add(&files).unwrap_err();
        assert_eq!(error.code(), Some("invalid_request"));
        assert_eq!(error.hint(), Some(USER_HINT));
        assert!(!files.config.exists());
        assert_eq!(
            state(&files),
            UserSetup::Incomplete(vec![
                format!("{} lacks the fold server", files.config.display()),
                format!("{} does not allow the fold tools", files.settings.display()),
                format!("{} lacks the fold hooks", files.settings.display()),
            ])
        );
        assert!(!remove(&files).is_ok_and(|changed| changed));
    }
}
