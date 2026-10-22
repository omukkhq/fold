use crate::actor::Actor;
use crate::agents::roles::Role;
use crate::agents::{AreaName, Tag};
use crate::error::codes;

use super::PolicyDenied;

/// Who asks to define an area or to start or stop a worker. The services work it out from the actor and the actor's own worker process, if fold supervises it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Starter {
    Human,
    /// The main worker of the agent with the `main` tag: the user's own interactive session, which `fld claude` starts, or the supervised worker of role `main` that fold runs in its place. Without that agent there is no main agent.
    MainAgent,
    /// The main worker of an agent that fold does not supervise and that lacks the `main` tag, such as a session opened by hand.
    Unsupervised,
    AreaLead(AreaName),
    ReviewLead,
    /// A task worker or a reviewer.
    ShortLived,
    System,
}

impl Starter {
    /// `supervised` is the role and area of the acting agent's worker process, `None` when fold does not supervise the agent.
    pub fn of(actor: &Actor, supervised: Option<(Role, Option<AreaName>)>) -> Starter {
        match actor {
            Actor::Human { .. } => Starter::Human,
            Actor::System => Starter::System,
            Actor::Agent(agent) => match supervised {
                None if agent.tags.iter().any(Tag::is_main) => Starter::MainAgent,
                None => Starter::Unsupervised,
                Some((Role::AreaLead, Some(area))) => Starter::AreaLead(area),
                Some((Role::ReviewLead, _)) => Starter::ReviewLead,
                Some((Role::Main, _)) => Starter::MainAgent,
                Some((Role::AreaLead, None) | (Role::TaskWorker | Role::Reviewer, _)) => {
                    Starter::ShortLived
                }
            },
        }
    }
}

/// The areas are the plan of the whole project, so the main agent or a named human defines them.
pub fn can_define_area(starter: &Starter) -> Result<(), PolicyDenied> {
    match starter {
        Starter::Human | Starter::MainAgent => Ok(()),
        Starter::Unsupervised => Err(not_the_main_agent("ask the main agent to call define_area")),
        _ => Err(PolicyDenied {
            code: codes::MAIN_AGENT_REQUIRED,
            message: "only the main agent or a human can define an area",
            hint: "ask the main agent to call define_area",
        }),
    }
}

/// An area's profile is part of the plan of the whole project, so whoever defines the areas changes it.
pub fn can_update_area(starter: &Starter) -> Result<(), PolicyDenied> {
    match starter {
        Starter::Human | Starter::MainAgent => Ok(()),
        Starter::Unsupervised => Err(not_the_main_agent("ask the main agent to call update_area")),
        _ => Err(PolicyDenied {
            code: codes::MAIN_AGENT_REQUIRED,
            message: "only the main agent or a human can change an area",
            hint: "ask the main agent to call update_area",
        }),
    }
}

/// A pause stops every worker of the project, so only the main agent, on the user's word, or a named human pauses or resumes.
pub fn can_pause_project(starter: &Starter) -> Result<(), PolicyDenied> {
    match starter {
        Starter::Human | Starter::MainAgent => Ok(()),
        Starter::Unsupervised => Err(not_the_main_agent(
            "ask the main agent to call pause_project or resume_project",
        )),
        _ => Err(PolicyDenied {
            code: codes::MAIN_AGENT_REQUIRED,
            message: "only the main agent or a human can pause or resume the agents",
            hint: "ask the user; the main agent calls pause_project or resume_project",
        }),
    }
}

/// The worker limits bound the whole project's processes, so the main agent or a named human changes them.
pub fn can_set_worker_limits(starter: &Starter) -> Result<(), PolicyDenied> {
    match starter {
        Starter::Human | Starter::MainAgent => Ok(()),
        Starter::Unsupervised => Err(not_the_main_agent(
            "ask the main agent to call set_worker_limits",
        )),
        _ => Err(PolicyDenied {
            code: codes::MAIN_AGENT_REQUIRED,
            message: "only the main agent or a human can change the worker limits",
            hint: "ask the main agent to call set_worker_limits",
        }),
    }
}

/// The main agent or a named human starts the leads; an area lead starts the task workers of its own area; the review lead starts the reviewers. A subagent worker, a short-lived worker and a lead of another kind start nothing, and nobody starts the supervised main: fold runs it when the project's setting says so.
pub fn can_start_worker(starter: &Starter, role: Role) -> Result<(), PolicyDenied> {
    match (role, starter) {
        (Role::AreaLead | Role::ReviewLead, Starter::Human | Starter::MainAgent)
        | (Role::TaskWorker, Starter::AreaLead(_))
        | (Role::Reviewer, Starter::ReviewLead) => Ok(()),
        (Role::AreaLead | Role::ReviewLead, Starter::Unsupervised) => Err(not_the_main_agent(
            "ask the main agent to call start_worker",
        )),
        (Role::AreaLead | Role::ReviewLead, _) => Err(PolicyDenied {
            code: codes::MAIN_AGENT_REQUIRED,
            message: "only the main agent or a human can start a lead",
            hint: "ask the main agent to call start_worker",
        }),
        (Role::TaskWorker, _) => Err(PolicyDenied {
            code: codes::WORKER_START_DENIED,
            message: "only an area lead can start a task worker",
            hint: "ask the lead of the task's area to call start_worker",
        }),
        (Role::Reviewer, _) => Err(PolicyDenied {
            code: codes::WORKER_START_DENIED,
            message: "only the review lead can start a reviewer",
            hint: "ask the review lead to call start_worker",
        }),
        (Role::Main, _) => Err(PolicyDenied {
            code: codes::WORKER_START_DENIED,
            message: "fold starts the supervised main agent itself",
            hint: "set supervised = true under [main] in .fold/config.toml; fold then runs the main agent and starts it for its work",
        }),
    }
}

/// Whoever may start a worker may stop it: the main agent, a named human or the daemon stops any worker, an area lead the task workers of its area, the review lead the reviewers. `area` is the target worker's area.
pub fn can_stop_worker(
    starter: &Starter,
    role: Role,
    area: Option<&AreaName>,
) -> Result<(), PolicyDenied> {
    let allowed = match starter {
        Starter::Human | Starter::MainAgent | Starter::System => true,
        Starter::AreaLead(own) => role == Role::TaskWorker && area == Some(own),
        Starter::ReviewLead => role == Role::Reviewer,
        Starter::Unsupervised | Starter::ShortLived => false,
    };
    if allowed {
        Ok(())
    } else {
        Err(PolicyDenied {
            code: codes::WORKER_STOP_DENIED,
            message: "only the worker's starter, the main agent or a human can stop it",
            hint: "call list_workers to see who leads the worker",
        })
    }
}

/// The main agent or a human publishes a whole plan; an area lead can split only the tasks and parents of its own area.
pub fn can_publish_task_plan(
    starter: &Starter,
    task_areas: &[AreaName],
) -> Result<(), PolicyDenied> {
    match starter {
        Starter::Human | Starter::MainAgent => Ok(()),
        Starter::AreaLead(own) if task_areas.contains(own) => Ok(()),
        Starter::AreaLead(_) => Err(PolicyDenied {
            code: codes::TASK_OUTSIDE_AREA,
            message: "an area lead publishes tasks and parent links only in its own area",
            hint: "give each new task your area's label and choose parents of that area, then call create_task_plan",
        }),
        Starter::Unsupervised => Err(not_the_main_agent(
            "ask the main agent to call create_task_plan",
        )),
        _ => Err(PolicyDenied {
            code: codes::MAIN_AGENT_REQUIRED,
            message: "only the main agent, a human or an area lead can publish a task plan",
            hint: "ask the main agent or the area's lead to call create_task_plan",
        }),
    }
}

/// A worker that starts again in a new session forgets what its old session knew, so only a named human or the main agent, on the user's word, asks for it, for a worker of any role.
pub fn can_restart_worker(starter: &Starter) -> Result<(), PolicyDenied> {
    match starter {
        Starter::Human | Starter::MainAgent => Ok(()),
        Starter::Unsupervised => Err(not_the_main_agent(
            "ask the main agent to call restart_worker",
        )),
        _ => Err(PolicyDenied {
            code: codes::MAIN_AGENT_REQUIRED,
            message: "only the main agent or a human can start a worker again in a new session",
            hint: "ask the user; the main agent calls restart_worker",
        }),
    }
}

/// A task without a proposal is done when a lead check or a person says so, so whoever plans the work decides at creation that it needs no proposal, for a `no_proposal` task and a `human` task alike: the main agent, a named human, or the lead of an area whose label the task carries. `task_areas` are the areas whose label the task carries.
pub fn can_choose_no_proposal(
    starter: &Starter,
    task_areas: &[AreaName],
) -> Result<(), PolicyDenied> {
    match starter {
        Starter::Human | Starter::MainAgent => Ok(()),
        Starter::AreaLead(own) if task_areas.contains(own) => Ok(()),
        Starter::Unsupervised => Err(not_the_main_agent(
            "create the task with the default kind, or ask the main agent",
        )),
        _ => Err(PolicyDenied {
            code: codes::TASK_KIND_DENIED,
            message: "only the main agent, a human or the lead of the task's area creates a task without a proposal",
            hint: "create the task with the default kind, or give it your area's label",
        }),
    }
}

/// The lead check of a task without a proposal stands in for the review of a proposal, so neither the task worker that did the work nor a review agent makes it: the main agent, a named human or the lead of an area whose label the task carries completes the task or sends it back. `task_areas` are the areas whose label the task carries.
pub fn can_check_task(starter: &Starter, task_areas: &[AreaName]) -> Result<(), PolicyDenied> {
    match starter {
        Starter::Human | Starter::MainAgent => Ok(()),
        Starter::AreaLead(own) if task_areas.contains(own) => Ok(()),
        Starter::Unsupervised => Err(not_the_main_agent(
            "ask the main agent or the lead of the task's area to check the task",
        )),
        _ => Err(PolicyDenied {
            code: codes::TASK_CHECK_DENIED,
            message: "only the lead of the task's area, the main agent or a human completes or sends back a task without a proposal",
            hint: "ask the lead of the task's area; get_task shows the task's labels",
        }),
    }
}

/// A hold keeps every agent off a task until a person says the work can start, so whoever plans the work sets it or changes its reason: a named human, the main agent, or the lead of an area whose label the task carries. `task_areas` are the areas whose label the task carries.
pub fn can_set_hold(starter: &Starter, task_areas: &[AreaName]) -> Result<(), PolicyDenied> {
    match starter {
        Starter::Human | Starter::MainAgent => Ok(()),
        Starter::AreaLead(own) if task_areas.contains(own) => Ok(()),
        Starter::Unsupervised => Err(not_the_main_agent(
            "ask the main agent or the lead of the task's area to call set_blocker",
        )),
        _ => Err(PolicyDenied {
            code: codes::TASK_HOLD_DENIED,
            message: "only a human, the main agent or the lead of the task's area sets a blocker",
            hint: "ask the lead of the task's area or the main agent to call set_blocker",
        }),
    }
}

/// Only a person knows that the reason of a hold is settled, so a named human clears it, or the main agent, the user's own session, once the user says so.
pub fn can_clear_hold(starter: &Starter) -> Result<(), PolicyDenied> {
    match starter {
        Starter::Human | Starter::MainAgent => Ok(()),
        Starter::Unsupervised => Err(not_the_main_agent(
            "ask the user to clear the blocker in the web interface",
        )),
        _ => Err(PolicyDenied {
            code: codes::MAIN_AGENT_REQUIRED,
            message: "only a human or the main agent clears a blocker",
            hint: "call send_message to ask a human to clear the blocker",
        }),
    }
}

/// A person does the work of a human task outside fold, so a named human marks it Done, or the main agent once the user says the work is done.
pub fn can_complete_human_task(starter: &Starter) -> Result<(), PolicyDenied> {
    match starter {
        Starter::Human | Starter::MainAgent => Ok(()),
        Starter::Unsupervised => Err(not_the_main_agent(
            "ask the user to mark the task Done in the web interface",
        )),
        _ => Err(PolicyDenied {
            code: codes::MAIN_AGENT_REQUIRED,
            message: "only a human or the main agent marks a human task Done",
            hint: "call send_message to ask a human to mark the task Done",
        }),
    }
}

/// A direct change reaches main without a task, a proposal or a review: the user's word in the conversation is its approval. Only the main agent talks with the user, so it alone opens the main workspace and submits a direct change. A lead, a worker, a subagent and the daemon are refused, and so is a human, who changes the files of the project folder and records them with `fld record`.
pub fn can_change_main_directly(starter: &Starter) -> Result<(), PolicyDenied> {
    match starter {
        Starter::MainAgent => Ok(()),
        Starter::Unsupervised => Err(not_the_main_agent(
            "plan the change as a task, or ask the user to make it through the main agent",
        )),
        Starter::Human => Err(PolicyDenied {
            code: codes::MAIN_AGENT_REQUIRED,
            message: "only the main agent makes a direct change to main",
            hint: "change the files in the project folder and run `fld record`, or ask the main agent",
        }),
        _ => Err(PolicyDenied {
            code: codes::MAIN_AGENT_REQUIRED,
            message: "only the main agent makes a direct change to main",
            hint: "record the work in a task workspace and open a proposal",
        }),
    }
}

/// An agent that fold does not supervise leaves the plan of the whole project to the main agent, which `fld claude` starts, whether or not it runs now. `hint` names the call the main agent makes instead.
fn not_the_main_agent(hint: &'static str) -> PolicyDenied {
    PolicyDenied {
        code: codes::ORCHESTRATOR_REQUIRED,
        message: "only the main agent, the agent with the main tag, can do this",
        hint,
    }
}

/// Only a long-lived worker gets a fresh context: a short-lived one stops when its work is done, and the interactive main agent is the user's own session, which clears its context itself. The supervised main, the worker of role `main` (`role`) that fold runs as the main agent, is long-lived and has no terminal to clear its context, so fold refreshes it from its note like a lead.
pub fn can_write_handoff(starter: &Starter, role: Option<Role>) -> Result<(), PolicyDenied> {
    match starter {
        Starter::AreaLead(_) | Starter::ReviewLead => Ok(()),
        Starter::MainAgent if role == Some(Role::Main) => Ok(()),
        Starter::MainAgent => Err(PolicyDenied {
            code: codes::HANDOFF_DENIED,
            message: "fold does not refresh the main agent's session",
            hint: "send yourself the note with send_message, then run /clear",
        }),
        _ => Err(PolicyDenied {
            code: codes::HANDOFF_DENIED,
            message: "only an area lead or the review lead writes a hand-off note",
            hint: "keep working; fold stops a short-lived worker when its work is done",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::AgentActor;
    use crate::agents::{HumanName, WorkerName};

    fn agent(worker: &str) -> Actor {
        let worker = WorkerName::parse(worker).unwrap();
        Actor::Agent(AgentActor {
            agent: worker.agent(),
            worker,
            tags: Vec::new(),
        })
    }

    fn area(name: &str) -> AreaName {
        AreaName::parse(name).unwrap()
    }

    #[test]
    fn the_supervised_main_acts_as_the_main_agent_and_nobody_starts_it() {
        assert_eq!(
            Starter::of(&agent("main"), Some((Role::Main, None))),
            Starter::MainAgent
        );
        for starter in [
            Starter::Human,
            Starter::MainAgent,
            Starter::AreaLead(area("web")),
            Starter::ReviewLead,
            Starter::System,
        ] {
            let refused = can_start_worker(&starter, Role::Main).unwrap_err();
            assert_eq!(refused.code(), codes::WORKER_START_DENIED);
            assert!(refused.hint().contains("[main]"));
        }
        assert!(can_stop_worker(&Starter::Human, Role::Main, None).is_ok());
        assert!(can_stop_worker(&Starter::ReviewLead, Role::Main, None).is_err());
    }

    #[test]
    fn publishing_a_plan_requires_the_main_agent_or_the_owning_area_lead() {
        let web = area("web");
        for starter in [
            Starter::Human,
            Starter::MainAgent,
            Starter::AreaLead(web.clone()),
        ] {
            assert!(can_publish_task_plan(&starter, std::slice::from_ref(&web)).is_ok());
        }
        assert_eq!(
            can_publish_task_plan(&Starter::AreaLead(area("api")), &[web])
                .unwrap_err()
                .code(),
            codes::TASK_OUTSIDE_AREA
        );
        assert_eq!(
            can_publish_task_plan(&Starter::Unsupervised, &[])
                .unwrap_err()
                .code(),
            codes::ORCHESTRATOR_REQUIRED
        );
        for starter in [Starter::ReviewLead, Starter::ShortLived, Starter::System] {
            assert_eq!(
                can_publish_task_plan(&starter, &[]).unwrap_err().code(),
                codes::MAIN_AGENT_REQUIRED
            );
        }
    }

    #[test]
    fn only_a_human_or_the_main_agent_restarts_a_worker_in_a_new_session() {
        for starter in [Starter::Human, Starter::MainAgent] {
            assert!(can_restart_worker(&starter).is_ok());
        }
        assert_eq!(
            can_restart_worker(&Starter::Unsupervised)
                .unwrap_err()
                .code(),
            codes::ORCHESTRATOR_REQUIRED
        );
        for starter in [
            Starter::AreaLead(area("web")),
            Starter::ReviewLead,
            Starter::ShortLived,
            Starter::System,
        ] {
            assert_eq!(
                can_restart_worker(&starter).unwrap_err().code(),
                codes::MAIN_AGENT_REQUIRED
            );
        }
    }

    #[test]
    fn only_the_main_agent_changes_main_directly() {
        assert!(can_change_main_directly(&Starter::MainAgent).is_ok());
        assert_eq!(
            can_change_main_directly(&Starter::Unsupervised)
                .unwrap_err()
                .code(),
            codes::ORCHESTRATOR_REQUIRED
        );
        let human = can_change_main_directly(&Starter::Human).unwrap_err();
        assert_eq!(human.code(), codes::MAIN_AGENT_REQUIRED);
        assert!(human.hint().contains("fld record"));
        for starter in [
            Starter::AreaLead(area("web")),
            Starter::ReviewLead,
            Starter::ShortLived,
            Starter::System,
        ] {
            let denied = can_change_main_directly(&starter).unwrap_err();
            assert_eq!(denied.code(), codes::MAIN_AGENT_REQUIRED);
            assert!(denied.hint().contains("proposal"));
        }
    }

    fn tagged(worker: &str, tag: &str) -> Actor {
        let Actor::Agent(mut agent) = agent(worker) else {
            unreachable!()
        };
        agent.tags.push(Tag::parse(tag).unwrap());
        Actor::Agent(agent)
    }

    #[test]
    fn the_starter_follows_the_actor_and_its_supervised_role() {
        let human = Actor::human(HumanName::parse("ana").unwrap());
        let of = |actor: &Actor, supervised| Starter::of(actor, supervised);
        assert_eq!(of(&human, None), Starter::Human);
        assert_eq!(of(&Actor::System, None), Starter::System);
        assert_eq!(of(&tagged("main", "main"), None), Starter::MainAgent);
        assert_eq!(
            of(
                &agent("web-lead"),
                Some((Role::AreaLead, Some(area("web"))))
            ),
            Starter::AreaLead(area("web"))
        );
        assert_eq!(
            of(&agent("review-lead"), Some((Role::ReviewLead, None))),
            Starter::ReviewLead
        );
        for role in [Role::TaskWorker, Role::Reviewer] {
            assert_eq!(of(&agent("w"), Some((role, None))), Starter::ShortLived);
        }
        assert_eq!(
            of(&agent("w"), Some((Role::AreaLead, None))),
            Starter::ShortLived
        );
    }

    #[test]
    fn only_the_agent_with_the_main_tag_is_the_main_agent() {
        assert_eq!(
            Starter::of(&tagged("lead", "main"), None),
            Starter::MainAgent
        );
        for untagged in [agent("frontend"), tagged("frontend", "review")] {
            assert_eq!(Starter::of(&untagged, None), Starter::Unsupervised);
        }
        assert_eq!(
            Starter::of(&agent("web-lead"), Some((Role::ReviewLead, None))),
            Starter::ReviewLead
        );
        // A session opened by hand is no main agent even while no agent has the tag, so the main agent's work and the lead check stay closed to it.
        let solo = Starter::of(&agent("solo"), None);
        let areas = [area("web")];
        assert_eq!(
            can_check_task(&solo, &areas).unwrap_err().code(),
            codes::ORCHESTRATOR_REQUIRED
        );
        assert_eq!(
            can_choose_no_proposal(&solo, &areas).unwrap_err().code(),
            codes::ORCHESTRATOR_REQUIRED
        );
        assert_eq!(
            can_update_area(&solo).unwrap_err().code(),
            codes::ORCHESTRATOR_REQUIRED
        );
        let denied = can_define_area(&Starter::Unsupervised).unwrap_err();
        assert_eq!(denied.code(), codes::ORCHESTRATOR_REQUIRED);
        assert!(denied.hint().contains("define_area"));
        let denied = can_set_worker_limits(&Starter::Unsupervised).unwrap_err();
        assert_eq!(denied.code(), codes::ORCHESTRATOR_REQUIRED);
        assert!(denied.hint().contains("set_worker_limits"));
        let denied = can_pause_project(&Starter::Unsupervised).unwrap_err();
        assert_eq!(denied.code(), codes::ORCHESTRATOR_REQUIRED);
        assert!(denied.hint().contains("pause_project"));
        let denied = can_update_area(&Starter::Unsupervised).unwrap_err();
        assert_eq!(denied.code(), codes::ORCHESTRATOR_REQUIRED);
        assert!(denied.hint().contains("update_area"));
        for role in [Role::AreaLead, Role::ReviewLead] {
            let denied = can_start_worker(&Starter::Unsupervised, role).unwrap_err();
            assert_eq!(denied.code(), codes::ORCHESTRATOR_REQUIRED);
            assert!(denied.hint().contains("start_worker"));
        }
        assert_eq!(
            can_start_worker(&Starter::Unsupervised, Role::TaskWorker)
                .unwrap_err()
                .code(),
            codes::WORKER_START_DENIED
        );
        assert!(can_stop_worker(&Starter::Unsupervised, Role::TaskWorker, None).is_err());
        assert_eq!(
            can_write_handoff(&Starter::Unsupervised, None)
                .unwrap_err()
                .code(),
            codes::HANDOFF_DENIED
        );
    }

    #[test]
    fn only_the_main_agent_or_a_human_defines_areas_and_starts_leads() {
        for starter in [Starter::Human, Starter::MainAgent] {
            assert!(can_define_area(&starter).is_ok());
            assert!(can_update_area(&starter).is_ok());
            assert!(can_set_worker_limits(&starter).is_ok());
            assert!(can_pause_project(&starter).is_ok());
            assert!(can_start_worker(&starter, Role::AreaLead).is_ok());
            assert!(can_start_worker(&starter, Role::ReviewLead).is_ok());
            assert!(can_start_worker(&starter, Role::TaskWorker).is_err());
        }
        for starter in [
            Starter::AreaLead(area("web")),
            Starter::ReviewLead,
            Starter::ShortLived,
            Starter::System,
        ] {
            assert_eq!(
                can_define_area(&starter).unwrap_err().code(),
                codes::MAIN_AGENT_REQUIRED
            );
            assert_eq!(
                can_update_area(&starter).unwrap_err().code(),
                codes::MAIN_AGENT_REQUIRED
            );
            assert_eq!(
                can_set_worker_limits(&starter).unwrap_err().code(),
                codes::MAIN_AGENT_REQUIRED
            );
            assert_eq!(
                can_pause_project(&starter).unwrap_err().code(),
                codes::MAIN_AGENT_REQUIRED
            );
            assert_eq!(
                can_start_worker(&starter, Role::AreaLead)
                    .unwrap_err()
                    .code(),
                codes::MAIN_AGENT_REQUIRED
            );
        }
    }

    #[test]
    fn task_workers_come_from_area_leads_and_reviewers_from_the_review_lead() {
        assert!(can_start_worker(&Starter::AreaLead(area("web")), Role::TaskWorker).is_ok());
        assert!(can_start_worker(&Starter::ReviewLead, Role::Reviewer).is_ok());
        for starter in [Starter::ReviewLead, Starter::ShortLived] {
            assert_eq!(
                can_start_worker(&starter, Role::TaskWorker)
                    .unwrap_err()
                    .code(),
                codes::WORKER_START_DENIED
            );
        }
        assert_eq!(
            can_start_worker(&Starter::AreaLead(area("web")), Role::Reviewer)
                .unwrap_err()
                .code(),
            codes::WORKER_START_DENIED
        );
    }

    #[test]
    fn a_lead_stops_only_its_own_short_lived_workers() {
        let web = area("web");
        let api = area("api");
        let lead = Starter::AreaLead(web.clone());
        assert!(can_stop_worker(&lead, Role::TaskWorker, Some(&web)).is_ok());
        assert!(can_stop_worker(&lead, Role::TaskWorker, Some(&api)).is_err());
        assert!(can_stop_worker(&lead, Role::AreaLead, Some(&web)).is_err());
        assert!(can_stop_worker(&Starter::ReviewLead, Role::Reviewer, None).is_ok());
        assert!(can_stop_worker(&Starter::ReviewLead, Role::TaskWorker, Some(&web)).is_err());
        for starter in [Starter::Human, Starter::MainAgent, Starter::System] {
            assert!(can_stop_worker(&starter, Role::AreaLead, Some(&web)).is_ok());
        }
        assert_eq!(
            can_stop_worker(&Starter::ShortLived, Role::TaskWorker, Some(&web))
                .unwrap_err()
                .code(),
            codes::WORKER_STOP_DENIED
        );
    }

    #[test]
    fn only_the_tasks_area_lead_the_main_agent_or_a_human_checks_a_task() {
        let web = area("web");
        let areas = [web.clone()];
        for starter in [
            Starter::Human,
            Starter::MainAgent,
            Starter::AreaLead(web.clone()),
        ] {
            assert!(can_check_task(&starter, &areas).is_ok());
            assert!(can_choose_no_proposal(&starter, &areas).is_ok());
        }
        for starter in [
            Starter::AreaLead(area("api")),
            Starter::ReviewLead,
            Starter::ShortLived,
            Starter::System,
        ] {
            assert_eq!(
                can_check_task(&starter, &areas).unwrap_err().code(),
                codes::TASK_CHECK_DENIED
            );
            assert_eq!(
                can_choose_no_proposal(&starter, &areas).unwrap_err().code(),
                codes::TASK_KIND_DENIED
            );
        }
        assert!(can_check_task(&Starter::AreaLead(web), &[]).is_err());
        assert_eq!(
            can_check_task(&Starter::Unsupervised, &areas)
                .unwrap_err()
                .code(),
            codes::ORCHESTRATOR_REQUIRED
        );
        assert_eq!(
            can_choose_no_proposal(&Starter::Unsupervised, &areas)
                .unwrap_err()
                .code(),
            codes::ORCHESTRATOR_REQUIRED
        );
    }

    #[test]
    fn a_human_the_main_agent_or_the_tasks_area_lead_sets_a_hold() {
        let web = area("web");
        let areas = [web.clone()];
        for starter in [
            Starter::Human,
            Starter::MainAgent,
            Starter::AreaLead(web.clone()),
        ] {
            assert!(can_set_hold(&starter, &areas).is_ok(), "{starter:?}");
        }
        assert!(can_set_hold(&Starter::Human, &[]).is_ok());
        for starter in [
            Starter::AreaLead(area("api")),
            Starter::ReviewLead,
            Starter::ShortLived,
            Starter::System,
        ] {
            let denied = can_set_hold(&starter, &areas).unwrap_err();
            assert_eq!(denied.code(), codes::TASK_HOLD_DENIED, "{starter:?}");
            assert!(denied.hint().contains("set_blocker"));
        }
        assert_eq!(
            can_set_hold(&Starter::Unsupervised, &areas)
                .unwrap_err()
                .code(),
            codes::ORCHESTRATOR_REQUIRED
        );
    }

    #[test]
    fn only_a_human_or_the_main_agent_clears_a_hold_or_completes_a_human_task() {
        for starter in [Starter::Human, Starter::MainAgent] {
            assert!(can_clear_hold(&starter).is_ok());
            assert!(can_complete_human_task(&starter).is_ok());
        }
        for starter in [
            Starter::AreaLead(area("web")),
            Starter::ReviewLead,
            Starter::ShortLived,
            Starter::System,
        ] {
            let clear = can_clear_hold(&starter).unwrap_err();
            let complete = can_complete_human_task(&starter).unwrap_err();
            assert_eq!(clear.code(), codes::MAIN_AGENT_REQUIRED, "{starter:?}");
            assert_eq!(complete.code(), codes::MAIN_AGENT_REQUIRED, "{starter:?}");
            assert!(clear.hint().contains("send_message"));
            assert!(complete.to_string().contains("human task"));
        }
        for denied in [
            can_clear_hold(&Starter::Unsupervised).unwrap_err(),
            can_complete_human_task(&Starter::Unsupervised).unwrap_err(),
        ] {
            assert_eq!(denied.code(), codes::ORCHESTRATOR_REQUIRED);
            assert!(denied.hint().contains("web interface"));
        }
    }

    #[test]
    fn only_a_lead_or_the_supervised_main_writes_a_hand_off_note() {
        assert!(can_write_handoff(&Starter::AreaLead(area("web")), Some(Role::AreaLead)).is_ok());
        assert!(can_write_handoff(&Starter::ReviewLead, Some(Role::ReviewLead)).is_ok());
        assert!(can_write_handoff(&Starter::MainAgent, Some(Role::Main)).is_ok());
        let interactive = can_write_handoff(&Starter::MainAgent, None).unwrap_err();
        assert_eq!(interactive.code(), codes::HANDOFF_DENIED);
        assert!(interactive.hint().contains("/clear"));
        for (starter, role) in [
            (Starter::Human, None),
            (Starter::ShortLived, Some(Role::TaskWorker)),
            (Starter::System, None),
        ] {
            assert_eq!(
                can_write_handoff(&starter, role).unwrap_err().code(),
                codes::HANDOFF_DENIED
            );
        }
    }
}
