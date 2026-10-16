use crate::actor::Actor;
use crate::agents::{AgentName, WorkerName};
use crate::error::codes;

use super::PolicyDenied;

/// The worker that holds a task's implementation claim and the agents that lead it: the area lead of a task coder's area, or the review lead of a batch reviewer. Any other holder has no lead.
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
            worker: WorkerName::parse("web-coder").unwrap(),
            leads: vec![AgentName::parse("web-lead").unwrap()],
        };
        let human = Actor::human(HumanName::parse("tanim").unwrap());
        for (case, actor, allowed) in [
            ("holder", agent("web-coder"), true),
            ("holder's other worker", agent("web-coder/tests"), true),
            ("lead", agent("web-lead"), true),
            ("lead's other worker", agent("web-lead/plan"), true),
            ("main agent", agent("main"), false),
            ("other coder", agent("api-coder"), false),
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

    #[test]
    fn a_task_nobody_holds_is_removed_by_anyone() {
        for actor in [agent("api-coder"), Actor::System] {
            assert!(can_archive_task(&actor, None).is_ok());
            assert!(can_delete_task(&actor, None).is_ok());
        }
    }
}
