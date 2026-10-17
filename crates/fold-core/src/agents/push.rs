use std::collections::BTreeSet;

use crate::agents::{AgentName, AreaName};
use crate::events::{Event, types};
use crate::ids::{MessageId, RequestId, TaskId};
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
    /// For a task that turned ready, the areas whose label it carries, led or not.
    pub areas: BTreeSet<AreaName>,
    /// Every agent tagged `main`.
    pub main_agents: BTreeSet<AgentName>,
    /// No agent can take the event's task now: it is a human task or on hold, or, for `task.hold_cleared`, it is not an unclaimed Todo task.
    pub held_from_agents: bool,
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
    let Some((agents, text, next_tool)) = rule(event, ctx) else {
        return Vec::new();
    };
    let own = event.actor.agent();
    let mut seen = BTreeSet::new();
    agents
        .into_iter()
        .filter(|agent| own != Some(agent.as_str()) && seen.insert(agent.clone()))
        .map(|agent| Notice {
            agent,
            text: format!("fold: {text}. Call {next_tool}."),
            next_tool,
        })
        .collect()
}

fn rule(event: &Event, ctx: &PushContext) -> Option<(Vec<AgentName>, String, &'static str)> {
    let kind = event.event_type;
    if kind == types::PROPOSAL_OPENED {
        let proposal = event.proposal?;
        let text = match event.task {
            Some(task) => format!("{proposal} opened for {task}"),
            None => format!("{proposal} opened"),
        };
        return Some((reviewers(ctx), text, "claim_review"));
    }
    if kind == types::PROPOSAL_VERSION_ADDED {
        let proposal = event.proposal?;
        let text = match event
            .data
            .get("version")
            .and_then(serde_json::Value::as_u64)
        {
            Some(version) => format!("{proposal} has version {version}"),
            None => format!("{proposal} has a new version"),
        };
        return Some((reviewers(ctx), text, "create_review_checkout"));
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
    if kind == types::PROPOSAL_REVIEW_AGAIN_REQUESTED {
        let proposal = event.proposal?;
        let version = event.data.get("version")?.as_u64()?;
        return Some((
            reviewers(ctx),
            format!("a human sent {proposal} version {version} for review again"),
            "create_review_checkout",
        ));
    }
    if kind == types::INTEGRATION_SUCCEEDED {
        let proposal = event.proposal?;
        let agents = ctx.author.iter().chain(&ctx.reviewer).cloned().collect();
        let text = match event.revision {
            Some(revision) => format!("{proposal} integrated as {revision}"),
            None => format!("{proposal} integrated"),
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

/// A ready task nobody claimed goes to whoever starts its builder. The lead of its area starts a task worker; while the area has no lead, the main agent starts one; a task in no area waits for the main agent to give it an area label. Both an approved request card and a dependent of an integrated task turn ready through `task.status_changed`, so this one rule routes them.
fn ready_route(task: TaskId, ctx: &PushContext) -> (Vec<AgentName>, String, &'static str) {
    let main = || ctx.main_agents.iter().cloned().collect();
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

/// Every review lead but the author's agent, then the holder of the review claim.
fn reviewers(ctx: &PushContext) -> Vec<AgentName> {
    ctx.review_leads
        .iter()
        .filter(|agent| ctx.author.as_ref() != Some(*agent))
        .chain(&ctx.review_claim)
        .cloned()
        .collect()
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
    use crate::ids::ProposalId;
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
                    "fold: P-3 opened for T-12. Call claim_review.",
                    "claim_review"
                ),
                row(
                    "review",
                    "fold: P-3 opened for T-12. Call claim_review.",
                    "claim_review"
                ),
            ]
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
            "fold: P-3 opened. Call claim_review."
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
    fn a_new_version_reaches_the_claim_holder_and_the_review_leads() {
        let event = with_data(
            event(types::PROPOSAL_VERSION_ADDED, "frontend/ui"),
            json!({"version": 2, "changeset": "C-4"}),
        );
        let text = "fold: P-3 has version 2. Call create_review_checkout.";
        assert_eq!(
            sent(&notice_for(&event, &ctx())),
            vec![
                row("lead", text, "create_review_checkout"),
                row("review", text, "create_review_checkout"),
            ]
        );
        let bare = with_data(event.clone(), json!({}));
        assert_eq!(
            notice_for(&bare, &ctx())[0].text,
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
        let text =
            "fold: a human sent P-3 version 4 for review again. Call create_review_checkout.";
        assert_eq!(
            sent(&notice_for(&again, &ctx())),
            vec![
                row("lead", text, "create_review_checkout"),
                row("review", text, "create_review_checkout"),
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
}
