use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::revision::Revision;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    Actor, EventFilter, HostPaths, HumanName, IntegrationId, ManualClock, ProjectPaths, RevisionId,
    Timestamp, codes, events::types,
};
use fold_daemon::http::socket_router;
use fold_daemon::projects::ProjectHandle;
use fold_daemon::services::events::BusMessage;
use fold_daemon::services::projects::{PushOutcome, init_project, init_project_from_git, push};
use fold_daemon::services::workspaces::commit_message;
use fold_daemon::{Daemon, DaemonConfig};
use fold_store::StoreError;
use fold_store::projects::ProjectRow;
use fold_store::workspaces::{self, advance_main, insert_revision};
use fold_vcs::git::Git;
use fold_vcs::git_sync_map::SYNC_MAP_FILE;
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

#[cfg(test)]
struct Host {
    dir: TempDir,
    paths: HostPaths,
    clock: ManualClock,
}

#[cfg(test)]
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
    fn remote(&self, name: &str) -> PathBuf {
        let remote = self.path(name);
        fs::create_dir(&remote).unwrap();
        git(
            &remote,
            &["init", "--quiet", "--bare", "--initial-branch=main"],
        );
        remote
    }

    async fn plain_project(&self) -> Project {
        let project = init_project(&self.paths, &self.path("app"), None, &self.clock)
            .await
            .unwrap();
        Project::open(&project.slug, &project.root, &self.clock).await
    }
}

#[cfg(test)]
struct Project {
    handle: Arc<ProjectHandle>,
    paths: ProjectPaths,
}

#[cfg(test)]
impl Project {
    async fn open(slug: &str, root: &Path, clock: &ManualClock) -> Self {
        let row = ProjectRow {
            slug: slug.to_owned(),
            name: slug.to_owned(),
            root: root.to_string_lossy().into_owned(),
        };
        let handle = Arc::new(
            ProjectHandle::open(&row, Arc::new(clock.clone()))
                .await
                .unwrap(),
        );
        Self {
            handle,
            paths: ProjectPaths::new(root),
        }
    }

    /// Points `[git]` at `remote`, as `fold remote set` does.
    fn set_remote(&self, remote: &Path) {
        let config = self.paths.config();
        let mut text = fs::read_to_string(&config).unwrap();
        text.push_str(&format!(
            "\n[git]\nremote = {:?}\n",
            remote.to_str().unwrap()
        ));
        fs::write(config, text).unwrap();
    }

    async fn push(&self) -> Result<PushOutcome, Option<&'static str>> {
        self.push_with(&Git::default()).await
    }

    async fn push_with(&self, git: &Git) -> Result<PushOutcome, Option<&'static str>> {
        let actor = Actor::human(HumanName::parse("ana").unwrap());
        push(&self.handle, &actor, git)
            .await
            .map_err(|error| error.code())
    }

    fn map(&self) -> String {
        fs::read_to_string(self.paths.fold_dir().join(SYNC_MAP_FILE)).unwrap_or_default()
    }

    async fn pushed_events(&self) -> Vec<fold_core::Event> {
        let filter = EventFilter {
            types: vec![types::REMOTE_PUSHED],
            ..EventFilter::default()
        };
        fold_store::events::read_after(self.handle.store.read(), 0, &filter, 100)
            .await
            .unwrap()
    }

    async fn seed(&self, sql: &'static str) {
        self.handle
            .store
            .write(async |tx| -> Result<(), StoreError> {
                sqlx::raw_sql(sql).execute(tx.conn()).await?;
                Ok(())
            })
            .await
            .unwrap();
    }

    /// Records a revision on top of `main` holding `files`, and moves `main` to it, as the integration procedure would.
    async fn add_revision(
        &self,
        files: &[(&str, &[u8])],
        integration: Option<IntegrationId>,
    ) -> RevisionId {
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
        let created = 1_700_000_000_000 + i64::from(number.number()) * 60_000;
        let revision = Revision {
            tree: write_initial_tree(&objects, flat).unwrap(),
            parent: Some(head.hash),
            number,
            integration,
            created_at: Timestamp::from_millis(created).unwrap(),
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

    async fn message_of(&self, number: u32) -> String {
        let row = workspaces::get_by_number(
            self.handle.store.read(),
            RevisionId::from_number(number).unwrap(),
        )
        .await
        .unwrap();
        commit_message(&row)
    }
}

#[cfg(test)]
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

/// The first-parent history of `main`, newest first.
#[cfg(test)]
fn log(repo: &Path) -> Vec<String> {
    git(repo, &["log", "--first-parent", "--format=%H", "main"])
        .lines()
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
fn id(number: u32) -> RevisionId {
    RevisionId::from_number(number).unwrap()
}

const PROPOSAL: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'frontend', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'frontend', 0);
INSERT INTO workers (id, agent_id, name, parent_worker_id, created_at) VALUES (2, 1, 'frontend/ui', 1, 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 3, 'three', 'frontend', 0, 0);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 2, 'task', 'ws/t-3', 1, 'active', 0);
INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, created_by, created_at) VALUES (1, 1, 1, 1, 1, 't', 'm', 'frontend/ui', 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 4, 1, 2, 'Login form', 0, 0);
INSERT INTO proposal_versions (id, proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_at) VALUES (1, 1, 1, 1, 't', 1, 0);
INSERT INTO integrations (id, number, kind, proposal_id, version_id, result, actor, created_at) VALUES (1, 1, 'proposal', 1, 1, 'succeeded', 'human:ana', 0);
";

#[tokio::test]
async fn three_revisions_become_three_commits_and_later_ones_go_on_top() {
    let host = Host::new();
    let project = host.plain_project().await;
    let remote = host.remote("remote.git");
    project.set_remote(&remote);
    project.seed(PROPOSAL).await;
    project
        .add_revision(&[("a.txt", b"one\n")], IntegrationId::from_number(1))
        .await;
    project
        .add_revision(&[("a.txt", b"two\n"), ("src/b.rs", b"fn b() {}\n")], None)
        .await;

    let outcome = project.push().await.unwrap();
    let commits = log(&remote);
    assert_eq!(commits.len(), 3);
    assert_eq!(
        outcome,
        PushOutcome::Synced {
            branch: "main".to_owned(),
            first: id(1),
            last: id(3),
            commits: 3,
            head: commits[0].parse().unwrap(),
            sent: true,
        }
    );
    // Newest first: r3, r2, r1.
    let trees = ["a.txt\nsrc/b.rs\n", "a.txt\n", ""];
    for (index, commit) in commits.iter().enumerate() {
        let files = git(&remote, &["ls-tree", "-r", "--name-only", commit]);
        assert_eq!(files, trees[index]);
        let number = u32::try_from(3 - index).unwrap();
        let message = git(&remote, &["log", "-1", "--format=%B", commit]);
        assert_eq!(message, format!("{}\n", project.message_of(number).await));
    }
    assert_eq!(
        git(&remote, &["show", &format!("{}:a.txt", commits[0])]),
        "two\n"
    );
    let people = |commit: &str| {
        git(
            &remote,
            &["log", "-1", "--format=%an <%ae>|%cn <%ce>", commit],
        )
    };
    assert_eq!(
        people(&commits[1]),
        "frontend/ui <frontend.ui@fold.local>|fold <fold@localhost>\n"
    );
    assert_eq!(
        people(&commits[0]),
        "fold <fold@localhost>|fold <fold@localhost>\n"
    );
    let map = project.map();
    assert_eq!(map.lines().count(), 3);
    assert!(map.ends_with(&format!("r3 {}\n", commits[0])));
    let events = project.pushed_events().await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].actor.as_str(), "human:ana");
    assert_eq!(
        events[0].data,
        serde_json::json!({"branch": "main", "first": "r1", "last": "r3", "commits": 3})
    );

    assert_eq!(
        project.push().await.unwrap(),
        PushOutcome::UpToDate {
            branch: "main".to_owned(),
            last: Some(id(3)),
        }
    );
    assert_eq!(project.pushed_events().await.len(), 1);
    assert_eq!(project.map().lines().count(), 3);

    project.add_revision(&[("a.txt", b"three\n")], None).await;
    project.add_revision(&[("a.txt", b"four\n")], None).await;
    let outcome = project.push().await.unwrap();
    assert!(matches!(
        outcome,
        PushOutcome::Synced { commits: 2, first, last, sent: true, .. } if first == id(4) && last == id(5)
    ));
    let after = log(&remote);
    assert_eq!(after.len(), 5);
    assert_eq!(after[2..], commits[..]);
    assert_eq!(project.map().lines().count(), 5);
    assert_eq!(project.pushed_events().await.len(), 2);
    // The mirror holds what it fetched and wrote, with no working tree.
    let mirror = project.paths.fold_dir().join("git");
    assert_eq!(
        git(&mirror, &["rev-parse", "--is-bare-repository"]),
        "true\n"
    );
}

#[tokio::test]
async fn a_commit_from_another_clone_makes_the_push_remote_ahead() {
    let host = Host::new();
    let project = host.plain_project().await;
    let remote = host.remote("remote.git");
    project.set_remote(&remote);
    project.add_revision(&[("a.txt", b"one\n")], None).await;
    project.push().await.unwrap();

    git(
        host.dir.path(),
        &["clone", "--quiet", remote.to_str().unwrap(), "clone"],
    );
    let clone = host.path("clone");
    fs::write(clone.join("b.txt"), b"theirs\n").unwrap();
    git(&clone, &["add", "b.txt"]);
    git(&clone, &["commit", "--quiet", "-m", "theirs"]);
    git(&clone, &["push", "--quiet", "origin", "main"]);
    let remote_log = log(&remote);
    let map = project.map();

    project.add_revision(&[("a.txt", b"two\n")], None).await;
    assert_eq!(project.push().await, Err(Some(codes::REMOTE_AHEAD)));
    assert_eq!(log(&remote), remote_log);
    assert_eq!(project.map(), map);
    assert_eq!(project.pushed_events().await.len(), 1);
}

#[tokio::test]
async fn a_push_needs_a_remote_and_the_git_program() {
    let host = Host::new();
    let project = host.plain_project().await;
    let worker = fold_core::WorkerName::parse("frontend/ui").unwrap();
    let agent = Actor::Agent(fold_core::AgentActor {
        agent: worker.agent(),
        worker,
        tags: Vec::new(),
    });
    let refused = push(&project.handle, &agent, &Git::default())
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some(codes::HUMAN_REQUIRED));
    assert_eq!(project.push().await, Err(Some(codes::REMOTE_NOT_SET)));
    let remote = host.remote("remote.git");
    project.set_remote(&remote);
    let missing = Git::new(host.path("bin/git"));
    assert_eq!(
        project.push_with(&missing).await,
        Err(Some(codes::GIT_NOT_FOUND))
    );
    assert_eq!(project.map(), "");
    assert!(project.pushed_events().await.is_empty());
    let config = project.paths.config();
    fs::write(&config, "[git]\nremote = 7\n").unwrap();
    assert_eq!(project.push().await, Err(Some(codes::INVALID_CONFIG)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_pushes_at_once_send_the_revisions_once() {
    let host = Host::new();
    let project = host.plain_project().await;
    let remote = host.remote("remote.git");
    project.set_remote(&remote);
    project.add_revision(&[("a.txt", b"one\n")], None).await;
    project.add_revision(&[("a.txt", b"two\n")], None).await;
    let (first, second) = tokio::join!(project.push(), project.push());
    let mut synced = 0;
    for outcome in [first.unwrap(), second.unwrap()] {
        match outcome {
            PushOutcome::Synced { commits, sent, .. } => {
                assert_eq!((commits, sent), (3, true));
                synced += 1;
            }
            PushOutcome::UpToDate { last, .. } => assert_eq!(last, Some(id(3))),
        }
    }
    assert_eq!(synced, 1);
    assert_eq!(log(&remote).len(), 3);
    assert_eq!(project.map().lines().count(), 3);
    assert_eq!(project.pushed_events().await.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_caller_that_stops_waiting_still_gets_the_map_and_the_event() {
    let host = Host::new();
    let project = host.plain_project().await;
    let remote = host.remote("remote.git");
    project.set_remote(&remote);
    project.add_revision(&[("a.txt", b"one\n")], None).await;
    let mut bus = project.handle.bus.subscribe();
    // One poll starts the push; the elapsed timeout then drops the caller's future, as a client that goes away does.
    let stopped = tokio::time::timeout(std::time::Duration::ZERO, project.push()).await;
    assert!(stopped.is_err());
    let published = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if let Ok(BusMessage::Durable(event)) = bus.recv().await
                && event.event_type == types::REMOTE_PUSHED
            {
                return event;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(published.data["commits"], 2);
    assert_eq!(project.map().lines().count(), 2);
    assert_eq!(log(&remote).len(), 2);
    assert_eq!(project.pushed_events().await.len(), 1);
}

#[tokio::test]
async fn lost_map_lines_are_recreated_without_sending_again() {
    let host = Host::new();
    let project = host.plain_project().await;
    let remote = host.remote("remote.git");
    project.set_remote(&remote);
    project.add_revision(&[("a.txt", b"one\n")], None).await;
    project.add_revision(&[("a.txt", b"two\n")], None).await;
    project.push().await.unwrap();
    let map = project.map();
    let commits = log(&remote);

    // Keep only r1, as if the map write of the push had been lost after r1 was synced.
    let first = map.lines().next().unwrap();
    fs::write(
        project.paths.fold_dir().join(SYNC_MAP_FILE),
        format!("{first}\n"),
    )
    .unwrap();
    let outcome = project.push().await.unwrap();
    assert_eq!(
        outcome,
        PushOutcome::Synced {
            branch: "main".to_owned(),
            first: id(2),
            last: id(3),
            commits: 2,
            head: commits[0].parse().unwrap(),
            sent: false,
        }
    );
    assert_eq!(project.map(), map);
    assert_eq!(log(&remote), commits);
    let events = project.pushed_events().await;
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].data["first"], "r2");

    fs::remove_file(project.paths.fold_dir().join(SYNC_MAP_FILE)).unwrap();
    let outcome = project.push().await.unwrap();
    assert!(matches!(
        outcome,
        PushOutcome::Synced {
            commits: 3,
            sent: false,
            ..
        }
    ));
    assert_eq!(project.map(), map);
}

#[tokio::test]
async fn a_project_imported_from_git_pushes_on_top_of_the_imported_head() {
    let host = Host::new();
    let work = host.path("work");
    fs::create_dir(&work).unwrap();
    git(&work, &["init", "--quiet", "--initial-branch=main"]);
    fs::write(work.join("README.md"), b"# app\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "--quiet", "-m", "start"]);
    git(
        host.dir.path(),
        &["clone", "--quiet", "--bare", "work", "source.git"],
    );
    let source = host.path("source.git");
    let head = log(&source)[0].clone();
    let imported = init_project_from_git(
        &host.paths,
        &host.path("app"),
        None,
        &source,
        Git::default(),
        &host.clock,
    )
    .await
    .unwrap();
    let project = Project::open(&imported.slug, &imported.root, &host.clock).await;
    project.set_remote(&source);
    assert_eq!(
        project.push().await.unwrap(),
        PushOutcome::UpToDate {
            branch: "main".to_owned(),
            last: Some(id(1)),
        }
    );
    project
        .add_revision(&[("README.md", b"# app v2\n")], None)
        .await;
    project
        .add_revision(&[("README.md", b"# app v3\n")], None)
        .await;
    let outcome = project.push().await.unwrap();
    assert!(matches!(
        outcome,
        PushOutcome::Synced {
            commits: 2,
            sent: true,
            ..
        }
    ));
    let commits = log(&source);
    assert_eq!(commits.len(), 3);
    assert_eq!(commits[2], head);
    assert_eq!(
        git(&source, &["show", &format!("{}:README.md", commits[0])]),
        "# app v3\n"
    );
}

#[tokio::test]
async fn a_remote_the_map_does_not_continue_is_export_diverged() {
    let host = Host::new();
    let project = host.plain_project().await;
    let first = host.remote("first.git");
    project.set_remote(&first);
    project.push().await.unwrap();
    project.add_revision(&[("a.txt", b"one\n")], None).await;
    let other = host.remote("other.git");
    let config = project.paths.config();
    let text = fs::read_to_string(&config)
        .unwrap()
        .replace(first.to_str().unwrap(), other.to_str().unwrap());
    fs::write(&config, text).unwrap();
    // The mirror was made for `first` and still holds its commits, so the fetch from an empty `other` finds them there.
    fs::remove_dir_all(project.paths.fold_dir().join("git")).unwrap();
    assert_eq!(project.push().await, Err(Some(codes::EXPORT_DIVERGED)));
    assert!(log_is_empty(&other));
    assert_eq!(project.map().lines().count(), 1);
}

#[cfg(test)]
fn log_is_empty(repo: &Path) -> bool {
    git(repo, &["for-each-ref"]).is_empty()
}

#[tokio::test]
async fn the_push_route_is_socket_only_and_names_the_human() {
    let host = Host::new();
    let project = init_project(&host.paths, &host.path("app"), None, &host.clock)
        .await
        .unwrap();
    let remote = host.remote("remote.git");
    let config = ProjectPaths::new(&project.root).config();
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str(&format!(
        "\n[git]\nremote = {:?}\n",
        remote.to_str().unwrap()
    ));
    fs::write(&config, text).unwrap();

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
    let path = format!("/api/v1/projects/{}/push", project.slug);
    let call = |router: axum::Router, human: Option<&str>| {
        let mut request = Request::builder().method(Method::POST).uri(&path);
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
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "pushed");
    assert_eq!(body["branch"], "main");
    assert_eq!(
        (&body["first"], &body["last"]),
        (&Value::from("r1"), &Value::from("r1"))
    );
    assert_eq!(body["commits"], 1);
    assert_eq!(body["head"], log(&remote)[0].as_str());
    let (status, body) = call(socket_router(state.clone()), Some("ana")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["outcome"], "up_to_date");
    assert_eq!(body["last"], "r1");
    assert!(body["head"].is_null() && body["first"].is_null());

    fs::write(&config, "").unwrap();
    let (status, body) = call(socket_router(state.clone()), Some("ana")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "remote_not_set");
    assert!(
        body["error"]["details"]["hint"]
            .as_str()
            .unwrap()
            .contains("fold remote set")
    );
    daemon.shutdown().await;
}
