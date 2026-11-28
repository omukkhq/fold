use std::collections::BTreeSet;

use serde_json::json;

use super::{check_model, is_command_word};
use crate::agents::AreaName;
use crate::config::RoleSettings;
use crate::error::{FoldError, codes};

/// The most characters an area's instructions hold, the same bound as a hand-off note, because both go into every start of a worker.
pub const MAX_AREA_INSTRUCTIONS_CHARS: usize = 8000;

/// The kind of work of an area, which its area lead and its task workers start with. A field left `None` falls back to the role's settings in `.fold/config.toml`, and a start's own `model` and `instructions` come before the profile. The four roles and their rights stay the same for every area.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AreaProfile {
    pub instructions: Option<String>,
    pub model: Option<String>,
    /// Never empty: `["fold"]` limits the workers to the fold server alone.
    pub mcp: Option<Vec<String>>,
    /// Never empty: the rules of [`check_deny_rules`] that the area's workers and the reviewers of its proposals get as `permissions.deny`, after the project's own list.
    pub deny_commands: Option<Vec<String>>,
}

impl AreaProfile {
    /// Checks the values of a define request: trimmed instructions within [`MAX_AREA_INSTRUCTIONS_CHARS`], a model [`check_model`] accepts, a list of distinct server names and a list of rules [`check_deny_rules`] accepts.
    pub fn from_request(
        instructions: Option<String>,
        model: Option<String>,
        mcp: Option<Vec<String>>,
        deny_commands: Option<Vec<String>>,
    ) -> Result<Self, FoldError> {
        Ok(Self {
            instructions: instructions
                .as_deref()
                .map(check_instructions)
                .transpose()?,
            model: model.map(checked_model).transpose()?,
            mcp: mcp.map(check_servers).transpose()?,
            deny_commands: deny_commands.map(check_area_denies).transpose()?,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.instructions.is_none()
            && self.model.is_none()
            && self.mcp.is_none()
            && self.deny_commands.is_none()
    }
}

impl RoleSettings {
    /// The settings a worker of an area gets: the area's model and MCP servers where the profile sets them, the role's otherwise, and the area's denied commands after the project's.
    #[must_use]
    pub fn with_area(mut self, profile: &AreaProfile) -> Self {
        if let Some(model) = &profile.model {
            self.model = Some(model.clone());
        }
        if let Some(mcp) = &profile.mcp {
            self.mcp = Some(mcp.clone());
        }
        self.with_area_denies(profile)
    }

    /// The settings a reviewer of a proposal of the area gets: only the area's denied commands, after the ones it has. A reviewer keeps its role's model and servers, because its proposals can span areas.
    #[must_use]
    pub fn with_area_denies(mut self, profile: &AreaProfile) -> Self {
        if let Some(rules) = &profile.deny_commands {
            self.deny_commands = merged_deny_rules(&self.deny_commands, [rules.as_slice()]);
        }
        self
    }
}

/// The most characters of one denied-command rule.
pub const MAX_DENY_RULE_CHARS: usize = 200;

/// The first words of a command that fold's own work needs: every worker runs `cd '<workspace>' && ...`, and fold's hooks and MCP server run `fld`.
const FOLD_COMMANDS: [&str; 2] = ["cd", "fld"];

/// The denied commands a worker gets: `project` in its order, then each list of `areas` in the order given (area-name order), each rule once at its first place.
pub fn merged_deny_rules<'a>(
    project: &'a [String],
    areas: impl IntoIterator<Item = &'a [String]>,
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    project
        .iter()
        .chain(areas.into_iter().flatten())
        .filter(|rule| seen.insert(rule.as_str()))
        .cloned()
        .collect()
}

/// Checks denied-command rules, each trimmed: a Claude Code rule of the shape `Bash(<spec>)` of at most [`MAX_DENY_RULE_CHARS`] characters, without control characters or parentheses inside the spec, whose spec is not only wildcards and starts with a command word without a wildcard that is not one of [`FOLD_COMMANDS`], each rule once. Only Bash rules are taken, so a list never denies the fold tools or the read rule fold writes for a worker. A refused rule gives `invalid_permission_rule` with the rule in `details.rule` and the reason in `details.reason`.
pub fn check_deny_rules(rules: Vec<String>) -> Result<Vec<String>, FoldError> {
    let mut seen = BTreeSet::new();
    let mut checked = Vec::with_capacity(rules.len());
    for rule in rules {
        let rule = rule.trim().to_owned();
        if let Err(reason) = deny_rule_problem(&rule) {
            return Err(bad_rule(&rule, reason));
        }
        if !seen.insert(rule.clone()) {
            return Err(bad_rule(&rule, "duplicate"));
        }
        checked.push(rule);
    }
    Ok(checked)
}

fn deny_rule_problem(rule: &str) -> Result<(), &'static str> {
    if rule.chars().count() > MAX_DENY_RULE_CHARS {
        return Err("too_long");
    }
    if rule.chars().any(char::is_control) {
        return Err("control_character");
    }
    let spec = rule
        .strip_prefix("Bash(")
        .and_then(|rest| rest.strip_suffix(')'))
        .ok_or("not_bash")?;
    if spec.contains(['(', ')']) {
        return Err("parenthesis");
    }
    if spec.chars().all(|c| matches!(c, '*' | ':' | ' ')) {
        return Err("wildcard_only");
    }
    let first = spec
        .trim_start()
        .split([' ', ':'])
        .next()
        .unwrap_or_default();
    // A `*` matches any text, so a wildcard in the command word could match `cd` or `fld` too.
    if first.contains('*') {
        return Err("wildcard_command");
    }
    if FOLD_COMMANDS.contains(&first) {
        return Err("fold_command");
    }
    Ok(())
}

fn bad_rule(rule: &str, reason: &'static str) -> FoldError {
    FoldError::new(
        codes::INVALID_PERMISSION_RULE,
        "a denied command must be one Claude Code Bash rule, such as Bash(psql:*), of at most 200 characters, that names its command without a wildcard and does not deny cd, fld or every command",
    )
    .with_details([("rule", json!(rule)), ("reason", json!(reason))])
    .with_hint("see details.rule and details.reason; give each rule once as Bash(<command>:*) or Bash(<command line>)")
}

fn check_area_denies(rules: Vec<String>) -> Result<Vec<String>, FoldError> {
    if rules.is_empty() {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            "an area's denied command list cannot be empty",
        )
        .with_hint("leave deny_commands out, or clear it, for no list of the area"));
    }
    check_deny_rules(rules)
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
    pub deny_commands: Option<Vec<String>>,
    pub clear_instructions: bool,
    pub clear_model: bool,
    pub clear_mcp: bool,
    pub clear_deny_commands: bool,
}

/// A checked change of an area's profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AreaProfileChange {
    pub instructions: FieldChange<String>,
    pub model: FieldChange<String>,
    pub mcp: FieldChange<Vec<String>>,
    pub deny_commands: FieldChange<Vec<String>>,
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
            deny_commands: field(
                "deny_commands",
                request.deny_commands.map(check_area_denies),
                request.clear_deny_commands,
            )?,
        };
        if change.fields().is_empty() {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                "update_area needs a field to change",
            )
            .with_hint("pass instructions, model, mcp or deny_commands, or clear one of them"));
        }
        Ok(change)
    }

    #[must_use]
    pub fn apply(&self, profile: &AreaProfile) -> AreaProfile {
        AreaProfile {
            instructions: self.instructions.apply(profile.instructions.as_ref()),
            model: self.model.apply(profile.model.as_ref()),
            mcp: self.mcp.apply(profile.mcp.as_ref()),
            deny_commands: self.deny_commands.apply(profile.deny_commands.as_ref()),
        }
    }

    /// The names of the fields the change sets or clears, in profile order.
    pub fn fields(&self) -> Vec<&'static str> {
        [
            ("instructions", self.instructions.changes()),
            ("model", self.model.changes()),
            ("mcp", self.mcp.changes()),
            ("deny_commands", self.deny_commands.changes()),
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
        "{}\n\n## Area {area}\n\nThese instructions specialize work in area {area} within your assigned role, file limits and approval rights. First-turn task instructions take precedence over area preferences, but cannot expand those limits or the approved scope. Report a conflict before acting.\n\n{instructions}\n",
        role_prompt.trim_end()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::roles::Role;
    use crate::config::ProjectConfig;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    fn rules(list: &[&str]) -> Vec<String> {
        names(list)
    }

    #[test]
    fn a_bash_rule_is_taken_trimmed() {
        assert_eq!(
            check_deny_rules(rules(&[
                "  Bash(psql:*) ",
                "Bash(docker exec:*)",
                "Bash(rm -rf /srv/data)"
            ]))
            .unwrap(),
            rules(&[
                "Bash(psql:*)",
                "Bash(docker exec:*)",
                "Bash(rm -rf /srv/data)"
            ])
        );
        assert_eq!(check_deny_rules(Vec::new()).unwrap(), Vec::<String>::new());
        let profile =
            AreaProfile::from_request(None, None, None, Some(rules(&["Bash(psql:*)"]))).unwrap();
        assert_eq!(profile.deny_commands, Some(rules(&["Bash(psql:*)"])));
        assert!(!profile.is_empty());
    }

    #[test]
    fn each_bad_rule_shape_is_refused_with_its_reason_and_no_rule_text_in_the_message() {
        let long = format!("Bash({})", "a".repeat(MAX_DENY_RULE_CHARS));
        let cases = [
            ("Read(/srv/**)", "not_bash"),
            ("mcp__fold__create_proposal", "not_bash"),
            ("Bash", "not_bash"),
            ("Bash(ls", "not_bash"),
            ("Bash()", "wildcard_only"),
            ("Bash(*)", "wildcard_only"),
            ("Bash(:*)", "wildcard_only"),
            ("Bash( * )", "wildcard_only"),
            ("Bash(c*)", "wildcard_command"),
            ("Bash(*fld*)", "wildcard_command"),
            ("Bash(*/*)", "wildcard_command"),
            ("Bash(ps* -U prod)", "wildcard_command"),
            ("Bash(cd:*)", "fold_command"),
            ("Bash(   cd /srv)", "fold_command"),
            ("Bash(fld:*)", "fold_command"),
            ("Bash(  fld record)", "fold_command"),
            ("Bash(echo $(psql))", "parenthesis"),
            ("Bash(psql\n:*)", "control_character"),
            (long.as_str(), "too_long"),
        ];
        for (rule, reason) in cases {
            let error = check_deny_rules(rules(&[rule])).unwrap_err();
            assert_eq!(error.code(), codes::INVALID_PERMISSION_RULE, "{rule}");
            let details = error.details().unwrap();
            assert_eq!(details["reason"], json!(reason), "{rule}");
            assert_eq!(details["rule"], json!(rule.trim()), "{rule}");
            assert!(
                !error.to_string().contains(rule.trim()) || rule == "Bash",
                "{rule}"
            );
            assert!(
                !error.hint().unwrap().contains(rule.trim()) || rule == "Bash",
                "{rule}"
            );
        }
        let twice = check_deny_rules(rules(&["Bash(psql:*)", " Bash(psql:*)"])).unwrap_err();
        assert_eq!(twice.details().unwrap()["reason"], json!("duplicate"));
        assert!(check_deny_rules(rules(&["Bash(cdrom:*)", "Bash(fldx)"])).is_ok());
        assert!(
            check_deny_rules(rules(&[
                "Bash(psql *)",
                "Bash(docker exec:*)",
                "Bash(kubectl * prod)"
            ]))
            .is_ok(),
            "a wildcard after the command word stays"
        );
    }

    #[test]
    fn an_area_list_is_never_empty_and_a_change_sets_and_clears_it() {
        assert_eq!(
            AreaProfile::from_request(None, None, None, Some(Vec::new()))
                .unwrap_err()
                .code(),
            codes::INVALID_REQUEST
        );
        let set = AreaProfileChange::from_request(AreaProfileRequest {
            deny_commands: Some(rules(&["Bash(psql:*)"])),
            ..AreaProfileRequest::default()
        })
        .unwrap();
        assert_eq!(set.fields(), ["deny_commands"]);
        let profile = set.apply(&AreaProfile::default());
        assert_eq!(profile.deny_commands, Some(rules(&["Bash(psql:*)"])));
        let clear = AreaProfileChange::from_request(AreaProfileRequest {
            clear_deny_commands: true,
            ..AreaProfileRequest::default()
        })
        .unwrap();
        assert_eq!(clear.apply(&profile).deny_commands, None);
        let bad = AreaProfileChange::from_request(AreaProfileRequest {
            deny_commands: Some(rules(&["Bash(cd:*)"])),
            ..AreaProfileRequest::default()
        })
        .unwrap_err();
        assert_eq!(bad.code(), codes::INVALID_PERMISSION_RULE);
    }

    #[test]
    fn the_project_list_comes_first_then_each_area_without_duplicates() {
        let project = rules(&["Bash(psql:*)", "Bash(kubectl:*)"]);
        let api = rules(&["Bash(docker exec:*)", "Bash(psql:*)"]);
        let web = rules(&["Bash(kubectl:*)", "Bash(ssh:*)", "Bash(docker exec:*)"]);
        assert_eq!(
            merged_deny_rules(&project, [api.as_slice(), web.as_slice()]),
            rules(&[
                "Bash(psql:*)",
                "Bash(kubectl:*)",
                "Bash(docker exec:*)",
                "Bash(ssh:*)"
            ])
        );
        assert_eq!(
            merged_deny_rules(&[], [web.as_slice(), web.as_slice()]),
            web
        );
        assert_eq!(merged_deny_rules(&project, []), project);
    }

    #[test]
    fn a_reviewer_of_two_areas_gets_both_lists_but_keeps_its_role_model() {
        let config = ProjectConfig::from_toml(
            "deny_commands = [\"Bash(psql:*)\"]\n[roles.reviewer]\nmodel = \"sonnet\"\n",
        )
        .unwrap();
        let api = AreaProfile {
            model: Some("opus".to_owned()),
            deny_commands: Some(rules(&["Bash(psql:*)", "Bash(docker exec:*)"])),
            ..AreaProfile::default()
        };
        let web = AreaProfile {
            deny_commands: Some(rules(&["Bash(ssh:*)"])),
            ..AreaProfile::default()
        };
        let reviewer = config
            .role(Role::Reviewer)
            .with_area_denies(&api)
            .with_area_denies(&web);
        assert_eq!(reviewer.model.as_deref(), Some("sonnet"));
        assert_eq!(
            reviewer.deny_commands,
            rules(&["Bash(psql:*)", "Bash(docker exec:*)", "Bash(ssh:*)"])
        );
        let worker = config.role(Role::TaskWorker).with_area(&api);
        assert_eq!(worker.model.as_deref(), Some("opus"));
        assert_eq!(
            worker.deny_commands,
            rules(&["Bash(psql:*)", "Bash(docker exec:*)"])
        );
        assert_eq!(
            config
                .role(Role::TaskWorker)
                .with_area(&AreaProfile::default())
                .deny_commands,
            rules(&["Bash(psql:*)"])
        );
    }

    #[test]
    fn a_define_request_is_checked_and_trimmed() {
        let profile = AreaProfile::from_request(
            Some("  draw screens in Pencil \n".to_owned()),
            Some("opus".to_owned()),
            Some(names(&["pencil", "fold"])),
            None,
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
            AreaProfile::from_request(None, None, None, None)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn bad_values_are_refused_with_invalid_request() {
        let refused = [
            AreaProfile::from_request(Some("  ".to_owned()), None, None, None),
            AreaProfile::from_request(
                Some("é".repeat(MAX_AREA_INSTRUCTIONS_CHARS + 1)),
                None,
                None,
                None,
            ),
            AreaProfile::from_request(None, Some("two words".to_owned()), None, None),
            AreaProfile::from_request(None, None, Some(Vec::new()), None),
            AreaProfile::from_request(None, None, Some(names(&["a b"])), None),
            AreaProfile::from_request(None, None, Some(names(&["pencil", "pencil"])), None),
        ];
        for result in refused {
            assert_eq!(result.unwrap_err().code(), codes::INVALID_REQUEST);
        }
        let long = AreaProfile::from_request(
            Some("é".repeat(MAX_AREA_INSTRUCTIONS_CHARS + 1)),
            None,
            None,
            None,
        )
        .unwrap_err();
        assert_eq!(
            long.details().unwrap()["max"],
            json!(MAX_AREA_INSTRUCTIONS_CHARS)
        );
        assert!(
            AreaProfile::from_request(
                Some("é".repeat(MAX_AREA_INSTRUCTIONS_CHARS)),
                None,
                None,
                None
            )
            .is_ok()
        );
    }

    #[test]
    fn a_change_sets_clears_and_keeps_each_field() {
        let profile = AreaProfile {
            instructions: Some("draw".to_owned()),
            model: Some("opus".to_owned()),
            mcp: Some(names(&["pencil"])),
            deny_commands: None,
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
                deny_commands: None,
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
            "[roles.task_worker]\nmodel = \"sonnet\"\nmcp = [\"fold\"]\nidle_minutes = 3\n",
        )
        .unwrap();
        let role = config.role(Role::TaskWorker);
        let area = AreaProfile {
            instructions: None,
            model: Some("opus".to_owned()),
            mcp: Some(names(&["pencil"])),
            deny_commands: None,
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
        let prompt = area_prompt("You are a task worker.\n", &area, "draw screens in Pencil");
        assert!(prompt.starts_with("You are a task worker.\n\n## Area design\n"));
        assert!(prompt.ends_with("\n\ndraw screens in Pencil\n"));
        assert!(prompt.contains("cannot expand those limits or the approved scope"));
    }
}
