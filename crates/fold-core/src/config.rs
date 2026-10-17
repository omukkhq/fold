use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::agents::roles::{CacheTtl, Role, WorkerLimits, is_command_word as plain_word};
use crate::error::{FoldError, codes};

/// The commented file `fld init` writes to `.fold/config.toml`. Every value line is commented out and holds the default.
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
    /// The remote Git repositories by name, as `[remotes.<name>]` sections.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub remotes: BTreeMap<String, RemoteConfig>,
    pub integration: IntegrationConfig,
    pub workers: WorkersConfig,
    /// The settings of each worker role by role name, as `[roles.<role>]` sections; a role without one takes its defaults.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub roles: BTreeMap<String, RoleConfig>,
}

/// The `[workers]` section: how many short-lived workers of each role fold runs at once, and optionally how many task coders in one area. The leads are outside these limits. A limit of 0 lets no worker of its role start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkersConfig {
    pub task_coder: u64,
    pub batch_reviewer: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_per_area: Option<u64>,
}

impl Default for WorkersConfig {
    fn default() -> Self {
        Self {
            task_coder: 4,
            batch_reviewer: 3,
            max_per_area: None,
        }
    }
}

impl WorkersConfig {
    pub fn limits(self) -> WorkerLimits {
        WorkerLimits {
            task_coder: self.task_coder,
            batch_reviewer: self.batch_reviewer,
            max_per_area: self.max_per_area,
        }
    }
}

/// One `[roles.<role>]` section. Every key is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleConfig {
    /// The Claude Code model name or alias, passed as `--model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_ttl: Option<CacheTtl>,
    /// Only for the short-lived roles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_minutes: Option<u64>,
    /// The MCP servers the role may load, by name: `fold` and servers of the project's `.mcp.json`. Without it the role loads every server the user's own Claude Code loads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<Vec<String>>,
    /// Only for the long-lived roles: the share of the model's context window, in percent, past which fold gives the worker a fresh context through a hand-off note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_at: Option<u64>,
}

/// A role's settings with the defaults filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleSettings {
    pub model: Option<String>,
    pub cache_ttl: CacheTtl,
    /// `None` for a long-lived role, which never stops for being idle.
    pub idle_minutes: Option<u64>,
    pub mcp: Option<Vec<String>>,
    /// `None` for a short-lived role, which stops when its work is done and is never refreshed.
    pub handoff_at: Option<u64>,
}

/// How long a short-lived worker may stay idle before fold stops it, when its role sets no `idle_minutes`.
pub const DEFAULT_IDLE_MINUTES: u64 = 15;

/// The share of its model's context window, in percent, past which a long-lived worker gets a fresh context, when its role sets no `handoff_at`.
pub const DEFAULT_HANDOFF_AT: u64 = 60;

/// The `[git]` section: the branch of the project folder's repository `.git` that every integration is committed to and the folder has checked out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GitConfig {
    pub branch: String,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            branch: "main".to_owned(),
        }
    }
}

/// One `[remotes.<name>]` section: a Git repository that `fld push` and `fld pull` sync main with. Its `Debug` form leaves the URL out.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteConfig {
    /// A URL or path, which can hold credentials, so it never goes in a log line or an error.
    pub url: String,
    /// The branch of the remote that main is pushed to and pulled from.
    pub branch: String,
    /// Whether fold pushes to this remote after each integration.
    #[serde(default)]
    pub push_on_integrate: bool,
}

impl std::fmt::Debug for RemoteConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteConfig")
            .field("branch", &self.branch)
            .field("push_on_integrate", &self.push_on_integrate)
            .finish_non_exhaustive()
    }
}

/// The section names in `config.toml`.
pub const GIT_SECTION: &str = "git";
pub const REMOTES_SECTION: &str = "remotes";
pub const INTEGRATION_SECTION: &str = "integration";
pub const WORKERS_SECTION: &str = "workers";
pub const ROLES_SECTION: &str = "roles";

/// The `[integration]` section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IntegrationConfig {
    /// fold integrates each proposal that becomes ready by itself. With `false`, a proposal integrates only when an agent that decides reviews or a human calls for it.
    pub automatic: bool,
}

impl Default for IntegrationConfig {
    fn default() -> Self {
        Self { automatic: true }
    }
}

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
            remotes: BTreeMap::new(),
            integration: IntegrationConfig::default(),
            workers: WorkersConfig::default(),
            roles: BTreeMap::new(),
        }
    }
}

impl ProjectConfig {
    pub fn role(&self, role: Role) -> RoleSettings {
        let section = self.roles.get(role.as_str()).cloned().unwrap_or_default();
        RoleSettings {
            model: section.model,
            cache_ttl: section.cache_ttl.unwrap_or(role.default_cache_ttl()),
            idle_minutes: (!role.is_long_lived())
                .then(|| section.idle_minutes.unwrap_or(DEFAULT_IDLE_MINUTES)),
            mcp: section.mcp,
            handoff_at: role
                .is_long_lived()
                .then(|| section.handoff_at.unwrap_or(DEFAULT_HANDOFF_AT)),
        }
    }

    /// Parses the text of a config file. A refused key is named in `details.key`. The keys `remote` and `push_on_integrate` of `[git]` name an unnamed remote, which is read as the remote `origin` on the `[git]` branch.
    pub fn from_toml(text: &str) -> Result<Self, FoldError> {
        let mut table: toml::Table = toml::from_str(text)
            .map_err(|error| invalid("config.toml is not valid TOML", error.message()))?;
        lift_unnamed_remote(&mut table)?;
        let known = known_keys();
        for (key, value) in &table {
            if key == GIT_SECTION {
                check_git_section(value)?;
                continue;
            }
            if key == REMOTES_SECTION {
                check_remotes_section(value)?;
                continue;
            }
            if key == INTEGRATION_SECTION {
                check_integration_section(value)?;
                continue;
            }
            if key == WORKERS_SECTION {
                check_workers_section(value)?;
                continue;
            }
            if key == ROLES_SECTION {
                check_roles_section(value)?;
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
            "branch" => check_branch_value(&name, value)?,
            _ => return Err(refused(&name, "is not a config key")),
        }
    }
    Ok(())
}

/// The name a remote from `[git].remote` takes.
pub const UNNAMED_REMOTE: &str = "origin";

/// Moves `remote` and `push_on_integrate` out of `[git]` into `[remotes.origin]`, whose branch is the `[git]` branch. `push_on_integrate` without a remote did nothing, so it is dropped. A file that also has `[remotes.origin]` names the remote twice and is refused.
fn lift_unnamed_remote(table: &mut toml::Table) -> Result<(), FoldError> {
    let Some(git) = table
        .get_mut(GIT_SECTION)
        .and_then(toml::Value::as_table_mut)
    else {
        return Ok(());
    };
    let url = git.remove("remote");
    let push_on_integrate = git.remove("push_on_integrate");
    let name = format!("{GIT_SECTION}.push_on_integrate");
    let push_on_integrate = match push_on_integrate {
        None => false,
        Some(toml::Value::Boolean(value)) => value,
        Some(_) => return Err(refused(&name, "must be true or false")),
    };
    let Some(url) = url else {
        return Ok(());
    };
    let key = format!("{GIT_SECTION}.remote");
    match url.as_str() {
        Some(text) => check_remote(text).map_err(|_| refused(&key, REMOTE_PROBLEM))?,
        None => return Err(refused(&key, "must be a string")),
    }
    let branch = git
        .get("branch")
        .cloned()
        .unwrap_or_else(|| toml::Value::String(GitConfig::default().branch));
    let remotes = table
        .entry(REMOTES_SECTION)
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let Some(remotes) = remotes.as_table_mut() else {
        return Ok(());
    };
    if remotes.contains_key(UNNAMED_REMOTE) {
        return Err(refused(
            &key,
            "names a second remote origin besides [remotes.origin]",
        )
        .with_hint("delete remote and push_on_integrate from [git] in .fold/config.toml"));
    }
    let mut origin = toml::Table::new();
    origin.insert("url".to_owned(), url);
    origin.insert("branch".to_owned(), branch);
    origin.insert(
        "push_on_integrate".to_owned(),
        toml::Value::Boolean(push_on_integrate),
    );
    remotes.insert(UNNAMED_REMOTE.to_owned(), toml::Value::Table(origin));
    Ok(())
}

const REMOTE_PROBLEM: &str =
    "must be a URL or path that does not start with `-` or use the ext or fd transport";

fn check_remotes_section(value: &toml::Value) -> Result<(), FoldError> {
    let table = value
        .as_table()
        .ok_or_else(|| refused(REMOTES_SECTION, "must be a table"))?;
    for (remote, value) in table {
        let section = format!("{REMOTES_SECTION}.{remote}");
        check_remote_name(remote).map_err(|_| refused(&section, "is not a valid remote name"))?;
        let fields = value
            .as_table()
            .ok_or_else(|| refused(&section, "must be a table"))?;
        for required in ["url", "branch"] {
            if !fields.contains_key(required) {
                return Err(refused(&format!("{section}.{required}"), "is required"));
            }
        }
        for (key, value) in fields {
            let name = format!("{section}.{key}");
            match key.as_str() {
                "url" => match value.as_str() {
                    Some(url) => check_remote(url).map_err(|_| refused(&name, REMOTE_PROBLEM))?,
                    None => return Err(refused(&name, "must be a string")),
                },
                "branch" => check_branch_value(&name, value)?,
                "push_on_integrate" => {
                    if !value.is_bool() {
                        return Err(refused(&name, "must be true or false"));
                    }
                }
                _ => return Err(refused(&name, "is not a config key")),
            }
        }
    }
    Ok(())
}

fn check_integration_section(value: &toml::Value) -> Result<(), FoldError> {
    let table = value
        .as_table()
        .ok_or_else(|| refused(INTEGRATION_SECTION, "must be a table"))?;
    for (key, value) in table {
        let name = format!("{INTEGRATION_SECTION}.{key}");
        match key.as_str() {
            "automatic" if value.is_bool() => {}
            "automatic" => return Err(refused(&name, "must be true or false")),
            _ => return Err(refused(&name, "is not a config key")),
        }
    }
    Ok(())
}

fn check_workers_section(value: &toml::Value) -> Result<(), FoldError> {
    let table = value
        .as_table()
        .ok_or_else(|| refused(WORKERS_SECTION, "must be a table"))?;
    for (key, value) in table {
        let name = format!("{WORKERS_SECTION}.{key}");
        match key.as_str() {
            "task_coder" | "batch_reviewer" | "max_per_area" => check_limit(&name, value)?,
            _ => return Err(refused(&name, "is not a config key")),
        }
    }
    Ok(())
}

fn check_roles_section(value: &toml::Value) -> Result<(), FoldError> {
    let table = value
        .as_table()
        .ok_or_else(|| refused(ROLES_SECTION, "must be a table"))?;
    for (role_name, value) in table {
        let section = format!("{ROLES_SECTION}.{role_name}");
        let role = Role::parse(role_name).ok_or_else(|| {
            refused(
                &section,
                "is not a role: use area_lead, review_lead, task_coder or batch_reviewer",
            )
        })?;
        let fields = value
            .as_table()
            .ok_or_else(|| refused(&section, "must be a table"))?;
        for (key, value) in fields {
            let name = format!("{section}.{key}");
            match key.as_str() {
                "model" => match value.as_str() {
                    Some(model) if plain_word(model) => {}
                    _ => return Err(refused(&name, "must be a model name such as sonnet")),
                },
                "cache_ttl" => {
                    if value.as_str().and_then(CacheTtl::parse).is_none() {
                        return Err(refused(&name, "must be \"1h\" or \"5m\""));
                    }
                }
                "idle_minutes" if role.is_long_lived() => {
                    return Err(refused(
                        &name,
                        "applies only to task_coder and batch_reviewer",
                    ));
                }
                "idle_minutes" => check_positive(&name, value)?,
                "mcp" => {
                    let names = value
                        .as_array()
                        .ok_or_else(|| refused(&name, "must be a list of server names"))?;
                    if !names
                        .iter()
                        .all(|server| server.as_str().is_some_and(plain_word))
                    {
                        return Err(refused(&name, "must be a list of server names"));
                    }
                }
                "handoff_at" if !role.is_long_lived() => {
                    return Err(refused(&name, "applies only to area_lead and review_lead"));
                }
                "handoff_at" => match value.as_integer() {
                    Some(percent) if (1..=99).contains(&percent) => {}
                    _ => return Err(refused(&name, "must be a whole percent from 1 to 99")),
                },
                _ => return Err(refused(&name, "is not a config key")),
            }
        }
    }
    Ok(())
}

fn check_positive(name: &str, value: &toml::Value) -> Result<(), FoldError> {
    match value.as_integer() {
        Some(number) if number > 0 => Ok(()),
        _ => Err(refused(name, "must be a whole number above 0")),
    }
}

/// A worker limit has no upper bound, and 0 lets no worker of its role start.
fn check_limit(name: &str, value: &toml::Value) -> Result<(), FoldError> {
    match value.as_integer() {
        Some(number) if number >= 0 => Ok(()),
        _ => Err(refused(name, "must be a whole number, 0 or more")),
    }
}

fn check_branch_value(name: &str, value: &toml::Value) -> Result<(), FoldError> {
    match value.as_str() {
        Some(branch) => {
            check_branch(branch).map_err(|_| refused(name, "must be a valid Git branch name"))
        }
        None => Err(refused(name, "must be a string")),
    }
}

/// A remote name is what Git accepts for one and fold can put in a ref and a TOML key unquoted: 1 to 64 ASCII letters, digits, `.`, `_` or `-`, not starting with `.` or `-` and without `..`.
pub fn check_remote_name(name: &str) -> Result<(), FoldError> {
    let valid = (1..=64).contains(&name.len())
        && !name.starts_with(['.', '-'])
        && !name.contains("..")
        && !name.ends_with(".lock")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if valid {
        Ok(())
    } else {
        Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!("{name:?} is not a valid remote name"),
        )
        .with_hint("name the remote with letters, digits, `.`, `_` or `-`, such as origin"))
    }
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
        // No remote exists by default, so the template names the section in a comment only.
        assert!(CONFIG_TEMPLATE.contains("`[remotes.<name>]`"));
        assert_eq!(named, fields);
        assert_eq!(fields.len(), 12);
        assert_eq!(known_keys().len(), 8);
        // Only comments: the file as written changes nothing.
        let as_written = ProjectConfig::from_toml(CONFIG_TEMPLATE).unwrap();
        assert_eq!(as_written, ProjectConfig::default());
    }

    #[test]
    fn the_workers_and_roles_sections_set_limits_and_role_settings() {
        let config = ProjectConfig::from_toml(
            "[workers]\ntask_coder = 5\nbatch_reviewer = 2\nmax_per_area = 1\n[roles.task_coder]\nmodel = \"sonnet\"\ncache_ttl = \"1h\"\nidle_minutes = 3\nmcp = [\"fold\", \"pencil\"]\n[roles.area_lead]\ncache_ttl = \"5m\"\n",
        )
        .unwrap();
        assert_eq!(
            config.workers.limits(),
            WorkerLimits {
                task_coder: 5,
                batch_reviewer: 2,
                max_per_area: Some(1)
            }
        );
        let open = ProjectConfig::from_toml(
            "[workers]\ntask_coder = 0\nbatch_reviewer = 500\nmax_per_area = 0\n",
        )
        .unwrap();
        assert_eq!(
            open.workers.limits(),
            WorkerLimits {
                task_coder: 0,
                batch_reviewer: 500,
                max_per_area: Some(0)
            }
        );
        let coder = config.role(Role::TaskCoder);
        assert_eq!(coder.model.as_deref(), Some("sonnet"));
        assert_eq!(coder.cache_ttl, CacheTtl::OneHour);
        assert_eq!(coder.idle_minutes, Some(3));
        assert_eq!(
            coder.mcp,
            Some(vec!["fold".to_owned(), "pencil".to_owned()])
        );
        assert_eq!(config.role(Role::AreaLead).cache_ttl, CacheTtl::FiveMinutes);
        let defaults = ProjectConfig::default();
        assert_eq!(
            defaults.workers.limits(),
            WorkerLimits {
                task_coder: 4,
                batch_reviewer: 3,
                max_per_area: None
            }
        );
        let reviewer = defaults.role(Role::BatchReviewer);
        assert_eq!(reviewer.cache_ttl, CacheTtl::OneHour);
        assert_eq!(reviewer.idle_minutes, Some(DEFAULT_IDLE_MINUTES));
        assert_eq!(reviewer.mcp, None);
        let lead = defaults.role(Role::ReviewLead);
        assert_eq!(lead.idle_minutes, None);
        assert_eq!(lead.model, None);
        assert_eq!(
            defaults.role(Role::TaskCoder).cache_ttl,
            CacheTtl::FiveMinutes
        );
    }

    #[test]
    fn handoff_at_is_a_percent_for_the_long_lived_roles() {
        let config = ProjectConfig::from_toml("[roles.review_lead]\nhandoff_at = 45\n").unwrap();
        assert_eq!(config.role(Role::ReviewLead).handoff_at, Some(45));
        assert_eq!(
            config.role(Role::AreaLead).handoff_at,
            Some(DEFAULT_HANDOFF_AT)
        );
        assert_eq!(config.role(Role::TaskCoder).handoff_at, None);
        assert_eq!(config.role(Role::BatchReviewer).handoff_at, None);
        for (text, key) in [
            (
                "[roles.task_coder]\nhandoff_at = 50",
                "roles.task_coder.handoff_at",
            ),
            (
                "[roles.area_lead]\nhandoff_at = 0",
                "roles.area_lead.handoff_at",
            ),
            (
                "[roles.area_lead]\nhandoff_at = 100",
                "roles.area_lead.handoff_at",
            ),
            (
                "[roles.area_lead]\nhandoff_at = \"60%\"",
                "roles.area_lead.handoff_at",
            ),
        ] {
            let error = ProjectConfig::from_toml(text).unwrap_err();
            assert_eq!(error.code(), "invalid_config", "{text}");
            assert_eq!(
                error.details().cloned().map(serde_json::Value::Object),
                Some(serde_json::json!({"key": key})),
                "{text}"
            );
        }
    }

    #[test]
    fn a_wrong_value_in_the_workers_or_roles_sections_is_invalid_config() {
        for (text, key) in [
            ("workers = 3", "workers"),
            ("[workers]\ntask_coder = -1", "workers.task_coder"),
            ("[workers]\nbatch_reviewer = -1", "workers.batch_reviewer"),
            ("[workers]\nmax_per_area = 1.5", "workers.max_per_area"),
            ("[workers]\nmax = 3", "workers.max"),
            ("[workers]\nmax_per_area = \"2\"", "workers.max_per_area"),
            ("[workers]\nmin = 1", "workers.min"),
            ("roles = 3", "roles"),
            ("[roles]\nmain = {}", "roles.main"),
            ("[roles]\ntask_coder = 3", "roles.task_coder"),
            (
                "[roles.task_coder]\nmodel = \"-x\"",
                "roles.task_coder.model",
            ),
            ("[roles.task_coder]\nmodel = 3", "roles.task_coder.model"),
            (
                "[roles.task_coder]\ncache_ttl = \"2h\"",
                "roles.task_coder.cache_ttl",
            ),
            (
                "[roles.area_lead]\nidle_minutes = 5",
                "roles.area_lead.idle_minutes",
            ),
            (
                "[roles.task_coder]\nidle_minutes = 0",
                "roles.task_coder.idle_minutes",
            ),
            ("[roles.task_coder]\nmcp = \"fold\"", "roles.task_coder.mcp"),
            (
                "[roles.task_coder]\nmcp = [\"a b\"]",
                "roles.task_coder.mcp",
            ),
            (
                "[roles.task_coder]\nprompt = \"x\"",
                "roles.task_coder.prompt",
            ),
        ] {
            let error = ProjectConfig::from_toml(text).unwrap_err();
            assert_eq!(error.code(), "invalid_config", "{text}");
            assert_eq!(
                error.details().cloned().map(serde_json::Value::Object),
                Some(serde_json::json!({"key": key})),
                "{text}"
            );
        }
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
    fn the_git_section_takes_a_branch_and_each_remote_its_own_section() {
        let config = ProjectConfig::from_toml(
            "lease_minutes = 3\n[git]\nbranch = \"trunk\"\n[remotes.origin]\nurl = \"../app.git\"\nbranch = \"release/1.0\"\npush_on_integrate = true\n[remotes.backup]\nurl = \"/srv/backup.git\"\nbranch = \"main\"\n",
        )
        .unwrap();
        assert_eq!(config.lease_minutes, 3);
        assert_eq!(config.git.branch, "trunk");
        assert_eq!(
            config.remotes["origin"],
            RemoteConfig {
                url: "../app.git".to_owned(),
                branch: "release/1.0".to_owned(),
                push_on_integrate: true,
            }
        );
        assert!(!config.remotes["backup"].push_on_integrate);
        let shown = format!("{:?}", config.remotes["origin"]);
        assert!(!shown.contains("app.git"), "{shown}");
        assert!(shown.contains("release/1.0"), "{shown}");
        let defaults = ProjectConfig::from_toml("[git]\n").unwrap();
        assert_eq!(defaults.git, GitConfig::default());
        assert_eq!(defaults.git.branch, "main");
        assert!(defaults.remotes.is_empty());
    }

    #[test]
    fn a_wrong_value_in_the_git_or_remote_sections_is_invalid_config() {
        for (text, key) in [
            ("git = 3", "git"),
            ("[git]\nbranch = 1", "git.branch"),
            ("[git]\nbranch = \"a..b\"", "git.branch"),
            ("[git]\nremote = 3", "git.remote"),
            ("[git]\nremote = \"-x\"", "git.remote"),
            ("remotes = 3\n[git]\nremote = \"/a.git\"", "remotes"),
            ("[git]\npush_on_integrate = 1", "git.push_on_integrate"),
            (
                "[git]\nremote = \"a\"\n[remotes.origin]\nurl = \"b\"\nbranch = \"main\"",
                "git.remote",
            ),
            ("[git]\nremotes = \"x\"", "git.remotes"),
            ("remotes = 3", "remotes"),
            ("[remotes]\norigin = 3", "remotes.origin"),
            (
                "[remotes.\"-x\"]\nurl = \"a\"\nbranch = \"main\"",
                "remotes.-x",
            ),
            ("[remotes.origin]\nbranch = \"main\"", "remotes.origin.url"),
            ("[remotes.origin]\nurl = \"a\"", "remotes.origin.branch"),
            (
                "[remotes.origin]\nurl = 3\nbranch = \"main\"",
                "remotes.origin.url",
            ),
            (
                "[remotes.origin]\nurl = \"-x\"\nbranch = \"main\"",
                "remotes.origin.url",
            ),
            (
                "[remotes.origin]\nurl = \"a\"\nbranch = \"a b\"",
                "remotes.origin.branch",
            ),
            (
                "[remotes.origin]\nurl = \"a\"\nbranch = \"main\"\npush_on_integrate = \"yes\"",
                "remotes.origin.push_on_integrate",
            ),
            (
                "[remotes.origin]\nurl = \"a\"\nbranch = \"main\"\nkind = \"x\"",
                "remotes.origin.kind",
            ),
        ] {
            let error = ProjectConfig::from_toml(text).unwrap_err();
            assert_eq!(error.code(), "invalid_config", "{text}");
            assert_eq!(
                error.details().cloned().map(serde_json::Value::Object),
                Some(serde_json::json!({"key": key})),
                "{text}"
            );
        }
        let twice = ProjectConfig::from_toml(
            "[git]\nremote = \"a\"\n[remotes.origin]\nurl = \"b\"\nbranch = \"main\"",
        )
        .unwrap_err();
        assert!(twice.hint().unwrap().contains("delete remote"));
        let secret = "https://user:token@example.com/app.git\n";
        let error = check_remote(secret).unwrap_err();
        assert!(!error.message().contains("token"));
    }

    #[test]
    fn the_integration_section_switches_automatic_integration() {
        assert!(ProjectConfig::default().integration.automatic);
        let off = ProjectConfig::from_toml("[integration]\nautomatic = false\n").unwrap();
        assert!(!off.integration.automatic);
        assert_eq!(off.git, GitConfig::default());
        let defaults = ProjectConfig::from_toml("[integration]\n").unwrap();
        assert!(defaults.integration.automatic);
        for (text, key) in [
            ("integration = true", "integration"),
            ("[integration]\nautomatic = 1", "integration.automatic"),
            ("[integration]\nauto = true", "integration.auto"),
        ] {
            let error = ProjectConfig::from_toml(text).unwrap_err();
            assert_eq!(error.code(), "invalid_config", "{text}");
            assert_eq!(
                error.details().cloned().map(serde_json::Value::Object),
                Some(serde_json::json!({"key": key})),
                "{text}"
            );
        }
    }

    #[test]
    fn the_unnamed_remote_of_the_git_section_is_origin() {
        let config = ProjectConfig::from_toml(
            "[git]\nremote = \"../app.git\"\nbranch = \"trunk\"\npush_on_integrate = true\n[remotes.backup]\nurl = \"/b.git\"\nbranch = \"main\"\n",
        )
        .unwrap();
        assert_eq!(config.git.branch, "trunk");
        assert_eq!(
            config.remotes[UNNAMED_REMOTE],
            RemoteConfig {
                url: "../app.git".to_owned(),
                branch: "trunk".to_owned(),
                push_on_integrate: true,
            }
        );
        assert_eq!(config.remotes.len(), 2);
        let plain = ProjectConfig::from_toml("[git]\nremote = \"/a.git\"\n").unwrap();
        assert_eq!(plain.remotes[UNNAMED_REMOTE].branch, "main");
        assert!(!plain.remotes[UNNAMED_REMOTE].push_on_integrate);
        let switch_only = ProjectConfig::from_toml("[git]\npush_on_integrate = true\n").unwrap();
        assert!(switch_only.remotes.is_empty());
    }

    #[test]
    fn remote_names_are_plain_words() {
        for good in ["origin", "back-up", "a.b", "v_2", "A1"] {
            assert!(check_remote_name(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            ".a",
            "-a",
            "a..b",
            "a/b",
            "a b",
            "a:b",
            "x.lock",
            &"a".repeat(65),
        ] {
            assert_eq!(
                check_remote_name(bad).unwrap_err().code(),
                codes::INVALID_REQUEST,
                "{bad:?}"
            );
        }
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
