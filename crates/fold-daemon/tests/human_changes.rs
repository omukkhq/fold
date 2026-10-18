#![cfg(test)]

use std::path::{Path, PathBuf};
use std::process::Command;

use fold_core::events::types;
use fold_core::roles::Role;
use fold_core::{Actor, EventFilter, HumanName, codes};
use fold_daemon::services::agents::{NeedsYouKind, needs_you};
use fold_daemon::services::projects::{
    ProjectError, PullOutcome, RecordOutcome, commit_main, pull, record,
};
use fold_store::testing::eventually;
use fold_vcs::git::Git;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, WAIT};
use crate::common::roles::give_role;

/// `git` in `dir` as a person runs it there, without the user's configuration or hooks.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Ana")
        .env("GIT_AUTHOR_EMAIL", "ana@example.com")
        .env("GIT_COMMITTER_NAME", "Ana")
        .env("GIT_COMMITTER_EMAIL", "ana@example.com")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn ana() -> Actor {
    Actor::human(HumanName::parse("ana").unwrap())
}

fn root(h: &Harness) -> PathBuf {
    PathBuf::from(h.root())
}

fn read(h: &Harness, path: &str) -> String {
    std::fs::read_to_string(root(h).join(path)).unwrap()
}

fn write(dir: &Path, path: &str, text: &str) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

fn tip(h: &Harness) -> String {
    git(&root(h), &["rev-parse", "refs/heads/main"])
        .trim()
        .to_owned()
}

/// The tracked files of the project folder that differ from its branch: empty once the folder follows main. Read without Git's optional index lock, so a poll never makes the commit writer's update of the folder fail.
fn changed(h: &Harness) -> String {
    git(
        &root(h),
        &[
            "--no-optional-locks",
            "status",
            "--porcelain",
            "--untracked-files=no",
        ],
    )
}

/// Waits until the Git job has written `r1`'s commit and created the branch on it.
pub async fn branch_born(h: &Harness) {
    let root = root(h);
    eventually(WAIT, async || {
        let born = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "--verify", "--quiet", "refs/heads/main"])
            .output()
            .unwrap()
            .status
            .success();
        if born { Ok(()) } else { Err("no branch") }
    })
    .await;
}

/// The next post into `inbox` that holds `needle`; the ones before it are about other events.
pub async fn notice_with(inbox: &mut crate::common::acceptance::FakeInbox, needle: &str) -> String {
    loop {
        let post = inbox.next().await;
        if post.contains(needle) {
            return post;
        }
    }
}

pub async fn record_as_ana(h: &Harness) -> Result<RecordOutcome, ProjectError> {
    record(&h.handle, &ana(), &Git::default()).await
}

/// The file `path` of `revision` as main holds it.
pub async fn file_of(h: &Harness, revision: &str, path: &str) -> Option<String> {
    let number: fold_core::RevisionId = revision.parse().unwrap();
    let tree = fold_store::workspaces::get_by_number(h.handle.store.read(), number)
        .await
        .unwrap()
        .tree_hash;
    let objects = fold_vcs::objects::ObjectStore::open(&h.handle.paths.fold_dir()).unwrap();
    let flat = fold_vcs::compare::load_flat(&objects, &tree).unwrap();
    let entry = flat.get(path.as_bytes())?;
    let mut text = String::new();
    std::io::Read::read_to_string(&mut objects.open_blob(&entry.hash).unwrap(), &mut text).unwrap();
    Some(text)
}

/// `agent` builds `task` with `files` and the review lead integrates it.
pub async fn integrate(h: &Harness, agent: &str, files: &[(&str, &str)]) -> String {
    let task = h.create_task(agent, "Agent work").await;
    let work = h.start_work(agent, &task).await;
    work.write(files);
    let proposal = h.propose(agent, &work, "Agent work").await;
    h.approve(&proposal).await;
    let integrated = h
        .call(
            "review",
            "integrate_proposal",
            json!({"id": proposal, "expected_version": 1}),
        )
        .await;
    integrated["revision"].as_str().unwrap().to_owned()
}

async fn human_change_states(h: &Harness) -> Vec<String> {
    sqlx::query_scalar("SELECT state FROM human_changes ORDER BY id")
        .fetch_all(h.handle.store.read())
        .await
        .unwrap()
}

async fn stored(h: &Harness, kind: fold_core::EventType) -> Vec<fold_core::Event> {
    let filter = EventFilter {
        types: vec![kind],
        ..EventFilter::default()
    };
    fold_store::events::read_after(h.handle.store.read(), 0, &filter, 100)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_is_recorded_without_fld_record() {
    let h = Harness::start().await;
    branch_born(&h).await;
    write(&root(&h), "README.md", "# mine\n");
    write(&root(&h), "notes.md", "half done\n");
    // An agent's integration in the meantime records nothing of the folder, and the folder waits.
    let revision = integrate(&h, "frontend", &[("src.txt", "agent\n")]).await;
    let committed = commit_main(
        &h.handle.store,
        &h.handle.paths,
        &Git::default(),
        h.handle.clock.as_ref(),
    )
    .await
    .unwrap();
    assert!(committed.uncommitted);
    assert_eq!(committed.outside, None);
    assert_eq!(revision, "r2");
    assert_eq!(h.head().await.number.to_string(), "r2");
    assert_eq!(file_of(&h, "r2", "notes.md").await, None);
    assert_eq!(
        file_of(&h, "r2", "README.md").await.as_deref(),
        Some("# demo\n")
    );
    assert!(human_change_states(&h).await.is_empty());
    assert_eq!(read(&h, "README.md"), "# mine\n");
    assert_eq!(read(&h, "notes.md"), "half done\n");

    // Only a human runs it.
    let refused = record(&h.handle, &Actor::System, &Git::default())
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some(codes::HUMAN_REQUIRED));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fld_record_makes_the_folder_one_commit_of_the_human_on_main_and_the_folder_follows() {
    let h = Harness::start().await;
    branch_born(&h).await;
    let before = tip(&h);
    let dir = root(&h);
    git(&dir, &["config", "user.name", "Ana Lima"]);
    git(&dir, &["config", "user.email", "ana@example.com"]);
    write(&dir, "README.md", "# mine\n");
    write(&dir, "docs/plan.md", "the plan\n");
    write(&dir, ".foldignore", "build/\n");
    write(&dir, "build/out.txt", "generated\n");

    let outcome = record_as_ana(&h).await.unwrap();
    let RecordOutcome::Recorded {
        commit,
        revision,
        files,
        touched,
        ..
    } = outcome
    else {
        panic!("a clean record: {outcome:?}");
    };
    assert_eq!(revision.to_string(), "r2");
    assert_eq!(files, [".foldignore", "README.md", "docs/plan.md"]);
    assert!(touched.is_empty());
    assert_eq!(
        file_of(&h, "r2", "docs/plan.md").await.as_deref(),
        Some("the plan\n")
    );
    assert_eq!(file_of(&h, "r2", "build/out.txt").await, None);
    let (kind, actor): (String, String) =
        sqlx::query_as("SELECT kind, actor FROM integrations ORDER BY number DESC LIMIT 1")
            .fetch_one(h.handle.store.read())
            .await
            .unwrap();
    assert_eq!((kind.as_str(), actor.as_str()), ("pull", "human:ana"));
    assert_eq!(human_change_states(&h).await, ["recorded"]);

    // Main did not move meanwhile, so the human's own commit is r2's commit and the branch follows it.
    assert_eq!(tip(&h), commit.as_str());
    assert_eq!(
        git(
            &dir,
            &[
                "log",
                "-1",
                "--format=%an <%ae>|%cn <%ce>|%s|%P",
                commit.as_str()
            ]
        )
        .trim(),
        format!(
            "Ana Lima <ana@example.com>|Ana Lima <ana@example.com>|Changes in the project folder|{before}"
        )
    );
    assert_eq!(changed(&h), "");
    assert_eq!(read(&h, "build/out.txt"), "generated\n");
    let recorded = stored(&h, types::HUMAN_CHANGE_RECORDED).await;
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].actor.as_str(), "human:ana");
    assert_eq!(
        recorded[0].revision.map(|r| r.to_string()).as_deref(),
        Some("r2")
    );
    assert_eq!(recorded[0].data["file_count"], 3);
    assert_eq!(recorded[0].data["commit"], commit.as_str());

    // A second record finds nothing new.
    let again = record_as_ana(&h).await.unwrap();
    assert!(
        matches!(again, RecordOutcome::UpToDate { revision } if revision.to_string() == "r2"),
        "{again:?}"
    );
    assert_eq!(human_change_states(&h).await, ["recorded"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_record_after_main_moved_merges_three_way_and_tells_the_leads_of_touched_tasks() {
    let mut h = Harness::start().await;
    branch_born(&h).await;
    h.connect_tagged("main", &["main"]).await;
    let mut main_inbox = h.register_inbox("main").await;
    let mut lead_inbox = h.register_inbox("backend").await;
    give_role(&h.handle.store, "backend", Role::AreaLead, Some("work")).await;
    let dir = root(&h);
    git(&dir, &["config", "user.name", "Ana Lima"]);
    git(&dir, &["config", "user.email", "ana@example.com"]);
    write(&dir, "README.md", "# mine\n");
    // An agent integrates while the folder holds the human's unrecorded edit, so the folder stays behind.
    integrate(&h, "frontend", &[("src.txt", "agent\n")]).await;
    let after_agent = h.head().await;
    assert!(!dir.join("src.txt").exists());
    // Another open task changes README.md too.
    let created = h
        .call(
            "backend",
            "create_task",
            json!({"title": "Readme work", "labels": ["work"]}),
        )
        .await;
    let task = created["id"].as_str().unwrap().to_owned();
    let work = h.start_work("backend", &task).await;
    work.write(&[("README.md", "# backend\n")]);
    h.call(
        "backend",
        "record_changes",
        json!({"ws": work.workspace, "message": "readme"}),
    )
    .await;

    let outcome = record_as_ana(&h).await.unwrap();
    let RecordOutcome::Recorded {
        commit,
        revision,
        touched,
        ..
    } = outcome
    else {
        panic!("a clean merge: {outcome:?}");
    };
    assert_eq!(revision.to_string(), "r3");
    assert_eq!(touched.len(), 1);
    assert_eq!(touched[0].task.to_string(), task);
    assert_eq!(touched[0].workspace.to_string(), work.workspace);
    assert_eq!(
        file_of(&h, "r3", "README.md").await.as_deref(),
        Some("# mine\n")
    );
    assert_eq!(
        file_of(&h, "r3", "src.txt").await.as_deref(),
        Some("agent\n")
    );

    // The revision's commit merges the human's commit, and the folder follows main with the agent's file.
    let merge = tip(&h);
    let parents = git(&dir, &["log", "-1", "--format=%P", &merge]);
    assert!(parents.trim().ends_with(commit.as_str()), "{parents}");
    assert_eq!(parents.split_whitespace().count(), 2);
    assert_eq!(
        git(&dir, &["log", "-1", "--format=%an", commit.as_str()]).trim(),
        "Ana Lima"
    );
    assert_ne!(merge, commit.as_str());
    assert_eq!(read(&h, "src.txt"), "agent\n");
    assert_eq!(changed(&h), "");
    assert!(h.head().await.number > after_agent.number);

    let to_main = notice_with(&mut main_inbox, "recorded r3").await;
    assert!(
        to_main.contains(&format!(
            "human:ana recorded r3 from the project folder: README.md; touches {task} ({}), whose leads were told; tell the user. Call compare_revisions.",
            work.workspace
        )),
        "{to_main}"
    );
    let to_lead = notice_with(&mut lead_inbox, "changed README.md").await;
    assert!(
        to_lead.contains(&format!(
            "human:ana changed README.md in main as r3, which {task} ({}) also changes. Call get_task.",
            work.workspace
        )),
        "{to_lead}"
    );
    let recorded = stored(&h, types::HUMAN_CHANGE_RECORDED).await;
    assert_eq!(recorded[0].data["leads"], json!(["backend"]));
    assert_eq!(recorded[0].data["tasks"], json!([task]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conflict_waits_in_needs_you_and_the_main_agent_resolves_it_in_its_main_workspace() {
    let mut h = Harness::start().await;
    branch_born(&h).await;
    h.connect_tagged("main", &["main"]).await;
    let mut inbox = h.register_inbox("main").await;
    let dir = root(&h);
    write(&dir, "README.md", "# mine\n");
    integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    let head = h.head().await.number;

    let outcome = record_as_ana(&h).await.unwrap();
    let RecordOutcome::Conflicted {
        commit,
        conflicted,
        created,
        ..
    } = outcome
    else {
        panic!("a conflict: {outcome:?}");
    };
    assert!(created);
    assert_eq!(conflicted.len(), 1);
    assert_eq!(conflicted[0].path, "README.md");
    // Main and the folder stay as they were, and the human's tree is kept for the merge.
    assert_eq!(h.head().await.number, head);
    assert_eq!(read(&h, "README.md"), "# mine\n");
    assert_eq!(human_change_states(&h).await, ["conflicted"]);
    let items = needs_you(&h.handle.store).await.unwrap();
    let item = items
        .iter()
        .find(|item| item.kind == NeedsYouKind::HumanChange)
        .unwrap();
    assert_eq!(item.commit.as_deref(), Some(commit.as_str()));
    assert_eq!(item.paths, ["README.md"]);
    assert_eq!(item.human.as_ref().map(|human| human.as_str()), Some("ana"));
    let notice = notice_with(&mut inbox, "conflicts with main").await;
    assert!(
        notice.contains("human:ana's change conflicts with main in README.md; main did not move; explain it and prepare the merge. Call open_main_workspace."),
        "{notice}"
    );

    // The same folder again writes nothing new.
    let again = record_as_ana(&h).await.unwrap();
    assert!(
        matches!(&again, RecordOutcome::Conflicted { created: false, commit: same, .. } if *same == commit),
        "{again:?}"
    );
    assert_eq!(human_change_states(&h).await, ["conflicted"]);

    // The main agent prepares the merge with the user in its main workspace.
    let opened = h.call("main", "open_main_workspace", json!({})).await;
    assert_eq!(opened["human_change"]["held"], true, "{opened}");
    assert_eq!(opened["human_change"]["commit"], commit.as_str());
    assert_eq!(opened["human_change"]["conflicted"], json!(["README.md"]));
    let ws = opened["workspace"].as_str().unwrap().to_owned();
    let main_dir = dir.join(opened["path"].as_str().unwrap());
    let marked = std::fs::read_to_string(main_dir.join("README.md")).unwrap();
    assert!(
        marked.contains("# mine") && marked.contains("# fold") && marked.contains("<<<<<<<"),
        "{marked}"
    );
    let early = h
        .refused(
            "main",
            "record_changes",
            json!({"ws": ws, "message": "merge"}),
        )
        .await;
    assert_eq!(early["code"], codes::UNRESOLVED_CONFLICTS, "{early}");
    std::fs::write(main_dir.join("README.md"), "# mine and fold\n").unwrap();
    let recorded = h
        .call(
            "main",
            "record_changes",
            json!({"ws": ws, "message": "merge"}),
        )
        .await;
    let changeset = recorded["changeset"].as_str().unwrap().to_owned();
    let submitted = h
        .call(
            "main",
            "submit_main_change",
            json!({"changeset": changeset, "message": "Merge Ana's readme"}),
        )
        .await;
    assert_eq!(submitted["result"], "succeeded", "{submitted}");
    let resolved_at = submitted["revision"].as_str().unwrap().to_owned();
    assert_eq!(human_change_states(&h).await, ["resolved"]);
    assert!(
        needs_you(&h.handle.store)
            .await
            .unwrap()
            .iter()
            .all(|item| item.kind != NeedsYouKind::HumanChange)
    );
    assert_eq!(stored(&h, types::HUMAN_CHANGE_RESOLVED).await.len(), 1);

    // The resolution's commit has the human's commit as its second parent, so the folder follows main again.
    eventually(WAIT, async || {
        let readme = read(&h, "README.md");
        if readme == "# mine and fold\n" && changed(&h).is_empty() {
            Ok(())
        } else {
            Err(readme)
        }
    })
    .await;
    let parents = git(&dir, &["log", "-1", "--format=%P", "refs/heads/main"]);
    assert!(parents.trim().ends_with(commit.as_str()), "{parents}");
    assert_eq!(
        file_of(&h, &resolved_at, "README.md").await.as_deref(),
        Some("# mine and fold\n")
    );
}

/// The leads of the open tasks that change the same files hear of a conflicted human change when the main agent's merge brings it onto main.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_leads_of_touched_tasks_hear_when_the_main_agent_resolves_a_human_change() {
    let mut h = Harness::start().await;
    branch_born(&h).await;
    h.connect_tagged("main", &["main"]).await;
    let mut lead_inbox = h.register_inbox("backend").await;
    give_role(&h.handle.store, "backend", Role::AreaLead, Some("work")).await;
    let dir = root(&h);
    write(&dir, "README.md", "# mine\n");
    integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    assert!(matches!(
        record_as_ana(&h).await.unwrap(),
        RecordOutcome::Conflicted { created: true, .. }
    ));
    let created = h
        .call(
            "backend",
            "create_task",
            json!({"title": "Readme work", "labels": ["work"]}),
        )
        .await;
    let task = created["id"].as_str().unwrap().to_owned();
    let work = h.start_work("backend", &task).await;
    work.write(&[("README.md", "# backend\n")]);
    h.call(
        "backend",
        "record_changes",
        json!({"ws": work.workspace, "message": "readme"}),
    )
    .await;
    let before = h.call("backend", "get_task", json!({"id": task})).await["status"].clone();

    let opened = h.call("main", "open_main_workspace", json!({})).await;
    let ws = opened["workspace"].as_str().unwrap().to_owned();
    let main_dir = dir.join(opened["path"].as_str().unwrap());
    std::fs::write(main_dir.join("README.md"), "# mine and fold\n").unwrap();
    let changeset = h
        .call(
            "main",
            "record_changes",
            json!({"ws": ws, "message": "merge"}),
        )
        .await["changeset"]
        .as_str()
        .unwrap()
        .to_owned();
    let submitted = h
        .call(
            "main",
            "submit_main_change",
            json!({"changeset": changeset, "message": "Merge Ana's readme"}),
        )
        .await;
    assert_eq!(submitted["result"], "succeeded", "{submitted}");
    let revision = submitted["revision"].as_str().unwrap().to_owned();
    let to_lead = notice_with(&mut lead_inbox, "changed README.md").await;
    assert!(
        to_lead.contains(&format!(
            "human:ana changed README.md in main as {revision}, which {task} ({}) also changes. Call get_task.",
            work.workspace
        )),
        "{to_lead}"
    );
    let resolved = stored(&h, types::HUMAN_CHANGE_RESOLVED).await;
    assert_eq!(resolved[0].data["leads"], json!(["backend"]));
    assert_eq!(resolved[0].data["human"], "human:ana");
    // The notice changes no task state.
    let got = h.call("backend", "get_task", json!({"id": task})).await;
    assert_eq!(got["status"], before, "{got}");
}

/// An integration whose tree holds the waiting human change already, such as an agent's proposal that took the human's lines, resolves it: it leaves Needs you, a merge prepared for it is refused, and the folder follows main through the human's commit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_integration_that_holds_a_waiting_human_change_resolves_it() {
    let mut h = Harness::start().await;
    branch_born(&h).await;
    h.connect_tagged("main", &["main"]).await;
    let mut inbox = h.register_inbox("main").await;
    let dir = root(&h);
    write(&dir, "README.md", "# mine\n");
    integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    let RecordOutcome::Conflicted { commit, .. } = record_as_ana(&h).await.unwrap() else {
        panic!("a conflict");
    };
    let opened = h.call("main", "open_main_workspace", json!({})).await;
    let ws = opened["workspace"].as_str().unwrap().to_owned();
    let main_dir = dir.join(opened["path"].as_str().unwrap());
    std::fs::write(main_dir.join("README.md"), "# mine and fold\n").unwrap();
    let changeset = h
        .call(
            "main",
            "record_changes",
            json!({"ws": ws, "message": "merge"}),
        )
        .await["changeset"]
        .as_str()
        .unwrap()
        .to_owned();

    // An agent integrates the human's own line.
    let revision = integrate(&h, "backend", &[("README.md", "# mine\n")]).await;
    assert_eq!(human_change_states(&h).await, ["resolved"]);
    assert!(
        needs_you(&h.handle.store)
            .await
            .unwrap()
            .iter()
            .all(|item| item.kind != NeedsYouKind::HumanChange)
    );
    let resolved = stored(&h, types::HUMAN_CHANGE_RESOLVED).await;
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].data["commit"], commit.as_str());
    assert_eq!(
        resolved[0].revision.map(|r| r.to_string()).as_deref(),
        Some(revision.as_str())
    );
    // The review lead integrated it, so the main agent hears that the change no longer waits.
    let notice = notice_with(&mut inbox, "waiting change is resolved").await;
    assert!(
        notice.contains(&format!("fold: human:ana's waiting change is resolved: main holds it as {revision}; tell the user. Call compare_revisions.")),
        "{notice}"
    );
    let refused = h
        .refused(
            "main",
            "submit_main_change",
            json!({"changeset": changeset, "message": "Merge"}),
        )
        .await;
    assert_eq!(
        refused["details"]["reason"], "human_change_resolved",
        "{refused}"
    );
    eventually(WAIT, async || {
        if changed(&h).is_empty() {
            Ok(())
        } else {
            Err(changed(&h))
        }
    })
    .await;
    let parents = git(&dir, &["log", "-1", "--format=%P", "refs/heads/main"]);
    assert!(parents.trim().ends_with(commit.as_str()), "{parents}");
    assert!(matches!(
        record_as_ana(&h).await.unwrap(),
        RecordOutcome::UpToDate { .. }
    ));
}

/// A pull whose remote commit holds the waiting human change resolves it, as any integration does: the main agent hears of it, and the revision's commit merges the pulled commit and then the human's commit, so the folder follows main through the human's commit. When main moved after the remote's commit, the revision is a merge of the pull; otherwise the remote only added to main, and the revision still gets a commit of its own rather than the remote commit, which does not hold the human's commit.
async fn a_pull_that_holds_the_waiting_human_change(main_moved: bool) {
    let mut h = Harness::start().await;
    branch_born(&h).await;
    h.connect_tagged("main", &["main"]).await;
    let mut inbox = h.register_inbox("main").await;
    let dir = root(&h);
    write(&dir, "README.md", "# mine\n");
    integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    let RecordOutcome::Conflicted { commit, .. } = record_as_ana(&h).await.unwrap() else {
        panic!("a conflict");
    };

    // The remote holds main's history and a commit on it that takes the human's line.
    let outside = tempfile::TempDir::new().unwrap();
    let remote = outside.path().join("remote.git");
    let remote_url = remote.to_str().unwrap();
    git(
        outside.path(),
        &[
            "init",
            "--quiet",
            "--bare",
            "--initial-branch=main",
            remote_url,
        ],
    );
    git(
        &dir,
        &[
            "push",
            "--quiet",
            remote_url,
            "refs/fold/head:refs/heads/main",
        ],
    );
    let clone = outside.path().join("clone");
    git(
        outside.path(),
        &["clone", "--quiet", remote_url, clone.to_str().unwrap()],
    );
    write(&clone, "README.md", "# mine\n");
    git(&clone, &["commit", "--quiet", "-am", "Take Ana's readme"]);
    git(&clone, &["push", "--quiet", "origin", "main"]);
    let pulled = git(&clone, &["rev-parse", "HEAD"]).trim().to_owned();
    if main_moved {
        integrate(&h, "backend", &[("notes.md", "fold\n")]).await;
    }
    let config = h.handle.paths.config();
    let mut text = std::fs::read_to_string(&config).unwrap_or_default();
    text.push_str(&format!(
        "\n[remotes.origin]\nurl = {remote_url:?}\nbranch = \"main\"\npush_on_integrate = false\n"
    ));
    std::fs::write(&config, text).unwrap();

    let outcome = pull(&h.handle, &ana(), &Git::default(), Some("origin"))
        .await
        .unwrap();
    let PullOutcome::Integrated { revision, .. } = outcome else {
        panic!("a clean pull: {outcome:?}");
    };
    assert_eq!(human_change_states(&h).await, ["resolved"]);
    let resolved = stored(&h, types::HUMAN_CHANGE_RESOLVED).await;
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].data["commit"], commit.as_str());
    assert_eq!(resolved[0].revision, Some(revision));
    let notice = notice_with(&mut inbox, "waiting change is resolved").await;
    assert!(
        notice.contains(&format!("fold: human:ana's waiting change is resolved: main holds it as {revision}; tell the user. Call compare_revisions.")),
        "{notice}"
    );

    eventually(WAIT, async || {
        let readme = read(&h, "README.md");
        if readme == "# mine\n" && changed(&h).is_empty() {
            Ok(())
        } else {
            Err((readme, changed(&h)))
        }
    })
    .await;
    assert_eq!(root(&h).join("notes.md").exists(), main_moved);
    let parents = git(&dir, &["log", "-1", "--format=%P", "refs/heads/main"]);
    let parents: Vec<&str> = parents.split_whitespace().collect();
    assert_eq!(parents.len(), 3, "{parents:?}");
    assert_eq!(parents[1..], [pulled.as_str(), commit.as_str()]);
    assert!(matches!(
        record_as_ana(&h).await.unwrap(),
        RecordOutcome::UpToDate { .. }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pull_that_holds_the_waiting_human_change_resolves_it_and_the_folder_follows() {
    a_pull_that_holds_the_waiting_human_change(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pull_that_only_adds_the_waiting_human_change_still_merges_the_humans_commit() {
    a_pull_that_holds_the_waiting_human_change(false).await;
}

/// While the folder holds more than the recorded human change, the commit writer leaves it and its index as the human left them, at every run; the next record brings the rest in and the folder follows main.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_commit_writer_keeps_the_index_of_a_folder_that_holds_more_than_the_recorded_change() {
    let mut h = Harness::start().await;
    branch_born(&h).await;
    h.connect_tagged("main", &["main"]).await;
    let dir = root(&h);
    write(&dir, "README.md", "# mine\n");
    write(&dir, "plan.md", "one\n");
    integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    assert!(matches!(
        record_as_ana(&h).await.unwrap(),
        RecordOutcome::Conflicted { created: true, .. }
    ));
    let tip_before = tip(&h);

    // The human goes on while the main agent resolves the conflict, and stages a new file.
    write(&dir, "plan.md", "two\n");
    write(&dir, "notes.md", "draft\n");
    git(&dir, &["add", "notes.md"]);
    let opened = h.call("main", "open_main_workspace", json!({})).await;
    let ws = opened["workspace"].as_str().unwrap().to_owned();
    let main_dir = dir.join(opened["path"].as_str().unwrap());
    std::fs::write(main_dir.join("README.md"), "# mine and fold\n").unwrap();
    let changeset = h
        .call(
            "main",
            "record_changes",
            json!({"ws": ws, "message": "merge"}),
        )
        .await["changeset"]
        .as_str()
        .unwrap()
        .to_owned();
    let submitted = h
        .call(
            "main",
            "submit_main_change",
            json!({"changeset": changeset, "message": "Merge Ana's readme"}),
        )
        .await;
    assert_eq!(submitted["result"], "succeeded", "{submitted}");
    let committed = commit_main(
        &h.handle.store,
        &h.handle.paths,
        &Git::default(),
        h.handle.clock.as_ref(),
    )
    .await
    .unwrap();
    assert!(committed.uncommitted);
    assert_eq!(tip(&h), tip_before);
    assert_eq!(
        git(&dir, &["diff", "--cached", "--name-only"]),
        "notes.md\n"
    );
    assert_eq!(read(&h, "plan.md"), "two\n");

    let outcome = record_as_ana(&h).await.unwrap();
    let RecordOutcome::Recorded { files, .. } = outcome else {
        panic!("a clean record: {outcome:?}");
    };
    assert_eq!(files, ["notes.md", "plan.md"]);
    eventually(WAIT, async || {
        let readme = read(&h, "README.md");
        if readme == "# mine and fold\n" && changed(&h).is_empty() {
            Ok(())
        } else {
            Err(readme)
        }
    })
    .await;
    assert_eq!(git(&dir, &["diff", "--cached", "--name-only"]), "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_newer_record_replaces_a_waiting_conflict_and_the_old_merge_is_refused() {
    let mut h = Harness::start().await;
    branch_born(&h).await;
    h.connect_tagged("main", &["main"]).await;
    let dir = root(&h);
    write(&dir, "README.md", "# mine\n");
    integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    assert!(matches!(
        record_as_ana(&h).await.unwrap(),
        RecordOutcome::Conflicted { created: true, .. }
    ));
    let opened = h.call("main", "open_main_workspace", json!({})).await;
    let ws = opened["workspace"].as_str().unwrap().to_owned();
    let main_dir = dir.join(opened["path"].as_str().unwrap());
    std::fs::write(main_dir.join("README.md"), "# both\n").unwrap();
    let changeset = h
        .call(
            "main",
            "record_changes",
            json!({"ws": ws, "message": "merge"}),
        )
        .await["changeset"]
        .as_str()
        .unwrap()
        .to_owned();

    // The human takes main's line and records again: that merges cleanly and replaces the waiting change.
    write(&dir, "README.md", "# fold\n");
    write(&dir, "extra.md", "more\n");
    let outcome = record_as_ana(&h).await.unwrap();
    assert!(
        matches!(outcome, RecordOutcome::Recorded { .. }),
        "{outcome:?}"
    );
    assert_eq!(human_change_states(&h).await, ["superseded", "recorded"]);
    let refused = h
        .refused(
            "main",
            "submit_main_change",
            json!({"changeset": changeset, "message": "Merge"}),
        )
        .await;
    assert_eq!(
        refused["details"]["reason"], "human_change_superseded",
        "{refused}"
    );
    assert_eq!(read(&h, "extra.md"), "more\n");
    assert_eq!(changed(&h), "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_folder_with_commits_fold_did_not_write_is_refused_until_fld_pull() {
    let h = Harness::start().await;
    branch_born(&h).await;
    let dir = root(&h);
    write(&dir, "committed.md", "mine\n");
    git(&dir, &["add", "committed.md"]);
    git(&dir, &["commit", "--quiet", "-m", "mine"]);
    write(&dir, "README.md", "# edited\n");
    let refused = record_as_ana(&h).await.unwrap_err();
    assert_eq!(refused.code(), Some(codes::INVALID_REQUEST));
    let ProjectError::Fold(refused) = refused else {
        panic!("a fold error");
    };
    assert_eq!(refused.details().unwrap()["reason"], "outside_commits");
    assert!(refused.hint().unwrap().contains("fld pull"));
    assert_eq!(h.head().await.number.to_string(), "r1");
    assert!(human_change_states(&h).await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_socket_route_records_for_the_named_human() {
    let h = Harness::start().await;
    branch_born(&h).await;
    write(&root(&h), "docs/plan.md", "the plan\n");
    let (status, body): (_, Value) = h
        .socket(
            axum::http::Method::POST,
            "/api/v1/projects/demo/record",
            None,
        )
        .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "recorded");
    assert_eq!(body["revision"], "r2");
    assert_eq!(body["files"], json!(["docs/plan.md"]));
    let (_, again) = h
        .socket(
            axum::http::Method::POST,
            "/api/v1/projects/demo/record",
            None,
        )
        .await;
    assert_eq!(again["outcome"], "up_to_date");
    assert_eq!(again["commit"], Value::Null);
}
