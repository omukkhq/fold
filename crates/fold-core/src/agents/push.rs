use std::collections::BTreeSet;

use crate::agents::roles::{CLAUDE_NOT_FOUND, FLD_MISSING, MCP_UNAVAILABLE};
use crate::agents::{AgentName, AreaName};
use crate::events::{Event, types};
use crate::ids::{MessageId, ProposalId, RequestId, TaskId};
use crate::tasks::status::TaskStatus;

/// The longest notice text. A notice names ids and a tool, so it never comes near it.
pub const MAX_NOTICE_LEN: usize = 200;

/// The facts about an event's task and proposal that decide who gets a notice. The daemon loads them from the read pool after the event commits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PushContext {
    /// Every agent that fold started as the review lead.
    pub review_leads: BTreeSet<AgentName>,
    /// The agent of the worker that authored the event's proposal.
    pub author: Option<AgentName>,
    /// The agent holding the unreleased review claim on the event's proposal.
    pub review_claim: Option<AgentName>,
    /// The agent holding the unreleased implementation claim on the event's task.
    pub implementation_claim: Option<AgentName>,
    /// The agents with a worker among the recipients of the event's message.
    pub recipients: BTreeSet<AgentName>,
    /// For an integrated proposal, the agent that reviewed it: the holder of the review claim the integration released, else the agent whose approval stands.
    pub reviewer: Option<AgentName>,
    /// The event's proposal is integrated or closed.
    pub retired: bool,
    /// The agent that decides on the event's request: the agent that created it, or its deputy once that agent's short-lived worker has stopped; `None` for a human.
    pub requester: Option<AgentName>,
    /// The agent of the worker drafting the event's request.
    pub drafter: Option<AgentName>,
    /// For a task that turned ready, the leads of the areas whose label it carries; for a new request, the lead of every area.
    pub area_leads: BTreeSet<AgentName>,
    /// For a task that turned ready, the areas whose label it carries, led or not; for `task.updated`, only the areas of the labels the edit added.
    pub areas: BTreeSet<AreaName>,
    /// Every agent tagged `main`.
    pub main_agents: BTreeSet<AgentName>,
    /// No agent can take the event's task now: it is a human task or on hold, or, for `task.hold_cleared` and `task.updated`, it is not an unclaimed Todo task.
    pub held_from_agents: bool,
    /// For `worker.failed` and `worker.model_unreachable`, the agent that started the worker; `None` for a human.
    pub starter: Option<AgentName>,
    /// The event's proposal belongs to a task of the area Main, which no review agent reviews.
    pub main_task: bool,
    /// For `worker.stopped` and `worker.failed` of a reviewer, the open proposals whose review claim it still holds, whose newest version has no verdict and for which fold owes it no resume; empty while the project is paused, because the resume of the pause runs the reviewer again.
    pub unreviewed: Vec<ProposalId>,
    /// The event's proposal waits in a review plan, whose own notices tell the review lead when to start its reviewer.
    pub planned: bool,
}

/// One line for one agent's Claude Code session. `text` holds public ids and the next tool only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub agent: AgentName,
    pub text: String,
    pub next_tool: &'static str,
}

/// The event types a rule reads; the daemon loads no context for any other type.
pub fn is_pushed(event: &Event) -> bool {
    [
        types::PROPOSAL_OPENED,
        types::REVIEW_SUBMITTED,
        types::INTEGRATION_CONFLICTED,
        types::TASK_STATUS_CHANGED,
        types::TASK_UPDATED,
        types::MESSAGE_SENT,
        types::PROPOSAL_VERSION_ADDED,
        types::INTEGRATION_SUCCEEDED,
        types::REQUEST_CREATED,
        types::REQUEST_DRAFT_SUBMITTED,
        types::REQUEST_CHANGES_REQUESTED,
        types::REQUEST_APPROVED,
        types::TASK_REPORTED,
        types::TASK_SENT_BACK,
        types::PROJECT_PAUSED,
        types::PROJECT_RESUMED,
        types::TASK_HOLD_CLEARED,
        types::PROPOSAL_HUMAN_EDITED,
        types::PROPOSAL_REVIEW_AGAIN_REQUESTED,
        types::HUMAN_CHANGE_RECORDED,
        types::HUMAN_CHANGE_CONFLICTED,
        types::HUMAN_CHANGE_RESOLVED,
        types::HUMAN_CHANGE_WITHDRAWN,
        types::PROPOSAL_REVIEWER_FIXED,
        types::INTEGRATION_HELD,
        types::WORKER_STOPPED,
        types::WORKER_FAILED,
        types::WORKER_MODEL_UNREACHABLE,
        types::REVIEW_PLAN_JOINED,
        types::REVIEW_PLAN_LEFT,
        types::REVIEW_PLAN_WAIT_ENDED,
        types::REVIEW_PLAN_ENDED,
        types::REVIEW_BATCH_WAITING,
    ]
    .contains(&event.event_type)
}

/// The request a `request.*` event names in `data.request`.
pub fn request_of(event: &Event) -> Option<RequestId> {
    event
        .data
        .get("request")
        .and_then(serde_json::Value::as_str)?
        .parse()
        .ok()
}

/// The notices one committed event sends. An agent never hears about what its own workers did, and an event that misses an id its rule names sends nothing.
pub fn notice_for(event: &Event, ctx: &PushContext) -> Vec<Notice> {
    let rules = match human_change_rules(event, ctx)
        .or_else(|| plan_rules(event, ctx))
        .or_else(|| review_rules(event, ctx))
    {
        Some(rules) => rules,
        None => rule(event, ctx).into_iter().collect(),
    };
    let own = event.actor.agent();
    let mut seen = BTreeSet::new();
    let mut notices = Vec::new();
    for (agents, text, next_tool) in rules {
        notices.extend(
            agents
                .into_iter()
                .filter(|agent| own != Some(agent.as_str()) && seen.insert(agent.clone()))
                .map(|agent| Notice {
                    agent,
                    text: format!("fold: {text}. Call {next_tool}."),
                    next_tool,
                }),
        );
    }
    notices
}

/// The longest file or task list a human-change notice spells out; a longer one is counted, so the notice stays within [`MAX_NOTICE_LEN`].
const MAX_LISTED_LEN: usize = 48;

/// A human change reaches the main agent, which tells the user about it in its own session: what was recorded as which revision, that it conflicts and main did not move, that an integration resolved the waiting change, or that the human took it back. The leads of the tasks whose open workspaces change the same files hear of a recorded change too, and the main agent's notice says they did. The daemon writes the files, the touched tasks and workspaces, and those leads into the event, so a notice tells what the record found.
fn human_change_rules(
    event: &Event,
    ctx: &PushContext,
) -> Option<Vec<(Vec<AgentName>, String, &'static str)>> {
    let kind = event.event_type;
    if ![
        types::HUMAN_CHANGE_RECORDED,
        types::HUMAN_CHANGE_CONFLICTED,
        types::HUMAN_CHANGE_RESOLVED,
        types::HUMAN_CHANGE_WITHDRAWN,
    ]
    .contains(&kind)
    {
        return None;
    }
    let main: Vec<AgentName> = ctx.main_agents.iter().cloned().collect();
    // A resolution is the act of the integration's actor, and a withdrawal that of the human who ran `fld record`, so their data names the human whose change it is.
    let human = match event.data.get("human").and_then(serde_json::Value::as_str) {
        Some(human)
            if kind == types::HUMAN_CHANGE_RESOLVED || kind == types::HUMAN_CHANGE_WITHDRAWN =>
        {
            human
        }
        _ => event.actor.as_str(),
    };
    let strings = |key: &str| -> Vec<String> {
        event
            .data
            .get(key)
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let count = |key: &str, listed: usize| {
        event
            .data
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(listed)
    };
    let files = strings("files");
    let file_count = count("file_count", files.len());
    let tasks = strings("tasks");
    let workspaces = strings("workspaces");
    // The lists are spelled out when the notice still fits; otherwise only counted.
    let fits =
        |text: &str, tool: &str| format!("fold: {text}. Call {tool}.").len() <= MAX_NOTICE_LEN;
    if kind == types::HUMAN_CHANGE_CONFLICTED {
        let paths = strings("conflicted");
        let conflicted = count("conflicted_count", paths.len());
        let compose = |spelled: bool| {
            let paths = listed(&paths, conflicted, "file", "files", spelled);
            let mut text =
                format!("{human}'s change conflicts with main in {paths}; main did not move");
            if let Some(tasks) = touched_text(&tasks, &workspaces, spelled) {
                text.push_str(&format!("; touches {tasks}"));
            }
            text.push_str("; explain it and prepare the merge");
            text
        };
        let tool = "open_main_workspace";
        let text = Some(compose(true))
            .filter(|text| fits(text, tool))
            .unwrap_or_else(|| compose(false));
        return Some(vec![(main, text, tool)]);
    }
    if kind == types::HUMAN_CHANGE_WITHDRAWN {
        // The folder holds none of the change any more, so a main workspace that holds its merge holds nothing to submit.
        let text = format!("{human} took back the waiting change; nothing is left to merge");
        let rule = match event
            .data
            .get("workspace")
            .and_then(serde_json::Value::as_str)
        {
            Some(workspace) => (
                main,
                format!("{text}; abandon {workspace}, which holds its merge, and tell the user"),
                "abandon_workspace",
            ),
            None => (
                main,
                format!("{text}; tell the user"),
                "get_repository_state",
            ),
        };
        return Some(vec![rule]);
    }
    let revision = event.revision?;
    let leads: Vec<AgentName> = strings("leads")
        .iter()
        .filter_map(|lead| AgentName::parse(lead).ok())
        .filter(|lead| !ctx.main_agents.contains(lead))
        .collect();
    let lead_rule = || {
        let compose = |spelled: bool| {
            let listed_files = listed(&files, file_count, "file", "files", spelled);
            let touched = touched_text(&tasks, &workspaces, spelled).unwrap_or_default();
            format!(
                "{human} changed {listed_files} in main as {revision}, which {touched} also changes"
            )
        };
        let tool = "get_task";
        let text = Some(compose(true))
            .filter(|text| fits(text, tool))
            .unwrap_or_else(|| compose(false));
        (leads.clone(), text, tool)
    };
    if kind == types::HUMAN_CHANGE_RESOLVED {
        // An agent never hears of its own act, so the main agent hears of a resolution only when another integration brings the change onto main.
        let text = format!(
            "{human}'s waiting change is resolved: main holds it as {revision}; tell the user"
        );
        let mut rules = vec![(main, text, "compare_revisions")];
        if !tasks.is_empty() {
            rules.push(lead_rule());
        }
        return Some(rules);
    }
    let compose = |spelled: bool| {
        let listed_files = listed(&files, file_count, "file", "files", spelled);
        let mut text =
            format!("{human} recorded {revision} from the project folder: {listed_files}");
        match touched_text(&tasks, &workspaces, spelled) {
            Some(tasks) if leads.is_empty() => text.push_str(&format!("; touches {tasks}")),
            Some(tasks) => text.push_str(&format!("; touches {tasks}, whose leads were told")),
            None => text.push_str("; touches no open task"),
        }
        text.push_str("; tell the user");
        text
    };
    let tool = "compare_revisions";
    let text = Some(compose(true))
        .filter(|text| fits(text, tool))
        .unwrap_or_else(|| compose(false));
    let mut rules = vec![(main, text, tool)];
    if !tasks.is_empty() {
        rules.push(lead_rule());
    }
    Some(rules)
}

/// `T-4 (ws-7)`, one task per workspace in the order the daemon wrote them, or a count when `spelled` is false or the list is long; `None` when no open workspace is touched.
fn touched_text(tasks: &[String], workspaces: &[String], spelled: bool) -> Option<String> {
    if tasks.is_empty() {
        return None;
    }
    let pairs: Vec<String> = tasks
        .iter()
        .zip(workspaces.iter().map(Some).chain(std::iter::repeat(None)))
        .map(|(task, workspace)| match workspace {
            Some(workspace) => format!("{task} ({workspace})"),
            None => task.clone(),
        })
        .collect();
    Some(listed(&pairs, pairs.len(), "task", "tasks", spelled))
}

/// The items joined with commas, or `<count> <noun>` when `spelled` is false, when they are fewer than `count` or when they are too long to list.
fn listed(items: &[String], count: usize, one: &str, many: &str, spelled: bool) -> String {
    let joined = items.join(", ");
    if spelled && !items.is_empty() && items.len() == count && joined.len() <= MAX_LISTED_LEN {
        joined
    } else if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

fn rule(event: &Event, ctx: &PushContext) -> Option<(Vec<AgentName>, String, &'static str)> {
    let kind = event.event_type;
    // A submitted batch holds its approved members while a member's current version has only comments; the daemon writes the event once per member and version.
    if kind == types::REVIEW_BATCH_WAITING {
        let proposal = event.proposal?;
        let batch = event.data.get("batch")?.as_str()?;
        let version = event.data.get("version")?.as_u64()?;
        return Some((
            ctx.review_leads.iter().cloned().collect(),
            format!(
                "{batch} waits for a verdict on {proposal} v{version}: its reviewer must approve or request changes"
            ),
            "get_review_batch",
        ));
    }
    if kind == types::PROPOSAL_HUMAN_EDITED {
        let proposal = event.proposal?;
        let version = event.data.get("version")?.as_u64()?;
        return Some((
            ctx.author.iter().cloned().collect(),
            format!(
                "a human edited {proposal} as version {version}; sync your workspace before you work in it"
            ),
            "sync_workspace",
        ));
    }
    if kind == types::PROPOSAL_REVIEWER_FIXED {
        let proposal = event.proposal?;
        let version = event.data.get("version")?.as_u64()?;
        return Some((
            ctx.author.iter().cloned().collect(),
            format!(
                "the reviewer fixed {proposal} as version {version}; sync your workspace before you work in it"
            ),
            "sync_workspace",
        ));
    }
    if kind == types::INTEGRATION_SUCCEEDED {
        let proposal = event.proposal?;
        let agents = ctx.author.iter().chain(&ctx.reviewer).cloned().collect();
        let with = event
            .data
            .get("batch")
            .and_then(serde_json::Value::as_str)
            .map(|batch| format!(" with batch {batch}"))
            .unwrap_or_default();
        let text = match event.revision {
            Some(revision) => format!("{proposal} integrated{with} as {revision}"),
            None => format!("{proposal} integrated{with}"),
        };
        return Some((agents, text, "get_inbox"));
    }
    if kind == types::REVIEW_SUBMITTED {
        let proposal = event.proposal?;
        return match verdict(event)? {
            "request_changes" => Some((
                ctx.author.iter().cloned().collect(),
                format!("{proposal} changes requested"),
                "get_review_comments",
            )),
            "approve" if event.actor.is_human() && !ctx.retired => Some((
                ctx.review_claim.iter().cloned().collect(),
                format!("{proposal} approved by a human"),
                "integrate_proposal",
            )),
            _ => None,
        };
    }
    if kind == types::INTEGRATION_CONFLICTED {
        let proposal = event.proposal?;
        return Some((
            ctx.author.iter().cloned().collect(),
            format!("{proposal} conflicts with main"),
            "sync_workspace",
        ));
    }
    if kind == types::TASK_STATUS_CHANGED {
        let task = event.task?;
        let to = event.data.get("to").and_then(serde_json::Value::as_str)?;
        if to != TaskStatus::Todo.as_str() {
            return None;
        }
        if ctx.held_from_agents {
            return None;
        }
        if let Some(holder) = &ctx.implementation_claim {
            return Some((
                vec![holder.clone()],
                format!("{task} is ready"),
                "create_workspace",
            ));
        }
        return Some(ready_route(task, ctx));
    }
    if kind == types::TASK_UPDATED {
        let task = event.task?;
        // Only a label that joins a ready task to an area is news; any other edit sends nothing.
        let added = event
            .data
            .get("labels_added")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|labels| !labels.is_empty());
        if !added || ctx.held_from_agents || ctx.areas.is_empty() {
            return None;
        }
        return Some(ready_route(task, ctx));
    }
    if kind == types::TASK_HOLD_CLEARED {
        let task = event.task?;
        if ctx.held_from_agents {
            return None;
        }
        return Some(ready_route(task, ctx));
    }
    if kind == types::TASK_REPORTED {
        let task = event.task?;
        let checkers = if ctx.area_leads.is_empty() {
            &ctx.main_agents
        } else {
            &ctx.area_leads
        };
        return Some((
            checkers.iter().cloned().collect(),
            format!("{task} is reported finished and waits for your check"),
            "get_task",
        ));
    }
    if kind == types::TASK_SENT_BACK {
        let task = event.task?;
        return Some((
            ctx.implementation_claim.iter().cloned().collect(),
            format!("{task} was sent back"),
            "get_task",
        ));
    }
    if kind == types::INTEGRATION_HELD {
        let head = event
            .revision
            .map_or_else(|| "main".to_owned(), |revision| format!("main {revision}"));
        return Some((
            ctx.main_agents.iter().cloned().collect(),
            format!(
                "fold-checks.toml on {head} is not valid; fold holds the integrations of proposals until it is fixed on main"
            ),
            "open_main_workspace",
        ));
    }
    if kind == types::PROJECT_PAUSED {
        return Some((
            ctx.main_agents.iter().cloned().collect(),
            "the agents are paused: every worker is stopped; start no work until a resume"
                .to_owned(),
            "list_workers",
        ));
    }
    if kind == types::PROJECT_RESUMED {
        return Some((
            ctx.main_agents.iter().cloned().collect(),
            "the agents are resumed: the paused workers run again".to_owned(),
            "list_workers",
        ));
    }
    if kind == types::WORKER_FAILED {
        return unreachable_worker_rule(event, ctx).or_else(|| ended_reviewer_rule(event, ctx));
    }
    if kind == types::WORKER_STOPPED {
        return ended_reviewer_rule(event, ctx);
    }
    if kind == types::WORKER_MODEL_UNREACHABLE {
        return model_unreachable_rule(event, ctx);
    }
    if let Some(rule) = request_rule(event, ctx) {
        return Some(rule);
    }
    if kind == types::MESSAGE_SENT {
        let message = event
            .data
            .get("message")
            .and_then(serde_json::Value::as_str)?
            .parse::<MessageId>()
            .ok()?;
        return Some((
            ctx.recipients.iter().cloned().collect(),
            format!("message {message} arrived"),
            "get_inbox",
        ));
    }
    None
}

/// A worker that failed because it cannot reach fold reaches the main agent and the agent that started it, so they tell the human what brings it back instead of waiting for the worker; the worker keeps its claims until then. Any other failure sends nothing: a new start fixes it.
fn unreachable_worker_rule(
    event: &Event,
    ctx: &PushContext,
) -> Option<(Vec<AgentName>, String, &'static str)> {
    let reason = event
        .data
        .get("reason")
        .and_then(serde_json::Value::as_str)?;
    let agent = event
        .data
        .get("agent")
        .and_then(serde_json::Value::as_str)?;
    let text = match reason {
        MCP_UNAVAILABLE => format!(
            "worker {agent} failed: its fold MCP server did not connect; ask the human to run `fld restart`, which resumes it"
        ),
        FLD_MISSING => format!(
            "worker {agent} failed: its fld is gone; ask the human to install fld and run `fld restart`, which resumes it, or call resume_worker once fld is back"
        ),
        CLAUDE_NOT_FOUND => format!(
            "worker {agent} failed: the daemon finds no claude; ask the human to install Claude Code or fix PATH, then run `fld restart`, which resumes it"
        ),
        _ => return None,
    };
    let agents = ctx
        .main_agents
        .iter()
        .chain(&ctx.starter)
        .filter(|starter| starter.as_str() != agent)
        .cloned()
        .collect();
    Some((agents, text, "list_workers"))
}

/// A reviewer that ended with versions that have no verdict, which fold does not resume it for, reaches the review lead: fold resumes a reviewer once for each version, and the claim it keeps hides the versions from the review lead's notices. `start_worker` takes over the claim of a reviewer that has ended.
fn ended_reviewer_rule(
    event: &Event,
    ctx: &PushContext,
) -> Option<(Vec<AgentName>, String, &'static str)> {
    if ctx.unreviewed.is_empty() {
        return None;
    }
    let agent = event
        .data
        .get("agent")
        .and_then(serde_json::Value::as_str)?;
    let proposals: Vec<String> = ctx.unreviewed.iter().map(ToString::to_string).collect();
    let text = format!(
        "{agent} ended with no verdict on {}; a reviewer you start takes over its claim",
        listed(&proposals, proposals.len(), "proposal", "proposals", true)
    );
    let leads = ctx
        .review_leads
        .iter()
        .filter(|lead| lead.as_str() != agent)
        .cloned()
        .collect();
    Some((leads, text, "start_worker"))
}

/// A worker whose turns keep failing before any token reached the model reaches the main agent and the agent that started it, so they ask the human to fix what Claude Code needs instead of sending the worker more work. Its error is on its row, which `list_workers` shows; the notice holds only the hint.
fn model_unreachable_rule(
    event: &Event,
    ctx: &PushContext,
) -> Option<(Vec<AgentName>, String, &'static str)> {
    let agent = event
        .data
        .get("agent")
        .and_then(serde_json::Value::as_str)?;
    let text = format!(
        "worker {agent} cannot reach the model; ask the human to {}",
        crate::agents::turn_errors::MODEL_UNREACHABLE_HINT
    );
    let agents = ctx
        .main_agents
        .iter()
        .chain(&ctx.starter)
        .filter(|starter| starter.as_str() != agent)
        .cloned()
        .collect();
    Some((agents, text, "list_workers"))
}

/// A ready task nobody claimed goes to whoever starts its builder. A task of Main goes to the main agent, which claims and builds it itself; the lead of any other area starts a task worker; while the area has no lead, the main agent starts one; a task in no area waits for the main agent to give it an area label. Both an approved request card and a dependent of an integrated task turn ready through `task.status_changed`, so this one rule routes them.
fn ready_route(task: TaskId, ctx: &PushContext) -> (Vec<AgentName>, String, &'static str) {
    let main = || ctx.main_agents.iter().cloned().collect();
    if ctx.areas.iter().any(AreaName::is_main) {
        return (
            main(),
            format!("{task} is ready in area main, for you to build"),
            "claim_task",
        );
    }
    if !ctx.area_leads.is_empty() {
        return (
            ctx.area_leads.iter().cloned().collect(),
            format!("{task} is ready in your area"),
            "start_worker",
        );
    }
    if let Some(area) = ctx.areas.first() {
        return (
            main(),
            format!("{task} is ready in area {area}, which has no lead"),
            "start_worker",
        );
    }
    (
        main(),
        format!("{task} is ready and has no area"),
        "update_task",
    )
}

/// A request asks the main agent and every area lead to claim it, so whoever finds it relevant drafts it, and each later step goes to whoever acts next: the requester's agent reads a draft, the drafter's agent revises it or starts the approved tasks. A human requester reads the web, so nothing is pushed to one.
fn request_rule(
    event: &Event,
    ctx: &PushContext,
) -> Option<(Vec<AgentName>, String, &'static str)> {
    let kind = event.event_type;
    if ![
        types::REQUEST_CREATED,
        types::REQUEST_DRAFT_SUBMITTED,
        types::REQUEST_CHANGES_REQUESTED,
        types::REQUEST_APPROVED,
    ]
    .contains(&kind)
    {
        return None;
    }
    let request = request_of(event)?;
    let drafter = || ctx.drafter.iter().cloned().collect();
    if kind == types::REQUEST_CREATED {
        let agents = ctx.main_agents.union(&ctx.area_leads).cloned().collect();
        return Some((agents, format!("{request} created"), "claim_request"));
    }
    if kind == types::REQUEST_DRAFT_SUBMITTED {
        let role = event.data.get("role").and_then(serde_json::Value::as_str)?;
        if role != "drafter" {
            return None;
        }
        let text = match event
            .data
            .get("version")
            .and_then(serde_json::Value::as_u64)
        {
            Some(version) => format!("{request} draft v{version} ready"),
            None => format!("{request} draft ready"),
        };
        return Some((ctx.requester.iter().cloned().collect(), text, "get_request"));
    }
    if kind == types::REQUEST_CHANGES_REQUESTED {
        return Some((
            drafter(),
            format!("{request} changes requested"),
            "get_request",
        ));
    }
    let tasks: Vec<TaskId> = event
        .data
        .get("tasks")
        .and_then(serde_json::Value::as_array)
        .map(|tasks| {
            tasks
                .iter()
                .filter_map(|task| task.as_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default();
    Some((drafter(), approved_text(request, &tasks), "get_ready_tasks"))
}

/// The longest task list an approval notice spells out; a longer draft is counted instead, so the notice stays within [`MAX_NOTICE_LEN`].
const MAX_LISTED_TASKS_LEN: usize = 120;

fn approved_text(request: RequestId, tasks: &[TaskId]) -> String {
    let listed = tasks
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    if tasks.is_empty() {
        format!("{request} approved")
    } else if listed.len() <= MAX_LISTED_TASKS_LEN {
        format!("{request} approved as {listed}")
    } else {
        format!("{request} approved as {} tasks", tasks.len())
    }
}

/// A proposal that needs a review reaches the review lead, which gives the review to a reviewer with `start_worker`, and the holder of its review claim, which checks out the version to review. A new version, or one a human sent for review again, reaches the review lead only while nobody holds the claim: fold resumes a holder that has ended for that version. A review lead that holds the claim cannot check the version out, so it is told to release the claim and give the review to a reviewer. No review lead hears of a proposal of a task of Main or of its own proposal.
fn review_rules(
    event: &Event,
    ctx: &PushContext,
) -> Option<Vec<(Vec<AgentName>, String, &'static str)>> {
    let kind = event.event_type;
    let proposal = event.proposal?;
    let (text, leads_too) = if kind == types::PROPOSAL_OPENED {
        let text = match event.task {
            Some(task) => format!("{proposal} opened for {task}"),
            None => format!("{proposal} opened"),
        };
        (text, !ctx.planned)
    } else if kind == types::PROPOSAL_VERSION_ADDED {
        let text = match event
            .data
            .get("version")
            .and_then(serde_json::Value::as_u64)
        {
            Some(version) => format!("{proposal} has version {version}"),
            None => format!("{proposal} has a new version"),
        };
        (text, ctx.review_claim.is_none())
    } else if kind == types::PROPOSAL_REVIEW_AGAIN_REQUESTED {
        let version = event.data.get("version")?.as_u64()?;
        let text = format!("a human sent {proposal} version {version} for review again");
        (text, ctx.review_claim.is_none())
    } else {
        return None;
    };
    let leads = ctx
        .review_leads
        .iter()
        .filter(|_| leads_too && !ctx.main_task)
        .filter(|agent| ctx.author.as_ref() != Some(*agent))
        .cloned()
        .collect();
    let holder = ctx.review_claim.iter().cloned().collect();
    let held_by_lead = ctx
        .review_claim
        .as_ref()
        .is_some_and(|agent| ctx.review_leads.contains(agent));
    let held = if held_by_lead {
        (
            holder,
            format!(
                "{text}; release your review claim with release_review, then give it to a reviewer"
            ),
            "start_worker",
        )
    } else {
        (holder, text.clone(), "create_review_checkout")
    };
    Some(vec![(leads, text, "start_worker"), held])
}

/// A review plan reaches the review lead when it changes without the lead: an awaited task opened its proposal and joined, or stopped being eligible and left; the wait ended at the deadline; or fold ended the plan because no member is left. Once no task is awaited, the notice names the proposals to give one reviewer with the plan. The daemon writes the plan's open proposals and awaited tasks after the change into the event.
fn plan_rules(
    event: &Event,
    ctx: &PushContext,
) -> Option<Vec<(Vec<AgentName>, String, &'static str)>> {
    let kind = event.event_type;
    if ![
        types::REVIEW_PLAN_JOINED,
        types::REVIEW_PLAN_LEFT,
        types::REVIEW_PLAN_WAIT_ENDED,
        types::REVIEW_PLAN_ENDED,
    ]
    .contains(&kind)
    {
        return None;
    }
    let leads: Vec<AgentName> = ctx.review_leads.iter().cloned().collect();
    let text = |key: &str| event.data.get(key).and_then(serde_json::Value::as_str);
    let strings = |key: &str| -> Vec<String> {
        event
            .data
            .get(key)
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let plan = text("plan")?;
    let proposals = strings("proposals");
    let awaited = strings("awaited");
    let start = |proposals: &[String]| match proposals {
        [one] => format!("start a reviewer for {one} with plan {plan}"),
        many => format!(
            "start one reviewer for {} with plan {plan}",
            many.join(", ")
        ),
    };
    let (body, tool) = if kind == types::REVIEW_PLAN_WAIT_ENDED {
        let left = strings("left");
        let left = if left.is_empty() {
            String::new()
        } else {
            format!("; {} left it", left.join(", "))
        };
        (
            format!("the wait of {plan} ended{left}; {}", start(&proposals)),
            "start_worker",
        )
    } else if kind == types::REVIEW_PLAN_ENDED {
        (
            format!("{plan} ended: no member is left to review"),
            "list_proposals",
        )
    } else {
        let task = text("task")?;
        let change = if kind == types::REVIEW_PLAN_JOINED {
            format!("{} of {task} joined {plan}", text("proposal")?)
        } else {
            format!("{task} left {plan} ({})", text("reason")?)
        };
        if !awaited.is_empty() {
            (
                format!("{change}; it still waits for {}", awaited.join(", ")),
                "get_review_plan",
            )
        } else if !proposals.is_empty() {
            (
                format!("{change}; no task is awaited now, so {}", start(&proposals)),
                "start_worker",
            )
        } else {
            // The plan has no member left and ends in the same write; its own notice follows.
            return Some(Vec::new());
        }
    };
    Some(vec![(leads, body, tool)])
}

/// `review.submitted` names its verdict in `data.verdict`, with the values of `reviews.verdict`.
fn verdict(event: &Event) -> Option<&str> {
    event
        .data
        .get("verdict")
        .and_then(serde_json::Value::as_str)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::events::EventActor;
    use crate::ids::{ProposalId, RevisionId};
    use crate::time::Timestamp;

    fn name(text: &str) -> AgentName {
        AgentName::parse(text).unwrap()
    }

    fn event(event_type: crate::events::EventType, actor: &str) -> Event {
        Event {
            seq: 7,
            event_type,
            actor: EventActor::parse(actor).unwrap(),
            task: Some(TaskId::from_number(12).unwrap()),
            proposal: Some(ProposalId::from_number(3).unwrap()),
            workspace: None,
            revision: None,
            data: json!({}),
            created_at: Timestamp::from_millis(0).unwrap(),
        }
    }

    fn with_data(event: Event, data: serde_json::Value) -> Event {
        Event { data, ..event }
    }

    fn ctx() -> PushContext {
        PushContext {
            review_leads: [name("lead"), name("review"), name("frontend")]
                .into_iter()
                .collect(),
            author: Some(name("frontend")),
            review_claim: Some(name("review")),
            implementation_claim: Some(name("backend")),
            recipients: [name("backend"), name("docs")].into_iter().collect(),
            reviewer: Some(name("review")),
            retired: false,
            requester: Some(name("frontend")),
            drafter: Some(name("backend")),
            area_leads: BTreeSet::new(),
            areas: BTreeSet::new(),
            main_agents: BTreeSet::new(),
            held_from_agents: false,
            starter: None,
            main_task: false,
            unreviewed: Vec::new(),
            planned: false,
        }
    }

    fn sent(notices: &[Notice]) -> Vec<(String, String, &'static str)> {
        notices
            .iter()
            .map(|notice| {
                (
                    notice.agent.to_string(),
                    notice.text.clone(),
                    notice.next_tool,
                )
            })
            .collect()
    }

    fn row(agent: &str, text: &str, tool: &'static str) -> (String, String, &'static str) {
        (agent.to_owned(), text.to_owned(), tool)
    }

    #[test]
    fn an_opened_proposal_reaches_every_review_lead_but_the_authors_agent() {
        let notices = notice_for(&event(types::PROPOSAL_OPENED, "frontend/ui"), &ctx());
        assert_eq!(
            sent(&notices),
            vec![
                row(
                    "lead",
                    "fold: P-3 opened for T-12. Call start_worker.",
                    "start_worker"
                ),
                row(
                    "review",
                    "fold: P-3 opened for T-12. Call start_worker.",
                    "start_worker"
                ),
            ]
        );
    }

    #[test]
    fn a_proposal_that_joins_a_plan_reaches_the_review_lead_only_through_the_plan() {
        // A first version that waits in a plan has no review claim yet.
        let planned = PushContext {
            planned: true,
            review_claim: None,
            ..ctx()
        };
        let notices = notice_for(&event(types::PROPOSAL_OPENED, "frontend/ui"), &planned);
        assert!(
            sent(&notices)
                .iter()
                .all(|(_, _, tool)| *tool != "start_worker")
        );
        let joined = with_data(
            event(types::REVIEW_PLAN_JOINED, "fold"),
            json!({"plan": "rp-1", "task": "T-12", "proposal": "P-3", "proposals": ["P-2", "P-3"], "awaited": ["T-14"]}),
        );
        assert_eq!(
            sent(&notice_for(&joined, &ctx()))[2],
            row(
                "review",
                "fold: P-3 of T-12 joined rp-1; it still waits for T-14. Call get_review_plan.",
                "get_review_plan"
            )
        );
        let last = with_data(
            event(types::REVIEW_PLAN_JOINED, "fold"),
            json!({"plan": "rp-1", "task": "T-12", "proposal": "P-3", "proposals": ["P-2", "P-3"], "awaited": []}),
        );
        assert_eq!(
            sent(&notice_for(&last, &ctx()))[2],
            row(
                "review",
                "fold: P-3 of T-12 joined rp-1; no task is awaited now, so start one reviewer for P-2, P-3 with plan rp-1. Call start_worker.",
                "start_worker"
            )
        );
    }

    #[test]
    fn a_plan_tells_its_review_lead_of_a_leave_the_deadline_and_its_end() {
        let left = with_data(
            event(types::REVIEW_PLAN_LEFT, "fold"),
            json!({"plan": "rp-1", "task": "T-14", "reason": "blocker", "proposals": ["P-3"], "awaited": []}),
        );
        assert_eq!(
            sent(&notice_for(&left, &ctx()))[2],
            row(
                "review",
                "fold: T-14 left rp-1 (blocker); no task is awaited now, so start a reviewer for P-3 with plan rp-1. Call start_worker.",
                "start_worker"
            )
        );
        let emptied = with_data(
            event(types::REVIEW_PLAN_LEFT, "fold"),
            json!({"plan": "rp-1", "task": "T-14", "reason": "archived", "proposals": [], "awaited": []}),
        );
        assert!(notice_for(&emptied, &ctx()).is_empty());
        let still = with_data(
            event(types::REVIEW_PLAN_LEFT, "fold"),
            json!({"plan": "rp-1", "task": "T-14", "reason": "backlog", "proposals": ["P-3"], "awaited": ["T-15"]}),
        );
        assert_eq!(sent(&notice_for(&still, &ctx()))[1].2, "get_review_plan");
        let mut ended = event(types::REVIEW_PLAN_WAIT_ENDED, "fold");
        ended.task = None;
        ended.proposal = None;
        let ended = with_data(
            ended,
            json!({"plan": "rp-1", "proposals": ["P-3", "P-4", "P-5", "P-6", "P-7"], "left": ["T-15"]}),
        );
        let notices = notice_for(&ended, &ctx());
        assert_eq!(notices.len(), 3, "every review lead");
        assert_eq!(
            notices[0].text,
            "fold: the wait of rp-1 ended; T-15 left it; start one reviewer for P-3, P-4, P-5, P-6, P-7 with plan rp-1. Call start_worker."
        );
        assert!(notices[0].text.len() <= MAX_NOTICE_LEN);
        let gone = with_data(
            event(types::REVIEW_PLAN_ENDED, "fold"),
            json!({"plan": "rp-1", "reason": "no_member"}),
        );
        assert_eq!(
            sent(&notice_for(&gone, &ctx()))[0].1,
            "fold: rp-1 ended: no member is left to review. Call list_proposals."
        );
        let by_lead = with_data(
            event(types::REVIEW_PLAN_ENDED, "review"),
            json!({"plan": "rp-1", "reason": "removed"}),
        );
        assert!(
            sent(&notice_for(&by_lead, &ctx()))
                .iter()
                .all(|(agent, _, _)| agent != "review")
        );
    }

    #[test]
    fn no_review_lead_hears_of_a_proposal_of_a_task_of_main() {
        let main_task = PushContext {
            main_task: true,
            review_claim: None,
            ..ctx()
        };
        for kind in [types::PROPOSAL_OPENED, types::PROPOSAL_VERSION_ADDED] {
            let event = with_data(event(kind, "main"), json!({"version": 2}));
            assert!(notice_for(&event, &main_task).is_empty(), "{kind}");
        }
        let again = with_data(
            event(types::PROPOSAL_REVIEW_AGAIN_REQUESTED, "human:ana"),
            json!({"version": 2}),
        );
        assert!(notice_for(&again, &main_task).is_empty());
    }

    #[test]
    fn a_ready_task_of_main_goes_to_the_main_agent_to_build() {
        let status = with_data(
            event(types::TASK_STATUS_CHANGED, "fold"),
            json!({"from": "backlog", "to": "todo"}),
        );
        let main_ready = ready(None, &[], &["main"]);
        let expected = vec![row(
            "lead",
            "fold: T-12 is ready in area main, for you to build. Call claim_task.",
            "claim_task",
        )];
        assert_eq!(sent(&notice_for(&status, &main_ready)), expected);
        assert_eq!(
            sent(&notice_for(&label_added(&["main"]), &main_ready)),
            expected
        );
        assert_eq!(
            sent(&notice_for(
                &event(types::TASK_HOLD_CLEARED, "human:ana"),
                &main_ready
            )),
            expected
        );
    }

    #[test]
    fn a_report_reaches_the_area_lead_or_else_the_main_agent() {
        let event = event(types::TASK_REPORTED, "backend");
        let mut ctx = ctx();
        ctx.area_leads = [name("web-lead")].into_iter().collect();
        ctx.main_agents = [name("main")].into_iter().collect();
        assert_eq!(
            sent(&notice_for(&event, &ctx)),
            vec![row(
                "web-lead",
                "fold: T-12 is reported finished and waits for your check. Call get_task.",
                "get_task"
            )]
        );
        ctx.area_leads.clear();
        assert_eq!(
            sent(&notice_for(&event, &ctx)),
            vec![row(
                "main",
                "fold: T-12 is reported finished and waits for your check. Call get_task.",
                "get_task"
            )]
        );
    }

    #[test]
    fn a_send_back_reaches_the_claim_holder() {
        let event = event(types::TASK_SENT_BACK, "web-lead");
        assert_eq!(
            sent(&notice_for(&event, &ctx())),
            vec![row(
                "backend",
                "fold: T-12 was sent back. Call get_task.",
                "get_task"
            )]
        );
        let mut nobody = ctx();
        nobody.implementation_claim = None;
        assert!(notice_for(&event, &nobody).is_empty());
    }

    #[test]
    fn requested_changes_reach_the_author() {
        let event = with_data(
            event(types::REVIEW_SUBMITTED, "review"),
            json!({"verdict": "request_changes"}),
        );
        assert_eq!(
            sent(&notice_for(&event, &ctx())),
            vec![row(
                "frontend",
                "fold: P-3 changes requested. Call get_review_comments.",
                "get_review_comments"
            )]
        );
    }

    #[test]
    fn a_conflict_reaches_the_author() {
        let event = event(types::INTEGRATION_CONFLICTED, "lead");
        assert_eq!(
            sent(&notice_for(&event, &ctx())),
            vec![row(
                "frontend",
                "fold: P-3 conflicts with main. Call sync_workspace.",
                "sync_workspace"
            )]
        );
    }

    #[test]
    fn a_task_turning_todo_reaches_its_claim_holder() {
        let event = with_data(
            event(types::TASK_STATUS_CHANGED, "fold"),
            json!({"from": "backlog", "to": "todo"}),
        );
        assert_eq!(
            sent(&notice_for(&event, &ctx())),
            vec![row(
                "backend",
                "fold: T-12 is ready. Call create_workspace.",
                "create_workspace"
            )]
        );
        let elsewhere = with_data(event.clone(), json!({"from": "todo", "to": "in_progress"}));
        assert!(notice_for(&elsewhere, &ctx()).is_empty());
        let unclaimed = PushContext {
            implementation_claim: None,
            ..ctx()
        };
        assert!(notice_for(&event, &unclaimed).is_empty());
    }

    fn ready(implementation_claim: Option<&str>, leads: &[&str], areas: &[&str]) -> PushContext {
        PushContext {
            implementation_claim: implementation_claim.map(name),
            area_leads: leads.iter().map(|lead| name(lead)).collect(),
            areas: areas
                .iter()
                .map(|area| AreaName::parse(area).unwrap())
                .collect(),
            main_agents: [name("lead")].into_iter().collect(),
            ..ctx()
        }
    }

    #[test]
    fn a_ready_task_nobody_claimed_goes_to_the_lead_of_its_area() {
        let event = with_data(
            event(types::TASK_STATUS_CHANGED, "fold"),
            json!({"from": "backlog", "to": "todo"}),
        );
        assert_eq!(
            sent(&notice_for(
                &event,
                &ready(None, &["backend-lead"], &["backend"])
            )),
            vec![row(
                "backend-lead",
                "fold: T-12 is ready in your area. Call start_worker.",
                "start_worker"
            )]
        );
        assert_eq!(
            sent(&notice_for(
                &event,
                &ready(Some("backend-worker"), &["backend-lead"], &["backend"])
            )),
            vec![row(
                "backend-worker",
                "fold: T-12 is ready. Call create_workspace.",
                "create_workspace"
            )],
            "the claim holder builds it, so its lead hears nothing"
        );
    }

    #[test]
    fn a_human_task_or_a_task_on_hold_is_offered_to_no_agent() {
        let event = with_data(
            event(types::TASK_STATUS_CHANGED, "fold"),
            json!({"from": "backlog", "to": "todo"}),
        );
        let held = PushContext {
            held_from_agents: true,
            ..ready(None, &["backend-lead"], &["backend"])
        };
        assert!(notice_for(&event, &held).is_empty());
    }

    #[test]
    fn a_cleared_hold_offers_the_ready_task_to_its_lead() {
        let event = event(types::TASK_HOLD_CLEARED, "human:ana");
        let open = ready(None, &["backend-lead"], &["backend"]);
        assert_eq!(
            sent(&notice_for(&event, &open)),
            vec![row(
                "backend-lead",
                "fold: T-12 is ready in your area. Call start_worker.",
                "start_worker"
            )]
        );
        let not_ready = PushContext {
            held_from_agents: true,
            ..open
        };
        assert!(notice_for(&event, &not_ready).is_empty());
    }

    #[test]
    fn a_held_task_turning_todo_sends_its_holder_nothing() {
        let status = with_data(
            event(types::TASK_STATUS_CHANGED, "fold"),
            json!({"from": "backlog", "to": "todo"}),
        );
        let held = PushContext {
            held_from_agents: true,
            ..ready(Some("frontend"), &["backend-lead"], &["backend"])
        };
        assert!(notice_for(&status, &held).is_empty());
    }

    #[test]
    fn a_ready_task_without_a_led_area_goes_to_the_main_agent() {
        let event = with_data(
            event(types::TASK_STATUS_CHANGED, "fold"),
            json!({"from": "backlog", "to": "todo"}),
        );
        assert_eq!(
            sent(&notice_for(&event, &ready(None, &[], &["backend"]))),
            vec![row(
                "lead",
                "fold: T-12 is ready in area backend, which has no lead. Call start_worker.",
                "start_worker"
            )]
        );
        assert_eq!(
            sent(&notice_for(&event, &ready(None, &[], &[]))),
            vec![row(
                "lead",
                "fold: T-12 is ready and has no area. Call update_task.",
                "update_task"
            )]
        );
        let no_main = PushContext {
            main_agents: BTreeSet::new(),
            ..ready(None, &[], &[])
        };
        assert!(notice_for(&event, &no_main).is_empty());
    }

    fn label_added(labels: &[&str]) -> Event {
        with_data(
            event(types::TASK_UPDATED, "frontend"),
            json!({"fields": ["labels"], "labels_added": labels}),
        )
    }

    #[test]
    fn an_area_label_added_to_a_ready_task_goes_to_the_lead_of_that_area() {
        assert!(is_pushed(&label_added(&["ui"])));
        assert_eq!(
            sent(&notice_for(
                &label_added(&["ui"]),
                &ready(None, &["web-lead"], &["web"])
            )),
            vec![row(
                "web-lead",
                "fold: T-12 is ready in your area. Call start_worker.",
                "start_worker"
            )]
        );
    }

    #[test]
    fn an_added_label_of_an_area_without_a_lead_goes_to_the_main_agent() {
        assert_eq!(
            sent(&notice_for(
                &label_added(&["ui"]),
                &ready(None, &[], &["web"])
            )),
            vec![row(
                "lead",
                "fold: T-12 is ready in area web, which has no lead. Call start_worker.",
                "start_worker"
            )]
        );
    }

    #[test]
    fn an_added_label_on_a_task_no_agent_can_take_sends_nothing() {
        let held = PushContext {
            held_from_agents: true,
            ..ready(None, &["web-lead"], &["web"])
        };
        assert!(notice_for(&label_added(&["ui"]), &held).is_empty());
    }

    #[test]
    fn an_edit_that_adds_no_area_label_sends_nothing() {
        let plain = with_data(
            event(types::TASK_UPDATED, "frontend"),
            json!({"fields": ["title"], "labels_added": []}),
        );
        assert!(notice_for(&plain, &ready(None, &["web-lead"], &["web"])).is_empty());
        let no_area = ready(None, &[], &[]);
        assert!(notice_for(&label_added(&["chore"]), &no_area).is_empty());
    }

    #[test]
    fn a_message_reaches_each_recipient_agent() {
        let event = with_data(
            event(types::MESSAGE_SENT, "human:ana"),
            json!({"message": "m-4", "recipients": ["backend", "tag:docs"]}),
        );
        let text = "fold: message m-4 arrived. Call get_inbox.";
        assert_eq!(
            sent(&notice_for(&event, &ctx())),
            vec![
                row("backend", text, "get_inbox"),
                row("docs", text, "get_inbox"),
            ]
        );
    }

    #[test]
    fn a_human_approval_reaches_the_review_claim_holder() {
        let event = with_data(
            event(types::REVIEW_SUBMITTED, "human:ana"),
            json!({"verdict": "approve"}),
        );
        let text = "fold: P-3 approved by a human. Call integrate_proposal.";
        assert_eq!(
            sent(&notice_for(&event, &ctx())),
            vec![row("review", text, "integrate_proposal")]
        );
        // Without a claim holder nobody is told: the automatic integration counts the human's approval, and the author's agent never integrates.
        let unclaimed = PushContext {
            review_claim: None,
            ..ctx()
        };
        assert!(notice_for(&event, &unclaimed).is_empty());
    }

    #[test]
    fn an_agents_approval_and_a_comment_send_nothing() {
        let by_agent = with_data(
            event(types::REVIEW_SUBMITTED, "review"),
            json!({"verdict": "approve"}),
        );
        assert!(notice_for(&by_agent, &ctx()).is_empty());
        let comment = with_data(
            event(types::REVIEW_SUBMITTED, "human:ana"),
            json!({"verdict": "comment"}),
        );
        assert!(notice_for(&comment, &ctx()).is_empty());
        let none = event(types::REVIEW_SUBMITTED, "human:ana");
        assert!(notice_for(&none, &ctx()).is_empty());
    }

    #[test]
    fn the_actors_own_agent_never_hears_its_own_event() {
        let own_message = with_data(
            event(types::MESSAGE_SENT, "backend/api"),
            json!({"message": "m-4"}),
        );
        assert_eq!(
            sent(&notice_for(&own_message, &ctx())),
            vec![row(
                "docs",
                "fold: message m-4 arrived. Call get_inbox.",
                "get_inbox"
            )]
        );
        let own_conflict = event(types::INTEGRATION_CONFLICTED, "frontend");
        assert!(notice_for(&own_conflict, &ctx()).is_empty());
        let own_opened = PushContext {
            author: None,
            ..ctx()
        };
        let notices = notice_for(&event(types::PROPOSAL_OPENED, "lead"), &own_opened);
        assert_eq!(
            notices
                .iter()
                .map(|notice| notice.agent.as_str())
                .collect::<Vec<_>>(),
            vec!["frontend", "review"]
        );
    }

    #[test]
    fn a_pause_and_a_resume_reach_the_main_agent_but_not_the_one_who_acted() {
        let mut ctx = ctx();
        ctx.main_agents = [name("main")].into_iter().collect();
        let paused = event(types::PROJECT_PAUSED, "human:rafeed");
        assert!(is_pushed(&paused));
        assert_eq!(
            sent(&notice_for(&paused, &ctx)),
            vec![row(
                "main",
                "fold: the agents are paused: every worker is stopped; start no work until a resume. Call list_workers.",
                "list_workers"
            )]
        );
        let resumed = event(types::PROJECT_RESUMED, "human:rafeed");
        assert_eq!(
            sent(&notice_for(&resumed, &ctx)),
            vec![row(
                "main",
                "fold: the agents are resumed: the paused workers run again. Call list_workers.",
                "list_workers"
            )]
        );
        assert!(notice_for(&event(types::PROJECT_PAUSED, "main"), &ctx).is_empty());
    }

    #[test]
    fn integrations_held_by_main_tell_the_main_agent() {
        let mut ctx = ctx();
        ctx.main_agents = [name("main")].into_iter().collect();
        let mut held = event(types::INTEGRATION_HELD, "fold");
        assert!(is_pushed(&held));
        held.revision = RevisionId::from_number(7);
        assert_eq!(
            sent(&notice_for(&held, &ctx)),
            vec![row(
                "main",
                "fold: fold-checks.toml on main r7 is not valid; fold holds the integrations of proposals until it is fixed on main. Call open_main_workspace.",
                "open_main_workspace"
            )]
        );
    }

    #[test]
    fn a_worker_that_cannot_reach_fold_tells_the_main_agent_and_its_starter() {
        let mut ctx = ctx();
        ctx.main_agents = [name("main")].into_iter().collect();
        ctx.starter = Some(name("backend-lead"));
        let failed = |reason: &str| {
            with_data(
                event(types::WORKER_FAILED, "fold"),
                serde_json::json!({"agent": "backend-worker", "reason": reason}),
            )
        };
        assert!(is_pushed(&failed("exited")));
        let text = "fold: worker backend-worker failed: its fold MCP server did not connect; ask the human to run `fld restart`, which resumes it. Call list_workers.";
        assert_eq!(
            sent(&notice_for(&failed(MCP_UNAVAILABLE), &ctx)),
            vec![
                row("main", text, "list_workers"),
                row("backend-lead", text, "list_workers"),
            ]
        );
        let missing = notice_for(&failed(FLD_MISSING), &ctx);
        assert_eq!(missing.len(), 2);
        assert_eq!(
            missing[0].text,
            "fold: worker backend-worker failed: its fld is gone; ask the human to install fld and run `fld restart`, which resumes it, or call resume_worker once fld is back. Call list_workers."
        );
        let no_claude = notice_for(&failed(CLAUDE_NOT_FOUND), &ctx);
        assert_eq!(no_claude.len(), 2);
        assert_eq!(
            no_claude[0].text,
            "fold: worker backend-worker failed: the daemon finds no claude; ask the human to install Claude Code or fix PATH, then run `fld restart`, which resumes it. Call list_workers."
        );
        let longest = "w".repeat(32);
        for reason in [MCP_UNAVAILABLE, FLD_MISSING, CLAUDE_NOT_FOUND] {
            let event = with_data(
                event(types::WORKER_FAILED, "fold"),
                serde_json::json!({"agent": longest, "reason": reason}),
            );
            assert!(
                notice_for(&event, &ctx)
                    .iter()
                    .all(|notice| notice.text.len() <= MAX_NOTICE_LEN),
                "{reason}"
            );
        }
        assert!(notice_for(&failed("exited"), &ctx).is_empty());
        let lead = with_data(
            event(types::WORKER_FAILED, "fold"),
            serde_json::json!({"agent": "backend-lead", "reason": MCP_UNAVAILABLE}),
        );
        assert_eq!(
            sent(&notice_for(&lead, &ctx))
                .into_iter()
                .map(|(agent, _, _)| agent)
                .collect::<Vec<_>>(),
            ["main"]
        );
        let bare = event(types::WORKER_FAILED, "fold");
        assert!(notice_for(&bare, &ctx).is_empty());
    }

    #[test]
    fn a_reviewer_that_ends_with_versions_nobody_reviews_tells_the_review_leads() {
        let ended = PushContext {
            unreviewed: vec![
                ProposalId::from_number(3).unwrap(),
                ProposalId::from_number(5).unwrap(),
            ],
            ..ctx()
        };
        let text = "fold: reviewer-1 ended with no verdict on P-3, P-5; a reviewer you start takes over its claim. Call start_worker.";
        for kind in [types::WORKER_STOPPED, types::WORKER_FAILED] {
            let event = with_data(event(kind, "fold"), json!({"agent": "reviewer-1"}));
            assert!(is_pushed(&event), "{kind}");
            assert_eq!(
                sent(&notice_for(&event, &ended)),
                vec![
                    row("frontend", text, "start_worker"),
                    row("lead", text, "start_worker"),
                    row("review", text, "start_worker"),
                ],
                "{kind}"
            );
            // A reviewer that left every version with a verdict, or that fold resumes, tells nobody.
            assert!(notice_for(&event, &ctx()).is_empty(), "{kind}");
        }
        // A failure that needs a restart says what brings the reviewer back instead.
        let unreachable = with_data(
            event(types::WORKER_FAILED, "fold"),
            json!({"agent": "reviewer-1", "reason": MCP_UNAVAILABLE}),
        );
        let notices = notice_for(
            &unreachable,
            &PushContext {
                starter: Some(name("review")),
                ..ended
            },
        );
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].next_tool, "list_workers");
    }

    #[test]
    fn a_worker_that_cannot_reach_the_model_tells_the_main_agent_and_its_starter() {
        let mut ctx = ctx();
        ctx.main_agents = [name("main")].into_iter().collect();
        ctx.starter = Some(name("backend-lead"));
        let unreachable = |agent: &str| {
            with_data(
                event(types::WORKER_MODEL_UNREACHABLE, "fold"),
                serde_json::json!({"agent": agent, "turns": 3}),
            )
        };
        assert!(is_pushed(&unreachable("backend-worker")));
        let text = "fold: worker backend-worker cannot reach the model; ask the human to check the Claude Code sign-in, model and usage limit, or run `fld doctor`. Call list_workers.";
        assert_eq!(
            sent(&notice_for(&unreachable("backend-worker"), &ctx)),
            vec![
                row("main", text, "list_workers"),
                row("backend-lead", text, "list_workers"),
            ]
        );
        assert_eq!(
            sent(&notice_for(&unreachable("backend-lead"), &ctx))
                .into_iter()
                .map(|(agent, _, _)| agent)
                .collect::<Vec<_>>(),
            ["main"]
        );
        let longest = "w".repeat(32);
        assert!(
            notice_for(&unreachable(&longest), &ctx)
                .iter()
                .all(|notice| notice.text.len() <= MAX_NOTICE_LEN)
        );
        let bare = event(types::WORKER_MODEL_UNREACHABLE, "fold");
        assert!(notice_for(&bare, &ctx).is_empty());
    }

    #[test]
    fn an_unrelated_event_sends_nothing() {
        for kind in [
            types::TASK_CREATED,
            types::MAIN_ADVANCED,
            types::MESSAGE_ACKNOWLEDGED,
            types::PROPOSAL_CLOSED,
            types::CLAIM_ACQUIRED,
        ] {
            let event = event(kind, "fold");
            assert!(!is_pushed(&event));
            assert!(notice_for(&event, &ctx()).is_empty(), "{kind}");
        }
        assert!(is_pushed(&event(types::MESSAGE_SENT, "fold")));
        assert!(notice_for(&event(types::TASK_STATUS_CHANGED, "fold"), &ctx()).is_empty());
    }

    #[test]
    fn an_event_without_its_id_sends_nothing() {
        let bare = |kind| Event {
            task: None,
            proposal: None,
            ..event(kind, "fold")
        };
        assert!(notice_for(&bare(types::PROPOSAL_OPENED), &ctx()).is_empty());
        assert!(notice_for(&bare(types::INTEGRATION_CONFLICTED), &ctx()).is_empty());
        assert!(
            notice_for(
                &with_data(bare(types::REVIEW_SUBMITTED), json!({"verdict": "approve"})),
                &ctx()
            )
            .is_empty()
        );
        assert!(
            notice_for(
                &with_data(bare(types::TASK_STATUS_CHANGED), json!({"to": "todo"})),
                &ctx()
            )
            .is_empty()
        );
        let unnamed = with_data(event(types::MESSAGE_SENT, "fold"), json!({"message": "x"}));
        assert!(notice_for(&unnamed, &ctx()).is_empty());
        let taskless = Event {
            task: None,
            ..event(types::PROPOSAL_OPENED, "fold")
        };
        assert_eq!(
            notice_for(&taskless, &ctx())[0].text,
            "fold: P-3 opened. Call start_worker."
        );
    }

    #[test]
    fn every_notice_is_one_short_line() {
        let large = |kind| Event {
            task: Some(TaskId::from_number(u32::MAX).unwrap()),
            proposal: Some(ProposalId::from_number(u32::MAX).unwrap()),
            data: json!({
                "to": "todo",
                "verdict": "request_changes",
                "message": format!("m-{}", u32::MAX),
                "request": format!("RQ-{}", u32::MAX),
                "role": "drafter",
                "version": u32::MAX,
                "tasks": (0..20).map(|n| format!("T-{}", u32::MAX - n)).collect::<Vec<_>>(),
            }),
            ..event(kind, "fold")
        };
        for kind in [
            types::PROPOSAL_OPENED,
            types::REVIEW_SUBMITTED,
            types::INTEGRATION_CONFLICTED,
            types::TASK_STATUS_CHANGED,
            types::MESSAGE_SENT,
            types::PROPOSAL_VERSION_ADDED,
            types::INTEGRATION_SUCCEEDED,
            types::REQUEST_CREATED,
            types::REQUEST_DRAFT_SUBMITTED,
            types::REQUEST_CHANGES_REQUESTED,
            types::REQUEST_APPROVED,
            types::PROPOSAL_HUMAN_EDITED,
            types::PROPOSAL_REVIEW_AGAIN_REQUESTED,
            types::PROPOSAL_REVIEWER_FIXED,
        ] {
            let long = AreaName::parse(&"a".repeat(32)).unwrap();
            let unled = PushContext {
                implementation_claim: None,
                areas: [long].into_iter().collect(),
                main_agents: [name("lead")].into_iter().collect(),
                ..ctx()
            };
            let notices: Vec<Notice> = notice_for(&large(kind), &ctx())
                .into_iter()
                .chain(notice_for(&large(kind), &unled))
                .collect();
            assert!(!notices.is_empty(), "{kind}");
            for notice in notices {
                assert!(notice.text.len() <= MAX_NOTICE_LEN);
                assert!(!notice.text.contains('\n'));
                assert!(
                    notice
                        .text
                        .ends_with(&format!("Call {}.", notice.next_tool))
                );
            }
        }
    }

    #[test]
    fn a_claim_holder_hears_of_an_opened_proposal_even_if_it_leads_no_review() {
        let ctx = PushContext {
            review_leads: [name("lead")].into_iter().collect(),
            review_claim: Some(name("docs")),
            ..ctx()
        };
        let notices = notice_for(&event(types::PROPOSAL_OPENED, "frontend"), &ctx);
        let agents: Vec<&str> = notices.iter().map(|n| n.agent.as_str()).collect();
        assert_eq!(agents, ["lead", "docs"]);
    }

    #[test]
    fn a_new_version_reaches_the_claim_holder_or_without_one_the_review_leads() {
        let event = with_data(
            event(types::PROPOSAL_VERSION_ADDED, "frontend/ui"),
            json!({"version": 2, "changeset": "C-4"}),
        );
        let claimed = PushContext {
            review_claim: Some(name("reviewer-1")),
            ..ctx()
        };
        let text = "fold: P-3 has version 2. Call create_review_checkout.";
        assert_eq!(
            sent(&notice_for(&event, &claimed)),
            vec![row("reviewer-1", text, "create_review_checkout")]
        );
        // A review lead that still holds a claim it took itself gives it up first.
        assert_eq!(
            sent(&notice_for(&event, &ctx())),
            vec![row(
                "review",
                "fold: P-3 has version 2; release your review claim with release_review, then give it to a reviewer. Call start_worker.",
                "start_worker"
            )]
        );
        let unclaimed = PushContext {
            review_claim: None,
            ..ctx()
        };
        let text = "fold: P-3 has version 2. Call start_worker.";
        assert_eq!(
            sent(&notice_for(&event, &unclaimed)),
            vec![
                row("lead", text, "start_worker"),
                row("review", text, "start_worker"),
            ]
        );
        let bare = with_data(event.clone(), json!({}));
        assert_eq!(
            notice_for(&bare, &claimed)[0].text,
            "fold: P-3 has a new version. Call create_review_checkout."
        );
    }

    #[test]
    fn a_human_edit_reaches_only_the_author_and_a_review_again_the_reviewers() {
        let edited = with_data(
            event(types::PROPOSAL_HUMAN_EDITED, "human:tanim"),
            json!({"version": 4, "from_version": 3}),
        );
        assert_eq!(
            sent(&notice_for(&edited, &ctx())),
            vec![row(
                "frontend",
                "fold: a human edited P-3 as version 4; sync your workspace before you work in it. Call sync_workspace.",
                "sync_workspace"
            )]
        );
        let again = with_data(
            event(types::PROPOSAL_REVIEW_AGAIN_REQUESTED, "human:tanim"),
            json!({"version": 4}),
        );
        let claimed = PushContext {
            review_claim: Some(name("reviewer-1")),
            ..ctx()
        };
        let text =
            "fold: a human sent P-3 version 4 for review again. Call create_review_checkout.";
        assert_eq!(
            sent(&notice_for(&again, &claimed)),
            vec![row("reviewer-1", text, "create_review_checkout")]
        );
        let held = notice_for(&again, &ctx());
        assert_eq!(held[0].next_tool, "start_worker");
        assert!(held[0].text.contains("release_review"), "{}", held[0].text);
        assert!(held[0].text.len() <= MAX_NOTICE_LEN);
        let unclaimed = PushContext {
            review_claim: None,
            ..ctx()
        };
        let text = "fold: a human sent P-3 version 4 for review again. Call start_worker.";
        assert_eq!(
            sent(&notice_for(&again, &unclaimed)),
            vec![
                row("lead", text, "start_worker"),
                row("review", text, "start_worker"),
            ]
        );
        for kind in [
            types::PROPOSAL_HUMAN_EDITED,
            types::PROPOSAL_REVIEW_AGAIN_REQUESTED,
        ] {
            assert!(is_pushed(&event(kind, "human:tanim")));
            assert!(notice_for(&event(kind, "human:tanim"), &ctx()).is_empty());
        }
    }

    #[test]
    fn a_reviewer_fix_reaches_only_the_author() {
        let fixed = with_data(
            event(types::PROPOSAL_REVIEWER_FIXED, "review"),
            json!({"version": 3, "from_version": 2}),
        );
        assert!(is_pushed(&fixed));
        assert_eq!(
            sent(&notice_for(&fixed, &ctx())),
            vec![row(
                "frontend",
                "fold: the reviewer fixed P-3 as version 3; sync your workspace before you work in it. Call sync_workspace.",
                "sync_workspace"
            )]
        );
        assert!(notice_for(&with_data(fixed, json!({})), &ctx()).is_empty());
    }

    #[test]
    fn a_batch_that_waits_on_a_comment_reaches_the_review_leads() {
        let waiting = Event {
            data: json!({"batch": "rb-2", "version": 3}),
            ..event(types::REVIEW_BATCH_WAITING, "fold")
        };
        assert!(is_pushed(&waiting));
        let text = "fold: rb-2 waits for a verdict on P-3 v3: its reviewer must approve or request changes. Call get_review_batch.";
        assert!(text.len() <= MAX_NOTICE_LEN);
        assert_eq!(
            sent(&notice_for(&waiting, &ctx())),
            vec![
                row("frontend", text, "get_review_batch"),
                row("lead", text, "get_review_batch"),
                row("review", text, "get_review_batch"),
            ]
        );
        let bare = Event {
            data: json!({}),
            ..event(types::REVIEW_BATCH_WAITING, "fold")
        };
        assert!(notice_for(&bare, &ctx()).is_empty());
    }

    #[test]
    fn an_integration_reaches_the_author_and_the_reviewer() {
        let integrated = Event {
            revision: Some(crate::ids::RevisionId::from_number(5).unwrap()),
            ..event(types::INTEGRATION_SUCCEEDED, "lead")
        };
        let text = "fold: P-3 integrated as r5. Call get_inbox.";
        assert_eq!(
            sent(&notice_for(&integrated, &ctx())),
            vec![
                row("frontend", text, "get_inbox"),
                row("review", text, "get_inbox"),
            ]
        );
        // The reviewer who integrates does not hear of it; a pull has no proposal and reaches nobody.
        let own = event(types::INTEGRATION_SUCCEEDED, "review");
        assert_eq!(
            sent(&notice_for(&own, &ctx())),
            vec![row(
                "frontend",
                "fold: P-3 integrated. Call get_inbox.",
                "get_inbox"
            )]
        );
        let pull = Event {
            task: None,
            proposal: None,
            ..event(types::INTEGRATION_SUCCEEDED, "human:ana")
        };
        assert!(is_pushed(&pull));
        assert!(notice_for(&pull, &ctx()).is_empty());
        let batched = Event {
            revision: Some(crate::ids::RevisionId::from_number(6).unwrap()),
            data: json!({"batch": "rb-2"}),
            ..event(types::INTEGRATION_SUCCEEDED, "fold")
        };
        assert_eq!(
            sent(&notice_for(&batched, &ctx()))[0].1,
            "fold: P-3 integrated with batch rb-2 as r6. Call get_inbox."
        );
    }

    #[test]
    fn a_human_approval_of_a_retired_proposal_sends_nothing() {
        let event = with_data(
            event(types::REVIEW_SUBMITTED, "human:ana"),
            json!({"verdict": "approve"}),
        );
        let retired = PushContext {
            retired: true,
            ..ctx()
        };
        assert!(notice_for(&event, &retired).is_empty());
    }

    fn request_event(
        kind: crate::events::EventType,
        actor: &str,
        data: serde_json::Value,
    ) -> Event {
        Event {
            task: None,
            proposal: None,
            ..with_data(event(kind, actor), data)
        }
    }

    #[test]
    fn a_new_request_reaches_the_main_agent_and_every_area_lead() {
        let leads = PushContext {
            main_agents: [name("lead")].into_iter().collect(),
            area_leads: [name("api-lead"), name("web-lead")].into_iter().collect(),
            ..ctx()
        };
        let text = "fold: RQ-4 created. Call claim_request.";
        let by_human = request_event(
            types::REQUEST_CREATED,
            "human:ana",
            json!({"request": "RQ-4", "attachments": 0}),
        );
        assert_eq!(
            sent(&notice_for(&by_human, &leads)),
            vec![
                row("api-lead", text, "claim_request"),
                row("lead", text, "claim_request"),
                row("web-lead", text, "claim_request"),
            ]
        );
        let by_lead = request_event(
            types::REQUEST_CREATED,
            "web-lead",
            json!({"request": "RQ-4"}),
        );
        assert_eq!(
            notice_for(&by_lead, &leads).len(),
            2,
            "never the requester's own agent"
        );
        assert!(
            notice_for(&by_human, &ctx()).is_empty(),
            "without a main agent or an area lead nobody is asked"
        );
    }

    #[test]
    fn a_new_request_reaches_only_the_main_agent_once_there_is_one() {
        let with_main = PushContext {
            main_agents: [name("lead")].into_iter().collect(),
            ..ctx()
        };
        for actor in ["review", "review/batch", "human:ana"] {
            let created = request_event(types::REQUEST_CREATED, actor, json!({"request": "RQ-5"}));
            assert_eq!(
                sent(&notice_for(&created, &with_main)),
                vec![row(
                    "lead",
                    "fold: RQ-5 created. Call claim_request.",
                    "claim_request"
                )],
                "{actor}"
            );
        }
        let own = request_event(types::REQUEST_CREATED, "lead", json!({"request": "RQ-5"}));
        assert!(notice_for(&own, &with_main).is_empty());
    }

    #[test]
    fn a_drafters_version_reaches_an_agent_requester_only() {
        let event = request_event(
            types::REQUEST_DRAFT_SUBMITTED,
            "backend",
            json!({"request": "RQ-4", "version": 2, "role": "drafter"}),
        );
        assert_eq!(
            sent(&notice_for(&event, &ctx())),
            vec![row(
                "frontend",
                "fold: RQ-4 draft v2 ready. Call get_request.",
                "get_request"
            )]
        );
        let human_requester = PushContext {
            requester: None,
            ..ctx()
        };
        assert!(notice_for(&event, &human_requester).is_empty());
        let edit = request_event(
            types::REQUEST_DRAFT_SUBMITTED,
            "frontend",
            json!({"request": "RQ-4", "version": 3, "role": "requester"}),
        );
        assert!(notice_for(&edit, &ctx()).is_empty());
        let unversioned = request_event(
            types::REQUEST_DRAFT_SUBMITTED,
            "backend",
            json!({"request": "RQ-4", "role": "drafter"}),
        );
        assert_eq!(
            notice_for(&unversioned, &ctx())[0].text,
            "fold: RQ-4 draft ready. Call get_request."
        );
    }

    #[test]
    fn requested_changes_and_an_approval_reach_the_drafter() {
        let changes = request_event(
            types::REQUEST_CHANGES_REQUESTED,
            "human:ana",
            json!({"request": "RQ-4", "version": 1}),
        );
        assert_eq!(
            sent(&notice_for(&changes, &ctx())),
            vec![row(
                "backend",
                "fold: RQ-4 changes requested. Call get_request.",
                "get_request"
            )]
        );
        let approved = request_event(
            types::REQUEST_APPROVED,
            "human:ana",
            json!({"request": "RQ-4", "version": 2, "tasks": ["T-12", "T-13"]}),
        );
        assert_eq!(
            sent(&notice_for(&approved, &ctx())),
            vec![row(
                "backend",
                "fold: RQ-4 approved as T-12, T-13. Call get_ready_tasks.",
                "get_ready_tasks"
            )]
        );
        let undrafted = PushContext {
            drafter: None,
            ..ctx()
        };
        assert!(notice_for(&approved, &undrafted).is_empty());
    }

    #[test]
    fn a_long_approval_counts_its_tasks() {
        let tasks: Vec<TaskId> = (1000..1020)
            .map(|n| TaskId::from_number(n).unwrap())
            .collect();
        let request = RequestId::from_number(4).unwrap();
        assert_eq!(approved_text(request, &tasks), "RQ-4 approved as 20 tasks");
        assert_eq!(approved_text(request, &[]), "RQ-4 approved");
    }

    #[test]
    fn a_request_event_without_its_request_sends_nothing() {
        for kind in [
            types::REQUEST_CREATED,
            types::REQUEST_CHANGES_REQUESTED,
            types::REQUEST_APPROVED,
        ] {
            let event = request_event(kind, "human:ana", json!({"request": "T-4"}));
            assert!(notice_for(&event, &ctx()).is_empty(), "{kind}");
        }
        let claimed = request_event(
            types::REQUEST_CLAIMED,
            "backend",
            json!({"request": "RQ-4"}),
        );
        assert!(!is_pushed(&claimed));
        assert!(notice_for(&claimed, &ctx()).is_empty());
    }

    fn human_change(kind: crate::events::EventType, data: serde_json::Value) -> Event {
        Event {
            task: None,
            proposal: None,
            revision: Some(RevisionId::from_number(14).unwrap()),
            ..with_data(event(kind, "human:ana"), data)
        }
    }

    fn main_ctx() -> PushContext {
        PushContext {
            main_agents: [name("main")].into_iter().collect(),
            ..PushContext::default()
        }
    }

    #[test]
    fn a_recorded_human_change_reaches_the_main_agent_and_the_leads_of_touched_tasks() {
        let event = human_change(
            types::HUMAN_CHANGE_RECORDED,
            json!({
                "files": ["README.md", "src/lib.rs"], "file_count": 2,
                "tasks": ["T-4"], "workspaces": ["ws-7"], "leads": ["backend", "main"],
            }),
        );
        assert!(is_pushed(&event));
        assert_eq!(
            sent(&notice_for(&event, &main_ctx())),
            vec![
                row(
                    "main",
                    "fold: human:ana recorded r14 from the project folder: README.md, src/lib.rs; touches T-4 (ws-7), whose leads were told; tell the user. Call compare_revisions.",
                    "compare_revisions"
                ),
                row(
                    "backend",
                    "fold: human:ana changed README.md, src/lib.rs in main as r14, which T-4 (ws-7) also changes. Call get_task.",
                    "get_task"
                ),
            ]
        );
        let alone = human_change(
            types::HUMAN_CHANGE_RECORDED,
            json!({"files": ["a.txt"], "file_count": 1, "tasks": [], "workspaces": [], "leads": []}),
        );
        assert_eq!(
            sent(&notice_for(&alone, &main_ctx())),
            vec![row(
                "main",
                "fold: human:ana recorded r14 from the project folder: a.txt; touches no open task; tell the user. Call compare_revisions.",
                "compare_revisions"
            )]
        );
        let unled = human_change(
            types::HUMAN_CHANGE_RECORDED,
            json!({"files": ["a.txt"], "file_count": 1, "tasks": ["T-4"], "workspaces": ["ws-7"], "leads": []}),
        );
        assert_eq!(
            sent(&notice_for(&unled, &main_ctx()))[0].1,
            "fold: human:ana recorded r14 from the project folder: a.txt; touches T-4 (ws-7); tell the user. Call compare_revisions."
        );
        let mut no_revision = alone;
        no_revision.revision = None;
        assert!(notice_for(&no_revision, &main_ctx()).is_empty());
    }

    #[test]
    fn a_resolved_human_change_reaches_the_main_agent_that_did_not_resolve_it_and_the_leads() {
        let data = json!({
            "human": "human:ana", "commit": "c1", "integration": "i-3",
            "files": ["README.md"], "file_count": 1,
            "tasks": ["T-4"], "workspaces": ["ws-7"], "leads": ["backend", "main"],
        });
        let mut event = human_change(types::HUMAN_CHANGE_RESOLVED, data.clone());
        event.actor = EventActor::parse("main").unwrap();
        assert!(is_pushed(&event));
        let to_lead = row(
            "backend",
            "fold: human:ana changed README.md in main as r14, which T-4 (ws-7) also changes. Call get_task.",
            "get_task",
        );
        assert_eq!(
            sent(&notice_for(&event, &main_ctx())),
            vec![to_lead.clone()]
        );
        let to_main = row(
            "main",
            "fold: human:ana's waiting change is resolved: main holds it as r14; tell the user. Call compare_revisions.",
            "compare_revisions",
        );
        let mut by_review = human_change(types::HUMAN_CHANGE_RESOLVED, data);
        by_review.actor = EventActor::parse("review").unwrap();
        assert_eq!(
            sent(&notice_for(&by_review, &main_ctx())),
            vec![to_main.clone(), to_lead]
        );
        let untouched = human_change(
            types::HUMAN_CHANGE_RESOLVED,
            json!({"human": "human:ana", "files": ["a.txt"], "file_count": 1, "tasks": [], "workspaces": [], "leads": []}),
        );
        assert_eq!(sent(&notice_for(&untouched, &main_ctx())), vec![to_main]);
        let mut no_revision = untouched;
        no_revision.revision = None;
        assert!(notice_for(&no_revision, &main_ctx()).is_empty());
    }

    #[test]
    fn a_withdrawn_human_change_tells_the_main_agent_to_drop_its_merge() {
        let mut event = human_change(
            types::HUMAN_CHANGE_WITHDRAWN,
            json!({
                "human": "human:ana", "commit": null, "conflicted": ["a.txt"], "conflicted_count": 1,
                "workspace": "ws-9",
            }),
        );
        event.revision = None;
        event.actor = EventActor::parse("human:bo").unwrap();
        assert!(is_pushed(&event));
        assert_eq!(
            sent(&notice_for(&event, &main_ctx())),
            vec![row(
                "main",
                "fold: human:ana took back the waiting change; nothing is left to merge; abandon ws-9, which holds its merge, and tell the user. Call abandon_workspace.",
                "abandon_workspace"
            )]
        );
        event.data["workspace"] = json!(null);
        assert_eq!(
            sent(&notice_for(&event, &main_ctx())),
            vec![row(
                "main",
                "fold: human:ana took back the waiting change; nothing is left to merge; tell the user. Call get_repository_state.",
                "get_repository_state"
            )]
        );
    }

    #[test]
    fn a_conflicted_human_change_asks_the_main_agent_to_prepare_the_merge() {
        let event = human_change(
            types::HUMAN_CHANGE_CONFLICTED,
            json!({
                "files": ["a.txt", "b.txt"], "file_count": 2,
                "conflicted": ["a.txt"], "conflicted_count": 1,
                "tasks": [], "workspaces": [], "leads": [],
            }),
        );
        assert!(is_pushed(&event));
        assert_eq!(
            sent(&notice_for(&event, &main_ctx())),
            vec![row(
                "main",
                "fold: human:ana's change conflicts with main in a.txt; main did not move; explain it and prepare the merge. Call open_main_workspace.",
                "open_main_workspace"
            )]
        );
    }

    #[test]
    fn a_long_human_change_counts_its_files_and_tasks_and_stays_one_short_line() {
        let many = |prefix: &str, n: u32| -> Vec<String> {
            (1..=n).map(|i| format!("{prefix}{i}")).collect()
        };
        let human = format!("human:{}", "a".repeat(32));
        let recorded = Event {
            actor: crate::events::EventActor::parse(&human).unwrap(),
            ..human_change(
                types::HUMAN_CHANGE_RECORDED,
                json!({
                    "files": many("docs/a-rather-long-file-name-", 40), "file_count": 400,
                    "tasks": many("T-", 30), "workspaces": many("ws-", 30), "leads": ["backend"],
                }),
            )
        };
        let conflicted = Event {
            event_type: types::HUMAN_CHANGE_CONFLICTED,
            data: json!({
                "files": many("f", 3), "file_count": 3,
                "conflicted": many("docs/a-rather-long-file-name-", 9), "conflicted_count": 9,
                "tasks": many("T-", 30), "workspaces": many("ws-", 30), "leads": [],
            }),
            ..recorded.clone()
        };
        let notices: Vec<Notice> = [recorded, conflicted]
            .iter()
            .flat_map(|event| notice_for(event, &main_ctx()))
            .collect();
        assert_eq!(notices.len(), 3);
        for notice in &notices {
            assert!(notice.text.len() <= MAX_NOTICE_LEN, "{}", notice.text);
            assert!(!notice.text.contains('\n'));
        }
        assert!(notices[0].text.contains(": 400 files; touches 30 tasks"));
        assert!(notices[2].text.contains("in 9 files"));
    }
}
