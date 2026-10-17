use std::collections::BTreeSet;

use serde_json::json;

use super::{ChecklistName, HoldReason, LabelName, StepText, TaskDescription, TaskKind, TaskTitle};
use crate::reviews::human_review_reason;
use crate::{FoldError, HumanName, TaskId, codes};

/// A single command bounds the work inside the project's writer transaction.
pub const MAX_PLAN_TASKS: usize = 100;

/// An existing task id or a key of another task in the same plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanRef {
    Existing(TaskId),
    Key(String),
}

impl PlanRef {
    pub fn parse(value: &str) -> Self {
        match value.parse::<TaskId>() {
            Ok(id) => Self::Existing(id),
            Err(_) => Self::Key(value.to_owned()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlannedReview {
    pub reviewer: Option<HumanName>,
    pub reason: String,
}

/// A plain checklist step a plan creates with its task. Both fields are checked by [`check_task_plan`] with the rules of `add_checklist_item`.
#[derive(Debug, Clone)]
pub struct PlannedChecklistItem {
    pub text: String,
    /// The checklist name; chosen by the same rule as `add_checklist_item` when `None`: `Subtasks` for a task without a checklist, the only checklist when there is one, a refusal when there are several.
    pub checklist: Option<String>,
}

/// A task and all inputs that must exist before workers can see it.
#[derive(Debug, Clone)]
pub struct PlannedTask {
    pub key: String,
    pub title: TaskTitle,
    pub description: TaskDescription,
    pub labels: Vec<LabelName>,
    pub kind: TaskKind,
    pub needs: Vec<PlanRef>,
    pub parent: Option<PlanRef>,
    pub human_review: Option<PlannedReview>,
    /// The reason of a blocker the task carries from its creation, so no agent sees it ready before the blocker exists.
    pub blocker: Option<String>,
    /// Plain steps created with the task, in order, before any worker sees it.
    pub checklist: Vec<PlannedChecklistItem>,
}

/// Checks plan-local names and review reasons. Edges use the same DAG rules as individual task commands after references resolve.
pub fn check_task_plan(tasks: &[PlannedTask]) -> Result<(), FoldError> {
    if tasks.is_empty() || tasks.len() > MAX_PLAN_TASKS {
        return Err(invalid("task_count", 0));
    }
    let mut keys = BTreeSet::new();
    for (position, task) in tasks.iter().enumerate() {
        if task.key.is_empty()
            || task.key.len() > 32
            || !task
                .key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || task.key.parse::<TaskId>().is_ok()
        {
            return Err(invalid("invalid_key", position + 1));
        }
        if !keys.insert(task.key.as_str()) {
            return Err(invalid("duplicate_key", position + 1));
        }
        if let Some(review) = &task.human_review {
            human_review_reason(&review.reason)?;
        }
        if let Some(reason) = &task.blocker {
            HoldReason::parse(reason).map_err(|error| {
                FoldError::from(error)
                    .with_details([("field", json!("blocker.reason")), ("position", json!(position + 1))])
                    .with_hint("say in a line or two why the task must wait for a person, then call create_task_plan again")
            })?;
        }
        for (index, item) in task.checklist.iter().enumerate() {
            let refuse = |error: super::TaskValueError, field: &'static str| {
                FoldError::from(error)
                    .with_details([
                        ("field", json!(field)),
                        ("position", json!(position + 1)),
                        ("item", json!(index + 1)),
                    ])
                    .with_hint("give each checklist item a text of one line and an optional checklist name of one line, then call create_task_plan again")
            };
            StepText::parse(&item.text).map_err(|error| refuse(error, "checklist.text"))?;
            if let Some(name) = &item.checklist {
                ChecklistName::parse(name).map_err(|error| refuse(error, "checklist.checklist"))?;
            }
        }
    }
    for (position, task) in tasks.iter().enumerate() {
        for reference in task.needs.iter().chain(task.parent.iter()) {
            if let PlanRef::Key(key) = reference
                && !keys.contains(key.as_str())
            {
                return Err(invalid("unknown_key", position + 1));
            }
        }
    }
    Ok(())
}

fn invalid(reason: &'static str, position: usize) -> FoldError {
    FoldError::new(codes::INVALID_REQUEST, "the task plan is invalid")
        .with_details([("reason", json!(reason)), ("position", json!(position))])
        .with_hint("give 1 to 100 tasks with unique keys of 1 to 32 letters, digits, - or _; references name an existing task id or a key in this plan, then call create_task_plan again")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(key: &str) -> PlannedTask {
        PlannedTask {
            key: key.to_owned(),
            title: TaskTitle::parse("Build").unwrap(),
            description: TaskDescription::new("Acceptance: check the result"),
            labels: Vec::new(),
            kind: TaskKind::Proposal,
            needs: Vec::new(),
            parent: None,
            human_review: None,
            blocker: None,
            checklist: Vec::new(),
        }
    }

    #[test]
    fn references_distinguish_existing_ids_and_plan_local_keys() {
        let mut child = task("child");
        child.parent = Some(PlanRef::parse("root"));
        child.needs = vec![PlanRef::parse("T-12"), PlanRef::parse("base")];
        child.human_review = Some(PlannedReview {
            reviewer: None,
            reason: " Check UI ".to_owned(),
        });
        child.blocker = Some(" Wait for the Meta review ".to_owned());
        assert!(check_task_plan(&[child.clone(), task("root"), task("base")]).is_ok());
        assert_eq!(
            child.needs[0],
            PlanRef::Existing(TaskId::from_number(12).unwrap())
        );
        assert_eq!(child.parent, Some(PlanRef::Key("root".to_owned())));
        assert!(
            check_task_plan(
                &(0..MAX_PLAN_TASKS)
                    .map(|n| task(&format!("t{n}")))
                    .collect::<Vec<_>>()
            )
            .is_ok()
        );
    }

    #[test]
    fn invalid_names_references_counts_and_review_reasons_are_refused() {
        for tasks in [
            Vec::new(),
            vec![task("x"); MAX_PLAN_TASKS + 1],
            vec![task("x"), task("x")],
        ] {
            assert_eq!(
                check_task_plan(&tasks).unwrap_err().code(),
                codes::INVALID_REQUEST
            );
        }
        for key in ["", "a b", "é", "T-12", &"a".repeat(33)] {
            assert_eq!(
                check_task_plan(&[task(key)]).unwrap_err().code(),
                codes::INVALID_REQUEST
            );
        }
        let mut bad = task("x");
        bad.needs = vec![PlanRef::parse("missing")];
        assert!(check_task_plan(&[bad.clone()]).is_err());
        bad.needs.clear();
        bad.parent = Some(PlanRef::parse("missing"));
        assert!(check_task_plan(&[bad.clone()]).is_err());
        bad.parent = None;
        for reason in [" ".to_owned(), "a".repeat(501)] {
            bad.human_review = Some(PlannedReview {
                reviewer: None,
                reason,
            });
            assert!(check_task_plan(&[bad.clone()]).is_err());
        }
        assert_eq!(human_review_reason(" Check ").unwrap(), "Check");
        bad.human_review = None;
        for reason in [" ".to_owned(), "a".repeat(501)] {
            bad.blocker = Some(reason);
            let error = check_task_plan(&[task("ok"), bad.clone()]).unwrap_err();
            assert_eq!(error.code(), codes::INVALID_HOLD_REASON);
            assert_eq!(error.details().unwrap()["position"], 2);
        }
    }

    #[test]
    fn an_invalid_checklist_item_refuses_the_plan_with_its_position() {
        let item = |text: &str, checklist: Option<&str>| PlannedChecklistItem {
            text: text.to_owned(),
            checklist: checklist.map(str::to_owned),
        };
        let mut good = task("ok");
        good.checklist = vec![item("Write tests", None), item("Ship", Some("QA"))];
        assert!(check_task_plan(&[good.clone()]).is_ok());
        for (bad_item, field, code) in [
            (item(" ", None), "checklist.text", codes::INVALID_STEP_TEXT),
            (
                item("two\nlines", None),
                "checklist.text",
                codes::INVALID_STEP_TEXT,
            ),
            (
                item("Fine", Some("")),
                "checklist.checklist",
                codes::INVALID_CHECKLIST_NAME,
            ),
        ] {
            let mut bad = task("bad");
            bad.checklist = vec![item("Fine", None), bad_item];
            let error = check_task_plan(&[good.clone(), bad]).unwrap_err();
            assert_eq!(error.code(), code);
            let details = error.details().unwrap();
            assert_eq!(details["position"], 2);
            assert_eq!(details["item"], 2);
            assert_eq!(details["field"], field);
        }
    }
}
