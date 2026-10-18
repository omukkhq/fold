#![cfg(test)]

use crate::common::roles::give_role;
use crate::reviews_support;
use fold_core::roles::Role;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::reviews_support::{Fixture, LIB, README, agent, by, human, p, t};
use fold_core::object::ObjectKind;
use fold_core::reviews::Verdict;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    Actor, Address, AgentName, MessageBody, RevisionId, TaskDescription, TaskId, TaskTitle,
};
use fold_daemon::push::{self, PushTargets, PushTask, PushToken};
use fold_daemon::services::messages::{MessageService, SendMessage};
use fold_daemon::services::reviews::{IntegrationOutcome, PullIntegration, PullOutcome};
use fold_daemon::services::tasks::CreateTask;
use fold_store::workspaces;
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::write_initial_tree;
use tempfile::TempDir;
use tokio::io::AsyncReadExt;
use tokio::net::UnixListener;
use tokio::sync::mpsc;

const WAIT: Duration = Duration::from_secs(10);
const TOKEN: &str = "0123456789abcdef0123456789abcdef";

/// A stand-in for a Claude Code session's inbox that hands back the notice text of each post.
struct Inbox {
    received: mpsc::UnboundedReceiver<String>,
}

impl Inbox {
    fn bind(path: &Path) -> Self {
        let listener = UnixListener::bind(path).unwrap();
        let (sender, received) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let sender = sender.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    stream.read_to_end(&mut bytes).await.unwrap();
                    let _ = sender.send(notice_text(&String::from_utf8(bytes).unwrap()));
                });
            }
        });
        Self { received }
    }

    async fn next(&mut self) -> String {
        tokio::time::timeout(WAIT, self.received.recv())
            .await
            .expect("a post within the wait")
            .unwrap()
    }
}

/// The notice line of a post, out of its envelope.
fn notice_text(post: &str) -> String {
    let line = post.lines().nth(1).unwrap();
    let value: serde_json::Value = serde_json::from_str(line).unwrap();
    let content = value["message"]["content"].as_str().unwrap();
    content.lines().nth(1).unwrap().to_owned()
}

fn name(text: &str) -> AgentName {
    AgentName::parse(text).unwrap()
}

fn r(number: u32) -> RevisionId {
    RevisionId::from_number(number).unwrap()
}

async fn until(mut done: impl FnMut() -> bool) {
    tokio::time::timeout(WAIT, async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the condition within the wait");
}

/// The review fixture with the project's push delivery running. `review` is the review lead.
struct Notified {
    fx: Fixture,
    targets: PushTargets,
    sockets: TempDir,
    _push: PushTask,
}

impl Notified {
    async fn new() -> Self {
        let fx = Fixture::new().await;
        let targets = PushTargets::new();
        let push = push::run(fx.project.store().clone(), &fx.bus, targets.clone());
        let notified = Self {
            fx,
            targets,
            sockets: TempDir::new().unwrap(),
            _push: push,
        };
        notified.lead("review").await;
        notified
    }

    /// Makes `agent` a review lead, as if fold had started it as one.
    async fn lead(&self, agent: &str) {
        give_role(self.fx.project.store(), agent, Role::ReviewLead, None).await;
    }

    async fn actor(&self, agent: &reviews_support::Agent) -> Actor {
        self.fx.agents.actor(&agent.ctx, None).await.unwrap()
    }

    fn register(&self, agent: &str, socket: PathBuf) {
        let token = PushToken::parse(TOKEN).unwrap();
        self.targets
            .register(&name(agent), socket, token, self.fx.project.now());
    }

    fn inbox(&self, agent: &str) -> Inbox {
        let path = self.sockets.path().join(format!("{agent}.sock"));
        let inbox = Inbox::bind(&path);
        self.register(agent, path);
        inbox
    }

    async fn approve(&self, actor: &Actor) {
        let read = self.fx.reviews.get_proposal(p(1), None).await.unwrap();
        self.fx
            .reviews
            .submit_review(
                actor,
                p(1),
                read.summary.current_version,
                Verdict::Approve,
                "ok",
                Vec::new(),
            )
            .await
            .unwrap();
    }

    async fn integrate(&self, actor: &Actor) -> RevisionId {
        match self
            .fx
            .reviews
            .integrate_proposal(actor, p(1), None)
            .await
            .unwrap()
        {
            IntegrationOutcome::Succeeded { revision } => revision,
            IntegrationOutcome::Conflicted(conflicts) => panic!("conflicted: {conflicts:?}"),
            other => panic!("expected an integration, got {other:?}"),
        }
    }

    /// `T-3`, claimed by `worker` and waiting on `T-1`.
    async fn dependent_task(&self, worker: &Actor) -> TaskId {
        let task = self
            .fx
            .tasks
            .create_task(
                &agent("backend"),
                CreateTask {
                    title: TaskTitle::parse("follow-up").unwrap(),
                    description: TaskDescription::new("after T-1"),
                    labels: crate::common::roles::work_labels(),
                },
            )
            .await
            .unwrap()
            .id;
        self.fx.tasks.claim_task(worker, task).await.unwrap();
        self.fx
            .tasks
            .add_dependency(&agent("backend"), task, t(1))
            .await
            .unwrap();
        task
    }

    /// The fold messages with their task, proposal and body.
    async fn fold_messages(&self) -> Vec<(i64, Option<i64>, Option<i64>, String)> {
        sqlx::query_as(
            "SELECT m.number, t.number, p.number, m.body FROM messages m
             LEFT JOIN tasks t ON t.id = m.task_id
             LEFT JOIN proposals p ON p.id = m.proposal_id
             WHERE m.sender_kind = 'fold' ORDER BY m.number",
        )
        .fetch_all(self.fx.project.store().read())
        .await
        .unwrap()
    }

    /// The recipient workers of message `number`, each with how it was addressed.
    async fn recipients(&self, number: i64) -> Vec<(String, String)> {
        sqlx::query_as(
            "SELECT w.name, r.addressed_as FROM message_recipients r
             JOIN messages m ON m.id = r.message_id
             JOIN workers w ON w.id = r.worker_id
             WHERE m.number = ? ORDER BY w.name",
        )
        .bind(number)
        .fetch_all(self.fx.project.store().read())
        .await
        .unwrap()
    }

    async fn send(&self, from: &Actor, to: &[&str]) {
        MessageService::new(
            self.fx.project.store().clone(),
            self.fx.project.registry().clone(),
            self.fx.bus.clone(),
            Arc::new(self.fx.project.clock().clone()),
        )
        .send(
            from,
            SendMessage {
                to: Address::parse_all(to).unwrap(),
                body: MessageBody::parse("sentinel").unwrap(),
                task: None,
                proposal: None,
                reply_to: None,
            },
        )
        .await
        .unwrap();
    }
}

fn pairs(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(worker, how)| ((*worker).to_owned(), (*how).to_owned()))
        .collect()
}

#[tokio::test]
async fn integration_messages_go_to_author_reviewer_and_owners_of_newly_todo_tasks() {
    let n = Notified::new().await;
    let ready = n.dependent_task(&by(&n.fx.backend.worker)).await;
    n.fx.proposal().await;
    let review = n.actor(&n.fx.review).await;
    n.fx.reviews.claim_review(&review, p(1)).await.unwrap();
    n.approve(&review).await;
    let mut frontend = n.inbox("frontend");

    assert_eq!(n.integrate(&review).await, r(2));

    let messages = n.fold_messages().await;
    assert_eq!(messages.len(), 1);
    let (number, task, proposal, body) = &messages[0];
    assert_eq!((*task, *proposal), (Some(1), Some(1)));
    assert_eq!(
        body,
        &format!("P-1 for T-1 is integrated into main as r2. Now ready: {ready}.")
    );
    assert_eq!(
        n.recipients(*number).await,
        pairs(&[
            ("backend", "agent"),
            ("frontend", "agent"),
            ("review", "agent")
        ])
    );
    assert_eq!(
        n.fx.count("SELECT COUNT(*) FROM events WHERE type = 'message.sent' AND actor = 'fold'")
            .await,
        1
    );
    // The integration notice comes first, then the message's own notice.
    assert_eq!(
        frontend.next().await,
        "fold: P-1 integrated as r2. Call get_inbox."
    );
    assert_eq!(
        frontend.next().await,
        format!("fold: message m-{number} arrived. Call get_inbox.")
    );
}

#[tokio::test]
async fn an_agent_with_several_roles_gets_one_message() {
    let n = Notified::new().await;
    let ui = n.fx.subagent(&n.fx.frontend, "ui").await;
    n.dependent_task(&by(&ui)).await;
    n.fx.proposal().await;
    let review = n.actor(&n.fx.review).await;
    // Nobody holds the review claim, so the reviewer is the agent whose approval stands.
    n.approve(&review).await;
    let mut reviewer = n.inbox("review");
    let mut author = n.inbox("frontend");

    n.integrate(&human()).await;

    let messages = n.fold_messages().await;
    assert_eq!(messages.len(), 1);
    assert_eq!(
        n.recipients(messages[0].0).await,
        pairs(&[("frontend", "agent"), ("review", "agent")])
    );
    // The released claim is gone, so the push finds the reviewer through the approval; the integrating human's own approval reaches nobody once the proposal is integrated.
    let integrated = "fold: P-1 integrated as r2. Call get_inbox.";
    let arrived = format!("fold: message m-{} arrived. Call get_inbox.", messages[0].0);
    assert_eq!(reviewer.next().await, integrated);
    assert_eq!(reviewer.next().await, arrived);
    assert_eq!(author.next().await, integrated);
}

#[tokio::test]
async fn push_goes_to_the_review_lead_on_proposal_opened() {
    let n = Notified::new().await;
    let mut review = n.inbox("review");
    let mut backend = n.inbox("backend");

    n.fx.proposal().await;

    let opened = "fold: P-1 opened for T-1. Call claim_review.";
    assert_eq!(review.next().await, opened);

    let reviewer = n.actor(&n.fx.review).await;
    n.fx.reviews.claim_review(&reviewer, p(1)).await.unwrap();
    n.fx.write("src/more.rs", "pub fn more() {}\n");
    n.fx.reviews
        .update_proposal(&by(&n.fx.frontend.worker), p(1), None)
        .await
        .unwrap()
        .detail()
        .unwrap();
    let version = "fold: P-1 has version 2. Call create_review_checkout.";
    assert_eq!(review.next().await, version);
    // `backend` leads no review, so the first notice it gets is the sentinel.
    n.send(&agent("frontend"), &["backend"]).await;
    assert_eq!(
        backend.next().await,
        "fold: message m-1 arrived. Call get_inbox."
    );
}

#[tokio::test]
async fn push_failure_is_dropped_and_the_event_stays() {
    let n = Notified::new().await;
    let missing = n.sockets.path().join("missing.sock");
    n.register("review", missing.clone());

    n.fx.proposal().await;

    let targets = n.targets.clone();
    until(|| targets.sockets(&name("review")).is_empty()).await;
    assert_eq!(
        n.fx.count("SELECT COUNT(*) FROM events WHERE type = 'proposal.opened'")
            .await,
        1
    );
    assert_eq!(
        n.fx.text("SELECT state FROM proposals WHERE number = 1")
            .await
            .as_deref(),
        Some("open")
    );
    let mut review = n.inbox("review");
    n.send(&agent("backend"), &["review"]).await;
    assert_eq!(
        review.next().await,
        "fold: message m-1 arrived. Call get_inbox."
    );
}

#[tokio::test]
async fn human_approval_pushes_only_the_review_claim_holder() {
    let n = Notified::new().await;
    n.fx.proposal().await;
    let mut review = n.inbox("review");
    let mut backend = n.inbox("backend");
    let mut author = n.inbox("frontend");
    let approved = "fold: P-1 approved by a human. Call integrate_proposal.";

    // Without a claim holder nobody is told, the review lead and the author's agent included.
    n.approve(&human()).await;
    n.send(&agent("backend"), &["review", "frontend"]).await;
    let arrived = "fold: message m-1 arrived. Call get_inbox.";
    assert_eq!(review.next().await, arrived);
    assert_eq!(author.next().await, arrived);

    let reviewer = n.actor(&n.fx.review).await;
    n.fx.reviews.claim_review(&reviewer, p(1)).await.unwrap();
    n.approve(&human()).await;
    assert_eq!(review.next().await, approved);
    n.send(&agent("frontend"), &["backend"]).await;
    assert_eq!(
        backend.next().await,
        "fold: message m-2 arrived. Call get_inbox."
    );
}

#[tokio::test]
async fn pull_integration_sends_no_message_or_push() {
    let n = Notified::new().await;
    let paths = n.fx.project.paths().clone();
    let objects = ObjectStore::open(&paths.fold_dir()).unwrap();
    let mut inboxes: Vec<Inbox> = ["frontend", "backend", "review"]
        .into_iter()
        .map(|agent| n.inbox(agent))
        .collect();
    let mut tree = FlatTree::new();
    for (path, content) in [
        ("src/lib.rs", LIB),
        ("README.md", README),
        ("NOTES.md", &b"notes\n"[..]),
    ] {
        tree.insert(
            path.as_bytes().to_vec(),
            FlatEntry {
                kind: EntryKind::File,
                hash: objects.write(ObjectKind::Blob, content).unwrap(),
            },
        );
    }
    let pull = PullIntegration {
        remote_commit: "4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_owned(),
        remote: "origin".to_owned(),
        branch: "main".to_owned(),
        base_tree: workspaces::get_by_number(n.fx.project.store().read(), r(1))
            .await
            .unwrap()
            .tree_hash,
        remote_tree: write_initial_tree(&objects, tree).unwrap(),
    };

    let outcome = n.fx.reviews.integrate_pull(&human(), pull).await.unwrap();

    assert_eq!(outcome, PullOutcome::Succeeded { revision: r(2) });
    assert_eq!(n.fx.count("SELECT COUNT(*) FROM messages").await, 0);
    // Push keeps event order, so a later message's notice arriving first means the pull sent none.
    n.send(&human(), &["frontend", "backend", "review"]).await;
    for inbox in &mut inboxes {
        assert_eq!(
            inbox.next().await,
            "fold: message m-1 arrived. Call get_inbox."
        );
    }
}
