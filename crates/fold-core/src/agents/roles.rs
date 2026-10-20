use serde::{Deserialize, Serialize};

use crate::agents::{AgentName, AreaName};
use crate::error::{FoldError, codes};
use crate::ids::{ProposalId, ReviewBatchId, TaskId};
use crate::tasks::{LabelName, define_names};

mod answers;
mod handoff;
mod profile;

pub use answers::{PermissionBehavior, ask_answer_turn, permission_answer_turn};
pub use handoff::{
    HANDOFF_REQUEST, HANDOFF_TURNS, MAX_HANDOFF_CHARS, RefreshFacts, SUMMARY_ITEMS,
    SummaryProposal, SummaryTask, check_handoff, handoff_due, handoff_overdue, refresh_turn,
};
pub use profile::{
    AreaProfile, AreaProfileChange, AreaProfileRequest, FieldChange, MAX_AREA_INSTRUCTIONS_CHARS,
    area_prompt,
};

define_names!(
    /// The kinds of worker agent fold starts and supervises. The main agent is the user's own interactive session and has no role.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Role {
        AreaLead = "area_lead",
        ReviewLead = "review_lead",
        TaskWorker = "task_worker",
        Reviewer = "reviewer",
    }
);

impl Role {
    /// A lead lives until someone stops it; a task worker or reviewer is bound to its work and stops when it is done or idle.
    pub fn is_long_lived(self) -> bool {
        matches!(self, Role::AreaLead | Role::ReviewLead)
    }

    /// Leads and reviewers keep long gaps between turns (a reviewer runs long tests), so their prompt cache lives an hour; a task worker works in short bursts.
    pub fn default_cache_ttl(self) -> CacheTtl {
        match self {
            Role::TaskWorker => CacheTtl::FiveMinutes,
            Role::AreaLead | Role::ReviewLead | Role::Reviewer => CacheTtl::OneHour,
        }
    }

    /// The prompt that ships with fold: the role's text followed by the shared rules. A project replaces the role's text with `.fold/roles/<role>.md`; [`with_shared_rules`] then adds the shared rules again.
    pub fn prompt(self) -> &'static str {
        match self {
            Role::AreaLead => concat!(
                include_str!("roles/area_lead.md"),
                "\n",
                include_str!("roles/shared.md")
            ),
            Role::ReviewLead => concat!(
                include_str!("roles/review_lead.md"),
                "\n",
                include_str!("roles/shared.md")
            ),
            Role::TaskWorker => concat!(
                include_str!("roles/task_worker.md"),
                "\n",
                include_str!("roles/shared.md")
            ),
            Role::Reviewer => concat!(
                include_str!("roles/reviewer.md"),
                "\n",
                include_str!("roles/shared.md")
            ),
        }
    }
}

/// The authority and recovery rules that every agent prompt ends with, the main agent's and the workers' alike, whether the role text ships with fold or comes from a project file.
pub fn shared_prompt() -> &'static str {
    include_str!("roles/shared.md")
}

/// A project's own role text followed by the shared rules, so an override never drops them. The daemon (workers) and `fld claude` (main agent) both build their override prompts here.
pub fn with_shared_rules(role_text: &str) -> String {
    let mut prompt = role_text.to_owned();
    if !prompt.ends_with('\n') {
        prompt.push('\n');
    }
    prompt.push('\n');
    prompt.push_str(shared_prompt());
    prompt
}

/// The prompt that ships with fold for the main agent, which `fld claude` appends to the system prompt of the user's session; a project replaces the role text with `.fold/roles/main.md`, and the shared rules follow it either way. It stands apart from [`Role`] because fold neither starts nor supervises the main agent.
pub fn main_prompt() -> &'static str {
    concat!(
        include_str!("roles/main.md"),
        "\n",
        include_str!("roles/shared.md")
    )
}

define_names!(
    /// The value of `CLAUDE_CODE_PROMPT_CACHE_TTL` and `CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL`. Claude Code writes the cache into the `ephemeral_1h` or `ephemeral_5m` bucket accordingly.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub enum CacheTtl {
        #[serde(rename = "1h")]
        OneHour = "1h",
        #[serde(rename = "5m")]
        FiveMinutes = "5m",
    }
);

define_names!(
    /// The supervised process of a worker agent. `Starting` lasts until Claude Code reports its session; `Idle` is between turns; `Stopped` and `Failed` keep the session so the worker can be resumed.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum WorkerState {
        Starting = "starting",
        Working = "working",
        Idle = "idle",
        Stopping = "stopping",
        Stopped = "stopped",
        Failed = "failed",
    }
);

impl WorkerState {
    /// A process exists or is about to: it counts against the worker limits.
    pub fn is_running(self) -> bool {
        !matches!(self, WorkerState::Stopped | WorkerState::Failed)
    }
}

/// The `worker.failed` reason of a worker whose fold MCP server did not connect, so its session has no fold tool.
pub const MCP_UNAVAILABLE: &str = "mcp_unavailable";

/// The `worker.failed` reason of a worker whose MCP configuration would start an fld binary that is not there.
pub const FLD_MISSING: &str = "fld_missing";

/// The `worker.failed` reason of a worker whose `claude` the daemon did not find on the `PATH` it gives workers. The daemon reads that `PATH` from the user's login shell once at its start, so a restart is what makes a newly installed Claude Code or a changed shell file count.
pub const CLAUDE_NOT_FOUND: &str = "claude_not_found";

/// A worker that failed for `reason` cannot reach fold or cannot run Claude Code, and starting it again fails the same way until the daemon starts again. Such a worker keeps its claims, as in a pause, and a daemon start resumes it in its own session: its cause lies outside the agent, and a start of the installed fld, which reads the shell's `PATH` again, removes it.
pub fn needs_restart(reason: &str) -> bool {
    reason == MCP_UNAVAILABLE || reason == FLD_MISSING || reason == CLAUDE_NOT_FOUND
}

/// What a human does about a failure that [`needs_restart`] accepts; `None` for any other reason.
pub fn restart_hint(reason: &str) -> Option<&'static str> {
    match reason {
        MCP_UNAVAILABLE => Some("run `fld restart`; the daemon then resumes the worker"),
        FLD_MISSING => Some(
            "install fld and run `fld restart`; the daemon then resumes the worker, or an agent resumes it with `resume_worker`",
        ),
        CLAUDE_NOT_FOUND => Some(
            "install Claude Code or add its folder to PATH in your shell's startup files, then run `fld restart`; the daemon then resumes the worker, and `fld doctor` shows where it looks for claude",
        ),
        _ => None,
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

/// The `[workers]` limits of a project, counted over the short-lived workers whose process is running. The long-lived leads are outside them, so a full limit never holds back the plan of an area or the review of the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerLimits {
    pub task_worker: u64,
    pub reviewer: u64,
    /// `None`: an area has no limit of its own.
    pub max_per_area: Option<u64>,
}

impl WorkerLimits {
    /// The limit of `role`, `None` for a long-lived role.
    pub fn of(self, role: Role) -> Option<u64> {
        match role {
            Role::TaskWorker => Some(self.task_worker),
            Role::Reviewer => Some(self.reviewer),
            Role::AreaLead | Role::ReviewLead => None,
        }
    }
}

/// What a change does to `max_per_area`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AreaLimitChange {
    #[default]
    Keep,
    Set(u64),
    Clear,
}

/// A request to change the `[workers]` limits: a limit left out stays as it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkerLimitsChange {
    pub task_worker: Option<u64>,
    pub reviewer: Option<u64>,
    pub max_per_area: AreaLimitChange,
}

impl WorkerLimitsChange {
    /// The change a request asks for; a value together with a clear of the same limit is refused.
    pub fn from_request(
        task_worker: Option<u64>,
        reviewer: Option<u64>,
        max_per_area: Option<u64>,
        clear_max_per_area: bool,
    ) -> Result<Self, FoldError> {
        let max_per_area = match (max_per_area, clear_max_per_area) {
            (Some(_), true) => {
                return Err(FoldError::new(
                    codes::INVALID_REQUEST,
                    "max_per_area and clear_max_per_area cannot be combined",
                )
                .with_hint(
                    "pass max_per_area to set the limit, or clear_max_per_area to remove it",
                ));
            }
            (Some(limit), false) => AreaLimitChange::Set(limit),
            (None, true) => AreaLimitChange::Clear,
            (None, false) => AreaLimitChange::Keep,
        };
        Ok(Self {
            task_worker,
            reviewer,
            max_per_area,
        })
    }

    /// Refuses a change that sets nothing. A limit has no upper bound, because how many workers a machine and a token budget carry is the user's choice, and a limit of 0 lets no worker of that role start.
    pub fn check(&self) -> Result<(), FoldError> {
        if self.task_worker.is_none()
            && self.reviewer.is_none()
            && self.max_per_area == AreaLimitChange::Keep
        {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                "set_worker_limits needs at least one limit",
            )
            .with_hint("pass task_worker, reviewer, max_per_area or clear_max_per_area"));
        }
        Ok(())
    }

    /// The limits after the change.
    pub fn apply(&self, limits: WorkerLimits) -> WorkerLimits {
        WorkerLimits {
            task_worker: self.task_worker.unwrap_or(limits.task_worker),
            reviewer: self.reviewer.unwrap_or(limits.reviewer),
            max_per_area: match self.max_per_area {
                AreaLimitChange::Keep => limits.max_per_area,
                AreaLimitChange::Set(value) => Some(value),
                AreaLimitChange::Clear => None,
            },
        }
    }
}

/// Refuses one more running worker of `role` when its role or its area is full. `running` counts the running workers of `role`, and `in_area` is the worker's area with the running workers of `role` in it, `None` for a worker without an area. A lead is never refused.
pub fn check_limits(
    limits: WorkerLimits,
    role: Role,
    running: u64,
    in_area: Option<(&AreaName, u64)>,
) -> Result<(), FoldError> {
    let Some(max) = limits.of(role) else {
        return Ok(());
    };
    if running >= max {
        // A limit of 0 frees no place when a worker stops, so only raising it helps.
        let (message, hint) = if max == 0 {
            (
                format!("the {role} limit is 0, so no worker of role {role} starts"),
                format!(
                    "ask the main agent or a human to raise the {role} limit with set_worker_limits"
                ),
            )
        } else {
            (
                format!("the project already runs {running} workers of role {role}, its limit"),
                format!(
                    "the {role} limit is {max}: wait until a {role} stops, stop one with stop_worker, or ask the main agent or a human to raise it with set_worker_limits"
                ),
            )
        };
        return Err(FoldError::new(codes::WORKER_LIMIT_REACHED, message)
            .with_details([
                ("role", serde_json::json!(role.as_str())),
                ("max", serde_json::json!(max)),
            ])
            .with_hint(hint));
    }
    if let Some(((area, count), max)) = in_area.zip(limits.max_per_area)
        && count >= max
    {
        let (message, hint) = if max == 0 {
            (
                format!("the limit of area {area} is 0, so no worker of role {role} starts there"),
                "ask the main agent or a human to raise max_per_area with set_worker_limits"
                    .to_owned(),
            )
        } else {
            (
                format!("area {area} already runs {count} workers of role {role}, its limit"),
                format!(
                    "the limit of area {area} is {max}: wait until a {role} of the area stops, stop one with stop_worker, or ask the main agent or a human to raise it with set_worker_limits"
                ),
            )
        };
        return Err(FoldError::new(codes::WORKER_LIMIT_REACHED, message)
            .with_details([
                ("role", serde_json::json!(role.as_str())),
                ("area", serde_json::json!(area.as_str())),
                ("max_per_area", serde_json::json!(max)),
            ])
            .with_hint(hint));
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

/// The name fold gives a worker started without one: `<area>-lead`, `review-lead`, `<area>-worker` or `reviewer`, with `-<n>` from the second name on. The area part is shortened so the name fits the agent name limit.
pub fn default_worker_name(role: Role, area: Option<&AreaName>, attempt: u32) -> Option<AgentName> {
    let suffix = match attempt {
        0 | 1 => String::new(),
        n => format!("-{n}"),
    };
    let (prefix, base) = match (role, area) {
        (Role::AreaLead, Some(area)) => (Some(area.as_str()), "lead"),
        (Role::TaskWorker, Some(area)) => (Some(area.as_str()), "worker"),
        (Role::AreaLead, None) => (None, "area-lead"),
        (Role::TaskWorker, None) => (None, "task-worker"),
        (Role::ReviewLead, _) => (None, "review-lead"),
        (Role::Reviewer, _) => (None, "reviewer"),
    };
    let tail = format!("{base}{suffix}");
    let name = match prefix {
        Some(prefix) => {
            let room = crate::agents::MAX_NAME_LEN.saturating_sub(tail.len() + 1);
            let prefix: String = prefix.chars().take(room).collect();
            let prefix = prefix.trim_end_matches(['-', '_']);
            format!("{prefix}-{tail}")
        }
        None => tail,
    };
    AgentName::parse(&name).ok()
}

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
    /// The batch fold made of a reviewer's proposals when the lead gave it two or more.
    pub batch: Option<ReviewBatchId>,
    pub instructions: Option<&'a str>,
}

/// The first user turn fold sends a worker: the role's start line, then the starter's instructions.
pub fn first_turn(facts: &StartFacts<'_>) -> String {
    let ids = |ids: Vec<String>| ids.join(", ");
    let line = match (facts.role, facts.area) {
        (Role::AreaLead, Some((area, label))) => format!(
            "fold started you as {}, the area lead of area {area}, which owns the tasks labelled {label}. Read your tasks with get_task_graph, split them where needed and start task workers for ready tasks with start_worker.",
            facts.name
        ),
        (Role::AreaLead, None) => format!("fold started you as {}, an area lead.", facts.name),
        (Role::ReviewLead, _) => format!(
            "fold started you as {}, the review lead. Review the open proposals yourself, or give one proposal or a group of related proposals to a reviewer with start_worker.",
            facts.name
        ),
        (Role::TaskWorker, _) => format!(
            "fold started you as {}, a task worker for {}, and gave you the claim on each of them. Read each task with get_task and reuse its workspace when present. For a proposal task, work only in its task workspace and send create_proposal or update_proposal. For a no_proposal task, create no workspace and send report_task with verifiable evidence.",
            facts.name,
            ids(facts.tasks.iter().map(ToString::to_string).collect())
        ),
        (Role::Reviewer, _) => {
            let proposals = ids(facts.proposals.iter().map(ToString::to_string).collect());
            match facts.batch {
                Some(batch) => format!(
                    "fold started you as {}, a reviewer for {proposals}, and gave you their review claims in batch {batch}. Review only these proposals: check out the batch with create_batch_checkout and submit each verdict with submit_batch_review. Read current versions before starting.",
                    facts.name
                ),
                None => format!(
                    "fold started you as {}, a reviewer for {proposals}, and gave you its review claim. Review only this proposal: check it out with create_review_checkout and submit its verdict with submit_review. Read its current version before starting.",
                    facts.name
                ),
            }
        }
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

/// What waits for a lead whose process starts again in its own session: for an area lead the ready tasks of its area in the ready order, for the review lead the open proposals that have no verdict on their current version. A claim hides a task or proposal only while its worker runs or the same start runs it again, because a start moves the claim of a worker that has ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatchUpFacts<'a> {
    pub role: Role,
    pub area: Option<&'a AreaName>,
    pub tasks: &'a [TaskId],
    pub proposals: &'a [ProposalId],
}

/// The part of a restarted lead's turn that names the work waiting for it, at most [`SUMMARY_ITEMS`] ids. fold stores no push notice, so the lead may never have heard of this work. `None` for the other roles and when nothing waits, so a lead with nothing to do stays idle.
pub fn catch_up_turn(facts: &CatchUpFacts<'_>) -> Option<String> {
    fn listed(ids: Vec<String>) -> String {
        let more = ids.len().saturating_sub(SUMMARY_ITEMS);
        let named = ids
            .into_iter()
            .take(SUMMARY_ITEMS)
            .collect::<Vec<_>>()
            .join(", ");
        if more == 0 {
            named
        } else {
            format!("{named} and {more} more")
        }
    }
    match facts.role {
        Role::AreaLead if !facts.tasks.is_empty() => {
            let area = facts
                .area
                .map(|area| format!(" of area {area}"))
                .unwrap_or_default();
            Some(format!(
                "These tasks{area} are ready and wait for a task worker: {}. fold may not have told you of each one. Read each with get_task, find the others with get_ready_tasks, and start task workers for them with start_worker, which also moves the claim of a worker that has ended.",
                listed(facts.tasks.iter().map(ToString::to_string).collect())
            ))
        }
        Role::ReviewLead if !facts.proposals.is_empty() => Some(format!(
            "These open proposals wait for a review: {}. fold may not have told you of each one. Read each with get_proposal, find the others with list_proposals, then review them yourself or give them to reviewers with start_worker, which also moves the claim of a reviewer that has ended.",
            listed(facts.proposals.iter().map(ToString::to_string).collect())
        )),
        Role::AreaLead | Role::ReviewLead | Role::TaskWorker | Role::Reviewer => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(name: &str) -> AreaName {
        AreaName::parse(name).unwrap()
    }

    #[test]
    fn only_a_worker_that_cannot_reach_fold_waits_for_a_restart() {
        assert!(needs_restart(MCP_UNAVAILABLE));
        assert!(needs_restart(FLD_MISSING));
        assert!(needs_restart(CLAUDE_NOT_FOUND));
        for reason in ["exited", "spawn", "daemon_restart"] {
            assert!(!needs_restart(reason), "{reason}");
        }
        assert_eq!(
            restart_hint(MCP_UNAVAILABLE),
            Some("run `fld restart`; the daemon then resumes the worker")
        );
        assert_eq!(
            restart_hint(FLD_MISSING),
            Some(
                "install fld and run `fld restart`; the daemon then resumes the worker, or an agent resumes it with `resume_worker`"
            )
        );
        assert!(
            restart_hint(CLAUDE_NOT_FOUND)
                .is_some_and(|hint| hint.contains("fld restart") && hint.contains("fld doctor"))
        );
        assert_eq!(restart_hint("exited"), None);
    }

    #[test]
    fn a_limits_change_takes_any_number_and_needs_a_limit() {
        let set = |task_worker| WorkerLimitsChange {
            task_worker: Some(task_worker),
            ..WorkerLimitsChange::default()
        };
        for value in [0, 1, 50, u64::from(u32::MAX)] {
            assert!(set(value).check().is_ok(), "{value}");
        }
        let area = WorkerLimitsChange {
            max_per_area: AreaLimitChange::Set(0),
            ..WorkerLimitsChange::default()
        };
        assert!(area.check().is_ok());
        let none = WorkerLimits {
            task_worker: 0,
            reviewer: 50,
            max_per_area: None,
        };
        let zero = check_limits(none, Role::TaskWorker, 0, None).unwrap_err();
        assert_eq!(zero.code(), codes::WORKER_LIMIT_REACHED);
        assert_eq!(
            zero.hint(),
            Some(
                "ask the main agent or a human to raise the task_worker limit with set_worker_limits"
            )
        );
        assert!(check_limits(none, Role::Reviewer, 49, None).is_ok());
        let web = AreaName::parse("web").unwrap();
        let closed = WorkerLimits {
            task_worker: 4,
            max_per_area: Some(0),
            ..none
        };
        let zero = check_limits(closed, Role::TaskWorker, 0, Some((&web, 0))).unwrap_err();
        assert_eq!(zero.code(), codes::WORKER_LIMIT_REACHED);
        assert_eq!(
            zero.hint(),
            Some("ask the main agent or a human to raise max_per_area with set_worker_limits")
        );
        assert_eq!(
            WorkerLimitsChange::default().check().unwrap_err().code(),
            codes::INVALID_REQUEST
        );
        let clear = WorkerLimitsChange::from_request(None, None, None, true).unwrap();
        assert!(clear.check().is_ok());
        assert_eq!(
            WorkerLimitsChange::from_request(None, None, Some(2), true)
                .unwrap_err()
                .code(),
            codes::INVALID_REQUEST
        );
    }

    #[test]
    fn a_limits_change_keeps_what_it_leaves_out() {
        let limits = WorkerLimits {
            task_worker: 4,
            reviewer: 3,
            max_per_area: Some(2),
        };
        let change = WorkerLimitsChange::from_request(Some(9), None, None, false).unwrap();
        assert_eq!(
            change.apply(limits),
            WorkerLimits {
                task_worker: 9,
                ..limits
            }
        );
        let change = WorkerLimitsChange::from_request(None, Some(5), None, true).unwrap();
        assert_eq!(
            change.apply(limits),
            WorkerLimits {
                reviewer: 5,
                max_per_area: None,
                ..limits
            }
        );
        let change = WorkerLimitsChange::from_request(None, None, Some(6), false).unwrap();
        assert_eq!(change.apply(limits).max_per_area, Some(6));
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
    fn leads_and_reviewers_cache_for_an_hour_and_task_workers_for_five_minutes() {
        assert_eq!(Role::AreaLead.default_cache_ttl(), CacheTtl::OneHour);
        assert_eq!(Role::ReviewLead.default_cache_ttl(), CacheTtl::OneHour);
        assert_eq!(Role::Reviewer.default_cache_ttl(), CacheTtl::OneHour);
        assert_eq!(Role::TaskWorker.default_cache_ttl(), CacheTtl::FiveMinutes);
        assert!(Role::AreaLead.is_long_lived() && Role::ReviewLead.is_long_lived());
        assert!(!Role::TaskWorker.is_long_lived() && !Role::Reviewer.is_long_lived());
    }

    #[test]
    fn every_role_prompt_says_that_only_the_main_agent_pauses() {
        for role in Role::ALL {
            let prompt = role.prompt();
            assert!(prompt.contains("resumed you after a pause"), "{role}");
            assert!(prompt.contains("only the main agent does"), "{role}");
        }
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
    fn the_main_prompt_asks_and_waits_for_an_ok_before_it_creates_anything() {
        let prompt = main_prompt();
        assert!(prompt.starts_with("# Main agent\n"));
        assert!(prompt.contains("`AskUserQuestion`"));
        assert!(prompt.contains("explicit OK"));
        assert!(prompt.contains("Never build a task yourself"));
        assert!(prompt.contains("Wait for the user's explicit OK to that diff"));
        assert!(prompt.contains("call `submit_main_change`"));
        assert!(prompt.contains("Delegate whenever a task fits"));
        assert!(prompt.contains("Never use a back door"));
        assert!(prompt.contains("map the code first"));
        assert!(prompt.contains("names the folders the area owns"));
        assert!(prompt.contains("`pause_project` or `resume_project` only when the user asks"));
        assert!(prompt.contains("`start_worker` fails with `project_paused`"));
        assert!(prompt.contains("only when the user tells you in this conversation"));
        assert!(prompt.contains("or claim a task or a review, only when the user tells you"));
        assert!(
            prompt.split_whitespace().count() < 1650,
            "the prompt stays in context every turn"
        );
    }

    #[test]
    fn shipped_prompts_include_shared_authority_and_recovery_rules() {
        for prompt in Role::ALL
            .into_iter()
            .map(Role::prompt)
            .chain([main_prompt()])
        {
            assert_eq!(prompt.matches(include_str!("roles/shared.md")).count(), 1);
        }
    }

    #[test]
    fn an_override_is_followed_by_the_shared_rules() {
        let prompt = with_shared_rules("Our own prompt.\n");
        assert!(prompt.starts_with("Our own prompt.\n\n"));
        assert!(prompt.ends_with(shared_prompt()));
        assert_eq!(
            with_shared_rules("No newline"),
            "No newline\n\n".to_owned() + shared_prompt()
        );
    }

    #[test]
    fn reviewer_prompts_require_current_evidence_and_handle_omitted_verdicts() {
        for role in [Role::ReviewLead, Role::Reviewer] {
            let prompt = role.prompt();
            assert!(prompt.contains("acceptance criteria"));
            assert!(prompt.contains("`submit_review_fix`"));
            assert!(prompt.contains("The kind of fix decides, not its size."));
            assert!(prompt.contains("never resolve it yourself"));
            assert!(prompt.contains("Never edit a batch checkout."));
            assert!(prompt.contains("still waits for the human after your approval"));
            assert!(prompt.contains("stale_version"));
            assert!(prompt.contains("left_out"));
            assert!(prompt.contains("`suggested_scope`"));
            assert!(prompt.contains("`base_conflicts`"));
            assert!(prompt.contains("do not run the same checks again"));
            assert!(prompt.contains("`get_check_run`"));
            assert!(
                prompt
                    .contains("public API, a schema, a migration, or security or permission code")
            );
            assert!(!prompt.contains("Change review:"), "{role:?}");
            assert!(!prompt.contains("Full review:"), "{role:?}");
        }
        let worker = Role::TaskWorker.prompt();
        assert!(worker.contains("call `run_checks` for the task before `create_proposal`"));
        assert!(worker.contains("Wait for `checks.finished` with `wait_for_events`"));
        let lead = Role::ReviewLead.prompt();
        assert!(lead.contains("before taking claims or creating a batch"));
        assert!(lead.contains("You decide the scope when you assign the review"));
        assert!(lead.contains("`set_review_scope`"));
        assert!(
            Role::Reviewer
                .prompt()
                .contains("Follow the scope you get.")
        );
        assert!(lead.contains("makes a batch of the group for it, in the same step"));
    }

    #[test]
    fn workers_get_their_claims_from_their_lead_and_never_claim() {
        for role in [Role::TaskWorker, Role::Reviewer] {
            let prompt = role.prompt();
            assert!(prompt.contains("`worker_claim_denied`"), "{role:?}");
            assert!(prompt.contains("gave you"), "{role:?}");
        }
        assert!(!Role::TaskWorker.prompt().contains("Claim each task"));
        assert!(!Role::Reviewer.prompt().contains("make a batch with"));
        assert!(Role::TaskWorker.prompt().contains("When your task is Done"));
        assert!(
            Role::AreaLead
                .prompt()
                .contains("Do not call `claim_task` yourself")
        );
    }

    #[test]
    fn an_area_lead_and_a_task_worker_leave_reviews_to_the_review_lead() {
        for role in [Role::AreaLead, Role::TaskWorker] {
            let prompt = role.prompt();
            assert!(
                prompt.contains("Do not claim or review proposals"),
                "{role:?}"
            );
            assert!(
                prompt.contains("tell the review lead with `send_message`"),
                "{role:?}"
            );
        }
    }

    #[test]
    fn a_task_worker_syncs_to_continue_from_a_reviewer_version() {
        let prompt = Role::TaskWorker.prompt();
        assert!(prompt.contains("A reviewer may fix your proposal itself"));
        assert!(prompt.contains("call `sync_workspace`: it writes that version into your files"));
    }

    #[test]
    fn limits_count_each_short_lived_role_and_never_a_lead() {
        let limits = WorkerLimits {
            task_worker: 4,
            reviewer: 3,
            max_per_area: None,
        };
        let web = area("web");
        assert!(check_limits(limits, Role::TaskWorker, 3, Some((&web, 3))).is_ok());
        let full = check_limits(limits, Role::TaskWorker, 4, Some((&web, 4))).unwrap_err();
        assert_eq!(full.code(), codes::WORKER_LIMIT_REACHED);
        assert_eq!(
            full.details().unwrap()["role"],
            serde_json::json!("task_worker")
        );
        assert_eq!(full.details().unwrap()["max"], serde_json::json!(4));
        assert!(full.hint().unwrap().contains("task_worker limit is 4"));
        assert!(full.hint().unwrap().contains("set_worker_limits"));
        assert!(check_limits(limits, Role::Reviewer, 2, None).is_ok());
        let reviewers = check_limits(limits, Role::Reviewer, 3, None).unwrap_err();
        assert!(reviewers.hint().unwrap().contains("reviewer limit is 3"));
        for lead in [Role::AreaLead, Role::ReviewLead] {
            assert!(check_limits(limits, lead, 100, Some((&web, 100))).is_ok());
        }

        let per_area = WorkerLimits {
            max_per_area: Some(2),
            ..limits
        };
        assert!(check_limits(per_area, Role::TaskWorker, 3, Some((&web, 1))).is_ok());
        assert!(check_limits(per_area, Role::TaskWorker, 3, None).is_ok());
        let area_full = check_limits(per_area, Role::TaskWorker, 2, Some((&web, 2))).unwrap_err();
        assert_eq!(area_full.code(), codes::WORKER_LIMIT_REACHED);
        assert_eq!(
            area_full.details().unwrap()["area"],
            serde_json::json!("web")
        );
        assert!(area_full.hint().unwrap().contains("limit of area web is 2"));
        assert!(area_full.hint().unwrap().contains("set_worker_limits"));
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
        assert_eq!(name(Role::TaskWorker, Some(&web), 2), "web-worker-2");
        assert_eq!(name(Role::ReviewLead, None, 1), "review-lead");
        assert_eq!(name(Role::Reviewer, None, 3), "reviewer-3");
        assert_eq!(name(Role::AreaLead, None, 0), "area-lead");
        assert_eq!(name(Role::TaskWorker, None, 0), "task-worker");
        let long = area(&"a".repeat(32));
        let task_worker = name(Role::TaskWorker, Some(&long), 12);
        assert!(
            task_worker.len() <= 32 && task_worker.ends_with("-worker-12"),
            "{task_worker}"
        );
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
        let task_worker = AgentName::parse("web-worker").unwrap();
        let task = TaskId::from_number(4).unwrap();
        let proposal = ProposalId::from_number(2).unwrap();
        let lead_turn = first_turn(&StartFacts {
            role: Role::AreaLead,
            name: &lead,
            area: Some((&web, &label)),
            tasks: &[],
            proposals: &[],
            batch: None,
            instructions: Some("  Start with the board.  "),
        });
        assert!(lead_turn.contains("area lead of area web"));
        assert!(lead_turn.contains("labelled frontend"));
        assert!(lead_turn.ends_with("\n\nStart with the board."));
        let task_worker_turn = first_turn(&StartFacts {
            role: Role::TaskWorker,
            name: &task_worker,
            area: Some((&web, &label)),
            tasks: &[task],
            proposals: &[],
            batch: None,
            instructions: Some(" "),
        });
        assert!(task_worker_turn.contains("task worker for T-4"));
        assert!(!task_worker_turn.contains("\n\n"));
        let reviewer_turn = first_turn(&StartFacts {
            role: Role::Reviewer,
            name: &task_worker,
            area: None,
            tasks: &[],
            proposals: &[proposal],
            batch: None,
            instructions: None,
        });
        assert!(reviewer_turn.contains("reviewer for P-2"));
        assert!(reviewer_turn.contains("create_review_checkout"));
        let other = ProposalId::from_number(3).unwrap();
        let batch_turn = first_turn(&StartFacts {
            role: Role::Reviewer,
            name: &task_worker,
            area: None,
            tasks: &[],
            proposals: &[proposal, other],
            batch: ReviewBatchId::from_number(7),
            instructions: None,
        });
        assert!(batch_turn.contains("reviewer for P-2, P-3"));
        assert!(batch_turn.contains("in batch rb-7"));
        assert!(!batch_turn.contains("create_review_batch"));
        assert!(task_worker_turn.contains("gave you the claim"));
        assert!(!task_worker_turn.contains("claim_task"));
        for role in [Role::ReviewLead, Role::AreaLead] {
            let turn = first_turn(&StartFacts {
                role,
                name: &lead,
                area: None,
                tasks: &[],
                proposals: &[],
                batch: None,
                instructions: None,
            });
            assert!(turn.starts_with("fold started you as web-lead"));
        }
    }

    #[test]
    fn a_catch_up_names_what_waits_for_the_lead_and_nothing_when_nothing_waits() {
        let web = area("web");
        let tasks: Vec<TaskId> = (1..=3).filter_map(TaskId::from_number).collect();
        let proposals = [ProposalId::from_number(4).unwrap()];
        let facts = |role, tasks, proposals| CatchUpFacts {
            role,
            area: Some(&web),
            tasks,
            proposals,
        };
        let lead = catch_up_turn(&facts(Role::AreaLead, &tasks, &[])).unwrap();
        assert!(
            lead.starts_with(
                "These tasks of area web are ready and wait for a task worker: T-1, T-2, T-3."
            ),
            "{lead}"
        );
        assert!(
            lead.contains("get_ready_tasks") && lead.contains("start_worker"),
            "{lead}"
        );
        let review = catch_up_turn(&CatchUpFacts {
            area: None,
            ..facts(Role::ReviewLead, &[], &proposals)
        })
        .unwrap();
        assert!(review.contains("wait for a review: P-4."), "{review}");
        assert!(
            review.contains("list_proposals") && review.contains("start_worker"),
            "{review}"
        );
        assert_eq!(catch_up_turn(&facts(Role::AreaLead, &[], &proposals)), None);
        assert_eq!(catch_up_turn(&facts(Role::ReviewLead, &tasks, &[])), None);
        for role in [Role::TaskWorker, Role::Reviewer] {
            assert_eq!(catch_up_turn(&facts(role, &tasks, &proposals)), None);
        }
        let many: Vec<TaskId> = (1..=25).filter_map(TaskId::from_number).collect();
        let long = catch_up_turn(&facts(Role::AreaLead, &many, &[])).unwrap();
        assert!(long.contains("T-20 and 5 more."), "{long}");
        assert!(!long.contains("T-21"), "{long}");
    }
}
