#![cfg(test)]

use crate::common::intents::Covering as _;
use fold_core::events::types;
use fold_core::reviews::Verdict;
use fold_core::roles::Role;
use fold_core::{Actor, AgentActor, ChangesetId, MAIN_TAG, RevisionId, Tag, codes};
use fold_daemon::services::reviews::{
    DirectChange, DirectOutcome, IntegrationOutcome, ReviewError,
};
use fold_store::agents::WorkerRow;
use fold_store::workspaces::{self, IntegrationKind, WorkspaceKind, WorkspaceState};

use crate::common::invariants::assert_consistent;
use crate::common::roles::give_role;
use crate::reviews_support::{
    Fixture, by, details, human, names, p, refusal, t, workspace_refusal,
};

fn r(number: u32) -> RevisionId {
    RevisionId::from_number(number).unwrap()
}

/// The worker as the session `fld claude` starts acts: with the `main` tag.
fn main_actor(worker: &WorkerRow) -> Actor {
    Actor::Agent(AgentActor {
        agent: worker.name.agent(),
        worker: worker.name.clone(),
        tags: vec![Tag::parse(MAIN_TAG).unwrap()],
    })
}

/// Opens the main workspace for `actor`, writes `files` into it and records them; returns the workspace directory and the changeset.
async fn recorded(
    fx: &Fixture,
    actor: &Actor,
    files: &[(&str, &str)],
) -> (std::path::PathBuf, ChangesetId) {
    let opened = fx.workspaces.open_main_workspace(actor).await.unwrap();
    let dir = fx.project.paths().root().join(&opened.workspace.path);
    for (path, content) in files {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
    }
    let changes = fx
        .workspaces
        .record_changes(actor, opened.workspace.id, "the user's change")
        .await
        .unwrap();
    (dir, changes.changeset)
}

/// The author proposes `T-1` from its workspace, and a reviewer approves and integrates it, so main moves under the main workspace.
async fn integrate_task(fx: &Fixture, author: &Actor) {
    fx.reviews
        .create_proposal_covering(author, t(1), "Readme", Some("tests pass"))
        .await
        .unwrap()
        .detail()
        .unwrap();
    give_role(fx.project.store(), "review", Role::Reviewer, None).await;
    let review = fx.agents.actor(&fx.review.ctx).await.unwrap();
    fx.reviews
        .submit_review(&review, p(1), 1, Verdict::Approve, "ok", Vec::new())
        .await
        .unwrap();
    let outcome = fx
        .reviews
        .integrate_proposal(&review, p(1), Some(1))
        .await
        .unwrap();
    assert!(matches!(outcome, IntegrationOutcome::Succeeded { .. }));
}

fn change(changeset: ChangesetId, message: &str) -> DirectChange {
    DirectChange {
        changeset,
        message: message.to_owned(),
    }
}

async fn submit(
    fx: &Fixture,
    actor: &Actor,
    changeset: ChangesetId,
) -> Result<DirectOutcome, ReviewError> {
    fx.reviews
        .integrate_direct(actor, change(changeset, "Add the agent rules"))
        .await
}

#[tokio::test]
async fn the_main_agent_opens_one_main_workspace_without_a_task_and_reuses_it() {
    let fx = Fixture::with_main_frontend().await;
    let main = main_actor(&fx.frontend.worker);
    let opened = fx.workspaces.open_main_workspace(&main).await.unwrap();
    let view = &opened.workspace;
    assert!(!opened.reused);
    assert_eq!(view.kind, WorkspaceKind::Main);
    assert_eq!(view.task, None);
    assert_eq!(view.path, ".fold/ws/main");
    assert_eq!(view.base, r(1));
    assert_eq!(view.worker.as_ref(), Some(&fx.frontend.worker.name));
    let dir = fx.project.paths().root().join(&view.path);
    assert_eq!(
        std::fs::read(dir.join("README.md")).unwrap(),
        crate::reviews_support::README
    );

    let again = fx.workspaces.open_main_workspace(&main).await.unwrap();
    assert!(again.reused);
    assert_eq!(again.workspace.id, view.id);
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM workspaces WHERE kind = 'main'")
            .await,
        1
    );
    let stored = fx.events_after(0).await;
    let created = stored
        .iter()
        .find(|event| event.event_type == types::WORKSPACE_CREATED)
        .unwrap();
    assert_eq!(created.task, None);
    assert_eq!(created.workspace, Some(view.id));
    assert_eq!(created.data["kind"], "main");

    // The web routes list the main workspace with the task workspaces, with no task and no attention.
    let entry = fx.workspaces.workspace_entry(view.id).await.unwrap();
    assert_eq!(entry.workspace.kind, WorkspaceKind::Main);
    assert_eq!(entry.task_title, None);
    assert!(entry.attention.is_empty());
    let task_ws = fx
        .workspaces
        .create_workspace(&by(&fx.backend.worker), t(1))
        .await
        .unwrap();
    let listed: Vec<_> = fx
        .workspaces
        .list_workspaces()
        .await
        .unwrap()
        .into_iter()
        .map(|entry| (entry.workspace.id, entry.task_title.is_some()))
        .collect();
    assert_eq!(listed, [(view.id, false), (task_ws.workspace.id, true)]);
    assert_consistent(&fx.project).await;
}

#[tokio::test]
async fn a_new_main_session_takes_over_the_main_workspace_with_its_files() {
    let fx = Fixture::with_main_frontend().await;
    let before = main_actor(&fx.frontend.worker);
    let (dir, _) = recorded(&fx, &before, &[("AGENTS.md", "rules\n")]).await;
    let after = main_actor(&fx.review.worker);
    let taken = fx.workspaces.open_main_workspace(&after).await.unwrap();
    assert!(taken.reused);
    assert_eq!(
        taken.workspace.worker.as_ref(),
        Some(&fx.review.worker.name)
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("AGENTS.md")).unwrap(),
        "rules\n"
    );
    let stored = fx.events_after(0).await;
    let adopted = stored
        .iter()
        .find(|event| event.event_type == types::WORKSPACE_ADOPTED)
        .unwrap();
    assert_eq!(adopted.data["reason"], "main_agent");
    assert_eq!(adopted.data["from"], "frontend");
    assert_eq!(adopted.data["to"], "review");
    // The earlier session no longer owns it.
    let refused = fx
        .workspaces
        .record_changes(&before, taken.workspace.id, "more")
        .await;
    assert_eq!(
        workspace_refusal(refused).code(),
        codes::NOT_WORKSPACE_OWNER
    );
}

#[tokio::test]
async fn only_the_main_agent_opens_the_main_workspace_or_submits_to_it() {
    let fx = Fixture::with_main_frontend().await;
    let main = main_actor(&fx.frontend.worker);
    let (_, changeset) = recorded(&fx, &main, &[("AGENTS.md", "rules\n")]).await;

    give_role(fx.project.store(), "review", Role::Reviewer, None).await;
    let lead = main_actor(&fx.backend.worker);
    let review = main_actor(&fx.review.worker);
    let unsupervised = by(&fx.frontend.worker);
    for (actor, code) in [
        (&lead, codes::MAIN_AGENT_REQUIRED),
        (&review, codes::MAIN_AGENT_REQUIRED),
        (&human(), codes::MAIN_AGENT_REQUIRED),
        (&Actor::System, codes::MAIN_AGENT_REQUIRED),
        (&unsupervised, codes::ORCHESTRATOR_REQUIRED),
    ] {
        let opened = fx.workspaces.open_main_workspace(actor).await;
        assert_eq!(workspace_refusal(opened).code(), code, "{actor:?}");
        let submitted = submit(&fx, actor, changeset).await;
        assert_eq!(refusal(submitted).code(), code, "{actor:?}");
    }
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM integrations").await,
        0,
        "a refused submit writes nothing"
    );
}

#[tokio::test]
async fn a_task_worker_with_the_main_tag_is_still_refused() {
    let fx = Fixture::with_main_frontend().await;
    give_role(fx.project.store(), "review", Role::TaskWorker, None).await;
    let worker = main_actor(&fx.review.worker);
    let opened = fx.workspaces.open_main_workspace(&worker).await;
    assert_eq!(workspace_refusal(opened).code(), codes::MAIN_AGENT_REQUIRED);
}

#[tokio::test]
async fn a_direct_change_becomes_one_revision_and_closes_the_main_workspace() {
    let fx = Fixture::with_main_frontend().await;
    let main = main_actor(&fx.frontend.worker);
    let (_, changeset) = recorded(&fx, &main, &[("AGENTS.md", "rules\n")]).await;
    let seq = fx.last_seq().await;

    let outcome = fx
        .reviews
        .integrate_direct(
            &main,
            change(
                changeset,
                "  Add the agent rules\n\nThe user asked for them.\n",
            ),
        )
        .await
        .unwrap();
    assert_eq!(outcome, DirectOutcome::Succeeded { revision: r(2) });

    let head = workspaces::head(fx.project.store().read())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(head.number, r(2));
    let integration = head.integration.unwrap();
    assert_eq!(integration.kind, IntegrationKind::Direct);
    assert_eq!(integration.actor, "frontend");
    assert_eq!(integration.changeset, Some(changeset));
    assert_eq!(
        integration.message.as_deref(),
        Some("Add the agent rules\n\nThe user asked for them.")
    );
    assert_eq!(integration.proposal, None);
    let main_ws = fx
        .text("SELECT state FROM workspaces WHERE kind = 'main'")
        .await;
    assert_eq!(main_ws.as_deref(), Some(WorkspaceState::Archived.as_str()));
    assert_eq!(fx.count("SELECT COUNT(*) FROM completions").await, 0);

    let stored = fx.events_after(seq).await;
    assert_eq!(
        names(&stored),
        [
            types::INTEGRATION_SUCCEEDED.as_str(),
            types::MAIN_ADVANCED.as_str(),
            types::WORKSPACE_ARCHIVED.as_str(),
        ]
    );
    let succeeded = &stored[0];
    assert_eq!(succeeded.actor.as_str(), "frontend");
    assert_eq!(succeeded.task, None);
    assert_eq!(succeeded.revision, Some(r(2)));
    assert_eq!(succeeded.data["kind"], "direct");
    assert_eq!(succeeded.data["changeset"], changeset.to_string());
    assert_eq!(succeeded.data["approval"], "user_in_conversation");
    assert_consistent(&fx.project).await;

    // The integrated changeset and its closed workspace take nothing more.
    let again = submit(&fx, &main, changeset).await;
    assert_eq!(refusal(again).code(), codes::WORKSPACE_NOT_ACTIVE);
    let next = fx.workspaces.open_main_workspace(&main).await.unwrap();
    assert!(!next.reused);
    assert_eq!(next.workspace.path, ".fold/ws/main-2");
    assert_eq!(next.workspace.base, r(2));
}

#[tokio::test]
async fn a_conflicting_direct_change_writes_nothing_and_goes_in_after_a_sync() {
    let fx = Fixture::with_main_frontend().await;
    let main = main_actor(&fx.frontend.worker);
    let (dir, first) = recorded(&fx, &main, &[("README.md", "# main's\n")]).await;
    // A task's proposal changes the same line and reaches main first.
    let author = by(&fx.backend.worker);
    let task_ws = fx.workspaces.create_workspace(&author, t(1)).await.unwrap();
    let task_dir = fx.project.paths().root().join(&task_ws.workspace.path);
    std::fs::write(task_dir.join("README.md"), "# the task's\n").unwrap();
    let task_change = fx
        .workspaces
        .record_changes(&author, task_ws.workspace.id, "readme")
        .await
        .unwrap();
    let integrated = fx
        .reviews
        .integrate_direct(&main, change(task_change.changeset, "x"))
        .await;
    let refused = refusal(integrated);
    assert_eq!(refused.code(), codes::INVALID_REQUEST);
    assert_eq!(details(&refused)["reason"], "not_main_workspace");
    integrate_task(&fx, &author).await;
    let head_before = workspaces::head(fx.project.store().read())
        .await
        .unwrap()
        .unwrap()
        .number;
    let seq = fx.last_seq().await;
    let integrations = fx.count("SELECT COUNT(*) FROM integrations").await;

    let outcome = submit(&fx, &main, first).await.unwrap();
    let DirectOutcome::Conflicted(conflicts) = outcome else {
        panic!("expected a conflict, got {outcome:?}");
    };
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].path, b"README.md");
    assert_eq!(
        workspaces::head(fx.project.store().read())
            .await
            .unwrap()
            .unwrap()
            .number,
        head_before
    );
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM integrations").await,
        integrations
    );
    assert!(fx.events_after(seq).await.is_empty());
    assert_eq!(
        fx.text("SELECT state FROM workspaces WHERE kind = 'main'")
            .await
            .as_deref(),
        Some("active")
    );

    let workspace = workspaces::live_main_workspace(fx.project.store().read())
        .await
        .unwrap()
        .unwrap()
        .id;
    let synced = fx
        .workspaces
        .sync_workspace(&main, workspace)
        .await
        .unwrap();
    assert_eq!(synced.conflicted, ["README.md"]);
    std::fs::write(dir.join("README.md"), "# both\n").unwrap();
    let resolved = fx
        .workspaces
        .record_changes(&main, workspace, "resolved")
        .await
        .unwrap();
    // The changeset the user saw first is not the newest any more.
    let stale = refusal(submit(&fx, &main, first).await);
    assert_eq!(details(&stale)["reason"], "stale_changeset");
    assert_eq!(details(&stale)["latest"], resolved.changeset.to_string());
    let outcome = submit(&fx, &main, resolved.changeset).await.unwrap();
    let next = RevisionId::from_number(head_before.number() + 1).unwrap();
    assert_eq!(outcome, DirectOutcome::Succeeded { revision: next });
    assert_consistent(&fx.project).await;
}

#[tokio::test]
async fn a_direct_change_needs_a_message_and_a_change() {
    let fx = Fixture::with_main_frontend().await;
    let main = main_actor(&fx.frontend.worker);
    let (_, changeset) = recorded(&fx, &main, &[]).await;
    let empty = refusal(
        fx.reviews
            .integrate_direct(&main, change(changeset, " \n "))
            .await,
    );
    assert_eq!(empty.code(), codes::INVALID_REQUEST);
    assert_eq!(details(&empty)["field"], "message");
    let long = refusal(
        fx.reviews
            .integrate_direct(&main, change(changeset, &"x".repeat(5001)))
            .await,
    );
    assert_eq!(details(&long)["field"], "message");
    let nothing = refusal(submit(&fx, &main, changeset).await);
    assert_eq!(details(&nothing)["reason"], "no_changes");
    let unknown = refusal(submit(&fx, &main, ChangesetId::from_number(99).unwrap()).await);
    assert_eq!(unknown.code(), codes::NOT_FOUND);
    assert_eq!(fx.count("SELECT COUNT(*) FROM integrations").await, 0);
}

#[tokio::test]
async fn abandoning_the_main_workspace_closes_it_without_a_task() {
    let fx = Fixture::with_main_frontend().await;
    let main = main_actor(&fx.frontend.worker);
    let opened = fx.workspaces.open_main_workspace(&main).await.unwrap();
    let abandoned = fx
        .workspaces
        .abandon_workspace(&main, opened.workspace.id, "the user changed their mind")
        .await
        .unwrap();
    assert_eq!(abandoned.task_status, None);
    assert_eq!(abandoned.workspace.state, WorkspaceState::Abandoned);
    let next = fx.workspaces.open_main_workspace(&main).await.unwrap();
    assert!(!next.reused);
    assert_ne!(next.workspace.id, opened.workspace.id);
    assert_consistent(&fx.project).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_live_state_names_the_session_that_took_the_main_workspace_over() {
    let h = crate::common::acceptance::Harness::start_without_leads().await;
    let workspaces = fold_daemon::services::workspaces::WorkspaceService::for_project(&h.handle);
    let as_main = |agent: &str| {
        let worker = fold_core::WorkerName::parse(agent).unwrap();
        Actor::Agent(AgentActor {
            agent: worker.agent(),
            worker,
            tags: vec![Tag::parse(MAIN_TAG).unwrap()],
        })
    };
    let opened = workspaces
        .open_main_workspace(&as_main("frontend"))
        .await
        .unwrap();
    let ws = opened.workspace.id;
    let owner = || h.handle.live.get(ws).map(|live| live.view.worker.clone());
    assert_eq!(owner().as_deref(), Some("frontend"));
    let taken = workspaces
        .open_main_workspace(&as_main("backend"))
        .await
        .unwrap();
    assert!(taken.reused);
    assert_eq!(owner().as_deref(), Some("backend"));
}
