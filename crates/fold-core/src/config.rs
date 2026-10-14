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

/// The key set comes from the serialised defaults, so it cannot drift from the struct.
fn known_keys() -> Vec<String> {
    toml::Table::try_from(ProjectConfig::default())
        .map(|table| table.keys().cloned().collect())
        .unwrap_or_default()
}

fn refused(key: &str, problem: &str) -> FoldError {
    FoldError::new(codes::INVALID_CONFIG, format!("config key {key} {problem}"))
        .with_details(serde_json::json!({ "key": key }))
}

fn invalid(message: &str, reason: &str) -> FoldError {
    FoldError::new(codes::INVALID_CONFIG, format!("{message}: {reason}"))
        .with_details(serde_json::json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uncommented_template() -> String {
        CONFIG_TEMPLATE
            .lines()
            .filter_map(|line| line.strip_prefix("# "))
            .filter(|rest| rest.contains(" = "))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn config_template_matches_the_defaults() {
        let text = uncommented_template();
        let parsed = ProjectConfig::from_toml(&text).unwrap();
        assert_eq!(parsed, ProjectConfig::default());
        let table: toml::Table = toml::from_str(&text).unwrap();
        let mut named: Vec<_> = table.keys().cloned().collect();
        named.sort();
        let mut fields = known_keys();
        fields.sort();
        assert_eq!(named, fields);
        assert_eq!(fields.len(), 8);
        // Only comments: the file as written changes nothing.
        let as_written = ProjectConfig::from_toml(CONFIG_TEMPLATE).unwrap();
        assert_eq!(as_written, ProjectConfig::default());
    }

    #[test]
    fn config_rejects_unknown_keys_and_zero_values() {
        let unknown = ProjectConfig::from_toml("lease_minute = 3").unwrap_err();
        assert_eq!(unknown.code(), "invalid_config");
        assert_eq!(
            unknown.details(),
            Some(&serde_json::json!({"key": "lease_minute"}))
        );
        let zero = ProjectConfig::from_toml("debounce_ms = 0").unwrap_err();
        assert_eq!(zero.code(), "invalid_config");
        assert_eq!(
            zero.details(),
            Some(&serde_json::json!({"key": "debounce_ms"}))
        );
        for bad in [
            "stall_minutes = -1",
            "stall_minutes = \"5\"",
            "stall_minutes = 1.5",
        ] {
            let error = ProjectConfig::from_toml(bad).unwrap_err();
            assert_eq!(
                error.details(),
                Some(&serde_json::json!({"key": "stall_minutes"}))
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
}
