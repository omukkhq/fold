use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::services::agents::claude::env_value;

/// The user's own Claude Code files, found from the launcher's environment as Claude Code finds them: below an absolute `CLAUDE_CONFIG_DIR`, else below `HOME`.
pub(super) struct UserClaude {
    /// `~/.claude.json`, which holds each project folder's state, such as its trust and its approved `.mcp.json` servers.
    config: PathBuf,
    /// `~/.claude/settings.json`.
    settings: PathBuf,
}

impl UserClaude {
    pub(super) fn of(env: &[(String, String)]) -> Option<Self> {
        let absolute = |name: &str| {
            env_value(env, name)
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

    /// The entry of `root` in `~/.claude.json`, under the project folder's path as given or as its real path.
    fn project_entry(&self, root: &Path) -> Value {
        let config = read_json(&self.config);
        let real = root.canonicalize().unwrap_or_else(|_| root.to_owned());
        [root, real.as_path()]
            .into_iter()
            .filter_map(Path::to_str)
            .find_map(|key| config["projects"].get(key).cloned())
            .unwrap_or(Value::Null)
    }

    /// Whether the user accepted Claude Code's trust dialog for the project folder `root`.
    pub(super) fn trusts(&self, root: &Path) -> bool {
        self.project_entry(root)["hasTrustDialogAccepted"].as_bool() == Some(true)
    }

    /// The user's approval of the project's `.mcp.json` servers for `root`, from the user's own files only: the project's entry in `~/.claude.json`, the user settings and the project folder's `.claude/settings.local.json`. The repository's shared `.claude/settings.json` never approves its own servers.
    pub(super) fn approval(&self, root: &Path) -> Approval {
        let sources = [
            self.project_entry(root),
            read_json(&self.settings),
            read_json(&root.join(fold_core::layout::LOCAL_SETTINGS_FILE)),
        ];
        Approval::of(&sources)
    }
}

/// Which `.mcp.json` servers the user approved.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Approval {
    all: bool,
    enabled: BTreeSet<String>,
    disabled: BTreeSet<String>,
}

impl Approval {
    fn of(sources: &[Value]) -> Self {
        let mut approval = Self::default();
        for source in sources {
            approval.all |= source["enableAllProjectMcpServers"].as_bool() == Some(true);
            for (key, into) in [
                ("enabledMcpjsonServers", &mut approval.enabled),
                ("disabledMcpjsonServers", &mut approval.disabled),
            ] {
                into.extend(
                    source[key]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_owned),
                );
            }
        }
        approval
    }

    /// A server the user turned off stays off, even when every server is on.
    pub(super) fn allows(&self, name: &str) -> bool {
        !self.disabled.contains(name) && (self.all || self.enabled.contains(name))
    }
}

/// The `permissions.allow` rules of the project's `.claude/settings.json`, which Claude Code applies only in a folder the user trusted. A worker runs in its start folder, which Claude Code never asked the user to trust, so fold passes the rules of a trusted project folder with the worker's own settings.
pub(super) fn trusted_allow_rules(root: &Path) -> Vec<Value> {
    read_json(&root.join(".claude/settings.json"))["permissions"]["allow"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// `text`, a settings file of the project, with every `$CLAUDE_PROJECT_DIR` and `${CLAUDE_PROJECT_DIR}` of its hook commands replaced by the project folder `root`: Claude Code sets the variable to the folder a session runs in, which for a worker is its start folder, so a project hook that names a script by it keeps finding the script. Text that is no JSON object is kept as it is.
pub(super) fn with_project_dir(text: &str, root: &Path) -> String {
    let Ok(mut settings) = serde_json::from_str::<Value>(text) else {
        return text.to_owned();
    };
    let Some(root) = root.to_str() else {
        return text.to_owned();
    };
    let Some(events) = settings.get_mut("hooks").and_then(Value::as_object_mut) else {
        return text.to_owned();
    };
    for groups in events.values_mut().filter_map(Value::as_array_mut) {
        for group in groups {
            for hook in group["hooks"].as_array_mut().into_iter().flatten() {
                if let Some(command) = hook["command"].as_str() {
                    let replaced = replace_project_dir(command, root);
                    hook["command"] = Value::String(replaced);
                }
            }
        }
    }
    settings.to_string()
}

/// `command` with the variable replaced by `root`, quoted for where it stands: inside double quotes the path escapes what the shell expands there, elsewhere it goes in single quotes.
fn replace_project_dir(command: &str, root: &str) -> String {
    let mut out = String::new();
    let mut rest = command;
    let mut in_double = false;
    let mut in_single = false;
    loop {
        let names = ["${CLAUDE_PROJECT_DIR}", "$CLAUDE_PROJECT_DIR"];
        let found = names
            .iter()
            .filter_map(|name| rest.find(name).map(|at| (at, *name)))
            .min_by_key(|(at, name)| (*at, std::cmp::Reverse(name.len())));
        let Some((at, name)) = found else {
            out.push_str(rest);
            return out;
        };
        let (before, after) = rest.split_at(at);
        let mut escaped = false;
        for c in before.chars() {
            match c {
                _ if escaped => escaped = false,
                '\\' if !in_single => escaped = true,
                '"' if !in_single => in_double = !in_double,
                '\'' if !in_double => in_single = !in_single,
                _ => {}
            }
        }
        out.push_str(before);
        let after = &after[name.len()..];
        // `$CLAUDE_PROJECT_DIRX` names another variable, and single quotes expand nothing.
        let other = name == "$CLAUDE_PROJECT_DIR"
            && after
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        if other || in_single {
            out.push_str(name);
        } else if in_double {
            for c in root.chars() {
                if matches!(c, '"' | '\\' | '$' | '`') {
                    out.push('\\');
                }
                out.push(c);
            }
        } else {
            out.push('\'');
            out.push_str(&root.replace('\'', "'\\''"));
            out.push('\'');
        }
        rest = after;
    }
}

fn read_json(path: &Path) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn only_a_server_the_user_approved_and_did_not_turn_off_is_allowed() {
        let none = Approval::of(&[Value::Null]);
        assert!(!none.allows("pencil"));
        let some = Approval::of(&[
            json!({"enabledMcpjsonServers": ["pencil"], "disabledMcpjsonServers": ["db"]}),
            json!({"enabledMcpjsonServers": ["db", "docs"]}),
        ]);
        assert!(some.allows("pencil") && some.allows("docs"));
        assert!(!some.allows("db") && !some.allows("other"));
        let all = Approval::of(&[
            json!({"enableAllProjectMcpServers": true}),
            json!({"disabledMcpjsonServers": ["db"]}),
        ]);
        assert!(all.allows("anything"));
        assert!(!all.allows("db"));
    }

    #[test]
    fn the_approval_and_the_trust_come_from_the_users_own_files_for_the_root() {
        let home = tempfile::TempDir::new().unwrap();
        let root = home.path().join("app");
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        let key = root.to_str().unwrap();
        std::fs::write(
            home.path().join(".claude.json"),
            json!({"projects": {(key): {"enabledMcpjsonServers": ["pencil"], "hasTrustDialogAccepted": true}}}).to_string(),
        )
        .unwrap();
        // The repository's shared settings never approve its own servers.
        std::fs::write(
            root.join(".claude/settings.json"),
            json!({"enableAllProjectMcpServers": true}).to_string(),
        )
        .unwrap();
        std::fs::write(
            root.join(".claude/settings.local.json"),
            json!({"enabledMcpjsonServers": ["docs"]}).to_string(),
        )
        .unwrap();
        let env = vec![(
            "HOME".to_owned(),
            home.path().to_string_lossy().into_owned(),
        )];
        let user = UserClaude::of(&env).unwrap();
        let approval = user.approval(&root);
        assert!(approval.allows("pencil") && approval.allows("docs"));
        assert!(!approval.allows("other"));
        assert!(user.trusts(&root));
        assert!(!user.trusts(&home.path().join("elsewhere")));
        assert!(UserClaude::of(&[("HOME".to_owned(), "relative".to_owned())]).is_none());
    }

    #[test]
    fn a_hook_command_names_the_project_folder_quoted_for_where_the_variable_stands() {
        let root = "/home/ana/my app";
        for (command, expected) in [
            (
                "$CLAUDE_PROJECT_DIR/.claude/hooks/check.sh",
                "'/home/ana/my app'/.claude/hooks/check.sh",
            ),
            (
                "\"$CLAUDE_PROJECT_DIR\"/hooks/a.sh",
                "\"/home/ana/my app\"/hooks/a.sh",
            ),
            (
                "bash \"${CLAUDE_PROJECT_DIR}/hooks/a.sh\"",
                "bash \"/home/ana/my app/hooks/a.sh\"",
            ),
            ("echo '$CLAUDE_PROJECT_DIR'", "echo '$CLAUDE_PROJECT_DIR'"),
            ("echo $CLAUDE_PROJECT_DIRS", "echo $CLAUDE_PROJECT_DIRS"),
            ("make lint", "make lint"),
        ] {
            assert_eq!(replace_project_dir(command, root), expected, "{command}");
        }
        assert_eq!(
            replace_project_dir("$CLAUDE_PROJECT_DIR/x", "/a/it's"),
            "'/a/it'\\''s'/x"
        );
        let settings = json!({
            "env": {"A": "$CLAUDE_PROJECT_DIR"},
            "hooks": {"PostToolUse": [{"matcher": "Edit", "hooks": [{"type": "command", "command": "$CLAUDE_PROJECT_DIR/fmt.sh"}]}]},
        })
        .to_string();
        let replaced: Value =
            serde_json::from_str(&with_project_dir(&settings, Path::new("/app"))).unwrap();
        assert_eq!(
            replaced["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
            "'/app'/fmt.sh"
        );
        assert_eq!(replaced["env"]["A"], "$CLAUDE_PROJECT_DIR");
        assert_eq!(with_project_dir("not json", Path::new("/app")), "not json");
    }
}
