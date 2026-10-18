use crate::actor::Actor;
use crate::agents::{AgentName, Tag, WorkerName};
use crate::error::codes;
use crate::tasks::status::TaskStatus;

use super::PolicyDenied;

/// The worker that holds a task's implementation claim and the agents that lead it: the area lead of a task worker's area, or the review lead of a reviewer. Any other holder has no lead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskHolder {
    pub worker: WorkerName,
    pub leads: Vec<AgentName>,
}

/// Archiving releases the holder's claim and archives its workspace, so a held task is archived only by the holding worker's agent, through any of its workers, by its lead or by a named human; the main agent asks the lead. A task nobody holds is archived by anyone.
pub fn can_archive_task(actor: &Actor, holder: Option<&TaskHolder>) -> Result<(), PolicyDenied> {
    if may_remove(actor, holder) {
        return Ok(());
    }
    Err(PolicyDenied {
        code: codes::NOT_CLAIM_HOLDER,
        message: "only the agent that holds the task, its lead or a human can archive it",
        hint: "call get_task to see the holder, then ask it or its lead",
    })
}

/// Deleting removes the holder's claim with the task, so the rule of [`can_archive_task`] applies.
pub fn can_delete_task(actor: &Actor, holder: Option<&TaskHolder>) -> Result<(), PolicyDenied> {
    if may_remove(actor, holder) {
        return Ok(());
    }
    Err(PolicyDenied {
        code: codes::NOT_CLAIM_HOLDER,
        message: "only the agent that holds the task, its lead or a human can delete it",
        hint: "call get_task to see the holder, then ask it or its lead",
    })
}

/// A plain step records what the task's work covers, so the agents that do or lead that work change its checklists: on a held task the holding worker's agent, through any of its workers, or its lead; on a task nobody holds the lead of an area whose label it carries (`area_leads`). The main agent, through any worker of the agent with the `main` tag, and a named human change every checklist. Linked items and the Done lock are the checklist commands' own rules.
pub fn can_change_checklist(
    actor: &Actor,
    holder: Option<&TaskHolder>,
    area_leads: &[AgentName],
) -> Result<(), PolicyDenied> {
    let allowed = match actor {
        Actor::Human { .. } => true,
        Actor::Agent(agent) if agent.tags.iter().any(Tag::is_main) => true,
        Actor::Agent(agent) => match holder {
            Some(holder) => {
                agent.agent == holder.worker.agent() || holder.leads.contains(&agent.agent)
            }
            None => area_leads.contains(&agent.agent),
        },
        Actor::System => false,
    };
    if allowed {
        return Ok(());
    }
    Err(match holder {
        Some(_) => PolicyDenied {
            code: codes::CHECKLIST_DENIED,
            message: "only the agent that holds the task, its lead, the main agent or a human can change its checklist",
            hint: "call get_task to see the holder, then send_message to it or its lead",
        },
        None => PolicyDenied {
            code: codes::CHECKLIST_DENIED,
            message: "only the lead of the task's area, the main agent or a human can change the checklist of a task nobody holds",
            hint: "call get_task to see the task's labels, then send_message to the lead of its area",
        },
    })
}

/// How far an agent has taken a task, which decides whether it can still get a blocker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskStart {
    pub status: TaskStatus,
    /// An unreleased implementation claim exists.
    pub claimed: bool,
    /// A live task workspace exists.
    pub workspace: bool,
    pub open_proposal: bool,
}

/// A blocker keeps agents from starting a task, so it is set while the task waits in Backlog or Todo and no agent has started it: no claim, no live task workspace and no open proposal. Work that an agent has started is never put on hold.
pub fn can_hold_task(start: &TaskStart) -> Result<(), PolicyDenied> {
    let waiting = matches!(start.status, TaskStatus::Backlog | TaskStatus::Todo);
    if waiting && !start.claimed && !start.workspace && !start.open_proposal {
        return Ok(());
    }
    Err(PolicyDenied {
        code: codes::TASK_HOLD_DENIED,
        message: "a blocker is set only on a task in Backlog or Todo that no agent has started",
        hint: "set the blocker before an agent claims the task; call get_task to see its claim, workspace and proposal",
    })
}

fn may_remove(actor: &Actor, holder: Option<&TaskHolder>) -> bool {
    let Some(holder) = holder else {
        return true;
    };
    match actor {
        Actor::Human { .. } => true,
        Actor::Agent(agent) => {
            agent.agent == holder.worker.agent() || holder.leads.contains(&agent.agent)
        }
        Actor::System => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::AgentActor;
    use crate::agents::HumanName;

    fn agent(worker: &str) -> Actor {
        let worker = WorkerName::parse(worker).unwrap();
        Actor::Agent(AgentActor {
            agent: worker.agent(),
            worker,
            tags: Vec::new(),
        })
    }

    #[test]
    fn a_held_task_is_removed_by_its_holder_its_lead_or_a_human() {
        let holder = TaskHolder {
            worker: WorkerName::parse("web-worker").unwrap(),
            leads: vec![AgentName::parse("web-lead").unwrap()],
        };
        let human = Actor::human(HumanName::parse("tanim").unwrap());
        for (case, actor, allowed) in [
            ("holder", agent("web-worker"), true),
            ("holder's other worker", agent("web-worker/tests"), true),
            ("lead", agent("web-lead"), true),
            ("lead's other worker", agent("web-lead/plan"), true),
            ("main agent", agent("main"), false),
            ("other task worker", agent("api-worker"), false),
            ("human", human, true),
            ("system", Actor::System, false),
        ] {
            let archive = can_archive_task(&actor, Some(&holder));
            let delete = can_delete_task(&actor, Some(&holder));
            assert_eq!(archive.is_ok(), allowed, "{case} archives");
            assert_eq!(delete.is_ok(), allowed, "{case} deletes");
            if !allowed {
                assert_eq!(archive.unwrap_err().code(), codes::NOT_CLAIM_HOLDER);
                let denied = delete.unwrap_err();
                assert!(denied.to_string().ends_with("can delete it"), "{case}");
                assert!(denied.hint().contains("get_task"));
            }
        }
    }

    fn tagged(worker: &str, tag: &str) -> Actor {
        let Actor::Agent(mut agent) = agent(worker) else {
            unreachable!("agent builds an agent actor");
        };
        agent.tags.push(Tag::parse(tag).unwrap());
        Actor::Agent(agent)
    }

    #[test]
    fn a_held_task_checklist_is_changed_by_its_holder_its_lead_the_main_agent_or_a_human() {
        let holder = TaskHolder {
            worker: WorkerName::parse("web-worker").unwrap(),
            leads: vec![AgentName::parse("web-lead").unwrap()],
        };
        let area_leads = [AgentName::parse("api-lead").unwrap()];
        let human = Actor::human(HumanName::parse("tanim").unwrap());
        for (case, actor, allowed) in [
            ("holder", agent("web-worker"), true),
            ("holder's other worker", agent("web-worker/tests"), true),
            ("lead", agent("web-lead"), true),
            ("lead's other worker", agent("web-lead/plan"), true),
            ("main agent", tagged("main", "main"), true),
            ("main agent's subagent", tagged("main/plan", "main"), true),
            ("other task worker", agent("api-worker"), false),
            ("other tag", tagged("api-worker", "backend"), false),
            (
                "area lead that does not lead the holder",
                agent("api-lead"),
                false,
            ),
            ("human", human, true),
            ("system", Actor::System, false),
        ] {
            let result = can_change_checklist(&actor, Some(&holder), &area_leads);
            assert_eq!(result.is_ok(), allowed, "{case}");
            if let Err(denied) = result {
                assert_eq!(denied.code(), codes::CHECKLIST_DENIED, "{case}");
                assert!(denied.to_string().contains("holds the task"), "{case}");
                assert!(denied.hint().contains("get_task"));
            }
        }
    }

    #[test]
    fn a_checklist_of_a_task_nobody_holds_is_changed_by_its_area_lead_the_main_agent_or_a_human() {
        let area_leads = [AgentName::parse("web-lead").unwrap()];
        let human = Actor::human(HumanName::parse("tanim").unwrap());
        for (case, actor, allowed) in [
            ("area lead", agent("web-lead"), true),
            ("area lead's other worker", agent("web-lead/plan"), true),
            ("main agent", tagged("main", "main"), true),
            ("other lead", agent("api-lead"), false),
            ("task worker", agent("web-worker"), false),
            ("human", human, true),
            ("system", Actor::System, false),
        ] {
            let result = can_change_checklist(&actor, None, &area_leads);
            assert_eq!(result.is_ok(), allowed, "{case}");
            if let Err(denied) = result {
                assert_eq!(denied.code(), codes::CHECKLIST_DENIED, "{case}");
                assert!(denied.to_string().contains("nobody holds"), "{case}");
                assert!(denied.hint().contains("lead of its area"));
            }
        }
        assert!(can_change_checklist(&agent("web-lead"), None, &[]).is_err());
    }

    #[test]
    fn only_a_task_no_agent_has_started_gets_a_blocker() {
        let waiting = TaskStart {
            status: TaskStatus::Todo,
            claimed: false,
            workspace: false,
            open_proposal: false,
        };
        assert!(can_hold_task(&waiting).is_ok());
        assert!(
            can_hold_task(&TaskStart {
                status: TaskStatus::Backlog,
                ..waiting
            })
            .is_ok()
        );
        for started in [
            TaskStart {
                claimed: true,
                ..waiting
            },
            TaskStart {
                workspace: true,
                ..waiting
            },
            TaskStart {
                open_proposal: true,
                ..waiting
            },
            TaskStart {
                status: TaskStatus::InProgress,
                ..waiting
            },
            TaskStart {
                status: TaskStatus::Review,
                ..waiting
            },
        ] {
            let denied = can_hold_task(&started).unwrap_err();
            assert_eq!(denied.code(), codes::TASK_HOLD_DENIED, "{started:?}");
            assert!(denied.hint().contains("get_task"));
        }
    }

    #[test]
    fn a_task_nobody_holds_is_removed_by_anyone() {
        for actor in [agent("api-worker"), Actor::System] {
            assert!(can_archive_task(&actor, None).is_ok());
            assert!(can_delete_task(&actor, None).is_ok());
        }
    }
}
