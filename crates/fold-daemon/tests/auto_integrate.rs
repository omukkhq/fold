#![cfg(test)]

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use fold_core::events::types;
use fold_core::reviews::Verdict;
use fold_core::roles::Role;
use fold_core::{
    Actor, AgentName, EventFilter, ProposalId, RevisionId, TaskDescription, TaskStatus, TaskTitle,
    codes,
};
use fold_daemon::daemon::AppState;
use fold_daemon::jobs::{Job, spawn_auto_integrate};
use fold_daemon::services::reviews::{BatchVerdict, IntegrationOutcome};
use fold_daemon::services::tasks::CreateTask;
use fold_store::events;
use fold_store::workspaces;
use tokio::sync::watch;

use crate::common::roles::give_role;
use crate::proposal_tools_support::{Fixture, human, t};

const WAIT: Duration = Duration::from_secs(20);

fn p(number: u32) -> ProposalId {
    ProposalId::from_number(number).unwrap()
}

fn r(number: u32) -> RevisionId {
    RevisionId::from_number(number).unwrap()
}

/// The job with a short discovery tick, and the sender that stops it.
pub struct Running {
    stop: watch::Sender<bool>,
    pub job: Job,
}

impl Running {
    pub fn start(state: &AppState) -> Self {
        let (stop, stopped) = watch::channel(false);
        let job = spawn_auto_integrate(state.clone(), Duration::from_millis(10), stopped);
        Self { stop, job }
    }

    /// Waits until the job has finished `count` runs.
    pub async fn runs(&self, count: u64) {
        let mut progress = self.job.progress();
        tokio::time::timeout(WAIT, progress.wait_for(|seen| seen.checks >= count))
            .await
            .expect("the job runs in time")
            .unwrap();
    }

    /// Waits until `count` runs have integrated at least one proposal.
    pub async fn cycles(&self, count: u64) {
        let mut progress = self.job.progress();
        tokio::time::timeout(WAIT, progress.wait_for(|seen| seen.cycles >= count))
            .await
            .expect("the job integrates in time")
            .unwrap();
    }

    pub fn checks(&self) -> u64 {
        self.job.progress().borrow().checks
    }

    pub async fn stop(self) {
        self.stop.send(true).unwrap();
        self.job.stopped(WAIT).await;
    }
}

/// Registers `name` as the review lead, returning its actor.
pub async fn review_lead(fx: &Fixture, name: &str) -> Actor {
    with_role(fx, name, Role::ReviewLead).await
}

/// Registers `name` as a worker that fold started in `role`, returning its actor.
async fn with_role(fx: &Fixture, name: &str, role: Role) -> Actor {
    let agents = fx.agents();
    let agent = AgentName::parse(name).unwrap();
    let mut session = agents.start_session(1).await.unwrap();
    agents
        .register_agent(&mut session, &agent, &[])
        .await
        .unwrap();
    give_role(&fx.handle.store, name, role, None).await;
    agents.actor(&session, None).await.unwrap()
}

/// Registers the main agent `main` with the `main` tag, as `fld claude` gives it, and returns its actor.
async fn main_agent(fx: &Fixture) -> Actor {
    let agents = fx.agents();
    let mut session = agents.start_session(1).await.unwrap();
    agents
        .register_agent(
            &mut session,
            &AgentName::parse("main").unwrap(),
            &[fold_core::Tag::parse("main").unwrap()],
        )
        .await
        .unwrap();
    agents.actor(&session, None).await.unwrap()
}

pub async fn approve(fx: &Fixture, reviewer: &Actor, proposal: u32) {
    let read = fx.reviews().get_proposal(p(proposal), None).await.unwrap();
    fx.reviews()
        .submit_review(
            reviewer,
            p(proposal),
            read.summary.current_version,
            Verdict::Approve,
            "looks right",
            Vec::new(),
        )
        .await
        .unwrap();
}

/// `author` creates the workspace of `task`, writes `content` to `path` and opens a proposal.
async fn propose(fx: &Fixture, author: &Actor, task: u32, path: &str, content: &str) {
    fx.workspaces()
        .create_workspace(author, t(task))
        .await
        .unwrap();
    fx.write(task, path, content);
    fx.reviews()
        .create_proposal(author, t(task), "Change it", None)
        .await
        .unwrap();
}

pub async fn head(fx: &Fixture) -> RevisionId {
    workspaces::head(fx.handle.store.read())
        .await
        .unwrap()
        .unwrap()
        .number
}

pub async fn state_of(fx: &Fixture, proposal: u32) -> String {
    sqlx::query_scalar("SELECT state FROM proposals WHERE number = ?1")
        .bind(i64::from(proposal))
        .fetch_one(fx.handle.store.read())
        .await
        .unwrap()
}

pub async fn status(fx: &Fixture, task: u32) -> TaskStatus {
    fx.handle.tasks.get(t(task)).await.unwrap().status
}

/// The proposal numbers of the succeeded integrations, in the order they ran.
async fn integrated_in_order(fx: &Fixture) -> Vec<i64> {
    sqlx::query_scalar(
        "SELECT p.number FROM integrations i JOIN proposals p ON p.id = i.proposal_id
         WHERE i.result = 'succeeded' ORDER BY i.id",
    )
    .fetch_all(fx.handle.store.read())
    .await
    .unwrap()
}

async fn count(fx: &Fixture, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(fx.handle.store.read())
        .await
        .unwrap()
}

fn write_config(fx: &Fixture, text: &str) {
    std::fs::write(fx.project.paths().config(), text).unwrap();
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_approved_proposal_integrates_with_no_tool_call_and_gets_its_git_commit() {
    let fx = Fixture::new().await;
    let author = fx.agent("backend").await;
    let review = review_lead(&fx, "review").await;
    propose(&fx, &author, 1, "src/lib.rs", "pub fn y() {}\n").await;
    let job = Running::start(&fx.state);
    job.runs(1).await;
    assert_eq!(head(&fx).await, r(1), "nothing is approved yet");

    approve(&fx, &review, 1).await;
    job.cycles(1).await;

    assert_eq!(head(&fx).await, r(2));
    assert_eq!(state_of(&fx, 1).await, "integrated");
    assert_eq!(status(&fx, 1).await, TaskStatus::Done);
    assert_eq!(
        count(
            &fx,
            "SELECT COUNT(*) FROM completions c JOIN integrations i ON i.id = c.integration_id
             WHERE i.result = 'succeeded' AND i.actor = 'fold'"
        )
        .await,
        1
    );
    let committed = "SELECT COUNT(*) FROM revision_commits c JOIN revisions r ON r.id = c.revision_id WHERE r.number = 2";
    tokio::time::timeout(WAIT, async {
        while count(&fx, committed).await == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the integration gets its Git commit");
    // The fixture's folder does not hold r1's files, so its branch stays; fold's commits are on its own ref.
    let repository = fx.project.paths().git_dir();
    assert_eq!(
        git(&repository, &["show", "refs/fold/head:src/lib.rs"]),
        "pub fn y() {}\n"
    );
    job.stop().await;
}

#[tokio::test]
async fn only_the_agent_with_the_main_tag_approves_as_the_main_agent() {
    let fx = Fixture::new().await;
    let author = fx.agent("backend").await;
    // Nobody has the `main` tag, and `solo` has no role: it is no main agent and decides nothing.
    let solo = fx.agent("solo").await;
    propose(&fx, &author, 1, "src/lib.rs", "pub fn y() {}\n").await;
    let refused = fx
        .reviews()
        .submit_review(&solo, p(1), 1, Verdict::Approve, "fine", Vec::new())
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some(codes::INTEGRATOR_REQUIRED));
    let job = Running::start(&fx.state);
    job.runs(1).await;

    let main = main_agent(&fx).await;
    approve(&fx, &main, 1).await;
    job.cycles(1).await;
    assert_eq!(head(&fx).await, r(2));
    assert_eq!(state_of(&fx, 1).await, "integrated");
    job.stop().await;
}

#[tokio::test]
async fn a_marked_proposal_waits_for_the_human_after_the_agent_approval() {
    let fx = Fixture::new().await;
    let author = fx.agent("backend").await;
    let review = review_lead(&fx, "review").await;
    fx.reviews()
        .require_human_review(&review, t(1), None, "touches the public API")
        .await
        .unwrap();
    propose(&fx, &author, 1, "src/lib.rs", "pub fn y() {}\n").await;
    let job = Running::start(&fx.state);
    job.runs(1).await;

    approve(&fx, &review, 1).await;
    job.runs(2).await;
    assert_eq!(head(&fx).await, r(1));
    assert_eq!(state_of(&fx, 1).await, "open");
    let waiting = fx.reviews().awaiting_human_review().await.unwrap();
    assert_eq!(
        waiting.len(),
        1,
        "the human sees it after the agent approval"
    );
    let items = fx.handle.agents().needs_you().await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].title.as_deref(), Some("Change it"));
    assert_eq!(count(&fx, "SELECT COUNT(*) FROM integrations").await, 0);

    approve(&fx, &human("tanim"), 1).await;
    job.cycles(1).await;
    assert_eq!(head(&fx).await, r(2));
    assert_eq!(state_of(&fx, 1).await, "integrated");
    assert_eq!(status(&fx, 1).await, TaskStatus::Done);
    assert_eq!(
        count(&fx, "SELECT COUNT(*) FROM reviews").await,
        2,
        "fold adds no approval of its own"
    );
    job.stop().await;
}

#[tokio::test]
async fn a_human_approval_alone_integrates_a_ready_proposal() {
    let fx = Fixture::new().await;
    let author = fx.agent("backend").await;
    propose(&fx, &author, 1, "src/lib.rs", "pub fn y() {}\n").await;
    let job = Running::start(&fx.state);
    job.runs(1).await;

    approve(&fx, &human("tanim"), 1).await;
    job.cycles(1).await;
    assert_eq!(head(&fx).await, r(2));
    assert_eq!(state_of(&fx, 1).await, "integrated");
    assert_eq!(
        count(&fx, "SELECT COUNT(*) FROM reviews").await,
        1,
        "no agent approved"
    );
    job.stop().await;
}

#[tokio::test]
async fn two_dependent_proposals_integrate_in_dependency_order() {
    let fx = Fixture::new().await;
    let author = fx.agent("backend").await;
    let review = review_lead(&fx, "review").await;
    propose(&fx, &author, 1, "src/lib.rs", "pub fn y() {}\n").await;
    propose(&fx, &author, 2, "README.md", "# demo, exported\n").await;
    fx.handle
        .tasks
        .add_dependency(&human("tanim"), t(2), t(1))
        .await
        .unwrap();
    let job = Running::start(&fx.state);
    job.runs(1).await;

    approve(&fx, &review, 2).await;
    job.runs(2).await;
    assert_eq!(head(&fx).await, r(1));
    assert_eq!(state_of(&fx, 2).await, "open", "T-1 is not Done yet");
    assert_eq!(
        count(&fx, "SELECT COUNT(*) FROM integrations").await,
        0,
        "a waiting proposal records no refusal"
    );

    approve(&fx, &review, 1).await;
    job.cycles(1).await;
    assert_eq!(integrated_in_order(&fx).await, [1, 2]);
    assert_eq!(head(&fx).await, r(3));
    assert_eq!(status(&fx, 1).await, TaskStatus::Done);
    assert_eq!(status(&fx, 2).await, TaskStatus::Done);
    job.stop().await;
}

#[tokio::test]
async fn a_conflict_leaves_the_proposal_open_and_is_recorded_once() {
    let fx = Fixture::new().await;
    let author = fx.agent("backend").await;
    let review = review_lead(&fx, "review").await;
    let other = fx.agent("frontend").await;
    propose(&fx, &author, 1, "src/lib.rs", "pub fn y() {}\n").await;
    propose(&fx, &other, 2, "src/lib.rs", "pub fn z() {}\n").await;
    let job = Running::start(&fx.state);
    job.runs(1).await;

    approve(&fx, &review, 1).await;
    job.cycles(1).await;
    assert_eq!(head(&fx).await, r(2));

    approve(&fx, &review, 2).await;
    job.cycles(2).await;
    assert_eq!(head(&fx).await, r(2));
    assert_eq!(state_of(&fx, 2).await, "open");
    assert_eq!(status(&fx, 2).await, TaskStatus::InProgress);
    let conflicted = || async {
        let filter = EventFilter {
            types: vec![types::INTEGRATION_CONFLICTED],
            ..EventFilter::default()
        };
        events::read_after(fx.handle.store.read(), 0, &filter, 100)
            .await
            .unwrap()
    };
    let recorded = conflicted().await;
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].proposal, Some(p(2)));
    assert_eq!(recorded[0].actor.as_str(), "fold");

    let before = job.checks();
    fx.reviews()
        .submit_review(
            &review,
            p(2),
            1,
            Verdict::Comment,
            "still looking",
            Vec::new(),
        )
        .await
        .unwrap();
    job.runs(before + 1).await;
    assert_eq!(
        conflicted().await.len(),
        1,
        "a flagged proposal waits for its next version"
    );
    job.stop().await;
}

#[tokio::test]
async fn with_automatic_off_nothing_integrates_on_its_own() {
    let fx = Fixture::new().await;
    write_config(&fx, "[integration]\nautomatic = false\n");
    let author = fx.agent("backend").await;
    let review = review_lead(&fx, "review").await;
    propose(&fx, &author, 1, "src/lib.rs", "pub fn y() {}\n").await;
    let job = Running::start(&fx.state);
    job.runs(1).await;

    approve(&fx, &review, 1).await;
    job.runs(2).await;
    assert_eq!(head(&fx).await, r(1));
    assert_eq!(state_of(&fx, 1).await, "open");
    assert_eq!(job.job.progress().borrow().cycles, 0);

    let refused = fx
        .reviews()
        .integrate_proposal(&Actor::System, p(1), Some(1))
        .await
        .unwrap_err();
    assert_eq!(
        refused.code(),
        Some(codes::INTEGRATOR_REQUIRED),
        "fold integrates only through the job"
    );
    let outcome = fx
        .reviews()
        .integrate_proposal(&review, p(1), Some(1))
        .await
        .unwrap();
    assert_eq!(outcome, IntegrationOutcome::Succeeded { revision: r(2) });
    job.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_batch_reviewers_batch_integrates_its_approved_members_in_dependency_order() {
    let fx = Fixture::new().await;
    fx.handle
        .tasks
        .create_task(
            &human("tanim"),
            CreateTask {
                title: TaskTitle::parse("docs").unwrap(),
                description: TaskDescription::new("what to build"),
                labels: Vec::new(),
            },
        )
        .await
        .unwrap();
    let author = fx.agent("backend").await;
    // A batch reviewer that the review lead started decides by its role.
    let review = with_role(&fx, "reviewer-1", Role::BatchReviewer).await;
    fx.reviews()
        .require_human_review(&review, t(3), None, "touches the public API")
        .await
        .unwrap();
    propose(&fx, &author, 1, "src/one.rs", "one\n").await;
    propose(&fx, &author, 2, "src/two.rs", "two\n").await;
    propose(&fx, &author, 3, "src/three.rs", "three\n").await;
    fx.handle
        .tasks
        .add_dependency(&human("tanim"), t(1), t(2))
        .await
        .unwrap();
    let job = Running::start(&fx.state);
    job.runs(1).await;

    let batch = fx
        .reviews()
        .create_review_batch(&review, vec![p(1), p(2), p(3)])
        .await
        .unwrap()
        .id;
    let approve_all = [1, 2, 3]
        .map(|number| BatchVerdict {
            proposal: p(number),
            verdict: Verdict::Approve,
            body: String::new(),
        })
        .to_vec();
    fx.reviews()
        .submit_batch_review(&review, batch, approve_all, Vec::new())
        .await
        .unwrap();
    tokio::time::timeout(WAIT, async {
        while integrated_in_order(&fx).await.len() < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the approved members integrate by themselves");
    assert_eq!(
        integrated_in_order(&fx).await,
        [2, 1],
        "T-1 needs T-2, so P-2 goes first"
    );
    assert_eq!(head(&fx).await, r(3));
    assert_eq!(status(&fx, 1).await, TaskStatus::Done);
    assert_eq!(status(&fx, 2).await, TaskStatus::Done);
    let committed = "SELECT COUNT(*) FROM revision_commits c JOIN revisions r ON r.id = c.revision_id WHERE r.number IN (2, 3)";
    tokio::time::timeout(WAIT, async {
        while count(&fx, committed).await < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("each integration gets its Git commit");
    let repository = fx.project.paths().git_dir();
    assert_eq!(
        git(&repository, &["show", "refs/fold/head:src/one.rs"]),
        "one\n"
    );
    assert_eq!(
        git(&repository, &["show", "refs/fold/head~1:src/two.rs"]),
        "two\n"
    );

    assert_eq!(state_of(&fx, 3).await, "open", "T-3 waits for the human");
    assert_eq!(fx.reviews().awaiting_human_review().await.unwrap().len(), 1);
    approve(&fx, &human("tanim"), 3).await;
    tokio::time::timeout(WAIT, async {
        while state_of(&fx, 3).await != "integrated" {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the marked member integrates after the human approves");
    assert_eq!(integrated_in_order(&fx).await, [2, 1, 3]);
    job.stop().await;
}
