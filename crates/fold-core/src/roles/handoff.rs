use std::fmt::Write as _;

use super::Role;
use crate::agents::{AgentName, AreaName};
use crate::error::{FoldError, codes};
use crate::ids::{ProposalId, TaskId};
use crate::status::TaskStatus;
use crate::task::LabelName;
use crate::text::shorten;

/// How many turns of a lead, counted from the turn that asks for its hand-off note, may end without a note before fold starts the lead fresh with the fold summary alone.
pub const HANDOFF_TURNS: u64 = 3;

/// The longest hand-off note fold keeps, in characters. The note goes into the first turn of the fresh session, so it stays short next to the context it replaces.
pub const MAX_HANDOFF_CHARS: usize = 8000;

/// How many open tasks and how many open proposals the fold summary names before it only counts the rest.
pub const SUMMARY_ITEMS: usize = 20;

/// The longest task or proposal title the fold summary repeats, in characters.
const SUMMARY_TITLE: usize = 100;

/// The turn fold sends a lead whose context passed its role's `handoff_at`.
pub const HANDOFF_REQUEST: &str = "fold: your context is large, so fold will start you again with a fresh context. Call write_handoff now with a short hand-off note for your next session: what you are doing, your open decisions and what comes next. Tasks, proposals, reviews and messages stay in fold, so do not copy them. Then end your turn.";

/// Whether a context of `context_tokens` has reached `percent` of a window of `context_window` tokens. An unknown window of 0 never does.
pub fn handoff_due(context_tokens: u64, context_window: u64, percent: u64) -> bool {
    context_window > 0
        && u128::from(context_tokens) * 100 >= u128::from(context_window) * u128::from(percent)
}

/// Whether a lead that fold asked for a note after its `asked_turns`-th turn has used up its turns at `turns`, without a note.
pub fn handoff_overdue(asked_turns: u64, turns: u64) -> bool {
    turns.saturating_sub(asked_turns) >= HANDOFF_TURNS
}

/// The note without its surrounding white space. An empty note or one longer than [`MAX_HANDOFF_CHARS`] is refused with `invalid_request`; the error names the length, never the text.
pub fn check_handoff(body: &str) -> Result<&str, FoldError> {
    let body = body.trim();
    if body.is_empty() {
        return Err(
            FoldError::new(codes::INVALID_REQUEST, "a hand-off note cannot be empty")
                .with_hint("write what you are doing, your open decisions and what comes next"),
        );
    }
    let length = body.chars().count();
    if length > MAX_HANDOFF_CHARS {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!("a hand-off note has at most {MAX_HANDOFF_CHARS} characters, not {length}"),
        )
        .with_details([
            ("length", serde_json::json!(length)),
            ("max", serde_json::json!(MAX_HANDOFF_CHARS)),
        ])
        .with_hint("shorten the note; tasks, proposals and messages stay in fold"));
    }
    Ok(body)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryTask {
    pub id: TaskId,
    pub status: TaskStatus,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryProposal {
    pub id: ProposalId,
    pub task: TaskId,
    pub title: String,
}

/// What the first turn of a lead's fresh session holds: its hand-off note, if it wrote one, and the fold state it works on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshFacts<'a> {
    pub role: Role,
    pub name: &'a AgentName,
    pub area: Option<(&'a AreaName, &'a LabelName)>,
    pub note: Option<&'a str>,
    /// The open tasks of the lead's area, every open task for the review lead, at most [`SUMMARY_ITEMS`].
    pub tasks: &'a [SummaryTask],
    /// How many open tasks there are in all.
    pub task_count: u64,
    /// The open proposals the lead works on, at most [`SUMMARY_ITEMS`].
    pub proposals: &'a [SummaryProposal],
    pub proposal_count: u64,
    pub unread: u64,
}

/// The first user turn of a lead's fresh session: who it is, its own hand-off note, then a compact fold summary of its open tasks, open proposals and unread messages.
pub fn refresh_turn(facts: &RefreshFacts<'_>) -> String {
    let mut text = String::new();
    let name = facts.name;
    let who = match (facts.role, facts.area) {
        (Role::AreaLead, Some((area, label))) => {
            format!("the area lead of area {area}, which owns the tasks labelled {label}")
        }
        (Role::ReviewLead, _) => "the review lead".to_owned(),
        (role, _) => format!("a worker of role {role}"),
    };
    let _ = writeln!(
        text,
        "fold started you again as {name}, {who}, with a fresh context. The state below comes from fold; read more with the fold tools."
    );
    match facts.note.map(str::trim).filter(|note| !note.is_empty()) {
        Some(note) => {
            let _ = write!(
                text,
                "\nYour hand-off note from your last session:\n{note}\n"
            );
        }
        None => text.push_str("\nYour last session wrote no hand-off note.\n"),
    }
    let tasks_heading = match facts.role {
        Role::AreaLead => "Open tasks of your area",
        _ => "Open tasks",
    };
    list(
        &mut text,
        tasks_heading,
        facts.task_count,
        facts.tasks.iter().map(|task| {
            format!(
                "{} {} {}",
                task.id,
                task.status,
                shorten(&task.title, SUMMARY_TITLE)
            )
        }),
    );
    list(
        &mut text,
        "Open proposals",
        facts.proposal_count,
        facts.proposals.iter().map(|proposal| {
            format!(
                "{} for {} {}",
                proposal.id,
                proposal.task,
                shorten(&proposal.title, SUMMARY_TITLE)
            )
        }),
    );
    let _ = write!(
        text,
        "\nUnread messages: {}.{}",
        facts.unread,
        if facts.unread > 0 {
            " Call get_inbox."
        } else {
            ""
        }
    );
    text
}

fn list(text: &mut String, heading: &str, count: u64, lines: impl Iterator<Item = String>) {
    if count == 0 {
        let _ = write!(text, "\n{heading}: none.\n");
        return;
    }
    let _ = write!(text, "\n{heading} ({count}):\n");
    let mut shown: u64 = 0;
    for line in lines {
        let _ = writeln!(text, "- {line}");
        shown += 1;
    }
    let more = count.saturating_sub(shown);
    if more > 0 {
        let _ = writeln!(text, "- and {more} more");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hand_off_is_due_at_the_share_of_the_window() {
        assert!(!handoff_due(599_999, 1_000_000, 60));
        assert!(handoff_due(600_000, 1_000_000, 60));
        assert!(handoff_due(700_000, 1_000_000, 60));
        assert!(!handoff_due(25_310, 1_000_000, 60));
        assert!(!handoff_due(u64::MAX, 0, 60));
        assert!(handoff_due(u64::MAX, u64::MAX, 99));
    }

    #[test]
    fn a_lead_without_a_note_is_overdue_after_its_turns() {
        assert!(!handoff_overdue(4, 4));
        assert!(!handoff_overdue(4, 6));
        assert!(handoff_overdue(4, 7));
        assert!(!handoff_overdue(9, 3));
    }

    #[test]
    fn a_note_is_trimmed_and_limited_by_its_length() {
        assert_eq!(check_handoff("  next: T-3  ").unwrap(), "next: T-3");
        assert_eq!(
            check_handoff(" \n ").unwrap_err().code(),
            codes::INVALID_REQUEST
        );
        let long = "é".repeat(MAX_HANDOFF_CHARS + 1);
        let error = check_handoff(&long).unwrap_err();
        assert_eq!(error.code(), codes::INVALID_REQUEST);
        assert!(!error.message().contains('é'));
        assert_eq!(
            error.details().unwrap()["length"],
            serde_json::json!(MAX_HANDOFF_CHARS + 1)
        );
        assert!(check_handoff(&"é".repeat(MAX_HANDOFF_CHARS)).is_ok());
    }

    #[test]
    fn the_refresh_turn_holds_the_note_and_the_fold_summary() {
        let name = AgentName::parse("web-lead").unwrap();
        let area = AreaName::parse("web").unwrap();
        let label = LabelName::parse("ui").unwrap();
        let tasks = vec![SummaryTask {
            id: TaskId::from_number(3).unwrap(),
            status: TaskStatus::InProgress,
            title: format!("Draw {}", "a".repeat(200)),
        }];
        let proposals = vec![SummaryProposal {
            id: ProposalId::from_number(2).unwrap(),
            task: TaskId::from_number(3).unwrap(),
            title: "Draw the page".to_owned(),
        }];
        let turn = refresh_turn(&RefreshFacts {
            role: Role::AreaLead,
            name: &name,
            area: Some((&area, &label)),
            note: Some("  Next: split T-3.  "),
            tasks: &tasks,
            task_count: 4,
            proposals: &proposals,
            proposal_count: 1,
            unread: 2,
        });
        assert!(turn.starts_with("fold started you again as web-lead, the area lead of area web"));
        assert!(turn.contains("hand-off note from your last session:\nNext: split T-3.\n"));
        assert!(turn.contains("Open tasks of your area (4):\n- T-3 in_progress Draw "));
        assert!(turn.contains("…\n- and 3 more\n"));
        assert!(turn.contains("Open proposals (1):\n- P-2 for T-3 Draw the page\n"));
        assert!(turn.ends_with("Unread messages: 2. Call get_inbox."));

        let review = AgentName::parse("review-lead").unwrap();
        let bare = refresh_turn(&RefreshFacts {
            role: Role::ReviewLead,
            name: &review,
            area: None,
            note: None,
            tasks: &[],
            task_count: 0,
            proposals: &[],
            proposal_count: 0,
            unread: 0,
        });
        assert!(bare.contains("as review-lead, the review lead,"));
        assert!(bare.contains("wrote no hand-off note"));
        assert!(bare.contains("Open tasks: none."));
        assert!(bare.contains("Open proposals: none."));
        assert!(bare.ends_with("Unread messages: 0."));
        let other = refresh_turn(&RefreshFacts {
            role: Role::AreaLead,
            note: Some(" "),
            ..RefreshFacts {
                role: Role::TaskCoder,
                name: &review,
                area: None,
                note: None,
                tasks: &[],
                task_count: 0,
                proposals: &[],
                proposal_count: 0,
                unread: 0,
            }
        });
        assert!(other.contains("a worker of role area_lead"));
        assert!(other.contains("wrote no hand-off note"));
    }
}
