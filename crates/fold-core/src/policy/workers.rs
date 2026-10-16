use crate::actor::Actor;
use crate::agents::{AreaName, Tag};
use crate::error::codes;
use crate::roles::Role;

use super::PolicyDenied;

/// Who asks to define an area or to start or stop a worker. The services work it out from the actor and the actor's own worker process, if fold supervises it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Starter {
    Human,
    /// The main worker of the agent with the `main` tag: the user's own interactive session. While no agent has the tag, the main worker of any agent that fold does not supervise counts as the main agent.
    MainAgent,
    /// The main worker of an agent that fold does not supervise and that lacks the `main` tag, while another agent has it.
    Unsupervised,
    AreaLead(AreaName),
    ReviewLead,
    /// A task coder or a batch reviewer.
    ShortLived,
    /// A child worker of any agent.
    Subagent,
    System,
}

impl Starter {
    /// `supervised` is the role and area of the acting agent's worker process, `None` when fold does not supervise the agent. `main_exists` is whether some agent has the `main` tag.
    pub fn of(
        actor: &Actor,
        supervised: Option<(Role, Option<AreaName>)>,
        main_exists: bool,
    ) -> Starter {
        match actor {
            Actor::Human { .. } => Starter::Human,
            Actor::System => Starter::System,
            Actor::Agent(agent) if !agent.worker.is_main() => Starter::Subagent,
            Actor::Agent(agent) => match supervised {
                None if !main_exists || agent.tags.iter().any(Tag::is_main) => Starter::MainAgent,
                None => Starter::Unsupervised,
                Some((Role::AreaLead, Some(area))) => Starter::AreaLead(area),
                Some((Role::ReviewLead, _)) => Starter::ReviewLead,
                Some((Role::AreaLead, None) | (Role::TaskCoder | Role::BatchReviewer, _)) => {
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

/// The main agent or a named human starts the leads; an area lead starts the task coders of its own area; the review lead starts the batch reviewers. A subagent worker, a short-lived worker and a lead of another kind start nothing.
pub fn can_start_worker(starter: &Starter, role: Role) -> Result<(), PolicyDenied> {
    match (role, starter) {
        (Role::AreaLead | Role::ReviewLead, Starter::Human | Starter::MainAgent)
        | (Role::TaskCoder, Starter::AreaLead(_))
        | (Role::BatchReviewer, Starter::ReviewLead) => Ok(()),
        (Role::AreaLead | Role::ReviewLead, Starter::Unsupervised) => Err(not_the_main_agent(
            "ask the main agent to call start_worker",
        )),
        (Role::AreaLead | Role::ReviewLead, _) => Err(PolicyDenied {
            code: codes::MAIN_AGENT_REQUIRED,
            message: "only the main agent or a human can start a lead",
            hint: "ask the main agent to call start_worker",
        }),
        (Role::TaskCoder, _) => Err(PolicyDenied {
            code: codes::WORKER_START_DENIED,
            message: "only an area lead can start a task coder",
            hint: "ask the lead of the task's area to call start_worker",
        }),
        (Role::BatchReviewer, _) => Err(PolicyDenied {
            code: codes::WORKER_START_DENIED,
            message: "only the review lead can start a batch reviewer",
            hint: "ask the review lead to call start_worker",
        }),
    }
}

/// Whoever may start a worker may stop it: the main agent, a named human or the daemon stops any worker, an area lead the task coders of its area, the review lead the batch reviewers. `area` is the target worker's area.
pub fn can_stop_worker(
    starter: &Starter,
    role: Role,
    area: Option<&AreaName>,
) -> Result<(), PolicyDenied> {
    let allowed = match starter {
        Starter::Human | Starter::MainAgent | Starter::System => true,
        Starter::AreaLead(own) => role == Role::TaskCoder && area == Some(own),
        Starter::ReviewLead => role == Role::BatchReviewer,
        Starter::Unsupervised | Starter::ShortLived | Starter::Subagent => false,
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

/// A task without a proposal is done when a lead check says so, so whoever may check it decides at creation that it needs no proposal: the main agent, a named human, or the lead of an area whose label the task carries. `task_areas` are the areas whose label the task carries.
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

/// The lead check of a task without a proposal stands in for the review of a proposal, so neither the coder that did the work nor a review agent makes it: the main agent, a named human or the lead of an area whose label the task carries completes the task or sends it back. `task_areas` are the areas whose label the task carries.
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

/// While the main agent exists, an agent that fold does not supervise leaves the plan of the whole project to it. `hint` names the call the main agent makes instead.
fn not_the_main_agent(hint: &'static str) -> PolicyDenied {
    PolicyDenied {
        code: codes::ORCHESTRATOR_REQUIRED,
        message: "only the agent with the main tag can do this while it exists",
        hint,
    }
}

/// Only a long-lived worker gets a fresh context: a short-lived one stops when its work is done, and the main agent is the user's own session, which clears its context itself.
pub fn can_write_handoff(starter: &Starter) -> Result<(), PolicyDenied> {
    match starter {
        Starter::AreaLead(_) | Starter::ReviewLead => Ok(()),
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
        let of = |actor: &Actor, supervised| Starter::of(actor, supervised, false);
        assert_eq!(of(&human, None), Starter::Human);
        assert_eq!(of(&Actor::System, None), Starter::System);
        assert_eq!(of(&agent("main"), None), Starter::MainAgent);
        assert_eq!(of(&agent("main/ui"), None), Starter::Subagent);
        assert_eq!(
            of(
                &agent("web-lead"),
                Some((Role::AreaLead, Some(area("web"))))
            ),
            Starter::AreaLead(area("web"))
        );
        assert_eq!(
            of(
                &agent("web-lead/x"),
                Some((Role::AreaLead, Some(area("web"))))
            ),
            Starter::Subagent
        );
        assert_eq!(
            of(&agent("review-lead"), Some((Role::ReviewLead, None))),
            Starter::ReviewLead
        );
        for role in [Role::TaskCoder, Role::BatchReviewer] {
            assert_eq!(of(&agent("w"), Some((role, None))), Starter::ShortLived);
        }
        assert_eq!(
            of(&agent("w"), Some((Role::AreaLead, None))),
            Starter::ShortLived
        );
    }

    #[test]
    fn once_an_agent_has_the_main_tag_only_it_is_the_main_agent() {
        assert_eq!(
            Starter::of(&tagged("lead", "main"), None, true),
            Starter::MainAgent
        );
        for untagged in [agent("frontend"), tagged("frontend", "review")] {
            assert_eq!(Starter::of(&untagged, None, true), Starter::Unsupervised);
        }
        assert_eq!(
            Starter::of(&tagged("lead/sub", "main"), None, true),
            Starter::Subagent
        );
        assert_eq!(
            Starter::of(&agent("web-lead"), Some((Role::ReviewLead, None)), true),
            Starter::ReviewLead
        );
        let denied = can_define_area(&Starter::Unsupervised).unwrap_err();
        assert_eq!(denied.code(), codes::ORCHESTRATOR_REQUIRED);
        assert!(denied.hint().contains("define_area"));
        let denied = can_set_worker_limits(&Starter::Unsupervised).unwrap_err();
        assert_eq!(denied.code(), codes::ORCHESTRATOR_REQUIRED);
        assert!(denied.hint().contains("set_worker_limits"));
        let denied = can_update_area(&Starter::Unsupervised).unwrap_err();
        assert_eq!(denied.code(), codes::ORCHESTRATOR_REQUIRED);
        assert!(denied.hint().contains("update_area"));
        for role in [Role::AreaLead, Role::ReviewLead] {
            let denied = can_start_worker(&Starter::Unsupervised, role).unwrap_err();
            assert_eq!(denied.code(), codes::ORCHESTRATOR_REQUIRED);
            assert!(denied.hint().contains("start_worker"));
        }
        assert_eq!(
            can_start_worker(&Starter::Unsupervised, Role::TaskCoder)
                .unwrap_err()
                .code(),
            codes::WORKER_START_DENIED
        );
        assert!(can_stop_worker(&Starter::Unsupervised, Role::TaskCoder, None).is_err());
        assert_eq!(
            can_write_handoff(&Starter::Unsupervised)
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
            assert!(can_start_worker(&starter, Role::AreaLead).is_ok());
            assert!(can_start_worker(&starter, Role::ReviewLead).is_ok());
            assert!(can_start_worker(&starter, Role::TaskCoder).is_err());
        }
        for starter in [
            Starter::AreaLead(area("web")),
            Starter::ReviewLead,
            Starter::ShortLived,
            Starter::Subagent,
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
                can_start_worker(&starter, Role::AreaLead)
                    .unwrap_err()
                    .code(),
                codes::MAIN_AGENT_REQUIRED
            );
        }
    }

    #[test]
    fn coders_come_from_area_leads_and_batch_reviewers_from_the_review_lead() {
        assert!(can_start_worker(&Starter::AreaLead(area("web")), Role::TaskCoder).is_ok());
        assert!(can_start_worker(&Starter::ReviewLead, Role::BatchReviewer).is_ok());
        for starter in [Starter::ReviewLead, Starter::ShortLived, Starter::Subagent] {
            assert_eq!(
                can_start_worker(&starter, Role::TaskCoder)
                    .unwrap_err()
                    .code(),
                codes::WORKER_START_DENIED
            );
        }
        assert_eq!(
            can_start_worker(&Starter::AreaLead(area("web")), Role::BatchReviewer)
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
        assert!(can_stop_worker(&lead, Role::TaskCoder, Some(&web)).is_ok());
        assert!(can_stop_worker(&lead, Role::TaskCoder, Some(&api)).is_err());
        assert!(can_stop_worker(&lead, Role::AreaLead, Some(&web)).is_err());
        assert!(can_stop_worker(&Starter::ReviewLead, Role::BatchReviewer, None).is_ok());
        assert!(can_stop_worker(&Starter::ReviewLead, Role::TaskCoder, Some(&web)).is_err());
        for starter in [Starter::Human, Starter::MainAgent, Starter::System] {
            assert!(can_stop_worker(&starter, Role::AreaLead, Some(&web)).is_ok());
        }
        assert_eq!(
            can_stop_worker(&Starter::ShortLived, Role::TaskCoder, Some(&web))
                .unwrap_err()
                .code(),
            codes::WORKER_STOP_DENIED
        );
        assert!(can_stop_worker(&Starter::Subagent, Role::TaskCoder, None).is_err());
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
            Starter::Subagent,
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
    fn only_a_lead_writes_a_hand_off_note() {
        assert!(can_write_handoff(&Starter::AreaLead(area("web"))).is_ok());
        assert!(can_write_handoff(&Starter::ReviewLead).is_ok());
        for starter in [
            Starter::MainAgent,
            Starter::Human,
            Starter::ShortLived,
            Starter::Subagent,
            Starter::System,
        ] {
            assert_eq!(
                can_write_handoff(&starter).unwrap_err().code(),
                codes::HANDOFF_DENIED
            );
        }
    }
}
