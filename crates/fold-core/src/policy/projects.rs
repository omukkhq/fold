use crate::actor::Actor;
use crate::error::codes;

use super::PolicyDenied;
use super::workers::Starter;

/// Bringing the remote branch into main changes main outside any proposal, so only a named human runs `fld pull`.
pub fn can_pull(actor: &Actor) -> Result<(), PolicyDenied> {
    if actor.is_human() {
        Ok(())
    } else {
        Err(PolicyDenied {
            code: codes::HUMAN_REQUIRED,
            message: "only a human brings the remote branch into main",
            hint: "ask a human to run `fld pull`",
        })
    }
}

/// A human change in the project folder reaches main with no task and no review: the human who made it records it, so only a named human runs `fld record`. A worker that writes the project folder is refused, and so is fold itself, which never records a change no human asked for.
pub fn can_record(actor: &Actor) -> Result<(), PolicyDenied> {
    if actor.is_human() {
        Ok(())
    } else {
        Err(PolicyDenied {
            code: codes::HUMAN_REQUIRED,
            message: "only a human records the changes of the project folder",
            hint: "ask a human to run `fld record` in the project folder",
        })
    }
}

/// The stash holds the human's own unrecorded changes and writes them into the project folder, so only a named human runs `fld stash` and `fld unstash`; an agent never puts the human's work aside.
pub fn can_stash(actor: &Actor) -> Result<(), PolicyDenied> {
    if actor.is_human() {
        Ok(())
    } else {
        Err(PolicyDenied {
            code: codes::HUMAN_REQUIRED,
            message: "only a human stashes the changes of the project folder",
            hint: "ask a human to run `fld stash` or `fld unstash` in the project folder",
        })
    }
}

/// Turning Git on writes `.git` into the human's project folder, records the `.gitignore` as the human's own revision and stays on for good, so only a named human runs `fld init --with-git`.
pub fn can_enable_git(actor: &Actor) -> Result<(), PolicyDenied> {
    if actor.is_human() {
        Ok(())
    } else {
        Err(PolicyDenied {
            code: codes::HUMAN_REQUIRED,
            message: "only a human turns Git on",
            hint: "ask a human to run `fld init --with-git` in the project folder",
        })
    }
}

/// Taking a project off fold, and bringing it back, changes what every person on the host sees, so only a named human does it; an agent that thinks a project should go asks the human.
pub fn can_remove_project(actor: &Actor) -> Result<(), PolicyDenied> {
    if actor.is_human() {
        Ok(())
    } else {
        Err(PolicyDenied {
            code: codes::HUMAN_REQUIRED,
            message: "only a human removes a project from fold or adds it back",
            hint: "ask a human to do it on the projects page",
        })
    }
}

/// `.fold/exclude` holds the human's local rules of what is never recorded, imported or proposed. The human writes it directly; of the agents only the main agent, which talks with the human, adds a pattern, so no worker can hide files from review by excluding them.
pub fn can_add_exclude_patterns(starter: &Starter) -> Result<(), PolicyDenied> {
    match starter {
        Starter::MainAgent => Ok(()),
        Starter::Human => Err(PolicyDenied {
            code: codes::MAIN_AGENT_REQUIRED,
            message: "a human writes .fold/exclude directly",
            hint: "add the pattern to .fold/exclude in the project folder, one per line",
        }),
        _ => Err(PolicyDenied {
            code: codes::MAIN_AGENT_REQUIRED,
            message: "only the main agent adds patterns to .fold/exclude",
            hint: "ask the main agent, which asks the user",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::AgentActor;
    use crate::agents::{AgentName, HumanName, WorkerName};

    #[test]
    fn only_a_human_removes_a_project_or_adds_it_back() {
        assert!(can_remove_project(&Actor::human(HumanName::parse("ana").unwrap())).is_ok());
        let agent = Actor::Agent(AgentActor {
            agent: AgentName::parse("main").unwrap(),
            worker: WorkerName::parse("main").unwrap(),
            tags: Vec::new(),
        });
        for actor in [agent, Actor::System] {
            let denied = can_remove_project(&actor).unwrap_err();
            assert_eq!(denied.code(), codes::HUMAN_REQUIRED);
            assert_eq!(denied.hint(), "ask a human to do it on the projects page");
        }
    }

    #[test]
    fn only_a_human_pulls() {
        assert!(can_pull(&Actor::human(HumanName::parse("ana").unwrap())).is_ok());
        let agent = Actor::Agent(AgentActor {
            agent: AgentName::parse("frontend").unwrap(),
            worker: WorkerName::parse("frontend").unwrap(),
            tags: Vec::new(),
        });
        for actor in [agent, Actor::System] {
            let denied = can_pull(&actor).unwrap_err();
            assert_eq!(denied.code(), codes::HUMAN_REQUIRED);
            assert_eq!(denied.hint(), "ask a human to run `fld pull`");
        }
    }

    #[test]
    fn only_a_human_records_the_project_folder() {
        assert!(can_record(&Actor::human(HumanName::parse("ana").unwrap())).is_ok());
        let agent = Actor::Agent(AgentActor {
            agent: AgentName::parse("main").unwrap(),
            worker: WorkerName::parse("main").unwrap(),
            tags: Vec::new(),
        });
        for actor in [agent, Actor::System] {
            let denied = can_record(&actor).unwrap_err();
            assert_eq!(denied.code(), codes::HUMAN_REQUIRED);
            assert!(denied.hint().contains("fld record"));
        }
    }

    #[test]
    fn only_a_human_stashes_the_project_folder() {
        assert!(can_stash(&Actor::human(HumanName::parse("ana").unwrap())).is_ok());
        let agent = Actor::Agent(AgentActor {
            agent: AgentName::parse("main").unwrap(),
            worker: WorkerName::parse("main").unwrap(),
            tags: Vec::new(),
        });
        for actor in [agent, Actor::System] {
            let denied = can_stash(&actor).unwrap_err();
            assert_eq!(denied.code(), codes::HUMAN_REQUIRED);
            assert!(denied.hint().contains("fld stash"));
        }
    }

    #[test]
    fn only_a_human_turns_git_on() {
        assert!(can_enable_git(&Actor::human(HumanName::parse("ana").unwrap())).is_ok());
        let agent = Actor::Agent(AgentActor {
            agent: AgentName::parse("main").unwrap(),
            worker: WorkerName::parse("main").unwrap(),
            tags: Vec::new(),
        });
        for actor in [agent, Actor::System] {
            let denied = can_enable_git(&actor).unwrap_err();
            assert_eq!(denied.code(), codes::HUMAN_REQUIRED);
            assert!(denied.hint().contains("fld init --with-git"));
        }
    }

    #[test]
    fn only_the_main_agent_adds_exclude_patterns() {
        use crate::agents::AreaName;
        assert!(can_add_exclude_patterns(&Starter::MainAgent).is_ok());
        for starter in [
            Starter::Human,
            Starter::Unsupervised,
            Starter::AreaLead(AreaName::parse("web").unwrap()),
            Starter::ReviewLead,
            Starter::ShortLived,
            Starter::System,
        ] {
            let denied = can_add_exclude_patterns(&starter).unwrap_err();
            assert_eq!(denied.code(), codes::MAIN_AGENT_REQUIRED, "{starter:?}");
        }
        assert!(
            can_add_exclude_patterns(&Starter::Human)
                .unwrap_err()
                .hint()
                .contains(".fold/exclude")
        );
    }
}
