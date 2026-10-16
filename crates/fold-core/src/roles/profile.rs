use std::collections::BTreeSet;

use serde_json::json;

use super::{check_model, is_command_word};
use crate::agents::AreaName;
use crate::config::RoleSettings;
use crate::error::{FoldError, codes};

/// The most characters an area's instructions hold, the same bound as a hand-off note, because both go into every start of a worker.
pub const MAX_AREA_INSTRUCTIONS_CHARS: usize = 8000;

/// The kind of work of an area, which its area lead and its task coders start with. A field left `None` falls back to the role's settings in `.fold/config.toml`, and a start's own `model` and `instructions` come before the profile. The four roles and their rights stay the same for every area.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AreaProfile {
    pub instructions: Option<String>,
    pub model: Option<String>,
    /// Never empty: `["fold"]` limits the workers to the fold server alone.
    pub mcp: Option<Vec<String>>,
}

impl AreaProfile {
    /// Checks the values of a define request: trimmed instructions within [`MAX_AREA_INSTRUCTIONS_CHARS`], a model [`check_model`] accepts and a list of distinct server names.
    pub fn from_request(
        instructions: Option<String>,
        model: Option<String>,
        mcp: Option<Vec<String>>,
    ) -> Result<Self, FoldError> {
        Ok(Self {
            instructions: instructions
                .as_deref()
                .map(check_instructions)
                .transpose()?,
            model: model.map(checked_model).transpose()?,
            mcp: mcp.map(check_servers).transpose()?,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.instructions.is_none() && self.model.is_none() && self.mcp.is_none()
    }
}

impl RoleSettings {
    /// The settings a worker of an area gets: the area's model and MCP servers where the profile sets them, the role's otherwise.
    #[must_use]
    pub fn with_area(mut self, profile: &AreaProfile) -> Self {
        if let Some(model) = &profile.model {
            self.model = Some(model.clone());
        }
        if let Some(mcp) = &profile.mcp {
            self.mcp = Some(mcp.clone());
        }
        self
    }
}

/// One field of an [`AreaProfileChange`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldChange<T> {
    Keep,
    Set(T),
    /// Back to the role's setting.
    Clear,
}

impl<T: Clone> FieldChange<T> {
    fn apply(&self, current: Option<&T>) -> Option<T> {
        match self {
            FieldChange::Keep => current.cloned(),
            FieldChange::Set(value) => Some(value.clone()),
            FieldChange::Clear => None,
        }
    }

    fn changes(&self) -> bool {
        !matches!(self, FieldChange::Keep)
    }
}

/// What an `update_area` call asks for; a field left out and not cleared stays as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AreaProfileRequest {
    pub instructions: Option<String>,
    pub model: Option<String>,
    pub mcp: Option<Vec<String>>,
    pub clear_instructions: bool,
    pub clear_model: bool,
    pub clear_mcp: bool,
}

/// A checked change of an area's profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AreaProfileChange {
    pub instructions: FieldChange<String>,
    pub model: FieldChange<String>,
    pub mcp: FieldChange<Vec<String>>,
}

impl AreaProfileChange {
    /// Refuses a request that changes nothing or that both sets and clears one field, with `invalid_request`.
    pub fn from_request(request: AreaProfileRequest) -> Result<Self, FoldError> {
        let change = Self {
            instructions: field(
                "instructions",
                request.instructions.as_deref().map(check_instructions),
                request.clear_instructions,
            )?,
            model: field(
                "model",
                request.model.map(checked_model),
                request.clear_model,
            )?,
            mcp: field("mcp", request.mcp.map(check_servers), request.clear_mcp)?,
        };
        if change.fields().is_empty() {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                "update_area needs a field to change",
            )
            .with_hint("pass instructions, model or mcp, or clear one of them"));
        }
        Ok(change)
    }

    #[must_use]
    pub fn apply(&self, profile: &AreaProfile) -> AreaProfile {
        AreaProfile {
            instructions: self.instructions.apply(profile.instructions.as_ref()),
            model: self.model.apply(profile.model.as_ref()),
            mcp: self.mcp.apply(profile.mcp.as_ref()),
        }
    }

    /// The names of the fields the change sets or clears, in profile order.
    pub fn fields(&self) -> Vec<&'static str> {
        [
            ("instructions", self.instructions.changes()),
            ("model", self.model.changes()),
            ("mcp", self.mcp.changes()),
        ]
        .into_iter()
        .filter_map(|(name, changes)| changes.then_some(name))
        .collect()
    }
}

fn field<T>(
    name: &str,
    value: Option<Result<T, FoldError>>,
    clear: bool,
) -> Result<FieldChange<T>, FoldError> {
    match (value, clear) {
        (Some(_), true) => Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!("{name} cannot be set and cleared in one call"),
        )
        .with_hint(format!("pass {name} or clear_{name}, not both"))),
        (Some(value), false) => Ok(FieldChange::Set(value?)),
        (None, true) => Ok(FieldChange::Clear),
        (None, false) => Ok(FieldChange::Keep),
    }
}

fn check_instructions(text: &str) -> Result<String, FoldError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            "an area's instructions cannot be empty",
        )
        .with_hint("leave instructions out to use none"));
    }
    let length = text.chars().count();
    if length > MAX_AREA_INSTRUCTIONS_CHARS {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!(
                "an area's instructions have at most {MAX_AREA_INSTRUCTIONS_CHARS} characters, not {length}"
            ),
        )
        .with_details([
            ("length", json!(length)),
            ("max", json!(MAX_AREA_INSTRUCTIONS_CHARS)),
        ])
        .with_hint("keep only what every worker of the area needs"));
    }
    Ok(text.to_owned())
}

fn checked_model(model: String) -> Result<String, FoldError> {
    check_model(&model)?;
    Ok(model)
}

fn check_servers(servers: Vec<String>) -> Result<Vec<String>, FoldError> {
    if servers.is_empty() {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            "an area's MCP server list cannot be empty",
        )
        .with_hint("pass [\"fold\"] for the fold server alone, or leave mcp out"));
    }
    let mut seen = BTreeSet::new();
    for server in &servers {
        if !is_command_word(server) {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                "an MCP server name is one word without spaces, such as pencil",
            )
            .with_hint("name servers of the project's .mcp.json, or fold"));
        }
        if !seen.insert(server.as_str()) {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("the MCP server {server} is named twice"),
            )
            .with_hint("name each server once"));
        }
    }
    Ok(servers)
}

/// The system prompt of a worker of an area whose profile has instructions: the role's prompt, then the area's instructions. It is written at each launch, so a changed profile reaches the next start and never a running process.
pub fn area_prompt(role_prompt: &str, area: &AreaName, instructions: &str) -> String {
    format!(
        "{}\n\n## Area {area}\n\nThese instructions hold for every worker of area {area}. The instructions of your first turn come after them and win where the two disagree.\n\n{instructions}\n",
        role_prompt.trim_end()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProjectConfig;
    use crate::roles::Role;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn a_define_request_is_checked_and_trimmed() {
        let profile = AreaProfile::from_request(
            Some("  draw screens in Pencil \n".to_owned()),
            Some("opus".to_owned()),
            Some(names(&["pencil", "fold"])),
        )
        .unwrap();
        assert_eq!(
            profile.instructions.as_deref(),
            Some("draw screens in Pencil")
        );
        assert_eq!(profile.model.as_deref(), Some("opus"));
        assert_eq!(profile.mcp, Some(names(&["pencil", "fold"])));
        assert!(!profile.is_empty());
        assert!(
            AreaProfile::from_request(None, None, None)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn bad_values_are_refused_with_invalid_request() {
        let refused = [
            AreaProfile::from_request(Some("  ".to_owned()), None, None),
            AreaProfile::from_request(
                Some("é".repeat(MAX_AREA_INSTRUCTIONS_CHARS + 1)),
                None,
                None,
            ),
            AreaProfile::from_request(None, Some("two words".to_owned()), None),
            AreaProfile::from_request(None, None, Some(Vec::new())),
            AreaProfile::from_request(None, None, Some(names(&["a b"]))),
            AreaProfile::from_request(None, None, Some(names(&["pencil", "pencil"]))),
        ];
        for result in refused {
            assert_eq!(result.unwrap_err().code(), codes::INVALID_REQUEST);
        }
        let long = AreaProfile::from_request(
            Some("é".repeat(MAX_AREA_INSTRUCTIONS_CHARS + 1)),
            None,
            None,
        )
        .unwrap_err();
        assert_eq!(
            long.details().unwrap()["max"],
            json!(MAX_AREA_INSTRUCTIONS_CHARS)
        );
        assert!(
            AreaProfile::from_request(Some("é".repeat(MAX_AREA_INSTRUCTIONS_CHARS)), None, None)
                .is_ok()
        );
    }

    #[test]
    fn a_change_sets_clears_and_keeps_each_field() {
        let profile = AreaProfile {
            instructions: Some("draw".to_owned()),
            model: Some("opus".to_owned()),
            mcp: Some(names(&["pencil"])),
        };
        let change = AreaProfileChange::from_request(AreaProfileRequest {
            model: Some("sonnet".to_owned()),
            clear_mcp: true,
            ..AreaProfileRequest::default()
        })
        .unwrap();
        assert_eq!(change.fields(), ["model", "mcp"]);
        assert_eq!(
            change.apply(&profile),
            AreaProfile {
                instructions: Some("draw".to_owned()),
                model: Some("sonnet".to_owned()),
                mcp: None,
            }
        );
        let instructions = AreaProfileChange::from_request(AreaProfileRequest {
            instructions: Some(" write tests ".to_owned()),
            ..AreaProfileRequest::default()
        })
        .unwrap();
        assert_eq!(
            instructions.apply(&AreaProfile::default()).instructions,
            Some("write tests".to_owned())
        );
    }

    #[test]
    fn a_change_of_nothing_or_a_set_and_clear_is_refused() {
        let nothing = AreaProfileChange::from_request(AreaProfileRequest::default()).unwrap_err();
        assert_eq!(nothing.code(), codes::INVALID_REQUEST);
        let both = AreaProfileChange::from_request(AreaProfileRequest {
            model: Some("opus".to_owned()),
            clear_model: true,
            ..AreaProfileRequest::default()
        })
        .unwrap_err();
        assert_eq!(both.code(), codes::INVALID_REQUEST);
        assert!(both.hint().unwrap().contains("clear_model"));
        let bad = AreaProfileChange::from_request(AreaProfileRequest {
            mcp: Some(Vec::new()),
            ..AreaProfileRequest::default()
        })
        .unwrap_err();
        assert_eq!(bad.code(), codes::INVALID_REQUEST);
    }

    #[test]
    fn the_area_comes_before_the_role_and_an_unset_field_keeps_the_role() {
        let config = ProjectConfig::from_toml(
            "[roles.task_coder]\nmodel = \"sonnet\"\nmcp = [\"fold\"]\nidle_minutes = 3\n",
        )
        .unwrap();
        let role = config.role(Role::TaskCoder);
        let area = AreaProfile {
            instructions: None,
            model: Some("opus".to_owned()),
            mcp: Some(names(&["pencil"])),
        };
        let settings = role.clone().with_area(&area);
        assert_eq!(settings.model.as_deref(), Some("opus"));
        assert_eq!(settings.mcp, Some(names(&["pencil"])));
        assert_eq!(settings.idle_minutes, Some(3));
        assert_eq!(role.clone().with_area(&AreaProfile::default()), role);
    }

    #[test]
    fn the_area_prompt_follows_the_role_prompt() {
        let area = AreaName::parse("design").unwrap();
        let prompt = area_prompt("You are a task coder.\n", &area, "draw screens in Pencil");
        assert!(prompt.starts_with("You are a task coder.\n\n## Area design\n"));
        assert!(prompt.ends_with("\n\ndraw screens in Pencil\n"));
    }
}
