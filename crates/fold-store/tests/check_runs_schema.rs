use fold_core::object::ObjectHash;
use fold_core::reviews::checks::{Outcome, Purpose, RunReason, RunState, Stage};
use fold_core::{CheckRunId, ProposalId, RevisionId, TaskId, Timestamp, WorkspaceId};
use fold_store::reviews::checks::{self, NewCheckResult, NewCheckRun};
use fold_store::{Store, StoreError};

use crate::common::{exec, open};

/// A task with a live task workspace `ws-1` of the worker `backend`, a revision, and a proposal `P-1` with v1.
const SEED: &str = r#"
INSERT INTO agents (id, name, created_at) VALUES (1, 'backend', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'backend', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'backend', 0, 0);
INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 0);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 1, 'task', 'ws/t-1', 1, 'active', 0);
INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, stats, created_by, created_at) VALUES (1, 1, 1, 1, 1, 't2', 'm', '{"files_added":1,"files_modified":0,"files_deleted":0}', 'backend', 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 1, 'p', 0, 0);
"#;

#[cfg(test)]
async fn seeded() -> (tempfile::TempDir, Store) {
    let (dir, _paths, store) = open().await;
    for statement in SEED.lines().filter(|line| !line.is_empty()) {
        exec(&store, statement).await.unwrap();
    }
    let tree = ObjectHash::from_bytes([7; 32]).to_string();
    exec(
        &store,
        &format!("INSERT INTO proposal_versions (proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (1, 1, 1, '{tree}', 1, 'backend', 0)"),
    )
    .await
    .unwrap();
    (dir, store)
}

#[cfg(test)]
fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).unwrap()
}

#[cfg(test)]
fn run_id(number: u32) -> CheckRunId {
    CheckRunId::from_number(number).unwrap()
}

#[cfg(test)]
fn workspace_run(number: u32) -> NewCheckRun<'static> {
    NewCheckRun {
        id: run_id(number),
        stage: Stage::Proposal,
        purpose: Purpose::Check,
        task: TaskId::from_number(1).unwrap(),
        workspace: WorkspaceId::from_number(1),
        worker_id: Some(1),
        proposal: None,
        version: None,
        base: RevisionId::from_number(1).unwrap(),
        tree: ObjectHash::from_bytes([7; 32]),
        actor: "backend",
        title: None,
        evidence: None,
        batch: None,
    }
}

#[cfg(test)]
fn integration_run(number: u32) -> NewCheckRun<'static> {
    NewCheckRun {
        stage: Stage::Integration,
        purpose: Purpose::Integrate,
        workspace: None,
        worker_id: None,
        proposal: ProposalId::from_number(1),
        version: Some(1),
        tree: ObjectHash::from_bytes([9; 32]),
        actor: "fold",
        ..workspace_run(number)
    }
}

#[cfg(test)]
async fn insert(store: &Store, new: &NewCheckRun<'_>) -> Result<(), StoreError> {
    store
        .write(async |tx| checks::insert_run(tx, new, at(1)).await)
        .await
}

#[cfg(test)]
fn result(position: u32, check: Option<&'static str>, outcome: Outcome) -> NewCheckResult<'static> {
    NewCheckResult {
        position,
        check,
        required: true,
        outcome,
        exit_code: Some(i32::from(outcome != Outcome::Passed)),
        duration_ms: 12,
        log: b"tail",
        log_bytes: 10,
    }
}

#[cfg(test)]
async fn finish(store: &Store, run: u32, state: RunState) -> Result<bool, StoreError> {
    store
        .write(async |tx| {
            checks::insert_result(tx, run_id(run), &result(0, None, Outcome::Passed)).await?;
            checks::insert_result(tx, run_id(run), &result(1, Some("test"), Outcome::Failed))
                .await?;
            checks::finish_run(tx, run_id(run), state, Some(RunReason::CheckFailed), at(5)).await
        })
        .await
}

#[tokio::test]
async fn a_workspace_runs_one_run_at_a_time_and_a_run_ends_once() {
    let (_dir, store) = seeded().await;
    insert(&store, &workspace_run(1)).await.unwrap();
    let second = insert(&store, &workspace_run(2)).await.unwrap_err();
    assert!(second.unique_violation(), "{second}");
    let running = checks::running_in_workspace(store.read(), WorkspaceId::from_number(1).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(running.id, run_id(1));
    assert_eq!(running.state, RunState::Running);
    assert_eq!(running.worker.unwrap().as_str(), "backend");

    assert!(finish(&store, 1, RunState::Failed).await.unwrap());
    let ended = checks::get_run(store.read(), run_id(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ended.state, RunState::Failed);
    assert_eq!(ended.reason, Some(RunReason::CheckFailed));
    assert_eq!(ended.finished_at, Some(at(5)));
    let again = store
        .write(async |tx| checks::finish_run(tx, run_id(1), RunState::Passed, None, at(6)).await)
        .await
        .unwrap();
    assert!(!again, "an ended run keeps its end");
    let trigger = exec(
        &store,
        "UPDATE check_runs SET state = 'passed' WHERE number = 1",
    )
    .await
    .unwrap_err();
    assert!(
        trigger.to_string().contains("check_runs_end_once"),
        "{trigger}"
    );
    let changed = exec(
        &store,
        "UPDATE check_runs SET tree_hash = 'x' WHERE number = 1",
    )
    .await
    .unwrap_err();
    assert!(changed.to_string().contains("check_runs_end_once"));
    let deleted = exec(&store, "DELETE FROM check_runs").await.unwrap_err();
    assert!(deleted.to_string().contains("append-only"));

    insert(&store, &workspace_run(2)).await.unwrap();
    let tracked =
        checks::last_finished_proposal_run(store.read(), WorkspaceId::from_number(1).unwrap())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(tracked.id, run_id(1), "the tracker reads only ended runs");
}

#[tokio::test]
async fn results_are_written_only_while_their_run_goes_and_never_change() {
    let (_dir, store) = seeded().await;
    insert(&store, &workspace_run(1)).await.unwrap();
    assert!(finish(&store, 1, RunState::Failed).await.unwrap());
    let results = checks::list_results(store.read(), run_id(1), true)
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].check, None);
    assert_eq!(results[1].check.as_deref(), Some("test"));
    assert_eq!(results[1].outcome, Outcome::Failed);
    assert_eq!(results[1].log, b"tail");
    assert!(results[1].truncated);
    let bare = checks::list_results(store.read(), run_id(1), false)
        .await
        .unwrap();
    assert!(bare[1].log.is_empty());
    assert_eq!(bare[1].log_bytes, 10);

    let late = store
        .write(async |tx| {
            checks::insert_result(tx, run_id(1), &result(2, Some("late"), Outcome::Passed)).await
        })
        .await
        .unwrap_err();
    assert!(late.to_string().contains("check_results_running_only"));
    for statement in [
        "UPDATE check_results SET outcome = 'passed'",
        "DELETE FROM check_results",
    ] {
        let error = exec(&store, statement).await.unwrap_err();
        assert!(error.to_string().contains("append-only"), "{statement}");
    }
}

#[tokio::test]
async fn an_integration_run_names_its_version_and_runs_alone() {
    let (_dir, store) = seeded().await;
    insert(&store, &integration_run(1)).await.unwrap();
    assert!(
        insert(&store, &integration_run(2))
            .await
            .unwrap_err()
            .unique_violation()
    );
    insert(&store, &workspace_run(3)).await.unwrap();
    let running = checks::running_runs(store.read()).await.unwrap();
    assert_eq!(running.len(), 2);
    assert_eq!(running[0].version, Some(1));
    assert_eq!(running[0].proposal, ProposalId::from_number(1));

    assert!(finish(&store, 1, RunState::Passed).await.unwrap());
    assert!(finish(&store, 3, RunState::Passed).await.unwrap());
    let runs = checks::runs_of_version(
        store.read(),
        TaskId::from_number(1).unwrap(),
        ProposalId::from_number(1).unwrap(),
        1,
    )
    .await
    .unwrap();
    let ids: Vec<_> = runs.iter().map(|run| run.id).collect();
    assert_eq!(
        ids,
        [run_id(3), run_id(1)],
        "the workspace run checked v1's tree"
    );
}

#[tokio::test]
async fn the_columns_match_the_stage_and_purpose() {
    let (_dir, store) = seeded().await;
    let mismatched = NewCheckRun {
        purpose: Purpose::Integrate,
        ..workspace_run(1)
    };
    assert!(insert(&store, &mismatched).await.is_err());
    let untitled = NewCheckRun {
        purpose: Purpose::CreateProposal,
        ..workspace_run(2)
    };
    assert!(insert(&store, &untitled).await.is_err());
    let titled = NewCheckRun {
        purpose: Purpose::CreateProposal,
        title: Some("Add the board"),
        ..workspace_run(3)
    };
    insert(&store, &titled).await.unwrap();
    let missing = store
        .write(async |tx| checks::finish_run(tx, run_id(3), RunState::Running, None, at(2)).await)
        .await;
    assert!(missing.is_err(), "a run never ends as running");
}

#[tokio::test]
async fn a_queued_integration_waits_until_it_ends_once() {
    let (_dir, store) = seeded().await;
    let proposal = ProposalId::from_number(1).unwrap();
    let id = store
        .write(async |tx| {
            checks::insert_integration_request(tx, proposal, 1, Some(1), "human:ana", at(1)).await
        })
        .await
        .unwrap();
    assert!(
        checks::integration_queued(store.read(), proposal, 1)
            .await
            .unwrap()
    );
    let second = store
        .write(async |tx| {
            checks::insert_integration_request(tx, proposal, 1, None, "human:bo", at(1)).await
        })
        .await;
    assert!(second.is_err(), "one waiting request per version");
    let waiting = checks::waiting_integration_requests(store.read())
        .await
        .unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].proposal, proposal);
    assert_eq!(waiting[0].version, 1);
    assert_eq!(waiting[0].expected_version, Some(1));
    assert_eq!(waiting[0].actor, "human:ana");
    let ended = store
        .write(async |tx| checks::end_integration_request(tx, id, at(2)).await)
        .await
        .unwrap();
    assert!(ended);
    let again = store
        .write(async |tx| checks::end_integration_request(tx, id, at(3)).await)
        .await
        .unwrap();
    assert!(!again, "a request ends once");
    assert!(
        !checks::integration_queued(store.read(), proposal, 1)
            .await
            .unwrap()
    );
    assert!(
        checks::waiting_integration_requests(store.read())
            .await
            .unwrap()
            .is_empty()
    );
    let deleted = exec(&store, "DELETE FROM integration_requests")
        .await
        .unwrap_err();
    assert!(deleted.to_string().contains("append-only"));
    let changed = exec(&store, "UPDATE integration_requests SET actor = 'x'")
        .await
        .unwrap_err();
    assert!(
        changed
            .to_string()
            .contains("integration_requests_end_once")
    );
    let missing = store
        .write(async |tx| {
            checks::insert_integration_request(
                tx,
                ProposalId::from_number(9).unwrap(),
                1,
                None,
                "human:ana",
                at(1),
            )
            .await
        })
        .await;
    assert!(missing.is_err());
}
