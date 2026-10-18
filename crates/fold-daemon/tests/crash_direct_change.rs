#![cfg(test)]

use fold_core::events::types;
use fold_core::{Actor, AgentActor, ChangesetId, MAIN_TAG, RevisionId, Tag};
use fold_daemon::services::reviews::{DirectChange, DirectOutcome};
use fold_store::workspaces;

use crate::common::invariants::assert_consistent;
use crate::reviews_support::{Fixture, names};

fn r(number: u32) -> RevisionId {
    RevisionId::from_number(number).unwrap()
}

fn main_actor(fx: &Fixture) -> Actor {
    let worker = &fx.frontend.worker;
    Actor::Agent(AgentActor {
        agent: worker.name.agent(),
        worker: worker.name.clone(),
        tags: vec![Tag::parse(MAIN_TAG).unwrap()],
    })
}

/// The main agent's recorded change of `AGENTS.md` in its main workspace.
async fn recorded(fx: &Fixture, main: &Actor) -> ChangesetId {
    let opened = fx.workspaces.open_main_workspace(main).await.unwrap();
    let dir = fx.project.paths().root().join(&opened.workspace.path);
    std::fs::write(dir.join("AGENTS.md"), "rules\n").unwrap();
    fx.workspaces
        .record_changes(main, opened.workspace.id, "rules")
        .await
        .unwrap()
        .changeset
}

fn change(changeset: ChangesetId) -> DirectChange {
    DirectChange {
        changeset,
        message: "Add the agent rules".to_owned(),
    }
}

async fn head(fx: &Fixture) -> RevisionId {
    workspaces::head(fx.project.store().read())
        .await
        .unwrap()
        .unwrap()
        .number
}

/// Submits with `failpoint` set to return, checks the command reports `failpoint`, and restarts the project as a crash there would.
async fn submit_stopped_at(
    fx: &mut Fixture,
    main: &Actor,
    changeset: ChangesetId,
    failpoint: &str,
) {
    fail::cfg(failpoint, "return").unwrap();
    let result = fx.reviews.integrate_direct(main, change(changeset)).await;
    fail::remove(failpoint);
    match result {
        Err(error) => assert_eq!(error.code(), Some("failpoint"), "{error}"),
        Ok(outcome) => panic!("expected the failpoint, got {outcome:?}"),
    }
    fx.restart().await;
}

/// A crash inside the submit before its commit leaves main, the integration table, the events and the open main workspace as they were; a submit after the restart integrates the same changeset.
async fn stopped_before_commit(failpoint: &str) {
    let scenario = fail::FailScenario::setup();
    let mut fx = Fixture::with_main_frontend().await;
    let main = main_actor(&fx);
    let changeset = recorded(&fx, &main).await;
    let seq = fx.last_seq().await;

    submit_stopped_at(&mut fx, &main, changeset, failpoint).await;

    assert_eq!(head(&fx).await, r(1));
    assert_eq!(fx.count("SELECT COUNT(*) FROM revisions").await, 1);
    assert_eq!(fx.count("SELECT COUNT(*) FROM integrations").await, 0);
    assert_eq!(
        fx.text("SELECT state FROM workspaces WHERE kind = 'main'")
            .await
            .as_deref(),
        Some("active")
    );
    assert_eq!(fx.last_seq().await, seq);
    assert_consistent(&fx.project).await;

    let outcome = fx
        .reviews
        .integrate_direct(&main, change(changeset))
        .await
        .unwrap();
    assert_eq!(outcome, DirectOutcome::Succeeded { revision: r(2) });
    assert_eq!(
        fx.count(
            "SELECT COUNT(*) FROM integrations WHERE kind = 'direct' AND result = 'succeeded'"
        )
        .await,
        1
    );
    assert_consistent(&fx.project).await;
    scenario.teardown();
}

#[tokio::test]
async fn a_crash_after_the_merge_of_a_direct_change_leaves_main_unchanged() {
    stopped_before_commit("integration_after_merge").await;
}

#[tokio::test]
async fn a_crash_after_the_objects_of_a_direct_change_leaves_only_orphan_objects() {
    stopped_before_commit("integration_after_objects").await;
}

#[tokio::test]
async fn a_crash_inside_the_transaction_of_a_direct_change_rolls_all_of_it_back() {
    stopped_before_commit("integration_in_transaction").await;
}

/// A crash after the commit keeps the whole direct change: the revision, main, the closed main workspace and the stored events.
#[tokio::test]
async fn a_crash_after_the_commit_of_a_direct_change_keeps_all_of_it() {
    let scenario = fail::FailScenario::setup();
    let mut fx = Fixture::with_main_frontend().await;
    let main = main_actor(&fx);
    let changeset = recorded(&fx, &main).await;
    let seq = fx.last_seq().await;

    submit_stopped_at(&mut fx, &main, changeset, "integration_after_commit").await;

    assert_eq!(head(&fx).await, r(2));
    assert_eq!(
        fx.text("SELECT state FROM workspaces WHERE kind = 'main'")
            .await
            .as_deref(),
        Some("archived")
    );
    let stored = fx.events_after(seq).await;
    assert_eq!(
        names(&stored),
        [
            types::INTEGRATION_SUCCEEDED.as_str(),
            types::MAIN_ADVANCED.as_str(),
            types::WORKSPACE_ARCHIVED.as_str(),
        ]
    );
    assert_consistent(&fx.project).await;
    let again = fx.reviews.integrate_direct(&main, change(changeset)).await;
    assert_eq!(
        again.unwrap_err().code(),
        Some(fold_core::codes::WORKSPACE_NOT_ACTIVE)
    );
    scenario.teardown();
}
