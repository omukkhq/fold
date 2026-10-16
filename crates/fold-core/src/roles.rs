use std::fmt;

use serde::{Deserialize, Serialize};

use crate::agents::{AgentName, AreaName};
use crate::error::{FoldError, codes};
use crate::ids::{ProposalId, TaskId};
use crate::task::LabelName;

mod handoff;

pub use handoff::{
    HANDOFF_REQUEST, HANDOFF_TURNS, MAX_HANDOFF_CHARS, RefreshFacts, SUMMARY_ITEMS,
    SummaryProposal, SummaryTask, check_handoff, handoff_due, handoff_overdue, refresh_turn,
};

/// The kinds of worker agent fold starts and supervises. The main agent is the user's own interactive session and has no role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    AreaLead,
    ReviewLead,
    TaskCoder,
    BatchReviewer,
}

impl Role {
    pub const ALL: [Role; 4] = [
        Role::AreaLead,
        Role::ReviewLead,
        Role::TaskCoder,
        Role::BatchReviewer,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Role::AreaLead => "area_lead",
            Role::ReviewLead => "review_lead",
            Role::TaskCoder => "task_coder",
            Role::BatchReviewer => "batch_reviewer",
        }
    }

    pub fn parse(text: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|role| role.as_str() == text)
    }

    /// A lead lives until someone stops it; a task coder or batch reviewer is bound to its work and stops when it is done or idle.
    pub fn is_long_lived(self) -> bool {
        matches!(self, Role::AreaLead | Role::ReviewLead)
    }

    /// Leads and batch reviewers keep long gaps between turns (a batch reviewer runs long tests), so their prompt cache lives an hour; a task coder works in short bursts.
    pub fn default_cache_ttl(self) -> CacheTtl {
        match self {
            Role::TaskCoder => CacheTtl::FiveMinutes,
            Role::AreaLead | Role::ReviewLead | Role::BatchReviewer => CacheTtl::OneHour,
        }
    }

    /// The prompt that ships with fold; a project overrides it with `.fold/roles/<role>.md`.
    pub fn prompt(self) -> &'static str {
        match self {
            Role::AreaLead => include_str!("roles/area_lead.md"),
            Role::ReviewLead => include_str!("roles/review_lead.md"),
            Role::TaskCoder => include_str!("roles/task_coder.md"),
            Role::BatchReviewer => include_str!("roles/batch_reviewer.md"),
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The value of `CLAUDE_CODE_PROMPT_CACHE_TTL` and `CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL`. Claude Code writes the cache into the `ephemeral_1h` or `ephemeral_5m` bucket accordingly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheTtl {
    #[serde(rename = "1h")]
    OneHour,
    #[serde(rename = "5m")]
    FiveMinutes,
}

impl CacheTtl {
    pub fn as_str(self) -> &'static str {
        match self {
            CacheTtl::OneHour => "1h",
            CacheTtl::FiveMinutes => "5m",
        }
    }

    pub fn parse(text: &str) -> Option<CacheTtl> {
        [CacheTtl::OneHour, CacheTtl::FiveMinutes]
            .into_iter()
            .find(|ttl| ttl.as_str() == text)
    }
}

/// The supervised process of a worker agent. `Starting` lasts until Claude Code reports its session; `Idle` is between turns; `Stopped` and `Failed` keep the session so the worker can be resumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkerState {
    Starting,
    Working,
    Idle,
    Stopping,
    Stopped,
    Failed,
}

impl WorkerState {
    pub const ALL: [WorkerState; 6] = [
        WorkerState::Starting,
        WorkerState::Working,
        WorkerState::Idle,
        WorkerState::Stopping,
        WorkerState::Stopped,
        WorkerState::Failed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            WorkerState::Starting => "starting",
            WorkerState::Working => "working",
            WorkerState::Idle => "idle",
            WorkerState::Stopping => "stopping",
            WorkerState::Stopped => "stopped",
            WorkerState::Failed => "failed",
        }
    }

    pub fn parse(text: &str) -> Option<WorkerState> {
        WorkerState::ALL
            .into_iter()
            .find(|state| state.as_str() == text)
    }

    /// A process exists or is about to: it counts against the worker limits.
    pub fn is_running(self) -> bool {
        !matches!(self, WorkerState::Stopped | WorkerState::Failed)
    }
}

impl fmt::Display for WorkerState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A model or MCP server name goes on Claude Code's command line or into its configuration, so it cannot look like an option or hold a control character or a space.
pub fn is_command_word(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 128
        && !text.starts_with('-')
        && !text.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// Refuses a model name that [`is_command_word`] refuses.
pub fn check_model(model: &str) -> Result<(), FoldError> {
    if is_command_word(model) {
        Ok(())
    } else {
        Err(FoldError::new(
            codes::INVALID_REQUEST,
            "a model is a Claude Code model name or alias without spaces, such as sonnet",
        )
        .with_hint("pass a model such as sonnet or opus, or leave it out"))
    }
}

/// The `[workers]` limits of a project, counted over the workers whose process is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerLimits {
    pub max: u64,
    pub max_per_area: u64,
}

/// Refuses one more running worker when the project or its area is full. `in_area` is `None` for a worker without an area.
pub fn check_limits(
    limits: WorkerLimits,
    running: u64,
    in_area: Option<(&AreaName, u64)>,
) -> Result<(), FoldError> {
    if running >= limits.max {
        return Err(FoldError::new(
            codes::WORKER_LIMIT_REACHED,
            format!("the project already runs {running} workers, its limit"),
        )
        .with_details([("max", serde_json::json!(limits.max))])
        .with_hint("stop a worker with stop_worker, or raise [workers] max in .fold/config.toml"));
    }
    if let Some((area, count)) = in_area
        && count >= limits.max_per_area
    {
        return Err(FoldError::new(
            codes::WORKER_LIMIT_REACHED,
            format!("area {area} already runs {count} workers, its limit"),
        )
        .with_details([
            ("area", serde_json::json!(area.as_str())),
            ("max_per_area", serde_json::json!(limits.max_per_area)),
        ])
        .with_hint(
            "stop a worker of the area with stop_worker, or raise [workers] max_per_area in .fold/config.toml",
        ));
    }
    Ok(())
}

/// The first task of `tasks` that does not carry `label`, each task given with its labels.
pub fn outside_area(label: &LabelName, tasks: &[(TaskId, Vec<LabelName>)]) -> Option<TaskId> {
    tasks
        .iter()
        .find(|(_, labels)| !labels.contains(label))
        .map(|(task, _)| *task)
}

/// The name fold gives a worker started without one: `<area>-lead`, `review-lead`, `<area>-coder` or `batch-reviewer`, with `-<n>` from the second name on. The area part is shortened so the name fits the agent name limit.
pub fn default_worker_name(role: Role, area: Option<&AreaName>, attempt: u32) -> Option<AgentName> {
    let suffix = match attempt {
        0 | 1 => String::new(),
        n => format!("-{n}"),
    };
    let (prefix, base) = match (role, area) {
        (Role::AreaLead, Some(area)) => (Some(area.as_str()), "lead"),
        (Role::TaskCoder, Some(area)) => (Some(area.as_str()), "coder"),
        (Role::AreaLead, None) => (None, "area-lead"),
        (Role::TaskCoder, None) => (None, "task-coder"),
        (Role::ReviewLead, _) => (None, "review-lead"),
        (Role::BatchReviewer, _) => (None, "batch-reviewer"),
    };
    let tail = format!("{base}{suffix}");
    let name = match prefix {
        Some(prefix) => {
            let room = MAX_AGENT_NAME.saturating_sub(tail.len() + 1);
            let prefix: String = prefix.chars().take(room).collect();
            let prefix = prefix.trim_end_matches(['-', '_']);
            format!("{prefix}-{tail}")
        }
        None => tail,
    };
    AgentName::parse(&name).ok()
}

const MAX_AGENT_NAME: usize = 32;

/// A version 4 UUID from 16 random bytes, the `--session-id` fold gives Claude Code. The daemon draws the bytes, so this stays pure.
pub fn session_uuid(mut bytes: [u8; 16]) -> String {
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// What a worker is bound to when it starts, for its first turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartFacts<'a> {
    pub role: Role,
    pub name: &'a AgentName,
    pub area: Option<(&'a AreaName, &'a LabelName)>,
    pub tasks: &'a [TaskId],
    pub proposals: &'a [ProposalId],
    pub instructions: Option<&'a str>,
}

/// The first user turn fold sends a worker: the role's start line, then the starter's instructions.
pub fn first_turn(facts: &StartFacts<'_>) -> String {
    let ids = |ids: Vec<String>| ids.join(", ");
    let line = match (facts.role, facts.area) {
        (Role::AreaLead, Some((area, label))) => format!(
            "fold started you as {}, the area lead of area {area}, which owns the tasks labelled {label}. Read your tasks with get_task_graph, split them where needed and start task coders for ready tasks with start_worker.",
            facts.name
        ),
        (Role::AreaLead, None) => format!("fold started you as {}, an area lead.", facts.name),
        (Role::ReviewLead, _) => format!(
            "fold started you as {}, the review lead. Review the open proposals yourself or start a batch reviewer for related proposals with start_worker.",
            facts.name
        ),
        (Role::TaskCoder, _) => format!(
            "fold started you as {}, a task coder for {}. Claim each task with claim_task, work only in its task workspace, and send a proposal for each task with create_proposal.",
            facts.name,
            ids(facts.tasks.iter().map(ToString::to_string).collect())
        ),
        (Role::BatchReviewer, _) => format!(
            "fold started you as {}, a batch reviewer for {}. Review them together with create_review_batch, create_batch_checkout and submit_batch_review, so each gets its own verdict.",
            facts.name,
            ids(facts.proposals.iter().map(ToString::to_string).collect())
        ),
    };
    match facts
        .instructions
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        Some(instructions) => format!("{line}\n\n{instructions}"),
        None => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(name: &str) -> AreaName {
        AreaName::parse(name).unwrap()
    }

    #[test]
    fn roles_and_states_round_trip_their_names() {
        for role in Role::ALL {
            assert_eq!(Role::parse(role.as_str()), Some(role));
            assert_eq!(role.to_string(), role.as_str());
            assert!(!role.prompt().is_empty());
        }
        assert_eq!(Role::parse("main"), None);
        for state in WorkerState::ALL {
            assert_eq!(WorkerState::parse(state.as_str()), Some(state));
            assert_eq!(state.to_string(), state.as_str());
        }
        assert_eq!(WorkerState::parse("gone"), None);
        assert!(WorkerState::Idle.is_running());
        assert!(!WorkerState::Stopped.is_running());
        assert!(!WorkerState::Failed.is_running());
        assert_eq!(CacheTtl::parse("1h"), Some(CacheTtl::OneHour));
        assert_eq!(CacheTtl::parse("5m"), Some(CacheTtl::FiveMinutes));
        assert_eq!(CacheTtl::parse("2h"), None);
    }

    #[test]
    fn a_model_is_one_plain_word() {
        assert!(check_model("claude-sonnet-5").is_ok());
        for bad in ["", "-x", "a b", "a\nb"] {
            assert_eq!(check_model(bad).unwrap_err().code(), codes::INVALID_REQUEST);
        }
        assert!(!is_command_word(&"a".repeat(129)));
    }

    #[test]
    fn leads_and_batch_reviewers_cache_for_an_hour_and_coders_for_five_minutes() {
        assert_eq!(Role::AreaLead.default_cache_ttl(), CacheTtl::OneHour);
        assert_eq!(Role::ReviewLead.default_cache_ttl(), CacheTtl::OneHour);
        assert_eq!(Role::BatchReviewer.default_cache_ttl(), CacheTtl::OneHour);
        assert_eq!(Role::TaskCoder.default_cache_ttl(), CacheTtl::FiveMinutes);
        assert!(Role::AreaLead.is_long_lived() && Role::ReviewLead.is_long_lived());
        assert!(!Role::TaskCoder.is_long_lived() && !Role::BatchReviewer.is_long_lived());
    }

    #[test]
    fn every_role_prompt_keeps_the_worker_in_its_workspace() {
        for role in Role::ALL {
            let prompt = role.prompt();
            assert!(prompt.contains("own workspace"), "{role}");
            assert!(
                prompt.contains("never edit the project's main files"),
                "{role}"
            );
        }
    }

    #[test]
    fn limits_count_the_project_and_the_area() {
        let limits = WorkerLimits {
            max: 3,
            max_per_area: 2,
        };
        let web = area("web");
        assert!(check_limits(limits, 2, Some((&web, 1))).is_ok());
        assert!(check_limits(limits, 0, None).is_ok());
        let full = check_limits(limits, 3, None).unwrap_err();
        assert_eq!(full.code(), codes::WORKER_LIMIT_REACHED);
        let area_full = check_limits(limits, 2, Some((&web, 2))).unwrap_err();
        assert_eq!(area_full.code(), codes::WORKER_LIMIT_REACHED);
        assert_eq!(
            area_full.details().unwrap()["area"],
            serde_json::json!("web")
        );
    }

    #[test]
    fn a_task_without_the_area_label_is_outside() {
        let web = LabelName::parse("web").unwrap();
        let api = LabelName::parse("api").unwrap();
        let one = TaskId::from_number(1).unwrap();
        let two = TaskId::from_number(2).unwrap();
        let tasks = vec![(one, vec![web.clone()]), (two, vec![api])];
        assert_eq!(outside_area(&web, &tasks), Some(two));
        assert_eq!(outside_area(&web, &tasks[..1]), None);
    }

    #[test]
    fn default_names_follow_the_role_and_fit_the_limit() {
        let web = area("web");
        let name = |role, area, n| default_worker_name(role, area, n).unwrap().to_string();
        assert_eq!(name(Role::AreaLead, Some(&web), 1), "web-lead");
        assert_eq!(name(Role::TaskCoder, Some(&web), 2), "web-coder-2");
        assert_eq!(name(Role::ReviewLead, None, 1), "review-lead");
        assert_eq!(name(Role::BatchReviewer, None, 3), "batch-reviewer-3");
        assert_eq!(name(Role::AreaLead, None, 0), "area-lead");
        assert_eq!(name(Role::TaskCoder, None, 0), "task-coder");
        let long = area(&"a".repeat(32));
        let coder = name(Role::TaskCoder, Some(&long), 12);
        assert!(coder.len() <= 32 && coder.ends_with("-coder-12"), "{coder}");
    }

    #[test]
    fn session_ids_are_version_4_uuids() {
        let id = session_uuid([0xff; 16]);
        assert_eq!(id, "ffffffff-ffff-4fff-bfff-ffffffffffff");
        assert_eq!(
            session_uuid([0; 16]),
            "00000000-0000-4000-8000-000000000000"
        );
    }

    #[test]
    fn the_first_turn_names_the_role_the_binding_and_the_instructions() {
        let web = area("web");
        let label = LabelName::parse("frontend").unwrap();
        let lead = AgentName::parse("web-lead").unwrap();
        let coder = AgentName::parse("web-coder").unwrap();
        let task = TaskId::from_number(4).unwrap();
        let proposal = ProposalId::from_number(2).unwrap();
        let lead_turn = first_turn(&StartFacts {
            role: Role::AreaLead,
            name: &lead,
            area: Some((&web, &label)),
            tasks: &[],
            proposals: &[],
            instructions: Some("  Start with the board.  "),
        });
        assert!(lead_turn.contains("area lead of area web"));
        assert!(lead_turn.contains("labelled frontend"));
        assert!(lead_turn.ends_with("\n\nStart with the board."));
        let coder_turn = first_turn(&StartFacts {
            role: Role::TaskCoder,
            name: &coder,
            area: Some((&web, &label)),
            tasks: &[task],
            proposals: &[],
            instructions: Some(" "),
        });
        assert!(coder_turn.contains("task coder for T-4"));
        assert!(!coder_turn.contains("\n\n"));
        let reviewer_turn = first_turn(&StartFacts {
            role: Role::BatchReviewer,
            name: &coder,
            area: None,
            tasks: &[],
            proposals: &[proposal],
            instructions: None,
        });
        assert!(reviewer_turn.contains("batch reviewer for P-2"));
        for role in [Role::ReviewLead, Role::AreaLead] {
            let turn = first_turn(&StartFacts {
                role,
                name: &lead,
                area: None,
                tasks: &[],
                proposals: &[],
                instructions: None,
            });
            assert!(turn.starts_with("fold started you as web-lead"));
        }
    }
}
