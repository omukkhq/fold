use thiserror::Error;

use crate::actor::{Actor, AgentActor};
use crate::agents::WorkerName;
use crate::error::{FoldError, codes};
use crate::reviews::{Reviewer, Verdict};
use crate::roles::Role;

mod batches;
mod human_review;
mod integrate;
mod projects;
mod questions;
mod requests;
mod sessions;
mod tasks;
mod workers;

pub use batches::can_review_batch;
pub use human_review::{
    MarkChange, can_clear_human_review, can_human_approve, can_set_human_review,
};
pub use integrate::{
    CallerFacts, Gate, GateRefusal, GateState, GateStatus, IntegrateInput, ProposalFacts,
    can_integrate, gate_report, human_check_done, prerequisites_done,
};
pub use projects::can_pull;
pub use questions::can_answer_worker_permission;
pub use requests::{can_claim_request, can_create_request, is_drafter, is_requester};
pub use sessions::{SessionPeer, can_bind_agent, can_name_human};
pub use tasks::{TaskHolder, TaskStart, can_archive_task, can_delete_task, can_hold_task};
pub use workers::{
    Starter, can_check_task, can_choose_no_proposal, can_clear_hold, can_complete_human_task,
    can_define_area, can_pause_project, can_publish_task_plan, can_set_hold, can_set_worker_limits,
    can_start_worker, can_stop_worker, can_update_area, can_write_handoff,
};

/// The facts behind an agent's rights that its name and tags do not carry, which the services read inside the command's transaction. `role` is the role of the agent's worker process, fixed when fold started it, and `None` for an agent that fold does not supervise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentStanding {
    pub holds_main: bool,
    pub role: Option<Role>,
}

impl AgentStanding {
    /// The standing passed for a human or the system, whose rights no rule reads from a standing: it carries no right of its own.
    pub const NO_RIGHTS: AgentStanding = AgentStanding {
        holds_main: false,
        role: None,
    };

    /// The user's own session, which `fld claude` starts with the `main` tag. While no agent has the tag there is no main agent, and a session opened by hand without a role decides nothing.
    pub fn is_main_agent(self) -> bool {
        self.holds_main
    }

    /// Approving, rejecting and integrating decide a proposal's fate. The main agent and the review lead act for the user, and a batch reviewer reviews for the review lead that started it; an area lead and a task coder only comment and ask for changes.
    pub fn decides_reviews(self) -> bool {
        self.is_main_agent() || matches!(self.role, Some(Role::ReviewLead | Role::BatchReviewer))
    }
}

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

/// Where a session's tags come from: the handshake line of its adapter, which carries `FOLD_AGENT_TAGS` as `fld claude` sets it, or a `register_agent` call by the session itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagOrigin {
    Handshake,
    RegisterTool,
}

/// The main agent is the user's own interactive session, which `fld claude` starts with the `main` tag in its adapter's handshake. So a worker that fold started (`supervised`) never takes `main`, a `register_agent` call never gives it to an agent that lacks it (`holds_main`), and while another agent holds it (`held_by_other`) nobody else takes it. The database holds the last rule too, with a unique index on the tag.
pub fn can_take_main_tag(
    origin: TagOrigin,
    holds_main: bool,
    supervised: bool,
    held_by_other: bool,
) -> Result<(), PolicyDenied> {
    if supervised {
        return Err(PolicyDenied {
            code: codes::MAIN_TAG_DENIED,
            message: "a worker that fold started cannot take the main tag",
            hint: "call register_agent without the main tag",
        });
    }
    if origin == TagOrigin::RegisterTool && !holds_main {
        return Err(PolicyDenied {
            code: codes::MAIN_TAG_DENIED,
            message: "only the session that fld claude starts takes the main tag",
            hint: "call register_agent without the main tag; run fld claude to start the main agent",
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
    owning_worker(
        actor,
        owner,
        "only the owning worker can abandon a workspace",
    )
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
    owning_worker(
        actor,
        owner,
        "only the owning worker can change a workspace",
    )
}

/// Admits only the worker `owner`; anyone else is `not_workspace_owner` with `message`.
fn owning_worker(
    actor: &Actor,
    owner: Option<&WorkerName>,
    message: &'static str,
) -> Result<(), PolicyDenied> {
    match actor {
        Actor::Agent(agent) if owner == Some(&agent.worker) => Ok(()),
        Actor::Agent(_) | Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::NOT_WORKSPACE_OWNER,
            message,
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

/// Closing withdraws a proposal. The author's agent withdraws its own work through any of its workers, and an agent that decides reviews or a human may close any proposal; the system never closes one on anybody's behalf. `standing` is the acting agent's.
pub fn can_close_proposal(
    actor: &Actor,
    standing: AgentStanding,
    author: &WorkerName,
) -> Result<(), PolicyDenied> {
    match actor {
        Actor::Human { .. } => Ok(()),
        Actor::Agent(agent) if author.belongs_to(&agent.agent) || standing.decides_reviews() => {
            Ok(())
        }
        Actor::Agent(_) | Actor::System => Err(PolicyDenied {
            code: codes::NOT_PROPOSAL_AUTHOR,
            message: "only the author's agent, the main agent, the review lead, a batch reviewer or a human can close a proposal",
            hint: "call get_proposal to see the author, or ask the review lead",
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

/// Messages are how agents and humans coordinate, so every registered agent sends them, a worker that fold started and an agent that a session registered with `fld mcp` alike; so does a named human, and fold itself as `fold`. The service checks that the agent's worker exists.
pub fn can_send_message(actor: &Actor) -> Result<(), PolicyDenied> {
    match actor {
        Actor::Agent(_) | Actor::Human { .. } | Actor::System => Ok(()),
    }
}

/// A reviewer judges someone else's work, so no worker of the author's agent reviews it, whatever its role. `comment` and `request_changes` are open to every other agent and to any human; `approve` and `reject` decide the proposal's fate, so they need an agent that [decides reviews](AgentStanding::decides_reviews) or a human. The system never reviews. `standing` is the acting agent's. It returns the admitted reviewer.
pub fn can_submit_verdict<'a>(
    actor: &'a Actor,
    standing: AgentStanding,
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
    if author.belongs_to(&agent.agent) {
        return Err(PolicyDenied {
            code: codes::AUTHOR_CANNOT_REVIEW,
            message: "the author's agent cannot review its own proposal",
            hint: "ask another agent or a human to review it",
        });
    }
    match verdict {
        Verdict::Comment | Verdict::RequestChanges => Ok(Reviewer::Agent(agent)),
        Verdict::Approve | Verdict::Reject if standing.decides_reviews() => {
            Ok(Reviewer::Agent(agent))
        }
        Verdict::Approve | Verdict::Reject => Err(PolicyDenied {
            code: codes::INTEGRATOR_REQUIRED,
            message: "only the main agent, the review lead, a batch reviewer or a human can approve or reject a proposal",
            hint: "submit comment or request_changes, or ask the review lead",
        }),
    }
}

/// A review is a second agent's look at the work, so the author's agent never claims its own proposal through any of its workers. Only an agent's worker holds a claim. It returns the admitted agent, which becomes the reviewer.
pub fn can_claim_review<'a>(
    actor: &'a Actor,
    author: &WorkerName,
) -> Result<&'a AgentActor, PolicyDenied> {
    match actor {
        Actor::Agent(agent) if author.belongs_to(&agent.agent) => Err(PolicyDenied {
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
        Actor::Agent(agent) if holder.belongs_to(&agent.agent) => Ok(agent),
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
        Actor::Agent(agent) if holder.is_some_and(|holder| holder.belongs_to(&agent.agent)) => {
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
    use crate::agents::{HumanName, Tag};

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
            ("review lead", agent("review", &[])),
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

    /// The standing of an agent fold supervises in `role`.
    fn supervised(role: Role) -> AgentStanding {
        AgentStanding {
            holds_main: false,
            role: Some(role),
        }
    }

    /// An agent fold does not supervise, without the `main` tag.
    const UNSUPERVISED: AgentStanding = AgentStanding {
        holds_main: false,
        role: None,
    };

    const MAIN: AgentStanding = AgentStanding {
        holds_main: true,
        role: None,
    };

    /// The standing [`actors`] act with: only `review` is the review lead.
    fn standing_of(case: &str) -> AgentStanding {
        if case == "review lead" {
            supervised(Role::ReviewLead)
        } else {
            UNSUPERVISED
        }
    }

    #[test]
    fn the_main_agent_is_only_the_agent_with_the_main_tag() {
        assert!(MAIN.is_main_agent());
        assert!(!UNSUPERVISED.is_main_agent());
        assert!(!UNSUPERVISED.decides_reviews());
        assert!(!AgentStanding::NO_RIGHTS.decides_reviews());
        assert!(!supervised(Role::AreaLead).is_main_agent());
        let deciders: Vec<Role> = Role::ALL
            .into_iter()
            .filter(|role| supervised(*role).decides_reviews())
            .collect();
        assert_eq!(deciders, [Role::ReviewLead, Role::BatchReviewer]);
        assert!(MAIN.decides_reviews());
        assert!(!UNSUPERVISED.decides_reviews());
    }

    #[test]
    fn the_author_agent_a_deciding_agent_or_a_human_closes_a_proposal() {
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
            let result = can_close_proposal(&actor, standing_of(case), &author);
            assert_eq!(outcome(result), expected, "{case}");
        }
        for (role, expected) in [
            (Role::BatchReviewer, None),
            (Role::AreaLead, Some(codes::NOT_PROPOSAL_AUTHOR)),
            (Role::TaskCoder, Some(codes::NOT_PROPOSAL_AUTHOR)),
        ] {
            let result = can_close_proposal(&agent("frontend", &[]), supervised(role), &author);
            assert_eq!(outcome(result), expected, "{role}");
        }
        assert!(can_close_proposal(&agent("frontend", &[]), MAIN, &author).is_ok());
        let denied =
            can_close_proposal(&agent("frontend", &[]), UNSUPERVISED, &author).unwrap_err();
        assert!(
            denied
                .to_string()
                .contains("the review lead, a batch reviewer or a human")
        );
    }

    #[test]
    fn verdict_rights_follow_the_author_and_the_role() {
        let author = worker("backend/api");
        let cases = [
            ("author", agent("backend/api", &[]), MAIN, None, None),
            (
                "author's other worker",
                agent("backend", &[]),
                supervised(Role::ReviewLead),
                None,
                None,
            ),
            (
                "main agent",
                agent("main", &["main"]),
                MAIN,
                Some(()),
                Some(()),
            ),
            (
                "an unsupervised agent while nobody has the main tag",
                agent("solo", &[]),
                UNSUPERVISED,
                Some(()),
                None,
            ),
            (
                "review lead",
                agent("review-lead", &[]),
                supervised(Role::ReviewLead),
                Some(()),
                Some(()),
            ),
            (
                "batch reviewer",
                agent("reviewer-1", &[]),
                supervised(Role::BatchReviewer),
                Some(()),
                Some(()),
            ),
            (
                "area lead",
                agent("web-lead", &[]),
                supervised(Role::AreaLead),
                Some(()),
                None,
            ),
            (
                "task coder",
                agent("web-coder", &[]),
                supervised(Role::TaskCoder),
                Some(()),
                None,
            ),
            (
                "unsupervised agent",
                agent("frontend", &["review"]),
                UNSUPERVISED,
                Some(()),
                None,
            ),
            (
                "human",
                human(),
                AgentStanding::NO_RIGHTS,
                Some(()),
                Some(()),
            ),
        ];
        for (case, actor, standing, may_comment, may_decide) in cases {
            for verdict in Verdict::ALL {
                let result = can_submit_verdict(&actor, standing, verdict, &author);
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
            let result = can_submit_verdict(&Actor::System, MAIN, verdict, &author);
            assert_eq!(
                outcome(result),
                Some(codes::HUMAN_REQUIRED),
                "system {verdict}"
            );
        }
        let other = agent("frontend", &[]);
        let admitted =
            can_submit_verdict(&other, UNSUPERVISED, Verdict::RequestChanges, &author).unwrap();
        assert!(matches!(admitted, Reviewer::Agent(agent) if agent.agent.as_str() == "frontend"));
        let tanim = human();
        let admitted = can_submit_verdict(&tanim, UNSUPERVISED, Verdict::Reject, &author).unwrap();
        assert!(matches!(admitted, Reviewer::Human(name) if name.as_str() == "tanim"));
        let denied =
            can_submit_verdict(&other, UNSUPERVISED, Verdict::Approve, &author).unwrap_err();
        assert!(denied.hint().contains("review lead"));
    }

    #[test]
    fn every_agent_a_human_and_fold_send_a_message() {
        for actor in [
            agent("frontend/ui", &[]),
            agent("main", &["main"]),
            human(),
            Actor::System,
        ] {
            assert_eq!(can_send_message(&actor), Ok(()), "{actor:?}");
        }
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
