use std::collections::BTreeSet;

use crate::agents::AgentName;
use crate::events::{Event, EventActor, types};
use crate::ids::MessageId;
use crate::status::TaskStatus;

/// The longest notice text. A notice names ids and a tool, so it never comes near it.
pub const MAX_NOTICE_LEN: usize = 200;

/// The facts about an event's task and proposal that decide who gets a notice. The daemon loads them from the read pool after the event commits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PushContext {
    /// Every agent tagged `integrator`.
    pub integrators: BTreeSet<AgentName>,
    /// The agent of the worker that authored the event's proposal.
    pub author: Option<AgentName>,
    /// The agent holding the unreleased review claim on the event's proposal.
    pub review_claim: Option<AgentName>,
    /// The agent holding the unreleased implementation claim on the event's task.
    pub implementation_claim: Option<AgentName>,
    /// The agents with a worker among the recipients of the event's message.
    pub recipients: BTreeSet<AgentName>,
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
    ]
    .contains(&event.event_type)
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
        let agents = ctx
            .integrators
            .iter()
            .filter(|agent| ctx.author.as_ref() != Some(*agent))
            .cloned()
            .collect();
        let text = match event.task {
            Some(task) => format!("{proposal} opened for {task}"),
            None => format!("{proposal} opened"),
        };
        return Some((agents, text, "claim_review"));
    }
    if kind == types::REVIEW_SUBMITTED {
        let proposal = event.proposal?;
        return match verdict(event)? {
            "request_changes" => Some((
                ctx.author.iter().cloned().collect(),
                format!("{proposal} changes requested"),
                "get_review_comments",
            )),
            "approve" if is_human(&event.actor) => {
                let agents = match &ctx.review_claim {
                    Some(holder) => vec![holder.clone()],
                    None => ctx.integrators.iter().cloned().collect(),
                };
                Some((
                    agents,
                    format!("{proposal} approved by a human"),
                    "integrate_proposal",
                ))
            }
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
        return Some((
            ctx.implementation_claim.iter().cloned().collect(),
            format!("{task} is ready"),
            "create_workspace",
        ));
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

/// `review.submitted` names its verdict in `data.verdict`, with the values of `reviews.verdict`.
fn verdict(event: &Event) -> Option<&str> {
    event
        .data
        .get("verdict")
        .and_then(serde_json::Value::as_str)
}

fn is_human(actor: &EventActor) -> bool {
    actor.as_str().starts_with("human:")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ids::{ProposalId, TaskId};
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
            integrators: [name("lead"), name("review"), name("frontend")]
                .into_iter()
                .collect(),
            author: Some(name("frontend")),
            review_claim: Some(name("review")),
            implementation_claim: Some(name("backend")),
            recipients: [name("backend"), name("docs")].into_iter().collect(),
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
    fn an_opened_proposal_reaches_every_integrator_but_the_authors_agent() {
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
        let unclaimed = PushContext {
            review_claim: None,
            ..ctx()
        };
        assert_eq!(
            sent(&notice_for(&event, &unclaimed)),
            vec![
                row("frontend", text, "integrate_proposal"),
                row("lead", text, "integrate_proposal"),
                row("review", text, "integrate_proposal"),
            ]
        );
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
            data: json!({"to": "todo", "verdict": "request_changes", "message": format!("m-{}", u32::MAX)}),
            ..event(kind, "fold")
        };
        for kind in [
            types::PROPOSAL_OPENED,
            types::REVIEW_SUBMITTED,
            types::INTEGRATION_CONFLICTED,
            types::TASK_STATUS_CHANGED,
            types::MESSAGE_SENT,
        ] {
            let notices = notice_for(&large(kind), &ctx());
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
}
