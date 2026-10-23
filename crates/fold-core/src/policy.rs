use thiserror::Error;

use crate::actor::{Actor, AgentActor};
use crate::agents::roles::Role;
use crate::agents::{HumanName, WorkerName};
use crate::error::{FoldError, codes};
use crate::reviews::{Reviewer, Verdict};
use crate::tasks::LabelName;

mod batches;
mod human_review;
mod integrate;
mod projects;
mod questions;
mod requests;
mod sessions;
mod tasks;
mod workers;

pub use batches::{can_change_batch, can_change_plan, can_plan_reviews, can_review_batch};
pub use human_review::{
    CHECKS_FILE_REASON, MAIN_AREA_REASON, MarkChange, can_clear_human_review, can_edit_as_human,
    can_human_approve, can_set_human_review, checks_file_mark, main_area_mark, main_area_unmark,
};
pub use integrate::{
    CallerFacts, Gate, GateRefusal, GateState, GateStatus, IntegrateInput, ProposalFacts,
    can_integrate, gate_report, human_check_done, prerequisites_done,
};
pub use projects::{
    can_add_exclude_patterns, can_enable_git, can_pull, can_record, can_remove_project, can_stash,
};
pub use questions::{can_answer_worker_permission, can_close_ask};
pub use requests::{can_claim_request, can_create_request, is_drafter, is_requester};
pub use sessions::{SessionPeer, can_bind_agent, can_name_human};
pub use tasks::{
    TaskHolder, TaskStart, can_archive_task, can_change_checklist, can_delete_task, can_hold_task,
};
pub use workers::{
    Starter, can_change_main_directly, can_check_task, can_choose_no_proposal, can_clear_hold,
    can_complete_human_task, can_define_area, can_pause_project, can_publish_task_plan,
    can_restart_worker, can_set_hold, can_set_worker_limits, can_start_worker, can_stop_worker,
    can_update_area, can_write_handoff,
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

    /// The user's own session, which `fld claude` starts with the `main` tag, or the supervised main, which fold runs with the tag. While no agent has the tag there is no main agent, and a session opened by hand without a role decides nothing.
    pub fn is_main_agent(self) -> bool {
        self.holds_main || self.role == Some(Role::Main)
    }

    /// Approving and rejecting decide a proposal's fate. The main agent acts for the user, and a reviewer reviews for the review lead that started it; an area lead and a task worker only comment and ask for changes. The review lead assigns every review to a reviewer it starts and never reviews itself ([`can_review`]): a long-lived lead that reads every diff fills its context and costs many times what short-lived reviewers with fresh contexts cost for the same proposals, so the lead only chooses who reviews what.
    pub fn decides_reviews(self) -> bool {
        self.is_main_agent() || self.role == Some(Role::Reviewer)
    }

    /// Integrating and closing act on a proposal's verdicts, and an approval counts toward integration, for the agents that [decide reviews](Self::decides_reviews) and for the review lead, which acts on the verdicts of the reviewers it assigns. fold refuses every verdict of the review lead, so the approvals of the review lead that count are those a project stored earlier.
    pub fn acts_on_reviews(self) -> bool {
        self.decides_reviews() || self.role == Some(Role::ReviewLead)
    }
}

/// The facts of a proposal's task that decide who reviews it. A task in the built-in area Main is the main agent's own preparation work, so no review agent reviews it: a human approves its proposal, and the main agent approves only while the task has no human review mark. fold puts the mark on every task that comes into Main and only a human takes it off, so its absence is the user's word for that task; a plan the user agreed to can leave it off with `main_approves`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReviewTask {
    /// The task carries the label of Main.
    pub main: bool,
    /// The task has the human review mark.
    pub marked: bool,
}

impl ReviewTask {
    /// The main agent decides the proposal of this task itself.
    pub fn main_approves(self) -> bool {
        self.main && !self.marked
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

/// Leads hand out the work: an area lead claims a task that carries its area's label (`lead_label`, the label of the area it leads), usually through the `start_worker` that gives the claim to a new task worker, and the main agent claims one when the user tells it to, or a task of the area main, which it builds itself. A worker that fold started never takes work of its own, so an agent whose worker process is a task worker or a reviewer is refused through every one of its workers, and the review lead and a session without a fold role are refused too. `task_labels` are the task's labels. It returns the admitted agent, which is the claimant.
pub fn can_claim_task<'a>(
    actor: &'a Actor,
    standing: AgentStanding,
    lead_label: Option<&LabelName>,
    task_labels: &[LabelName],
) -> Result<&'a AgentActor, PolicyDenied> {
    let agent = can_claim(actor)?;
    if standing.is_main_agent() {
        return Ok(agent);
    }
    match standing.role {
        Some(Role::TaskWorker | Role::Reviewer) => Err(worker_claim_denied()),
        Some(Role::AreaLead) if lead_label.is_some_and(|label| task_labels.contains(label)) => {
            Ok(agent)
        }
        Some(Role::AreaLead) => Err(PolicyDenied {
            code: codes::TASK_OUTSIDE_AREA,
            message: "an area lead claims only a task of its own area",
            hint: "call get_task to see the task's labels, and start a task worker with start_worker for a task of your area",
        }),
        Some(Role::ReviewLead | Role::Main) | None => Err(PolicyDenied {
            code: codes::ORCHESTRATOR_REQUIRED,
            message: "only the lead of the task's area or the main agent claims a task",
            hint: "ask the lead of the task's area to start a task worker for it with start_worker",
        }),
    }
}

/// The review lead assigns every review to a reviewer with `start_worker` and never claims, checks out, fixes or gives a verdict itself, for the reason [`AgentStanding::decides_reviews`] gives; every other agent goes on to the command's own rules. `standing` is the acting agent's.
pub fn can_review(standing: AgentStanding) -> Result<(), PolicyDenied> {
    if standing.role == Some(Role::ReviewLead) {
        return Err(review_lead_delegates());
    }
    Ok(())
}

fn review_lead_delegates() -> PolicyDenied {
    PolicyDenied {
        code: codes::REVIEW_LEAD_DELEGATES,
        message: "the review lead assigns each review to a reviewer and never reviews itself",
        hint: "call start_worker with role reviewer and the proposal ids; fold gives the reviewer the review claim, or a batch of a group",
    }
}

/// The refusal of a claim by a worker that fold started for its lead's work.
fn worker_claim_denied() -> PolicyDenied {
    PolicyDenied {
        code: codes::WORKER_CLAIM_DENIED,
        message: "a worker that fold started never claims work; its lead gives it the claims when it starts it",
        hint: "work on the tasks or proposals named in your first turn, and send_message to your lead for other work",
    }
}

/// A push publishes main to a shared Git remote with the user's own Git credentials, so a human runs it, or the daemon itself when it pushes after an integration. An agent never pushes.
pub fn can_push(actor: &Actor) -> Result<(), PolicyDenied> {
    match actor {
        Actor::Human { .. } | Actor::System => Ok(()),
        Actor::Agent(_) => Err(PolicyDenied {
            code: codes::HUMAN_REQUIRED,
            message: "only a human can push main to the Git remote",
            hint: "ask a human to run `fld push`",
        }),
    }
}

/// Where a session's tags come from: the handshake line of its adapter, which carries `FOLD_AGENT_TAGS` as `fld claude` sets it, or a `register_agent` call by the session itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagOrigin {
    Handshake,
    RegisterTool,
}

/// The main agent is the user's own interactive session, which `fld claude` starts with the `main` tag in its adapter's handshake, or the supervised main, which fold gives the tag when it first starts it. So a worker that fold started in another role (`supervised`) never takes `main`, a `register_agent` call never gives it to an agent that lacks it (`holds_main`), and while another agent holds it (`held_by_other`) nobody else takes it. The database holds the last rule too, with a unique index on the tag.
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
            hint: "call register_agent without the main tag; run `fld claude` to start the main agent",
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

/// The checks run in a task workspace and their commands write into it, so only the worker that owns the workspace asks fold to run them there.
pub fn can_run_checks(actor: &Actor, owner: Option<&WorkerName>) -> Result<(), PolicyDenied> {
    owning_worker(
        actor,
        owner,
        "only the owning worker can run the checks in a workspace",
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

/// Closing withdraws a proposal. The author's agent withdraws its own work, and an agent that [acts on reviews](AgentStanding::acts_on_reviews) or a human may close any proposal; the system never closes one on anybody's behalf. `standing` is the acting agent's.
pub fn can_close_proposal(
    actor: &Actor,
    standing: AgentStanding,
    author: &WorkerName,
) -> Result<(), PolicyDenied> {
    match actor {
        Actor::Human { .. } => Ok(()),
        Actor::Agent(agent) if author.belongs_to(&agent.agent) || standing.acts_on_reviews() => {
            Ok(())
        }
        Actor::Agent(_) | Actor::System => Err(PolicyDenied {
            code: codes::NOT_PROPOSAL_AUTHOR,
            message: "only the author's agent, the main agent, the review lead, a reviewer or a human can close a proposal",
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

/// A human marks only their own recipient rows read, because a row's read time states what that one human has seen. An agent acknowledges through its worker's inbox, and the system reads nothing. It returns the human whose rows change.
pub fn can_mark_read(actor: &Actor) -> Result<&HumanName, PolicyDenied> {
    match actor {
        Actor::Human { name } => Ok(name),
        Actor::Agent(_) | Actor::System => Err(PolicyDenied {
            code: codes::HUMAN_REQUIRED,
            message: "only a human marks a thread read",
            hint: "call acknowledge_message to acknowledge a message of the inbox",
        }),
    }
}

/// Messages are how agents and humans coordinate, so every registered agent sends them, a worker that fold started and an agent that a session registered with `fld mcp` alike; so does a named human, and fold itself as `fold`. The service checks that the agent's worker exists.
pub fn can_send_message(actor: &Actor) -> Result<(), PolicyDenied> {
    match actor {
        Actor::Agent(_) | Actor::Human { .. } | Actor::System => Ok(()),
    }
}

/// A reviewer judges someone else's work, so no worker of the author's agent reviews it, whatever its role, and the review lead gives no verdict at all ([`can_review`]). `comment` and `request_changes` are open to every other agent and to any human; `approve` and `reject` decide the proposal's fate, so they need an agent that [decides reviews](AgentStanding::decides_reviews) or a human. The system never reviews. On a task of Main (`task`) only a human decides, or the main agent while the task has no human review mark, [its own proposal](ReviewTask::main_approves) included; every other agent is refused with `main_area`. `standing` is the acting agent's. It returns the admitted reviewer.
pub fn can_submit_verdict<'a>(
    actor: &'a Actor,
    standing: AgentStanding,
    verdict: Verdict,
    author: &WorkerName,
    task: ReviewTask,
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
    let main_decides = task.main_approves() && standing.is_main_agent();
    if author.belongs_to(&agent.agent) {
        if main_decides && verdict == Verdict::Approve {
            return Ok(Reviewer::Agent(agent));
        }
        if task.main {
            return Err(PolicyDenied {
                code: codes::AUTHOR_CANNOT_REVIEW,
                message: "the main agent approves its own proposal of a task of main only when the task has no human review mark",
                hint: "a human approves it, or clears the task's human review mark when you may approve it yourself; call wait_for_events for the review",
            });
        }
        return Err(PolicyDenied {
            code: codes::AUTHOR_CANNOT_REVIEW,
            message: "the author's agent cannot review its own proposal",
            hint: "ask another agent or a human to review it",
        });
    }
    can_review(standing)?;
    match verdict {
        Verdict::Comment | Verdict::RequestChanges => Ok(Reviewer::Agent(agent)),
        Verdict::Approve | Verdict::Reject if main_decides => Ok(Reviewer::Agent(agent)),
        Verdict::Approve | Verdict::Reject if task.main => Err(main_review_denied()),
        Verdict::Approve | Verdict::Reject if standing.decides_reviews() => {
            Ok(Reviewer::Agent(agent))
        }
        Verdict::Approve | Verdict::Reject => Err(PolicyDenied {
            code: codes::INTEGRATOR_REQUIRED,
            message: "only the main agent, a reviewer or a human can approve or reject a proposal",
            hint: "submit comment or request_changes, or ask the review lead",
        }),
    }
}

/// The author's update of a proposal, or a reviewer's fix of it, whose files have proposal checks runs them before fold makes the next version, which starts unapproved. An approval of the current version while that run is in progress would let the older version integrate first, which ends the run as `workspace_changed`, so `approve` waits for the run to end (`update_running`). `comment`, `request_changes` and `reject` stay open: none of them lets a version integrate.
pub fn can_review_while_updating(
    verdict: Verdict,
    update_running: bool,
) -> Result<(), PolicyDenied> {
    if verdict == Verdict::Approve && update_running {
        return Err(PolicyDenied {
            code: codes::UPDATE_CHECKS_RUNNING,
            message: "an update or a reviewer's fix of the proposal runs the checks of its next version, so the current version cannot be approved",
            hint: "wait for checks.finished with wait_for_events, then call get_proposal: review the new version, or the current one again when the checks failed",
        });
    }
    Ok(())
}

/// An integration of the current version while an update or a reviewer's fix of the proposal runs the checks of its next version (`update_running`) would end that run as `workspace_changed`, and a human's integration approves the version too, so no caller integrates it until the run ends, as [`can_review_while_updating`] holds back an approval.
pub fn can_integrate_while_updating(update_running: bool) -> Result<(), PolicyDenied> {
    if update_running {
        return Err(PolicyDenied {
            code: codes::UPDATE_CHECKS_RUNNING,
            message: "an update or a reviewer's fix of the proposal runs the checks of its next version, so the current version cannot be integrated",
            hint: "wait for checks.finished with wait_for_events, then call get_proposal: review the new version, or integrate the current one again when the checks failed",
        });
    }
    Ok(())
}

/// A review is a second agent's look at the work, so the author's agent never claims its own proposal. A claim lets its holder check out, fix and verdict the proposal for the user, so only the main agent claims one, for a review of its own. The review lead gives a reviewer its claims when it starts it and never claims ([`can_review`]), and the reviewer never claims. An area lead or a task worker asks the review lead instead. Nobody claims the review of a task of Main (`main_task`), as [`can_assign_review`] says. Only an agent's worker holds a claim. `standing` is the acting agent's. It returns the admitted agent, which becomes the reviewer.
pub fn can_claim_review<'a>(
    actor: &'a Actor,
    standing: AgentStanding,
    author: &WorkerName,
    main_task: bool,
) -> Result<&'a AgentActor, PolicyDenied> {
    match actor {
        Actor::Agent(agent) if author.belongs_to(&agent.agent) => Err(PolicyDenied {
            code: codes::AUTHOR_CANNOT_REVIEW,
            message: "an agent cannot review its own proposal",
            hint: "ask another agent to call claim_review",
        }),
        Actor::Agent(_) if standing.role == Some(Role::Reviewer) => Err(worker_claim_denied()),
        Actor::Agent(_) if standing.role == Some(Role::ReviewLead) => Err(review_lead_delegates()),
        Actor::Agent(_) if main_task => Err(main_review_denied()),
        Actor::Agent(agent) if standing.decides_reviews() => Ok(agent),
        Actor::Agent(_) => Err(PolicyDenied {
            code: codes::INTEGRATOR_REQUIRED,
            message: "only the main agent claims a review; the review lead assigns one to a reviewer",
            hint: "send_message to the review lead about the proposal",
        }),
        Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::ACTOR_CANNOT_CLAIM,
            message: "only an agent's worker can claim a review",
            hint: "ask an agent to claim the review with claim_review",
        }),
    }
}

/// A task of Main is the main agent's own preparation work, such as the project's checks or its agent notes, so no review agent reviews it: the review lead claims no review of it and starts no reviewer for it, and a human approves it. `main_task` is whether the proposal's task is in Main.
pub fn can_assign_review(main_task: bool) -> Result<(), PolicyDenied> {
    if main_task {
        Err(main_review_denied())
    } else {
        Ok(())
    }
}

fn main_review_denied() -> PolicyDenied {
    PolicyDenied {
        code: codes::MAIN_AREA,
        message: "a proposal of a task of main is approved by a human, or by the main agent when the user said so; no review agent reviews it",
        hint: "leave this proposal to a human; call list_proposals and review the proposals of the other areas",
    }
}

/// The review lead decides how much of a revised version a review reads, so it and the main agent, which keeps the user's rights, choose a review scope, through any of their workers; a reviewer follows the scope it gets, and a human or the system chooses none. `standing` is the acting agent's.
pub fn can_choose_review_scope(actor: &Actor, standing: AgentStanding) -> Result<(), PolicyDenied> {
    match actor {
        Actor::Agent(_) if standing.is_main_agent() || standing.role == Some(Role::ReviewLead) => {
            Ok(())
        }
        Actor::Agent(_) | Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::INTEGRATOR_REQUIRED,
            message: "only the review lead or the main agent chooses a review scope",
            hint: "follow the scope you got, and send_message to the review lead when it does not fit",
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

/// A review checkout serves the review the claim holds, so only the holder's agent creates one: each review worker gets a directory of its own. The review lead never reviews ([`can_review`]), whatever it holds. `holder` is the claim's worker; an unclaimed proposal is refused too. `standing` is the acting agent's.
pub fn can_create_review_checkout<'a>(
    actor: &'a Actor,
    standing: AgentStanding,
    holder: Option<&WorkerName>,
) -> Result<&'a AgentActor, PolicyDenied> {
    can_review(standing)?;
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

/// A reviewer fixes what it finds inside the task's agreed scope itself, in its own review checkout, and the fix becomes the next version under the reviewer's name. Only the agent that holds the review claim makes it, because the fix is part of the review the claim holds; the author's agent never does, since its change goes through `update_proposal`, and the review lead never reviews ([`can_review`]). A human changes a proposal only through a human edit, and the system never. `holder` is the claim's worker, `author` the proposal's author and `standing` the acting agent's. It returns the admitted reviewer, which becomes the new version's author.
pub fn can_fix_in_review<'a>(
    actor: &'a Actor,
    standing: AgentStanding,
    holder: Option<&WorkerName>,
    author: &WorkerName,
) -> Result<&'a AgentActor, PolicyDenied> {
    match actor {
        Actor::Agent(agent) if author.belongs_to(&agent.agent) => Err(PolicyDenied {
            code: codes::AUTHOR_CANNOT_REVIEW,
            message: "the author's agent cannot fix its own proposal as a reviewer",
            hint: "change the task workspace, then call update_proposal",
        }),
        Actor::Agent(_) if standing.role == Some(Role::ReviewLead) => Err(review_lead_delegates()),
        Actor::Agent(agent) if holder.is_some_and(|holder| holder.belongs_to(&agent.agent)) => {
            Ok(agent)
        }
        Actor::Agent(_) | Actor::Human { .. } | Actor::System => Err(PolicyDenied {
            code: codes::REVIEW_CLAIM_REQUIRED,
            message: "only the agent that holds the review claim can fix the proposal",
            hint: "call claim_review and create_review_checkout, fix the files there, then call submit_review_fix",
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
        assert_eq!(deciders, [Role::Reviewer, Role::Main]);
        assert!(supervised(Role::Main).is_main_agent());
        assert!(MAIN.decides_reviews());
        assert!(!UNSUPERVISED.decides_reviews());
        let actors: Vec<Role> = Role::ALL
            .into_iter()
            .filter(|role| supervised(*role).acts_on_reviews())
            .collect();
        assert_eq!(actors, [Role::ReviewLead, Role::Reviewer, Role::Main]);
        assert!(MAIN.acts_on_reviews());
        assert!(!UNSUPERVISED.acts_on_reviews());
        assert!(!AgentStanding::NO_RIGHTS.acts_on_reviews());
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
            (Role::Reviewer, None),
            (Role::AreaLead, Some(codes::NOT_PROPOSAL_AUTHOR)),
            (Role::TaskWorker, Some(codes::NOT_PROPOSAL_AUTHOR)),
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
                .contains("the review lead, a reviewer or a human")
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
                "reviewer",
                agent("reviewer-1", &[]),
                supervised(Role::Reviewer),
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
                "task worker",
                agent("web-worker", &[]),
                supervised(Role::TaskWorker),
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
                let result =
                    can_submit_verdict(&actor, standing, verdict, &author, ReviewTask::default());
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
            let result = can_submit_verdict(
                &Actor::System,
                MAIN,
                verdict,
                &author,
                ReviewTask::default(),
            );
            assert_eq!(
                outcome(result),
                Some(codes::HUMAN_REQUIRED),
                "system {verdict}"
            );
        }
        let lead = agent("review-lead", &[]);
        for verdict in Verdict::ALL {
            let result = can_submit_verdict(
                &lead,
                supervised(Role::ReviewLead),
                verdict,
                &author,
                ReviewTask::default(),
            );
            assert_eq!(
                outcome(result),
                Some(codes::REVIEW_LEAD_DELEGATES),
                "review lead {verdict}"
            );
        }
        let other = agent("frontend", &[]);
        let admitted = can_submit_verdict(
            &other,
            UNSUPERVISED,
            Verdict::RequestChanges,
            &author,
            ReviewTask::default(),
        )
        .unwrap();
        assert!(matches!(admitted, Reviewer::Agent(agent) if agent.agent.as_str() == "frontend"));
        let tanim = human();
        let admitted = can_submit_verdict(
            &tanim,
            UNSUPERVISED,
            Verdict::Reject,
            &author,
            ReviewTask::default(),
        )
        .unwrap();
        assert!(matches!(admitted, Reviewer::Human(name) if name.as_str() == "tanim"));
        let denied = can_submit_verdict(
            &other,
            UNSUPERVISED,
            Verdict::Approve,
            &author,
            ReviewTask::default(),
        )
        .unwrap_err();
        assert!(denied.hint().contains("review lead"));
    }

    #[test]
    fn a_running_update_holds_back_only_the_approval_and_the_integration() {
        for verdict in Verdict::ALL {
            let expected = (verdict == Verdict::Approve).then_some(codes::UPDATE_CHECKS_RUNNING);
            assert_eq!(
                outcome(can_review_while_updating(verdict, true)),
                expected,
                "{verdict}"
            );
            assert_eq!(outcome(can_review_while_updating(verdict, false)), None);
        }
        let denied = can_review_while_updating(Verdict::Approve, true).unwrap_err();
        assert!(denied.hint().contains("checks.finished"));
        assert!(denied.hint().contains("wait_for_events"));
        assert_eq!(
            outcome(can_integrate_while_updating(true)),
            Some(codes::UPDATE_CHECKS_RUNNING)
        );
        assert_eq!(outcome(can_integrate_while_updating(false)), None);
    }

    #[test]
    fn on_a_task_of_main_only_a_human_or_the_main_agent_on_the_users_word_decides() {
        let author = worker("main");
        let marked = ReviewTask {
            main: true,
            marked: true,
        };
        let cleared = ReviewTask {
            main: true,
            marked: false,
        };
        let main = agent("main", &["main"]);
        let denied =
            can_submit_verdict(&main, MAIN, Verdict::Approve, &author, marked).unwrap_err();
        assert_eq!(denied.code(), codes::AUTHOR_CANNOT_REVIEW);
        assert!(denied.hint().contains("human"));
        let admitted = can_submit_verdict(&main, MAIN, Verdict::Approve, &author, cleared).unwrap();
        assert!(matches!(admitted, Reviewer::Agent(agent) if agent.agent.as_str() == "main"));
        for verdict in [Verdict::Comment, Verdict::RequestChanges, Verdict::Reject] {
            let result = can_submit_verdict(&main, MAIN, verdict, &author, cleared);
            assert_eq!(
                outcome(result),
                Some(codes::AUTHOR_CANNOT_REVIEW),
                "{verdict}"
            );
        }
        let other_main =
            can_submit_verdict(&main, MAIN, Verdict::Reject, &worker("web-worker"), cleared);
        assert_eq!(outcome(other_main), None);
        let review = agent("review", &[]);
        for (standing, task) in [(supervised(Role::Reviewer), cleared), (MAIN, marked)] {
            for verdict in [Verdict::Approve, Verdict::Reject] {
                let result = can_submit_verdict(&review, standing, verdict, &author, task);
                assert_eq!(
                    outcome(result),
                    Some(codes::MAIN_AREA),
                    "{standing:?} {verdict}"
                );
            }
            let result =
                can_submit_verdict(&review, standing, Verdict::RequestChanges, &author, task);
            assert_eq!(outcome(result), None, "{standing:?}");
        }
        for task in [marked, cleared] {
            let result = can_submit_verdict(
                &review,
                supervised(Role::ReviewLead),
                Verdict::RequestChanges,
                &author,
                task,
            );
            assert_eq!(outcome(result), Some(codes::REVIEW_LEAD_DELEGATES));
        }
        let tanim = human();
        for task in [marked, cleared] {
            let result = can_submit_verdict(
                &tanim,
                AgentStanding::NO_RIGHTS,
                Verdict::Approve,
                &author,
                task,
            );
            assert_eq!(outcome(result), None);
        }
        assert!(!ReviewTask::default().main_approves());
        assert!(!marked.main_approves());
        assert!(cleared.main_approves());
    }

    #[test]
    fn nobody_claims_or_assigns_the_review_of_a_task_of_main() {
        let author = worker("main");
        let review = agent("review", &[]);
        let result = can_claim_review(&review, MAIN, &author, true);
        assert_eq!(outcome(result), Some(codes::MAIN_AREA));
        let result = can_claim_review(&review, supervised(Role::ReviewLead), &author, true);
        assert_eq!(outcome(result), Some(codes::REVIEW_LEAD_DELEGATES));
        let main = agent("main", &["main"]);
        let result = can_claim_review(&main, MAIN, &author, true);
        assert_eq!(outcome(result), Some(codes::AUTHOR_CANNOT_REVIEW));
        let result = can_claim_review(&review, supervised(Role::Reviewer), &author, true);
        assert_eq!(outcome(result), Some(codes::WORKER_CLAIM_DENIED));
        assert!(can_claim_review(&review, MAIN, &author, false).is_ok());
        assert_eq!(outcome(can_assign_review(true)), Some(codes::MAIN_AREA));
        assert_eq!(can_assign_review(false), Ok(()));
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
    fn only_the_main_agent_claims_a_review_and_never_the_author() {
        let author = worker("backend/api");
        let expected = [
            Some(codes::AUTHOR_CANNOT_REVIEW),
            Some(codes::AUTHOR_CANNOT_REVIEW),
            Some(codes::INTEGRATOR_REQUIRED),
            Some(codes::REVIEW_LEAD_DELEGATES),
            Some(codes::ACTOR_CANNOT_CLAIM),
            Some(codes::ACTOR_CANNOT_CLAIM),
        ];
        for ((case, actor), expected) in actors().into_iter().zip(expected) {
            let result = can_claim_review(&actor, standing_of(case), &author, false);
            assert_eq!(outcome(result), expected, "{case}");
        }
        let reviewer = agent("frontend", &[]);
        for (standing, expected) in [
            (MAIN, None),
            (
                supervised(Role::ReviewLead),
                Some(codes::REVIEW_LEAD_DELEGATES),
            ),
            (supervised(Role::Reviewer), Some(codes::WORKER_CLAIM_DENIED)),
            (supervised(Role::AreaLead), Some(codes::INTEGRATOR_REQUIRED)),
            (
                supervised(Role::TaskWorker),
                Some(codes::INTEGRATOR_REQUIRED),
            ),
            (UNSUPERVISED, Some(codes::INTEGRATOR_REQUIRED)),
        ] {
            let result = can_claim_review(&reviewer, standing, &author, false);
            assert_eq!(outcome(result), expected, "{standing:?}");
        }
        let admitted = can_claim_review(&reviewer, MAIN, &author, false).unwrap();
        assert_eq!(admitted.worker, worker("frontend"));
        let denied =
            can_claim_review(&reviewer, supervised(Role::AreaLead), &author, false).unwrap_err();
        assert!(denied.hint().contains("review lead"));
        let denied =
            can_claim_review(&reviewer, supervised(Role::ReviewLead), &author, false).unwrap_err();
        assert!(denied.hint().contains("start_worker"));
        // The author's agent is refused first, whatever its role.
        let author_agent = agent("backend", &[]);
        let result = can_claim_review(&author_agent, supervised(Role::ReviewLead), &author, false);
        assert_eq!(outcome(result), Some(codes::AUTHOR_CANNOT_REVIEW));
        let result = can_claim_review(&author_agent, MAIN, &author, false);
        assert_eq!(outcome(result), Some(codes::AUTHOR_CANNOT_REVIEW));
    }

    #[test]
    fn only_an_area_lead_in_its_area_or_the_main_agent_claims_a_task() {
        let label = |name: &str| LabelName::parse(name).unwrap();
        let web = [label("web")];
        let claimant = agent("web-lead", &[]);
        for (standing, lead_label, expected) in [
            (MAIN, None, None),
            (UNSUPERVISED, None, Some(codes::ORCHESTRATOR_REQUIRED)),
            (
                supervised(Role::ReviewLead),
                None,
                Some(codes::ORCHESTRATOR_REQUIRED),
            ),
            (supervised(Role::AreaLead), Some(label("web")), None),
            (
                supervised(Role::AreaLead),
                Some(label("api")),
                Some(codes::TASK_OUTSIDE_AREA),
            ),
            (
                supervised(Role::AreaLead),
                None,
                Some(codes::TASK_OUTSIDE_AREA),
            ),
            (
                supervised(Role::TaskWorker),
                None,
                Some(codes::WORKER_CLAIM_DENIED),
            ),
            (
                supervised(Role::Reviewer),
                None,
                Some(codes::WORKER_CLAIM_DENIED),
            ),
        ] {
            let result = can_claim_task(&claimant, standing, lead_label.as_ref(), &web);
            assert_eq!(outcome(result), expected, "{standing:?} {lead_label:?}");
        }
        let admitted = can_claim_task(&claimant, MAIN, None, &web).unwrap();
        assert_eq!(admitted.worker, worker("web-lead"));
        for actor in [human(), Actor::System] {
            let result = can_claim_task(&actor, AgentStanding::NO_RIGHTS, None, &web);
            assert_eq!(outcome(result), Some(codes::ACTOR_CANNOT_CLAIM));
        }
    }

    #[test]
    fn only_the_review_lead_or_the_main_agent_chooses_a_review_scope() {
        let lead = agent("review", &[]);
        for (actor, standing, allowed) in [
            (&lead, supervised(Role::ReviewLead), true),
            (&lead, MAIN, true),
            (&lead, supervised(Role::Reviewer), false),
            (&lead, supervised(Role::AreaLead), false),
            (&lead, supervised(Role::TaskWorker), false),
            (&lead, UNSUPERVISED, false),
            (&human(), AgentStanding::NO_RIGHTS, false),
            (&Actor::System, AgentStanding::NO_RIGHTS, false),
        ] {
            let result = can_choose_review_scope(actor, standing);
            assert_eq!(result.is_ok(), allowed, "{standing:?}");
            if let Err(denied) = result {
                assert_eq!(denied.code(), codes::INTEGRATOR_REQUIRED);
            }
        }
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
    fn only_the_holder_agent_checks_out_a_proposal_and_never_the_review_lead() {
        let holder = worker("frontend/r1");
        let expected = [
            Some(codes::REVIEW_CLAIM_REQUIRED),
            Some(codes::REVIEW_CLAIM_REQUIRED),
            None,
            Some(codes::REVIEW_LEAD_DELEGATES),
            Some(codes::REVIEW_CLAIM_REQUIRED),
            Some(codes::REVIEW_CLAIM_REQUIRED),
        ];
        for ((case, actor), expected) in actors().into_iter().zip(expected) {
            let result = can_create_review_checkout(&actor, standing_of(case), Some(&holder));
            assert_eq!(outcome(result), expected, "{case}");
        }
        for (case, actor) in actors() {
            let result = can_create_review_checkout(&actor, standing_of(case), None);
            let expected = if case == "review lead" {
                codes::REVIEW_LEAD_DELEGATES
            } else {
                codes::REVIEW_CLAIM_REQUIRED
            };
            assert_eq!(
                outcome(result),
                Some(expected),
                "{case} on an unclaimed proposal"
            );
        }
        // A claim the review lead holds still never lets it check out.
        let lead = agent("review", &[]);
        let result = can_create_review_checkout(
            &lead,
            supervised(Role::ReviewLead),
            Some(&worker("review")),
        );
        assert_eq!(outcome(result), Some(codes::REVIEW_LEAD_DELEGATES));
    }

    #[test]
    fn only_the_holder_agent_fixes_a_proposal_and_never_the_author() {
        let author = worker("backend/api");
        let holder = worker("frontend/r1");
        let expected = [
            Some(codes::AUTHOR_CANNOT_REVIEW),
            Some(codes::AUTHOR_CANNOT_REVIEW),
            None,
            Some(codes::REVIEW_LEAD_DELEGATES),
            Some(codes::REVIEW_CLAIM_REQUIRED),
            Some(codes::REVIEW_CLAIM_REQUIRED),
        ];
        for ((case, actor), expected) in actors().into_iter().zip(expected) {
            let result = can_fix_in_review(&actor, standing_of(case), Some(&holder), &author);
            assert_eq!(outcome(result), expected, "{case}");
        }
        // A claim that names the author's agent still never lets it fix as a reviewer.
        let actors = actors();
        let result = can_fix_in_review(&actors[1].1, UNSUPERVISED, Some(&author), &author);
        assert_eq!(outcome(result), Some(codes::AUTHOR_CANNOT_REVIEW));
        let result = can_fix_in_review(&actors[2].1, UNSUPERVISED, None, &author);
        assert_eq!(outcome(result), Some(codes::REVIEW_CLAIM_REQUIRED));
        let lead = agent("review", &[]);
        let result = can_fix_in_review(
            &lead,
            supervised(Role::ReviewLead),
            Some(&worker("review")),
            &author,
        );
        assert_eq!(outcome(result), Some(codes::REVIEW_LEAD_DELEGATES));
    }
}
