use serde::{Deserialize, Serialize};

use crate::error::{FoldError, codes};

/// The commented file `fold init` writes to `.fold/config.toml`. Every value line is commented out and holds the default.
pub const CONFIG_TEMPLATE: &str = include_str!("config_template.toml");

/// The per-project settings of `.fold/config.toml`. Every key is optional in the file and a value of 0 is refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectConfig {
    pub lease_minutes: u64,
    pub debounce_ms: u64,
    pub rescan_seconds: u64,
    pub auto_checkpoint_quiet_seconds: u64,
    pub auto_checkpoints_kept: u64,
    pub max_file_bytes: u64,
    pub stall_minutes: u64,
    pub closed_workspace_days: u64,
    pub git: GitConfig,
}

/// The `[git]` section: the remote Git repository and branch that `fold push` keeps current with main.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GitConfig {
    /// A URL or path, which can hold credentials, so it never goes in a log line or an error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    pub branch: String,
    pub push_on_integrate: bool,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            remote: None,
            branch: "main".to_owned(),
            push_on_integrate: false,
        }
    }
}

/// The section name in `config.toml`.
pub const GIT_SECTION: &str = "git";

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            lease_minutes: 30,
            debounce_ms: 500,
            rescan_seconds: 60,
            auto_checkpoint_quiet_seconds: 120,
            auto_checkpoints_kept: 20,
            max_file_bytes: 52_428_800,
            stall_minutes: 5,
            closed_workspace_days: 14,
            git: GitConfig::default(),
        }
    }
}

impl ProjectConfig {
    /// Parses the text of a config file. A refused key is named in `details.key`.
    pub fn from_toml(text: &str) -> Result<Self, FoldError> {
        let table: toml::Table = toml::from_str(text)
            .map_err(|error| invalid("config.toml is not valid TOML", error.message()))?;
        let known = known_keys();
        for (key, value) in &table {
            if key == GIT_SECTION {
                check_git_section(value)?;
                continue;
            }
            if !known.contains(key) {
                return Err(refused(key, "is not a config key"));
            }
            match value.as_integer() {
                Some(number) if number > 0 => {}
                _ => return Err(refused(key, "must be a whole number above 0")),
            }
        }
        table
            .try_into()
            .map_err(|error: toml::de::Error| invalid("config.toml is not valid", error.message()))
    }
}

/// The top-level keys outside a section come from the serialised defaults, so they cannot drift from the struct.
fn known_keys() -> Vec<String> {
    toml::Table::try_from(ProjectConfig::default())
        .map(|table| {
            table
                .into_iter()
                .filter(|(_, value)| !value.is_table())
                .map(|(key, _)| key)
                .collect()
        })
        .unwrap_or_default()
}

fn check_git_section(value: &toml::Value) -> Result<(), FoldError> {
    let table = value
        .as_table()
        .ok_or_else(|| refused(GIT_SECTION, "must be a table"))?;
    for (key, value) in table {
        let name = format!("{GIT_SECTION}.{key}");
        match key.as_str() {
            "remote" => match value.as_str() {
                Some(remote) => check_remote(remote).map_err(|_| {
                    refused(
                        &name,
                        "must be a URL or path that does not start with `-` or use the ext or fd transport",
                    )
                })?,
                None => return Err(refused(&name, "must be a string")),
            },
            "branch" => match value.as_str() {
                Some(branch) => check_branch(branch)
                    .map_err(|_| refused(&name, "must be a valid Git branch name"))?,
                None => return Err(refused(&name, "must be a string")),
            },
            "push_on_integrate" => {
                if !value.is_bool() {
                    return Err(refused(&name, "must be true or false"));
                }
            }
            _ => return Err(refused(&name, "is not a config key")),
        }
    }
    Ok(())
}

/// A remote Git would read as an option, that holds a control character, or that names the `ext::` transport, which runs a command, or `fd::`, is refused. The error never repeats the text, which can hold credentials.
pub fn check_remote(remote: &str) -> Result<(), FoldError> {
    let transport = remote
        .split_once("::")
        .map(|(transport, _)| transport.to_ascii_lowercase());
    if remote.is_empty()
        || remote.starts_with('-')
        || remote.chars().any(char::is_control)
        || matches!(transport.as_deref(), Some("ext" | "fd"))
    {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            "a remote must not be empty, start with `-`, hold a control character or use the ext or fd transport",
        )
        .with_hint("pass the URL or path of the remote Git repository"));
    }
    Ok(())
}

/// The branch names `git check-ref-format --branch` accepts, so `refs/heads/<branch>` is a valid ref in every repository fold names it in.
pub fn check_branch(branch: &str) -> Result<(), FoldError> {
    let bad_char =
        |c: char| c.is_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\');
    let bad_component =
        |part: &str| part.is_empty() || part.starts_with('.') || part.ends_with(".lock");
    let valid = !branch.is_empty()
        && branch != "@"
        && !branch.starts_with('-')
        && !branch.ends_with('.')
        && !branch.contains("..")
        && !branch.contains("@{")
        && !branch.chars().any(bad_char)
        && !branch.split('/').any(bad_component);
    if valid {
        Ok(())
    } else {
        Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!("{branch:?} is not a valid Git branch name"),
        )
        .with_hint("pass a branch name such as main or release/1.0"))
    }
}

fn refused(key: &str, problem: &str) -> FoldError {
    FoldError::new(codes::INVALID_CONFIG, format!("config key {key} {problem}"))
        .with_details([("key", serde_json::json!(key))])
}

fn invalid(message: &str, reason: &str) -> FoldError {
    FoldError::new(codes::INVALID_CONFIG, format!("{message}: {reason}")).with_details([])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uncommented_template() -> String {
        CONFIG_TEMPLATE
            .lines()
            .filter_map(|line| line.strip_prefix("# "))
            .filter(|rest| rest.contains(" = ") || (rest.starts_with('[') && rest.ends_with(']')))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Every key of a table, a section's keys as `section.key`.
    fn dotted_keys(table: &toml::Table) -> Vec<String> {
        let mut keys = Vec::new();
        for (key, value) in table {
            match value.as_table() {
                Some(section) => keys.extend(section.keys().map(|inner| format!("{key}.{inner}"))),
                None => keys.push(key.clone()),
            }
        }
        keys.sort();
        keys
    }

    #[test]
    fn config_template_matches_the_defaults() {
        let text = uncommented_template();
        let parsed = ProjectConfig::from_toml(&text).unwrap();
        assert_eq!(parsed, ProjectConfig::default());
        let table: toml::Table = toml::from_str(&text).unwrap();
        let named = dotted_keys(&table);
        let fields = dotted_keys(&toml::Table::try_from(ProjectConfig::default()).unwrap());
        // `remote` has no default, so the template names it in a comment only.
        assert!(CONFIG_TEMPLATE.contains("`remote`"));
        assert_eq!(named, fields);
        assert_eq!(fields.len(), 10);
        assert_eq!(known_keys().len(), 8);
        // Only comments: the file as written changes nothing.
        let as_written = ProjectConfig::from_toml(CONFIG_TEMPLATE).unwrap();
        assert_eq!(as_written, ProjectConfig::default());
    }

    #[test]
    fn config_rejects_unknown_keys_and_zero_values() {
        let unknown = ProjectConfig::from_toml("lease_minute = 3").unwrap_err();
        assert_eq!(unknown.code(), "invalid_config");
        assert_eq!(
            unknown.details().cloned().map(serde_json::Value::Object),
            Some(serde_json::json!({"key": "lease_minute"}))
        );
        let zero = ProjectConfig::from_toml("debounce_ms = 0").unwrap_err();
        assert_eq!(zero.code(), "invalid_config");
        assert_eq!(
            zero.details().cloned().map(serde_json::Value::Object),
            Some(serde_json::json!({"key": "debounce_ms"}))
        );
        for bad in [
            "stall_minutes = -1",
            "stall_minutes = \"5\"",
            "stall_minutes = 1.5",
        ] {
            let error = ProjectConfig::from_toml(bad).unwrap_err();
            assert_eq!(
                error.details().cloned().map(serde_json::Value::Object),
                Some(serde_json::json!({"key": "stall_minutes"}))
            );
        }
        assert_eq!(
            ProjectConfig::from_toml("= nope").unwrap_err().code(),
            "invalid_config"
        );
    }

    #[test]
    fn present_keys_override_and_absent_keys_keep_defaults() {
        let config = ProjectConfig::from_toml("lease_minutes = 7").unwrap();
        assert_eq!(config.lease_minutes, 7);
        assert_eq!(config.debounce_ms, 500);
        assert_eq!(
            ProjectConfig::from_toml("").unwrap(),
            ProjectConfig::default()
        );
    }

    #[test]
    fn the_git_section_takes_a_remote_a_branch_and_a_switch() {
        let config = ProjectConfig::from_toml(
            "lease_minutes = 3\n[git]\nremote = \"../app.git\"\nbranch = \"release/1.0\"\npush_on_integrate = true\n",
        )
        .unwrap();
        assert_eq!(config.lease_minutes, 3);
        assert_eq!(
            config.git,
            GitConfig {
                remote: Some("../app.git".to_owned()),
                branch: "release/1.0".to_owned(),
                push_on_integrate: true,
            }
        );
        let defaults = ProjectConfig::from_toml("[git]\n").unwrap();
        assert_eq!(defaults.git, GitConfig::default());
        assert_eq!(defaults.git.branch, "main");
    }

    #[test]
    fn a_wrong_type_in_the_git_section_is_invalid_config() {
        for (text, key) in [
            ("git = 3", "git"),
            ("[git]\nremote = 3", "git.remote"),
            ("[git]\nremote = \"-x\"", "git.remote"),
            ("[git]\nbranch = 1", "git.branch"),
            ("[git]\nbranch = \"a..b\"", "git.branch"),
            (
                "[git]\npush_on_integrate = \"yes\"",
                "git.push_on_integrate",
            ),
            ("[git]\nremotes = \"x\"", "git.remotes"),
        ] {
            let error = ProjectConfig::from_toml(text).unwrap_err();
            assert_eq!(error.code(), "invalid_config", "{text}");
            assert_eq!(
                error.details().cloned().map(serde_json::Value::Object),
                Some(serde_json::json!({"key": key})),
                "{text}"
            );
        }
        let secret = "https://user:token@example.com/app.git\n";
        let error = check_remote(secret).unwrap_err();
        assert!(!error.message().contains("token"));
    }

    #[test]
    fn branch_names_follow_the_git_rules() {
        for good in ["main", "release/1.0", "feature-x", "a_b", "v1@2"] {
            assert!(check_branch(good).is_ok(), "{good}");
        }
        for bad in [
            "", "@", "-x", "a..b", "a b", "a~1", "a^", "a:b", "a?", "a*", "a[", "a\\b", "a@{1}",
            "/a", "a/", "a//b", ".a", "a/.b", "a.lock", "a.", "a\nb",
        ] {
            assert_eq!(
                check_branch(bad).unwrap_err().code(),
                codes::INVALID_REQUEST,
                "{bad:?}"
            );
        }
        for bad in ["", "-u", "a\nb", "ext::sh -c touch% x", "EXT::sh", "fd::7"] {
            assert_eq!(
                check_remote(bad).unwrap_err().code(),
                codes::INVALID_REQUEST
            );
        }
        assert!(check_remote("git@example.com:app.git").is_ok());
        assert!(check_remote("https://example.com/ext::x.git").is_ok());
        assert!(check_remote("gcrypt::https://example.com/app.git").is_ok());
    }
}
