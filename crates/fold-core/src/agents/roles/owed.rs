use serde_json::json;

use super::{Role, WorkerLimits, check_limits};
use crate::agents::{AgentName, AreaName};
use crate::error::{FoldError, codes};
use crate::ids::{ProposalId, TaskId};
use crate::time::Timestamp;

/// The `reason` detail of a start that a `worker_limit_reached` refuses because the place it would take goes to a resume fold owes.
pub const OWED_RESUME_WAITS: &str = "owed_resume_waits";

/// A resume that fold owes a stopped or failed worker of a limited role, such as requested changes, a conflict, failed checks or a send-back for a task worker, or a new version for a reviewer that holds the review claim. fold serves it once the limits let the worker start again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwedResume {
    pub agent: AgentName,
    /// The worker's area with the running workers of its role in it; `None` for a worker without an area.
    pub area: Option<(AreaName, u64)>,
    /// When the oldest work that fold owes the worker came.
    pub since: Timestamp,
    /// The task and the proposal of that work, for the refusal's text.
    pub task: Option<TaskId>,
    pub proposal: Option<ProposalId>,
}

impl OwedResume {
    /// The worker's area has room for one more worker of its role, so the resume can take a place of the role once one is free.
    fn area_room(&self, limits: WorkerLimits) -> bool {
        match (&self.area, limits.max_per_area) {
            (Some((_, running)), Some(max)) => *running < max,
            _ => true,
        }
    }

    fn work(&self) -> String {
        match (self.proposal, self.task) {
            (Some(proposal), Some(task)) => format!("{proposal} of {task}"),
            (Some(proposal), None) => proposal.to_string(),
            (None, Some(task)) => task.to_string(),
            (None, None) => "its work".to_owned(),
        }
    }
}

/// The owed resumes of one role that a start of `starting` must let go first, oldest first: every other owed resume whose area has room, or for a worker that is owed a resume itself only those that became owed before its own. `starting` is `None` for a new agent. Two resumes owed at the same time go in name order.
pub fn owed_ahead<'a>(
    limits: WorkerLimits,
    owed: &'a [OwedResume],
    starting: Option<&AgentName>,
) -> Vec<&'a OwedResume> {
    let own = starting.and_then(|agent| owed.iter().find(|owed| owed.agent == *agent));
    let mut ahead: Vec<&OwedResume> = owed
        .iter()
        .filter(|owed| Some(&owed.agent) != starting)
        .filter(|owed| own.is_none_or(|own| (owed.since, &owed.agent) < (own.since, &own.agent)))
        .filter(|owed| owed.area_room(limits))
        .collect();
    ahead.sort_by(|a, b| (a.since, &a.agent).cmp(&(b.since, &b.agent)));
    ahead
}

/// Refuses a start of a worker of `role` that would take a place an owed resume of the role waits for, so fold serves its owed resumes, in the order they became owed, before any other start. `running` and `in_area` are those of [`check_limits`], `owed` the owed resumes of `role`, and `starting` the worker that starts again, `None` for a new agent. The refusal is `worker_limit_reached` with the reason [`OWED_RESUME_WAITS`]; its hint names `tool`, the call to make again. A person's own start never comes here.
pub fn check_owed_first(
    limits: WorkerLimits,
    role: Role,
    running: u64,
    in_area: Option<(&AreaName, u64)>,
    owed: &[OwedResume],
    starting: Option<&AgentName>,
    tool: &str,
) -> Result<(), FoldError> {
    let Some(max) = limits.of(role) else {
        return Ok(());
    };
    let ahead = owed_ahead(limits, owed, starting);
    let free = max.saturating_sub(running);
    let first = match ahead.first() {
        Some(first) if ahead.len() as u64 >= free => Some(*first),
        _ => None,
    };
    let first = first.or_else(|| {
        let (area, count) = in_area?;
        let free = limits.max_per_area?.saturating_sub(count);
        let mut in_same = ahead
            .iter()
            .filter(|owed| owed.area.as_ref().is_some_and(|(name, _)| name == area));
        let first = in_same.next()?;
        (1 + in_same.count() as u64 >= free).then_some(*first)
    });
    let Some(first) = first else {
        return Ok(());
    };
    let agent = &first.agent;
    let work = first.work();
    let mut details = vec![
        ("reason", json!(OWED_RESUME_WAITS)),
        ("role", json!(role.as_str())),
        ("agent", json!(agent.as_str())),
        ("max", json!(max)),
    ];
    if let Some(task) = first.task {
        details.push(("task", json!(task.to_string())));
    }
    if let Some(proposal) = first.proposal {
        details.push(("proposal", json!(proposal.to_string())));
    }
    Err(FoldError::new(
        codes::WORKER_LIMIT_REACHED,
        format!("fold owes @{agent} a resume for {work}, so the next free {role} place goes to it first"),
    )
    .with_details(details)
    .with_hint(format!(
        "fold resumes @{agent} first: wait with wait_for_events until it started and one more {role} stopped, then call {tool} again, or ask the main agent or a human to raise the {role} limit with set_worker_limits"
    )))
}

/// Whether the owed resume of `agent` waits for a place: the limits refuse its start now, or an older owed resume of its role takes the free place first. `false` for a worker that fold owes no resume.
pub fn waits_for_place(
    limits: WorkerLimits,
    role: Role,
    running: u64,
    in_area: Option<(&AreaName, u64)>,
    owed: &[OwedResume],
    agent: &AgentName,
) -> bool {
    owed.iter().any(|owed| owed.agent == *agent)
        && (check_limits(limits, role, running, in_area).is_err()
            || check_owed_first(limits, role, running, in_area, owed, Some(agent), "").is_err())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: WorkerLimits = WorkerLimits {
        task_worker: 2,
        reviewer: 1,
        max_per_area: None,
    };

    fn name(text: &str) -> AgentName {
        AgentName::parse(text).unwrap()
    }

    fn owed_to(agent: &str, since: i64) -> OwedResume {
        OwedResume {
            agent: name(agent),
            area: None,
            since: Timestamp::from_millis(since).unwrap(),
            task: Some(TaskId::from_number(1).unwrap()),
            proposal: Some(ProposalId::from_number(1).unwrap()),
        }
    }

    fn names(ahead: &[&OwedResume]) -> Vec<String> {
        ahead.iter().map(|owed| owed.agent.to_string()).collect()
    }

    #[test]
    fn owed_resumes_go_in_the_order_they_became_owed() {
        let owed = [
            owed_to("c-worker", 30),
            owed_to("a-worker", 10),
            owed_to("b-worker", 20),
        ];
        assert_eq!(
            names(&owed_ahead(LIMITS, &owed, None)),
            ["a-worker", "b-worker", "c-worker"]
        );
        assert_eq!(
            names(&owed_ahead(LIMITS, &owed, Some(&name("b-worker")))),
            ["a-worker"],
            "an owed worker lets only the older ones go first"
        );
        assert!(owed_ahead(LIMITS, &owed, Some(&name("a-worker"))).is_empty());
        assert_eq!(
            names(&owed_ahead(LIMITS, &owed, Some(&name("new-worker")))),
            ["a-worker", "b-worker", "c-worker"]
        );
        let same = [owed_to("b-worker", 10), owed_to("a-worker", 10)];
        assert_eq!(
            names(&owed_ahead(LIMITS, &same, None)),
            ["a-worker", "b-worker"]
        );
        assert_eq!(
            names(&owed_ahead(LIMITS, &same, Some(&name("b-worker")))),
            ["a-worker"]
        );
    }

    #[test]
    fn a_start_is_refused_while_an_owed_resume_waits_for_the_last_free_place() {
        let waiting = [owed_to("b-worker", 10)];
        let refused = check_owed_first(
            LIMITS,
            Role::TaskWorker,
            1,
            None,
            &waiting,
            None,
            "start_worker",
        )
        .unwrap_err();
        assert_eq!(refused.code(), codes::WORKER_LIMIT_REACHED);
        let details = refused.details().unwrap();
        assert_eq!(details["reason"], OWED_RESUME_WAITS);
        assert_eq!(details["agent"], "b-worker");
        assert_eq!(details["role"], "task_worker");
        assert_eq!(details["task"], "T-1");
        assert_eq!(details["proposal"], "P-1");
        assert_eq!(
            refused.message(),
            "fold owes @b-worker a resume for P-1 of T-1, so the next free task_worker place goes to it first"
        );
        assert_eq!(
            refused.hint(),
            Some(
                "fold resumes @b-worker first: wait with wait_for_events until it started and one more task_worker stopped, then call start_worker again, or ask the main agent or a human to raise the task_worker limit with set_worker_limits"
            )
        );
        let full = check_owed_first(LIMITS, Role::TaskWorker, 2, None, &waiting, None, "x");
        assert_eq!(
            full.unwrap_err().details().unwrap()["reason"],
            OWED_RESUME_WAITS,
            "a full role names the owed resume too"
        );
    }

    #[test]
    fn a_start_goes_when_no_owed_resume_waits_or_a_place_is_left_for_each() {
        assert!(check_owed_first(LIMITS, Role::TaskWorker, 1, None, &[], None, "x").is_ok());
        let waiting = [owed_to("b-worker", 10)];
        assert!(
            check_owed_first(LIMITS, Role::TaskWorker, 0, None, &waiting, None, "x").is_ok(),
            "two free places: one for the owed resume, one for the start"
        );
        assert!(
            check_owed_first(
                LIMITS,
                Role::TaskWorker,
                1,
                None,
                &waiting,
                Some(&name("b-worker")),
                "x"
            )
            .is_ok(),
            "the owed worker itself takes its place"
        );
        assert!(
            check_owed_first(LIMITS, Role::AreaLead, 9, None, &waiting, None, "x").is_ok(),
            "a lead has no limit"
        );
        let older = [owed_to("a-worker", 5), owed_to("b-worker", 10)];
        let refused = check_owed_first(
            LIMITS,
            Role::TaskWorker,
            1,
            None,
            &older,
            Some(&name("b-worker")),
            "x",
        )
        .unwrap_err();
        assert_eq!(refused.details().unwrap()["agent"], "a-worker");
    }

    #[test]
    fn an_owed_resume_whose_area_is_full_takes_no_place_and_an_area_keeps_its_last_place() {
        let web = AreaName::parse("web").unwrap();
        let api = AreaName::parse("api").unwrap();
        let per_area = WorkerLimits {
            task_worker: 4,
            reviewer: 1,
            max_per_area: Some(1),
        };
        let full_area = [OwedResume {
            area: Some((web.clone(), 1)),
            ..owed_to("b-worker", 10)
        }];
        assert!(
            check_owed_first(
                per_area,
                Role::TaskWorker,
                3,
                Some((&api, 0)),
                &full_area,
                None,
                "x"
            )
            .is_ok(),
            "the owed worker cannot start in its full area, so it holds no place"
        );
        let room = [OwedResume {
            area: Some((web.clone(), 0)),
            ..owed_to("b-worker", 10)
        }];
        let refused = check_owed_first(
            per_area,
            Role::TaskWorker,
            1,
            Some((&web, 0)),
            &room,
            None,
            "x",
        )
        .unwrap_err();
        assert_eq!(refused.details().unwrap()["reason"], OWED_RESUME_WAITS);
        assert!(
            check_owed_first(
                per_area,
                Role::TaskWorker,
                1,
                Some((&api, 0)),
                &room,
                None,
                "x"
            )
            .is_ok(),
            "another area keeps its own place"
        );
    }

    #[test]
    fn an_owed_worker_waits_for_a_place_while_the_limits_or_an_older_resume_refuse_it() {
        let b = name("b-worker");
        let waiting = [owed_to("b-worker", 10)];
        assert!(waits_for_place(
            LIMITS,
            Role::TaskWorker,
            2,
            None,
            &waiting,
            &b
        ));
        assert!(!waits_for_place(
            LIMITS,
            Role::TaskWorker,
            1,
            None,
            &waiting,
            &b
        ));
        assert!(
            !waits_for_place(
                LIMITS,
                Role::TaskWorker,
                2,
                None,
                &waiting,
                &name("c-worker")
            ),
            "fold owes it nothing"
        );
        let older = [owed_to("a-worker", 5), owed_to("b-worker", 10)];
        assert!(
            waits_for_place(LIMITS, Role::TaskWorker, 1, None, &older, &b),
            "the free place goes to the older resume"
        );
        assert!(!waits_for_place(
            LIMITS,
            Role::TaskWorker,
            1,
            None,
            &older,
            &name("a-worker")
        ));
    }
}
