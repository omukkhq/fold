use thiserror::Error;

use crate::actor::{Actor, AgentActor};
use crate::agents::{Tag, WorkerName};
use crate::error::{FoldError, codes};
use crate::reviews::{Reviewer, Verdict};

mod batches;
mod human_review;
mod integrate;
mod questions;
mod requests;
mod workers;

pub use batches::can_review_batch;
pub use human_review::{MarkChange, can_clear_human_review, can_set_human_review};
pub use integrate::{
    CallerFacts, Gate, GateRefusal, GateState, GateStatus, IntegrateInput, ProposalFacts,
    can_integrate, gate_report,
};
pub use questions::can_answer_worker_permission;
pub use requests::{can_claim_request, can_create_request, is_drafter, is_requester};
pub use workers::{
    Starter, can_define_area, can_set_worker_limits, can_start_worker, can_stop_worker,
    can_write_handoff,
};

/// A refusal by a policy gate. The code is a stable error code, so every transport reports it unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct PolicyDenied {
    code: &'static str,
    message: &'static str,
    hint: &'static str,
}

impl PolicyDenied {
    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn hint(&self) -> &'static str {
        self.hint
    }
}

impl From<PolicyDenied> for FoldError {
    fn from(denied: PolicyDenied) -> Self {
        FoldError::new(denied.code, denied.message).with_hint(denied.hint)
    }
}

/// Only an agent's worker holds a claim, because a claim names the worker whose workspace does the work. A human or the system works on a task through an agent. It returns the admitted agent, which is the claimant.
pub fn can_claim(actor: &Actor) -> Result<&AgentActor, PolicyDenied> {
    match actor {
        Actor::Agent(agent) => Ok(agent),
        Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::ACTOR_CANNOT_CLAIM,
            message: "only an agent's worker can claim a task",
            hint: "ask an agent to claim the task with claim_task",
        }),
    }
}

/// A push publishes main to a shared Git remote with the user's own Git credentials, so a human runs it, or the daemon itself when it pushes after an integration. An agent never pushes.
pub fn can_push(actor: &Actor) -> Result<(), PolicyDenied> {
    match actor {
        Actor::Human { .. } | Actor::System => Ok(()),
        Actor::Agent(_) => Err(PolicyDenied {
            code: codes::HUMAN_REQUIRED,
            message: "only a human can push main to the Git remote",
            hint: "ask a human to run fld push",
        }),
    }
}

/// Only a human grants or removes `integrator`, because the integrator moves main. Any other tag is a label that an agent sets on itself at registration, and `main` also passes [`can_take_main_tag`].
pub fn can_set_tag(actor: &Actor, tag: &Tag, add: bool) -> Result<(), PolicyDenied> {
    if !tag.is_integrator() || actor.is_human() {
        return Ok(());
    }
    Err(PolicyDenied {
        code: codes::HUMAN_REQUIRED,
        message: if add {
            "only a human can grant the integrator tag"
        } else {
            "only a human can remove the integrator tag"
        },
        hint: "ask a human to run fld agent tag <agent> integrator",
    })
}

/// The main agent is the user's own interactive session, so a worker that fold started (`supervised`) never takes `main`, and while another agent holds it (`held_by_other`) nobody else does. The database holds the second rule too, with a unique index on the tag.
pub fn can_take_main_tag(supervised: bool, held_by_other: bool) -> Result<(), PolicyDenied> {
    if supervised {
        return Err(PolicyDenied {
            code: codes::MAIN_TAG_DENIED,
            message: "a worker that fold started cannot take the main tag",
            hint: "call register_agent without the main tag",
        });
    }
    if held_by_other {
        return Err(PolicyDenied {
            code: codes::MAIN_TAG_DENIED,
            message: "another agent holds the main tag",
            hint: "call register_agent without the main tag, and send_message to the main agent",
        });
    }
    Ok(())
}

/// Only the worker that owns a workspace abandons it, because abandoning gives up that worker's own work. `owner` is the workspace's worker; a human, the system and every other worker are refused.
pub fn can_abandon_workspace(
    actor: &Actor,
    owner: Option<&WorkerName>,
) -> Result<(), PolicyDenied> {
    match actor {
        Actor::Agent(agent) if owner == Some(&agent.worker) => Ok(()),
        Actor::Agent(_) | Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::NOT_WORKSPACE_OWNER,
            message: "only the owning worker can abandon a workspace",
            hint: "call get_workspace to see the owner",
        }),
    }
}

/// Feedback is the human path onto a task: it is stored as a message from the named human, so an agent or the system sends an ordinary message instead.
pub fn can_send_feedback(actor: &Actor) -> Result<(), PolicyDenied> {
    if actor.is_human() {
        return Ok(());
    }
    Err(PolicyDenied {
        code: codes::HUMAN_REQUIRED,
        message: "only a human writes feedback on a task",
        hint: "send a message linked to the task with send_message",
    })
}

/// Only the worker that owns a workspace records, restores or syncs in it, because a workspace holds that one worker's unfinished work. `owner` is the workspace's worker; a human, the system and every other worker are refused.
pub fn can_change_workspace(actor: &Actor, owner: Option<&WorkerName>) -> Result<(), PolicyDenied> {
    match actor {
        Actor::Agent(agent) if owner == Some(&agent.worker) => Ok(()),
        Actor::Agent(_) | Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::NOT_WORKSPACE_OWNER,
            message: "only the owning worker can change a workspace",
            hint: "call get_workspace to see the owner",
        }),
    }
}

/// An automatic checkpoint only reads the files, so besides the owning worker the daemon itself takes one, as the quiet-period job does. A human and every other worker are refused.
pub fn can_take_auto_checkpoint(
    actor: &Actor,
    owner: Option<&WorkerName>,
) -> Result<(), PolicyDenied> {
    match actor {
        Actor::System => Ok(()),
        _ => can_change_workspace(actor, owner),
    }
}

/// A proposal offers the work done under the implementation claim, so only the worker that holds the claim opens one. `holder` is the claim's worker; a human, the system, another worker and an unclaimed task are refused. It returns the admitted agent, which becomes the author.
pub fn can_open_proposal<'a>(
    actor: &'a Actor,
    holder: Option<&WorkerName>,
) -> Result<&'a AgentActor, PolicyDenied> {
    match actor {
        Actor::Agent(agent) if holder == Some(&agent.worker) => Ok(agent),
        Actor::Agent(_) | Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::NOT_CLAIM_HOLDER,
            message: "only the worker that holds the implementation claim can open a proposal",
            hint: "call claim_task and create_workspace, then create_proposal",
        }),
    }
}

/// A version is the author's own work, so only the author's worker adds one; another worker of the same agent is refused too.
pub fn can_update_proposal<'a>(
    actor: &'a Actor,
    author: &WorkerName,
) -> Result<&'a AgentActor, PolicyDenied> {
    match actor {
        Actor::Agent(agent) if agent.worker == *author => Ok(agent),
        Actor::Agent(_) | Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::NOT_PROPOSAL_AUTHOR,
            message: "only the author's worker can add a version",
            hint: "call get_proposal to see the author",
        }),
    }
}

/// Closing withdraws a proposal. The author's agent withdraws its own work through any of its workers, and an integrator or a human may close any proposal; the system never closes one on anybody's behalf.
pub fn can_close_proposal(actor: &Actor, author: &WorkerName) -> Result<(), PolicyDenied> {
    match actor {
        Actor::Human { .. } => Ok(()),
        Actor::Agent(agent)
            if agent.agent == author.agent() || agent.tags.iter().any(Tag::is_integrator) =>
        {
            Ok(())
        }
        Actor::Agent(_) | Actor::System => Err(PolicyDenied {
            code: codes::NOT_PROPOSAL_AUTHOR,
            message: "only the author's agent, an integrator or a human can close a proposal",
            hint: "call get_proposal to see the author, or ask an integrator",
        }),
    }
}

/// Only an agent's worker has an inbox, because a recipient row that delivery and acknowledgement change names a worker. A human reads and answers messages through the thread listing instead.
pub fn can_use_inbox(actor: &Actor) -> Result<&AgentActor, PolicyDenied> {
    match actor {
        Actor::Agent(agent) => Ok(agent),
        Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::INVALID_REQUEST,
            message: "only an agent's worker has an inbox",
            hint: "read the threads on the Messages screen or with GET /messages",
        }),
    }
}

/// A reviewer judges someone else's work, so no worker of the author's agent reviews it, whatever its tags. `comment` and `request_changes` are open to every other agent and to any human; `approve` and `reject` decide the proposal's fate, so they need the `integrator` tag or a human. The system never reviews. It returns the admitted reviewer.
pub fn can_submit_verdict<'a>(
    actor: &'a Actor,
    verdict: Verdict,
    author: &WorkerName,
) -> Result<Reviewer<'a>, PolicyDenied> {
    let agent = match actor {
        Actor::Human { name } => return Ok(Reviewer::Human(name)),
        Actor::System => {
            return Err(PolicyDenied {
                code: codes::HUMAN_REQUIRED,
                message: "only an agent or a human can review a proposal",
                hint: "ask an agent or a human to call submit_review",
            });
        }
        Actor::Agent(agent) => agent,
    };
    if agent.agent == author.agent() {
        return Err(PolicyDenied {
            code: codes::AUTHOR_CANNOT_REVIEW,
            message: "the author's agent cannot review its own proposal",
            hint: "ask another agent or a human to review it",
        });
    }
    match verdict {
        Verdict::Comment | Verdict::RequestChanges => Ok(Reviewer::Agent(agent)),
        Verdict::Approve | Verdict::Reject if agent.tags.iter().any(Tag::is_integrator) => {
            Ok(Reviewer::Agent(agent))
        }
        Verdict::Approve | Verdict::Reject => Err(PolicyDenied {
            code: codes::INTEGRATOR_REQUIRED,
            message: "only an integrator or a human can approve or reject a proposal",
            hint: "submit comment or request_changes, or ask an integrator",
        }),
    }
}

/// A review is a second agent's look at the work, so the author's agent never claims its own proposal through any of its workers. Only an agent's worker holds a claim. It returns the admitted agent, which becomes the reviewer.
pub fn can_claim_review<'a>(
    actor: &'a Actor,
    author: &WorkerName,
) -> Result<&'a AgentActor, PolicyDenied> {
    match actor {
        Actor::Agent(agent) if agent.agent == author.agent() => Err(PolicyDenied {
            code: codes::AUTHOR_CANNOT_REVIEW,
            message: "an agent cannot review its own proposal",
            hint: "ask another agent to call claim_review",
        }),
        Actor::Agent(agent) => Ok(agent),
        Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::ACTOR_CANNOT_CLAIM,
            message: "only an agent's worker can claim a review",
            hint: "ask an agent to claim the review with claim_review",
        }),
    }
}

/// A review claim is the reviewing agent's, so any worker of the holder's agent gives it up; a human, the system and every other agent are refused. `holder` is the claim's worker.
pub fn can_release_review<'a>(
    actor: &'a Actor,
    holder: &WorkerName,
) -> Result<&'a AgentActor, PolicyDenied> {
    match actor {
        Actor::Agent(agent) if agent.agent == holder.agent() => Ok(agent),
        Actor::Agent(_) | Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::NOT_CLAIM_HOLDER,
            message: "only the agent that holds the review claim can release it",
            hint: "call get_proposal to see the reviewer",
        }),
    }
}

/// A review checkout serves the review the claim holds, so only the holder's agent creates one, through any of its workers: each review worker gets a directory of its own. `holder` is the claim's worker; an unclaimed proposal is refused too.
pub fn can_create_review_checkout<'a>(
    actor: &'a Actor,
    holder: Option<&WorkerName>,
) -> Result<&'a AgentActor, PolicyDenied> {
    match actor {
        Actor::Agent(agent) if holder.is_some_and(|holder| agent.agent == holder.agent()) => {
            Ok(agent)
        }
        Actor::Agent(_) | Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::REVIEW_CLAIM_REQUIRED,
            message: "only the agent that holds the review claim can check out the proposal",
            hint: "call claim_review, then create_review_checkout",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::HumanName;

    fn agent(worker: &str, tags: &[&str]) -> Actor {
        let worker = WorkerName::parse(worker).unwrap();
        Actor::Agent(AgentActor {
            agent: worker.agent(),
            worker,
            tags: tags.iter().map(|tag| Tag::parse(tag).unwrap()).collect(),
        })
    }

    fn worker(name: &str) -> WorkerName {
        WorkerName::parse(name).unwrap()
    }

    fn human() -> Actor {
        Actor::human(HumanName::parse("tanim").unwrap())
    }

    /// The author is `backend/api`; `backend` is another worker of its agent.
    fn actors() -> [(&'static str, Actor); 6] {
        [
            ("author", agent("backend/api", &[])),
            ("author's other worker", agent("backend", &[])),
            ("other worker", agent("frontend", &[])),
            ("integrator", agent("review", &["integrator"])),
            ("human", human()),
            ("system", Actor::System),
        ]
    }

    fn outcome<T>(result: Result<T, PolicyDenied>) -> Option<&'static str> {
        result.err().map(|denied| denied.code())
    }

    #[test]
    fn only_the_claim_holder_opens_a_proposal() {
        let holder = worker("backend/api");
        let expected = [
            None,
            Some(codes::NOT_CLAIM_HOLDER),
            Some(codes::NOT_CLAIM_HOLDER),
            Some(codes::NOT_CLAIM_HOLDER),
            Some(codes::NOT_CLAIM_HOLDER),
            Some(codes::NOT_CLAIM_HOLDER),
        ];
        for ((case, actor), expected) in actors().into_iter().zip(expected) {
            let result = can_open_proposal(&actor, Some(&holder));
            assert_eq!(outcome(result), expected, "{case}");
        }
        let author = agent("backend/api", &[]);
        let admitted = can_open_proposal(&author, Some(&holder)).unwrap();
        assert_eq!(admitted.worker, holder);
        for (case, actor) in actors() {
            let result = can_open_proposal(&actor, None);
            assert_eq!(
                outcome(result),
                Some(codes::NOT_CLAIM_HOLDER),
                "{case} on an unclaimed task"
            );
        }
        let denied = can_open_proposal(&human(), Some(&holder)).unwrap_err();
        assert!(denied.hint().contains("claim_task"));
    }

    #[test]
    fn only_the_author_worker_updates_a_proposal() {
        let author = worker("backend/api");
        let expected = [
            None,
            Some(codes::NOT_PROPOSAL_AUTHOR),
            Some(codes::NOT_PROPOSAL_AUTHOR),
            Some(codes::NOT_PROPOSAL_AUTHOR),
            Some(codes::NOT_PROPOSAL_AUTHOR),
            Some(codes::NOT_PROPOSAL_AUTHOR),
        ];
        for ((case, actor), expected) in actors().into_iter().zip(expected) {
            let result = can_update_proposal(&actor, &author);
            assert_eq!(outcome(result), expected, "{case}");
        }
        let actor = agent("backend/api", &[]);
        let admitted = can_update_proposal(&actor, &author).unwrap();
        assert_eq!(admitted.worker, author);
    }

    #[test]
    fn the_author_agent_an_integrator_or_a_human_closes_a_proposal() {
        let author = worker("backend/api");
        let expected = [
            None,
            None,
            Some(codes::NOT_PROPOSAL_AUTHOR),
            None,
            None,
            Some(codes::NOT_PROPOSAL_AUTHOR),
        ];
        for ((case, actor), expected) in actors().into_iter().zip(expected) {
            let result = can_close_proposal(&actor, &author);
            assert_eq!(outcome(result), expected, "{case}");
        }
        let denied = can_close_proposal(&agent("frontend", &["review"]), &author).unwrap_err();
        assert_eq!(
            denied.to_string(),
            "only the author's agent, an integrator or a human can close a proposal"
        );
    }

    #[test]
    fn verdict_rights_follow_the_author_and_the_integrator_tag() {
        let author = worker("backend/api");
        let author_integrator = agent("backend/ci", &["integrator"]);
        let cases = [
            ("author", agent("backend/api", &[]), None, None),
            ("author's other worker", agent("backend", &[]), None, None),
            ("author-integrator", author_integrator, None, None),
            ("other agent", agent("frontend", &[]), Some(()), None),
            (
                "integrator",
                agent("review", &["integrator"]),
                Some(()),
                Some(()),
            ),
            ("human", human(), Some(()), Some(())),
        ];
        for (case, actor, may_comment, may_decide) in cases {
            for verdict in Verdict::ALL {
                let result = can_submit_verdict(&actor, verdict, &author);
                let allowed = match verdict {
                    Verdict::Comment | Verdict::RequestChanges => may_comment,
                    Verdict::Approve | Verdict::Reject => may_decide,
                };
                let expected = match (allowed, case.starts_with("author")) {
                    (Some(()), _) => None,
                    (None, true) => Some(codes::AUTHOR_CANNOT_REVIEW),
                    (None, false) => Some(codes::INTEGRATOR_REQUIRED),
                };
                assert_eq!(outcome(result), expected, "{case} {verdict}");
            }
        }
        for verdict in Verdict::ALL {
            let result = can_submit_verdict(&Actor::System, verdict, &author);
            assert_eq!(
                outcome(result),
                Some(codes::HUMAN_REQUIRED),
                "system {verdict}"
            );
        }
        let other = agent("frontend", &[]);
        let admitted = can_submit_verdict(&other, Verdict::RequestChanges, &author).unwrap();
        assert!(matches!(admitted, Reviewer::Agent(agent) if agent.agent.as_str() == "frontend"));
        let tanim = human();
        let admitted = can_submit_verdict(&tanim, Verdict::Reject, &author).unwrap();
        assert!(matches!(admitted, Reviewer::Human(name) if name.as_str() == "tanim"));
        let denied = can_submit_verdict(&other, Verdict::Approve, &author).unwrap_err();
        assert!(denied.hint().contains("integrator"));
    }

    #[test]
    fn the_author_agent_never_claims_its_own_review() {
        let author = worker("backend/api");
        let expected = [
            Some(codes::AUTHOR_CANNOT_REVIEW),
            Some(codes::AUTHOR_CANNOT_REVIEW),
            None,
            None,
            Some(codes::ACTOR_CANNOT_CLAIM),
            Some(codes::ACTOR_CANNOT_CLAIM),
        ];
        for ((case, actor), expected) in actors().into_iter().zip(expected) {
            let result = can_claim_review(&actor, &author);
            assert_eq!(outcome(result), expected, "{case}");
        }
        let reviewer = agent("frontend", &[]);
        let admitted = can_claim_review(&reviewer, &author).unwrap();
        assert_eq!(admitted.worker, worker("frontend"));
    }

    #[test]
    fn only_the_holder_agent_releases_a_review_claim() {
        let holder = worker("frontend/r1");
        let expected = [
            Some(codes::NOT_CLAIM_HOLDER),
            Some(codes::NOT_CLAIM_HOLDER),
            None,
            Some(codes::NOT_CLAIM_HOLDER),
            Some(codes::NOT_CLAIM_HOLDER),
            Some(codes::NOT_CLAIM_HOLDER),
        ];
        for ((case, actor), expected) in actors().into_iter().zip(expected) {
            let result = can_release_review(&actor, &holder);
            assert_eq!(outcome(result), expected, "{case}");
        }
    }

    #[test]
    fn only_the_holder_agent_checks_out_a_proposal() {
        let holder = worker("frontend/r1");
        let expected = [
            Some(codes::REVIEW_CLAIM_REQUIRED),
            Some(codes::REVIEW_CLAIM_REQUIRED),
            None,
            Some(codes::REVIEW_CLAIM_REQUIRED),
            Some(codes::REVIEW_CLAIM_REQUIRED),
            Some(codes::REVIEW_CLAIM_REQUIRED),
        ];
        for ((case, actor), expected) in actors().into_iter().zip(expected) {
            let result = can_create_review_checkout(&actor, Some(&holder));
            assert_eq!(outcome(result), expected, "{case}");
        }
        for (case, actor) in actors() {
            let result = can_create_review_checkout(&actor, None);
            assert_eq!(
                outcome(result),
                Some(codes::REVIEW_CLAIM_REQUIRED),
                "{case} on an unclaimed proposal"
            );
        }
    }
}
