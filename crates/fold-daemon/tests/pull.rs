#![cfg(test)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::revision::Revision;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    Actor, AgentName, Event, EventFilter, EventType, HostPaths, HumanName, ManualClock,
    ProjectPaths, ProposalId, RevisionId, TaskDescription, TaskId, TaskStatus, TaskTitle,
    Timestamp, codes, events::types,
};
use fold_daemon::http::host::CLIENT_HEADER;
use fold_daemon::http::socket_router;
use fold_daemon::projects::ProjectHandle;
use fold_daemon::services::events::BusMessage;
use fold_daemon::services::projects::{PullOutcome, PushOutcome, git_status, pull, push};
use fold_daemon::services::reviews::{IntegrationOutcome, ReviewService};
use fold_daemon::services::tasks::CreateTask;
use fold_daemon::services::workspaces::{WorkspaceError, WorkspaceRef, WorkspaceService};
use fold_daemon::{Daemon, DaemonConfig};
use fold_store::projects::ProjectRow;
use fold_store::workspaces::{self, advance_main, insert_revision};
use fold_vcs::git::Git;
use fold_vcs::git_sync_map::SYNC_MAP_FILE;
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::AssertSqlSafe;
use tempfile::TempDir;
use tower::ServiceExt;

struct Host {
    dir: TempDir,
    paths: HostPaths,
    clock: ManualClock,
}

impl Host {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let paths = HostPaths::new(dir.path().join("data").to_str(), None).unwrap();
        Self {
            dir,
            paths,
            clock: ManualClock::new(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// An empty bare repository standing in for the remote.
    fn remote(&self) -> PathBuf {
        let remote = self.path("remote.git");
        fs::create_dir(&remote).unwrap();
        git(
            &remote,
            &["init", "--quiet", "--bare", "--initial-branch=main"],
        );
        remote
    }

    /// A clone of the remote that makes the remote's own commits.
    fn clone_remote(&self, remote: &Path) -> PathBuf {
        git(
            self.dir.path(),
            &["clone", "--quiet", remote.to_str().unwrap(), "clone"],
        );
        self.path("clone")
    }

    async fn project(&self) -> Project {
        let project = crate::common::init_with_git(&self.paths, &self.path("app"), &self.clock)
            .await
            .unwrap();
        let row = ProjectRow {
            slug: project.slug.clone(),
            name: project.slug.clone(),
            root: project.root.to_string_lossy().into_owned(),
        };
        let handle = Arc::new(
            ProjectHandle::open(&row, Arc::new(self.clock.clone()))
                .await
                .unwrap(),
        );
        Project {
            handle,
            paths: ProjectPaths::new(&project.root),
            slug: project.slug,
        }
    }
}

struct Project {
    handle: Arc<ProjectHandle>,
    paths: ProjectPaths,
    slug: String,
}

impl Project {
    /// Adds `remote` as the remote `origin`, as `fld remote add` does.
    fn set_remote(&self, remote: &Path, push_on_integrate: bool) {
        let config = self.paths.config();
        let mut text = fs::read_to_string(&config).unwrap();
        text.push_str(&format!(
            "\n[remotes.origin]\nurl = {:?}\nbranch = \"main\"\npush_on_integrate = {push_on_integrate}\n",
            remote.to_str().unwrap()
        ));
        fs::write(config, text).unwrap();
    }

    async fn pull(&self) -> Result<PullOutcome, Option<&'static str>> {
        pull(&self.handle, &ana(), &Git::default(), None)
            .await
            .map_err(|error| error.code())
    }

    async fn push(&self) -> PushOutcome {
        push(&self.handle, &ana(), &Git::default(), None)
            .await
            .unwrap()
    }

    fn map(&self) -> String {
        fs::read_to_string(self.paths.fold_dir().join(SYNC_MAP_FILE)).unwrap_or_default()
    }

    async fn events(&self, kinds: &[EventType]) -> Vec<Event> {
        let filter = EventFilter {
            types: kinds.to_vec(),
            ..EventFilter::default()
        };
        fold_store::events::read_after(self.handle.store.read(), 0, &filter, 1000)
            .await
            .unwrap()
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(AssertSqlSafe(sql))
            .fetch_one(self.handle.store.read())
            .await
            .unwrap()
    }

    async fn head(&self) -> RevisionId {
        workspaces::head(self.handle.store.read())
            .await
            .unwrap()
            .unwrap()
            .number
    }

    /// Records a revision on top of `main` holding `files`, and moves `main` to it.
    async fn add_revision(&self, files: &[(&str, &[u8])]) -> RevisionId {
        let store = &self.handle.store;
        let head = workspaces::head(store.read()).await.unwrap().unwrap();
        let objects = ObjectStore::open(&self.paths.fold_dir()).unwrap();
        let mut flat = FlatTree::new();
        for (path, bytes) in files {
            let hash = objects
                .write(fold_core::object::ObjectKind::Blob, bytes)
                .unwrap();
            flat.insert(
                path.as_bytes().to_vec(),
                FlatEntry {
                    kind: EntryKind::File,
                    hash,
                },
            );
        }
        let number = RevisionId::from_number(head.number.number() + 1).unwrap();
        let revision = Revision {
            tree: write_initial_tree(&objects, flat).unwrap(),
            parent: Some(head.hash),
            number,
            integration: None,
            created_at: Timestamp::from_millis(1_700_000_000_000).unwrap(),
        };
        let stored = write_revision(&objects, &revision).unwrap();
        store
            .write(async |tx| {
                insert_revision(tx, &stored).await?;
                advance_main(tx, head.number, number).await
            })
            .await
            .unwrap();
        number
    }

    /// The files of `revision` in the Git sense: `git ls-tree` of its exported commit is compared instead where a commit exists.
    async fn files(&self, revision: RevisionId) -> Vec<(String, String)> {
        let row = workspaces::get_by_number(self.handle.store.read(), revision)
            .await
            .unwrap();
        let objects = ObjectStore::open(&self.paths.fold_dir()).unwrap();
        let load = |hash: &fold_core::object::ObjectHash| {
            let object = objects.read_expecting(hash, fold_core::object::ObjectKind::Tree)?;
            Ok(fold_core::tree::Tree::decode(&object.payload)?)
        };
        fold_core::tree::flatten(&row.tree_hash, &load)
            .unwrap()
            .into_iter()
            .map(|(path, entry)| {
                let bytes = objects.read(&entry.hash).unwrap().payload;
                (
                    String::from_utf8(path).unwrap(),
                    String::from_utf8(bytes).unwrap(),
                )
            })
            .collect()
    }

    fn workspaces(&self) -> WorkspaceService {
        WorkspaceService::for_project(&self.handle)
    }

    fn reviews(&self) -> ReviewService {
        ReviewService::new(
            self.handle.store.clone(),
            self.handle.bus.clone(),
            self.handle.clock.clone(),
            self.workspaces(),
            self.handle.integration.clone(),
        )
    }

    /// A registered agent's acting identity.
    async fn agent(&self, name: &str) -> Actor {
        let agents = self.handle.agents();
        let mut ctx = agents.start_session(1).await.unwrap();
        agents
            .register_agent(&mut ctx, &AgentName::parse(name).unwrap(), &[])
            .await
            .unwrap();
        agents.actor(&ctx).await.unwrap()
    }

    /// `frontend` creates a task, writes `files` into its workspace and opens a proposal, which `ana` integrates.
    async fn integrate_task(&self, files: &[(&str, &str)]) -> RevisionId {
        let author = self.agent("frontend").await;
        crate::common::roles::lead_work_areas(&self.handle.store).await;
        let task = self
            .handle
            .tasks
            .create_task(
                &author,
                CreateTask {
                    title: TaskTitle::parse("Change files").unwrap(),
                    description: TaskDescription::new(""),
                    labels: crate::common::roles::work_labels(),
                },
            )
            .await
            .unwrap()
            .id;
        let created = self
            .workspaces()
            .create_workspace(&author, task)
            .await
            .unwrap();
        for (path, content) in files {
            let file = self.paths.root().join(&created.workspace.path).join(path);
            fs::write(file, content).unwrap();
        }
        let proposal = self
            .reviews()
            .create_proposal(&author, task, "Change files", None)
            .await
            .unwrap()
            .detail()
            .unwrap()
            .summary
            .id;
        integrated(
            self.reviews()
                .integrate_proposal(&ana(), proposal, None)
                .await
                .unwrap(),
        )
    }
}

fn ana() -> Actor {
    Actor::human(HumanName::parse("ana").unwrap())
}

fn integrated(outcome: IntegrationOutcome) -> RevisionId {
    match outcome {
        IntegrationOutcome::Succeeded { revision } => revision,
        IntegrationOutcome::Conflicted(conflicts) => panic!("conflicted: {conflicts:?}"),
        other => panic!("expected an integration, got {other:?}"),
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// Commits `files` in the clone and pushes them to the remote; returns the commit.
fn remote_commit(clone: &Path, files: &[(&str, &str)]) -> String {
    for (path, content) in files {
        fs::write(clone.join(path), content).unwrap();
        git(clone, &["add", path]);
    }
    git(clone, &["commit", "--quiet", "-m", "theirs"]);
    git(clone, &["push", "--quiet", "origin", "main"]);
    git(clone, &["rev-parse", "HEAD"]).trim().to_owned()
}

fn r(number: u32) -> RevisionId {
    RevisionId::from_number(number).unwrap()
}

fn short(commit: &str) -> &str {
    &commit[..7]
}

/// Waits for the first durable event of `kind` on the bus.
async fn next_event(
    bus: &mut tokio::sync::broadcast::Receiver<BusMessage>,
    kind: EventType,
) -> Arc<Event> {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let Ok(BusMessage::Durable(event)) = bus.recv().await
                && event.event_type == kind
            {
                return event;
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn a_pull_needs_a_remote_and_a_human() {
    let host = Host::new();
    let project = host.project().await;
    assert_eq!(project.pull().await, Err(Some(codes::REMOTE_NOT_SET)));
    let agent = project.agent("frontend").await;
    let refused = pull(&project.handle, &agent, &Git::default(), None)
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some(codes::HUMAN_REQUIRED));
}

#[tokio::test]
async fn an_unchanged_remote_is_up_to_date_and_records_nothing() {
    let host = Host::new();
    let project = host.project().await;
    let remote = host.remote();
    project.set_remote(&remote, false);
    assert_eq!(
        project.pull().await.unwrap(),
        PullOutcome::UpToDate {
            remote: "origin".to_owned(),
            branch: "main".to_owned(),
            remote_commit: None,
        }
    );
    project.add_revision(&[("a.txt", b"one\n")]).await;
    project.push().await;
    let tip = git(&remote, &["rev-parse", "main"]).trim().to_owned();
    let map = project.map();

    assert_eq!(
        project.pull().await.unwrap(),
        PullOutcome::UpToDate {
            remote: "origin".to_owned(),
            branch: "main".to_owned(),
            remote_commit: Some(tip.parse().unwrap()),
        }
    );
    assert_eq!(
        project
            .count("SELECT COUNT(*) FROM integrations WHERE kind = 'pull'")
            .await,
        0
    );
    assert_eq!(project.head().await, r(2));
    assert_eq!(project.map(), map);
    assert!(
        project
            .events(&[types::REMOTE_PULLED, types::REMOTE_PULL_CONFLICTED])
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn a_fast_forward_records_the_remote_tree_and_the_map_line() {
    let host = Host::new();
    let project = host.project().await;
    let remote = host.remote();
    project.set_remote(&remote, false);
    project.add_revision(&[("a.txt", b"one\n")]).await;
    project.push().await;
    let clone = host.clone_remote(&remote);
    let tip = remote_commit(&clone, &[("b.txt", "theirs\n")]);

    let outcome = project.pull().await.unwrap();

    assert_eq!(
        outcome,
        PullOutcome::Integrated {
            remote: "origin".to_owned(),
            branch: "main".to_owned(),
            remote_commit: tip.parse().unwrap(),
            revision: r(3),
        }
    );
    assert_eq!(project.head().await, r(3));
    assert_eq!(
        project.files(r(3)).await,
        [
            ("a.txt".to_owned(), "one\n".to_owned()),
            ("b.txt".to_owned(), "theirs\n".to_owned())
        ]
    );
    assert_eq!(
        project
            .count(&format!(
                "SELECT COUNT(*) FROM integrations i JOIN revisions r ON r.id = i.revision_id
                 WHERE i.kind = 'pull' AND i.result = 'succeeded' AND i.remote_commit = '{tip}'
                   AND i.actor = 'human:ana' AND r.number = 3"
            ))
            .await,
        1
    );
    let events = project
        .events(&[
            types::INTEGRATION_SUCCEEDED,
            types::MAIN_ADVANCED,
            types::REMOTE_PULLED,
        ])
        .await;
    let names: Vec<&str> = events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect();
    // The first two record the folder's files as `r1`.
    assert_eq!(
        names,
        [
            "integration.succeeded",
            "main.advanced",
            "integration.succeeded",
            "main.advanced",
            "remote.pulled"
        ]
    );
    assert_eq!(
        events[4].data,
        json!({"remote": "origin", "branch": "main", "remote_commit": tip, "revision": "r3"})
    );
    assert!(project.map().ends_with(&format!("r3 {tip}\n")));
    assert_eq!(
        project.push().await,
        PushOutcome::UpToDate {
            remote: "origin".to_owned(),
            branch: "main".to_owned(),
            last: Some(r(3)),
        }
    );
    assert_eq!(
        project.pull().await.unwrap(),
        PullOutcome::UpToDate {
            remote: "origin".to_owned(),
            branch: "main".to_owned(),
            remote_commit: Some(tip.parse().unwrap()),
        }
    );
}

#[tokio::test]
async fn both_sides_moved_cleanly_and_the_next_push_is_a_merge() {
    let host = Host::new();
    let project = host.project().await;
    // The merge commit of the pull takes the repository's Git identity, never the pulling human's name.
    git(
        &project.paths.git_dir(),
        &["config", "user.name", "Ana Lima"],
    );
    git(
        &project.paths.git_dir(),
        &["config", "user.email", "ana@example.com"],
    );
    let remote = host.remote();
    project.set_remote(&remote, false);
    project.add_revision(&[("a.txt", b"one\n")]).await;
    project.push().await;
    let clone = host.clone_remote(&remote);
    let tip = remote_commit(&clone, &[("b.txt", "theirs\n")]);
    project.add_revision(&[("a.txt", b"two\n")]).await;

    let outcome = project.pull().await.unwrap();

    assert!(matches!(outcome, PullOutcome::Integrated { revision, .. } if revision == r(4)));
    assert_eq!(
        project.files(r(4)).await,
        [
            ("a.txt".to_owned(), "two\n".to_owned()),
            ("b.txt".to_owned(), "theirs\n".to_owned())
        ]
    );
    let pushed = project.push().await;
    assert!(matches!(pushed, PushOutcome::Synced { commits: 2, .. }));
    assert_eq!(project.map().lines().count(), 4);
    let merges = git(&remote, &["log", "--merges", "--format=%P", "main"]);
    let parents: Vec<&str> = merges.split_whitespace().collect();
    assert_eq!(parents.len(), 2);
    assert_eq!(parents[1], tip);
    assert_eq!(
        git(
            &remote,
            &["log", "-1", "--format=%an <%ae>|%cn <%ce>|%B", "main"]
        ),
        format!(
            "Ana Lima <ana@example.com>|Ana Lima <ana@example.com>|Merge remote commit {}\n\nFold-Revision: r4\n\n",
            &tip[..12]
        )
    );
    assert_eq!(git(&remote, &["show", "main:a.txt"]), "two\n");
    assert_eq!(
        project.pull().await.unwrap(),
        PullOutcome::UpToDate {
            remote: "origin".to_owned(),
            branch: "main".to_owned(),
            remote_commit: Some(git(&remote, &["rev-parse", "main"]).trim().parse().unwrap()),
        }
    );
}

/// Main moved twice after the commit the remote builds on: the merge base is that commit, so both fold revisions stay.
#[tokio::test]
async fn a_remote_commit_on_an_older_main_commit_keeps_every_later_fold_revision() {
    let host = Host::new();
    let project = host.project().await;
    let remote = host.remote();
    project.set_remote(&remote, false);
    project
        .add_revision(&[("a.txt", b"one\n"), ("c.txt", b"one\n")])
        .await;
    project.push().await;
    let pushed = git(&remote, &["rev-parse", "main"]).trim().to_owned();
    let clone = host.clone_remote(&remote);
    let tip = remote_commit(&clone, &[("b.txt", "theirs\n")]);
    project
        .add_revision(&[("a.txt", b"two\n"), ("c.txt", b"one\n")])
        .await;
    project
        .add_revision(&[("a.txt", b"two\n"), ("c.txt", b"three\n")])
        .await;

    let outcome = project.pull().await.unwrap();

    assert!(matches!(outcome, PullOutcome::Integrated { revision, .. } if revision == r(5)));
    assert_eq!(
        project.files(r(5)).await,
        [
            ("a.txt".to_owned(), "two\n".to_owned()),
            ("b.txt".to_owned(), "theirs\n".to_owned()),
            ("c.txt".to_owned(), "three\n".to_owned()),
        ]
    );
    assert!(matches!(
        project.push().await,
        PushOutcome::Synced { commits: 3, .. }
    ));
    let parents = git(&remote, &["log", "-1", "--format=%P", "main"]);
    let parents: Vec<&str> = parents.split_whitespace().collect();
    assert_eq!(parents.len(), 2);
    assert_eq!(parents[1], tip);
    assert_eq!(git(&remote, &["merge-base", "main^1", &tip]).trim(), pushed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_conflict_becomes_a_sync_task_that_reaches_the_remote() {
    let host = Host::new();
    let project = host.project().await;
    let remote = host.remote();
    project.set_remote(&remote, false);
    project.add_revision(&[("a.txt", b"one\n")]).await;
    project.push().await;
    let base = git(&remote, &["rev-parse", "main"]).trim().to_owned();
    let clone = host.clone_remote(&remote);
    let tip = remote_commit(&clone, &[("a.txt", "theirs\n")]);
    project.add_revision(&[("a.txt", b"ours\n")]).await;

    let outcome = project.pull().await.unwrap();

    let PullOutcome::Conflicted {
        task,
        workspace,
        conflicted,
        created: true,
        ..
    } = outcome
    else {
        panic!("expected a new sync task, got {outcome:?}");
    };
    assert_eq!(project.head().await, r(3));
    assert_eq!(conflicted.len(), 1);
    assert_eq!(conflicted[0].path, "a.txt");
    let view = project.handle.tasks.get(task).await.unwrap();
    assert_eq!(
        view.title.as_str(),
        format!("Sync origin/main @{}", short(&tip))
    );
    assert_eq!(view.created_by, "human:ana");
    assert_eq!(view.status, TaskStatus::InProgress);
    assert!(view.description.as_str().contains(&tip));
    let ws = project
        .workspaces()
        .get(WorkspaceRef::Workspace(workspace.unwrap()))
        .await
        .unwrap();
    assert_eq!(
        ws.path,
        format!(".fold/ws/{}", task.to_string().to_lowercase())
    );
    assert_eq!(ws.worker, None);
    assert_eq!(ws.base, r(3));
    let markers = fs::read_to_string(project.paths.root().join(&ws.path).join("a.txt")).unwrap();
    assert_eq!(
        markers,
        format!(
            "<<<<<<< workspace\ntheirs\n||||||| base {}\none\n=======\nours\n>>>>>>> main r3\n",
            short(&base)
        )
    );
    let conflicted_events = project.events(&[types::REMOTE_PULL_CONFLICTED]).await;
    assert_eq!(conflicted_events.len(), 1);
    assert_eq!(conflicted_events[0].task, Some(task));
    assert_eq!(conflicted_events[0].workspace, workspace);
    assert_eq!(
        conflicted_events[0].data,
        json!({"remote": "origin", "branch": "main", "remote_commit": tip, "conflicted": 1})
    );

    // The same tip again names the open task and creates nothing.
    let again = project.pull().await.unwrap();
    assert!(matches!(
        again,
        PullOutcome::Conflicted { task: named, created: false, ref conflicted, .. } if named == task && conflicted.len() == 1
    ));
    assert_eq!(project.count("SELECT COUNT(*) FROM tasks").await, 1);
    assert_eq!(project.count("SELECT COUNT(*) FROM workspaces").await, 1);

    // The main agent, on the user's word, adopts the workspace with the claim of the task, which has no area, resolves the markers and integrates.
    let agents = project.handle.agents();
    let mut ctx = agents.start_session(1).await.unwrap();
    agents
        .register_agent(
            &mut ctx,
            &AgentName::parse("main").unwrap(),
            &[fold_core::Tag::parse("main").unwrap()],
        )
        .await
        .unwrap();
    let agent = agents.actor(&ctx).await.unwrap();
    let claimed = project.handle.tasks.claim_task(&agent, task).await.unwrap();
    assert!(claimed.adopted);
    let id = workspace.unwrap();
    let refused = project
        .workspaces()
        .record_changes(&agent, id, "resolve")
        .await
        .unwrap_err();
    let WorkspaceError::Fold(refused) = refused else {
        panic!("expected a refusal, got {refused:?}");
    };
    assert_eq!(refused.code(), codes::UNRESOLVED_CONFLICTS);
    fs::write(
        project.paths.root().join(&ws.path).join("a.txt"),
        "ours and theirs\n",
    )
    .unwrap();
    project
        .workspaces()
        .record_changes(&agent, id, "resolve")
        .await
        .unwrap();
    let proposal = project
        .reviews()
        .create_proposal(&agent, task, "Sync with the remote", None)
        .await
        .unwrap()
        .detail()
        .unwrap()
        .summary
        .id;
    assert_eq!(proposal, ProposalId::from_number(1).unwrap());
    let revision = integrated(
        project
            .reviews()
            .integrate_proposal(&ana(), proposal, None)
            .await
            .unwrap(),
    );
    assert_eq!(revision, r(4));
    assert_eq!(
        project
            .count(&format!(
                "SELECT COUNT(*) FROM integrations
                 WHERE kind = 'proposal' AND result = 'succeeded' AND remote_commit = '{tip}'"
            ))
            .await,
        1
    );
    assert_eq!(
        project.handle.tasks.get(task).await.unwrap().status,
        TaskStatus::Done
    );

    let pushed = project.push().await;
    assert!(matches!(pushed, PushOutcome::Synced { .. }));
    let merges = git(&remote, &["log", "--merges", "--format=%P", "main"]);
    let parents: Vec<&str> = merges.split_whitespace().collect();
    assert_eq!(parents.len(), 2);
    assert_eq!(parents[1], tip);
    assert_eq!(git(&remote, &["show", "main:a.txt"]), "ours and theirs\n");
    assert!(matches!(
        project.pull().await.unwrap(),
        PullOutcome::UpToDate { .. }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_on_integrate_pushes_after_an_integration_and_records_a_failure() {
    let host = Host::new();
    let project = host.project().await;
    let remote = host.remote();
    project.set_remote(&remote, true);
    let mut bus = project.handle.bus.subscribe();

    let first = project.integrate_task(&[("a.txt", "one\n")]).await;

    let pushed = next_event(&mut bus, types::REMOTE_PUSHED).await;
    assert_eq!(pushed.actor.as_str(), "fold");
    assert_eq!(pushed.data["last"], first.to_string());
    assert_eq!(git(&remote, &["show", "main:a.txt"]), "one\n");
    let status = git_status(&project.handle).await.unwrap();
    let origin = &status.remotes[0];
    assert_eq!(
        origin.pushed.as_ref().map(|pushed| pushed.revision),
        Some(first)
    );
    assert_eq!(origin.failed, None);

    // A missing local path would become a new repository, so a plain file stands in for a broken remote.
    fs::remove_dir_all(&remote).unwrap();
    fs::write(&remote, "not a repository\n").unwrap();
    let second = project.integrate_task(&[("a.txt", "two\n")]).await;

    let failed = next_event(&mut bus, types::REMOTE_PUSH_FAILED).await;
    assert_eq!(failed.actor.as_str(), "fold");
    assert_eq!(
        failed.data,
        json!({"remote": "origin", "branch": "main", "code": codes::GIT_FAILED})
    );
    let status = git_status(&project.handle).await.unwrap();
    let origin = &status.remotes[0];
    assert_eq!(
        origin.pushed.as_ref().map(|pushed| pushed.revision),
        Some(first)
    );
    assert_eq!(
        origin.failed.as_ref().map(|failed| failed.code.as_str()),
        Some(codes::GIT_FAILED)
    );
    assert_eq!(project.head().await, second);
    assert_eq!(
        project.files(second).await.last(),
        Some(&("a.txt".to_owned(), "two\n".to_owned()))
    );
    assert_eq!(project.count("SELECT COUNT(*) FROM completions").await, 2);
    let task = TaskId::from_number(2).unwrap();
    assert_eq!(
        project.handle.tasks.get(task).await.unwrap().status,
        TaskStatus::Done
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_on_integrate_still_pushes_while_the_agents_are_paused() {
    let host = Host::new();
    let project = host.project().await;
    let remote = host.remote();
    project.set_remote(&remote, true);
    project.handle.agents().pause_project(&ana()).await.unwrap();
    let mut bus = project.handle.bus.subscribe();

    let first = project.integrate_task(&[("a.txt", "one\n")]).await;

    let pushed = next_event(&mut bus, types::REMOTE_PUSHED).await;
    assert_eq!(pushed.data["last"], first.to_string());
    assert_eq!(git(&remote, &["show", "main:a.txt"]), "one\n");
    let pause = project.handle.agents().pause_state().await.unwrap();
    assert!(pause.paused_since.is_some());
}

#[tokio::test]
async fn the_socket_route_pulls_for_the_named_human() {
    let host = Host::new();
    let project = host.project().await;
    let remote = host.remote();
    let daemon_config = DaemonConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        socket: host.path("run").join("fold.sock"),
        data_dir: host.paths.registry_db().parent().unwrap().to_owned(),
        runtime_dir: host.path("run"),
        web_dir: None,
        session_lifetime: fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME,
    };
    let daemon = Daemon::start(daemon_config, Arc::new(host.clock.clone()))
        .await
        .unwrap();
    let state = daemon.state().clone();
    let path = format!("/api/v1/projects/{}/pull", project.slug);
    let call = |router: axum::Router, human: Option<&str>| {
        let mut request = Request::builder()
            .header(CLIENT_HEADER, "test")
            .method(Method::POST)
            .uri(&path);
        if let Some(human) = human {
            request = request.header("x-fold-human", human);
        }
        let request = request.body(Body::empty()).unwrap();
        async move {
            let response = router.oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            (status, serde_json::from_slice::<Value>(&bytes).unwrap())
        }
    };
    let (status, body) = call(fold_daemon::http::router(state.clone()), Some("ana")).await;
    assert_eq!(
        (status, &body["error"]["code"]),
        (StatusCode::NOT_FOUND, &Value::from("not_found"))
    );
    let (status, body) = call(socket_router(state.clone()), None).await;
    assert_eq!(
        (status, &body["error"]["code"]),
        (
            StatusCode::UNAUTHORIZED,
            &Value::from("human_name_required")
        )
    );
    let (status, body) = call(socket_router(state.clone()), Some("ana")).await;
    assert_eq!(
        (status, &body["error"]["code"]),
        (StatusCode::CONFLICT, &Value::from(codes::REMOTE_NOT_SET))
    );

    project.set_remote(&remote, false);
    project.push().await;
    let clone = host.clone_remote(&remote);
    let tip = remote_commit(&clone, &[("b.txt", "theirs\n")]);
    let (status, body) = call(socket_router(state.clone()), Some("ana")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({
            "outcome": "integrated",
            "remote": "origin",
            "branch": "main",
            "remote_commit": tip,
            "revision": "r2",
            "task": null,
            "workspace": null,
            "conflicted": [],
            "created": false,
        })
    );
    let (status, body) = call(socket_router(state), Some("ana")).await;
    assert_eq!(
        (status, &body["outcome"]),
        (StatusCode::OK, &Value::from("up_to_date"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_remote_that_never_answers_fails_at_the_deadline_and_frees_both_locks() {
    let host = Host::new();
    let project = host.project().await;
    // The listener accepts and never answers.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "git://127.0.0.1:{}/x.git",
        listener.local_addr().unwrap().port()
    );
    project.set_remote(Path::new(&url), false);
    let git = Git::default().with_network_timeout(Duration::from_millis(500));
    // A second push and a pull get both locks again only when the first push let them go.
    for _ in 0..2 {
        let pushed = tokio::time::timeout(
            Duration::from_secs(30),
            push(&project.handle, &ana(), &git, None),
        )
        .await
        .expect("the push outlived the deadline");
        assert_eq!(pushed.unwrap_err().code(), Some(codes::GIT_FAILED));
    }
    let pulled = tokio::time::timeout(
        Duration::from_secs(30),
        pull(&project.handle, &ana(), &git, None),
    )
    .await
    .expect("the pull outlived the deadline");
    assert_eq!(pulled.unwrap_err().code(), Some(codes::GIT_FAILED));
    drop(listener);
}
