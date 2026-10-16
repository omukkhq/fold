#![cfg(test)]

use crate::reviews_support;

use crate::reviews_support::{Fixture, by, human, p, t, ws};
use fold_core::reviews::{ProposalStanding, Verdict};
use fold_core::{AgentName, TaskStatus};
use fold_daemon::services::reviews::IntegrationOutcome;
use fold_daemon::services::tasks::{self, HISTORY_LIMIT};
use fold_store::workspaces;

fn column(board: &tasks::Board, status: TaskStatus) -> &tasks::BoardColumn {
    board
        .columns
        .iter()
        .find(|column| column.status == status)
        .unwrap()
}

#[tokio::test]
async fn a_task_with_a_workspace_names_it_and_its_rule() {
    let fx = Fixture::new().await;
    let author = by(&fx.frontend.worker);
    fx.workspaces.create_workspace(&author, t(1)).await.unwrap();

    let view = fx.tasks.get_view(t(1)).await.unwrap();
    assert_eq!(view.task.status, TaskStatus::InProgress);
    assert_eq!(
        view.status_reason,
        "Calculated: workspace ws-1 is open, no proposal yet"
    );
    let info = view.workspace_info.unwrap();
    assert_eq!(info.directory, ".fold/ws/t-1");
    assert_eq!(
        (info.base.to_string(), info.main.to_string()),
        ("r1".into(), "r1".into())
    );
    assert_eq!(info.behind, 0);
    assert!(view.proposal.is_none() && view.integration.is_none());
    assert!(view.changesets.is_empty());
    let types: Vec<&str> = view.history.iter().map(|e| e.event_type.as_str()).collect();
    assert_eq!(types.first(), Some(&"task.status_changed"));
    assert!(types.contains(&"task.created") && types.contains(&"workspace.created"));
    assert!(view.history.len() <= HISTORY_LIMIT);
    assert!(
        view.history
            .windows(2)
            .all(|pair| pair[0].seq > pair[1].seq)
    );

    let untouched = fx.tasks.get_view(t(2)).await.unwrap();
    assert_eq!(
        untouched.status_reason,
        "Calculated: no workspace yet and every prerequisite is Done"
    );
    assert!(untouched.workspace_info.is_none() && untouched.activity.is_none());
}

#[tokio::test]
async fn the_proposal_the_reviews_and_the_integration_follow_the_task_to_done() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    fx.agents
        .set_integrator(&human(), &AgentName::parse("review").unwrap(), true)
        .await
        .unwrap();
    let review = fx.agents.actor(&fx.review.ctx, None).await.unwrap();

    let open = fx.tasks.get_view(t(1)).await.unwrap();
    assert_eq!(open.task.status, TaskStatus::Review);
    assert_eq!(
        open.status_reason,
        "Calculated: proposal P-1 waits for a reviewer"
    );
    let proposal = open.proposal.unwrap();
    assert_eq!((proposal.id, proposal.version), (p(1), 1));
    assert_eq!(proposal.standing, ProposalStanding::Open);
    assert!(proposal.reviews.is_empty());
    assert_eq!(open.changesets.len(), 1);
    assert!(open.integration.is_none());
    let card = column(&fx.tasks.board().await.unwrap(), TaskStatus::Review).cards[0].clone();
    let pill = card.proposal.unwrap();
    assert_eq!(
        (pill.id, pill.standing, pill.revision),
        (p(1), ProposalStanding::Open, None)
    );

    fx.reviews
        .submit_review(
            &review,
            p(1),
            Verdict::RequestChanges,
            "rename it",
            Vec::new(),
        )
        .await
        .unwrap();
    let asked = fx.tasks.get_view(t(1)).await.unwrap();
    assert_eq!(
        asked.proposal.as_ref().unwrap().standing,
        ProposalStanding::ChangesRequested
    );
    assert_eq!(
        asked.status_reason,
        "Calculated: workspace ws-1 is open and proposal P-1 waits for its author"
    );

    fx.reviews
        .submit_review(&review, p(1), Verdict::Approve, "better", Vec::new())
        .await
        .unwrap();
    let approved = fx.tasks.get_view(t(1)).await.unwrap().proposal.unwrap();
    assert_eq!(approved.standing, ProposalStanding::Approved);
    let verdicts: Vec<(Verdict, String)> = approved
        .reviews
        .iter()
        .map(|r| (r.verdict, r.reviewer.to_string()))
        .collect();
    assert_eq!(
        verdicts,
        vec![
            (Verdict::Approve, "review".to_owned()),
            (Verdict::RequestChanges, "review".to_owned())
        ]
    );

    let outcome = fx
        .reviews
        .integrate_proposal(&review, p(1), Some(1))
        .await
        .unwrap();
    assert!(matches!(outcome, IntegrationOutcome::Succeeded { .. }));
    let done = fx.tasks.get_view(t(1)).await.unwrap();
    assert_eq!(done.status_reason, "Completed: a completion record exists");
    assert_eq!(
        done.proposal.unwrap().standing,
        ProposalStanding::Integrated
    );
    let integration = done.integration.unwrap();
    assert_eq!(
        (
            integration.revision.to_string(),
            integration.proposal,
            integration.by.as_str()
        ),
        ("r2".to_owned(), p(1), "review")
    );
    assert!(done.workspace_info.is_none());
    assert_eq!(
        done.changesets.len(),
        1,
        "the archived workspace's changesets stay"
    );
    let card = column(&fx.tasks.board().await.unwrap(), TaskStatus::Done).cards[0].clone();
    let pill = card.proposal.unwrap();
    assert_eq!(pill.standing, ProposalStanding::Integrated);
    assert_eq!(pill.revision.map(|r| r.to_string()), Some("r2".to_owned()));
}

#[tokio::test]
async fn the_checkpoints_of_the_workspace_come_newest_first_and_stay_after_the_task_is_done() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    let author = by(&fx.frontend.worker);
    fx.workspaces
        .record_checkpoint(&author, ws(1), Some("first"))
        .await
        .unwrap();
    fx.write("src/more.rs", "pub fn w() {}\n");
    fx.workspaces
        .record_checkpoint(&author, ws(1), Some("second"))
        .await
        .unwrap();

    let labels = |view: &tasks::TaskDetail| -> Vec<Option<String>> {
        view.checkpoints
            .iter()
            .map(|checkpoint| checkpoint.label.clone())
            .collect()
    };
    let open = fx.tasks.get_view(t(1)).await.unwrap();
    assert_eq!(
        labels(&open),
        vec![Some("second".to_owned()), Some("first".to_owned()), None],
        "the workspace opened with an automatic checkpoint, the oldest"
    );

    fx.agents
        .set_integrator(&human(), &AgentName::parse("review").unwrap(), true)
        .await
        .unwrap();
    let review = fx.agents.actor(&fx.review.ctx, None).await.unwrap();
    fx.reviews
        .submit_review(&review, p(1), Verdict::Approve, "fine", Vec::new())
        .await
        .unwrap();
    fx.reviews
        .integrate_proposal(&review, p(1), Some(1))
        .await
        .unwrap();
    let done = fx.tasks.get_view(t(1)).await.unwrap();
    assert!(done.workspace.is_none());
    assert_eq!(labels(&done), labels(&open));
}

#[tokio::test]
async fn a_closed_proposal_is_not_shown_and_the_board_reports_the_last_edit() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    fx.reviews
        .submit_review(&human(), p(1), Verdict::Reject, "no", Vec::new())
        .await
        .unwrap();
    let view = fx.tasks.get_view(t(1)).await.unwrap();
    assert!(view.proposal.is_none());
    let board = fx.tasks.board().await.unwrap();
    let card = &column(&board, TaskStatus::InProgress).cards[0];
    assert!(card.proposal.is_none());
    assert_eq!(card.workspace, Some(ws(1)));

    let at = fx.project.now();
    fx.project
        .store()
        .write(async |tx| workspaces::touch(tx, ws(1), 2, at).await)
        .await
        .unwrap();
    let board = fx.tasks.board().await.unwrap();
    let activity = column(&board, TaskStatus::InProgress).cards[0]
        .activity
        .clone()
        .unwrap();
    assert_eq!((activity.at, activity.path), (at, None));
    assert_eq!(
        fx.tasks.get_view(t(1)).await.unwrap().activity.unwrap().at,
        at
    );
}

#[tokio::test]
async fn the_mark_carries_the_time_it_was_last_set() {
    let fx = Fixture::new().await;
    assert_eq!(
        fx.tasks.get_view(t(1)).await.unwrap().human_review_set_at,
        None
    );
    fx.reviews
        .require_human_review(&human(), t(1), None, "touches auth")
        .await
        .unwrap();
    let first = fx.project.now();
    assert_eq!(
        fx.tasks.get_view(t(1)).await.unwrap().human_review_set_at,
        Some(first)
    );
    fx.project
        .clock()
        .advance(std::time::Duration::from_secs(60))
        .unwrap();
    fx.reviews
        .require_human_review(&human(), t(1), None, "touches auth and billing")
        .await
        .unwrap();
    let changed = fx.tasks.get_view(t(1)).await.unwrap();
    assert_eq!(changed.human_review_set_at, Some(fx.project.now()));
    assert_ne!(changed.human_review_set_at, Some(first));
    fx.reviews.clear_human_review(&human(), t(1)).await.unwrap();
    assert_eq!(
        fx.tasks.get_view(t(1)).await.unwrap().human_review_set_at,
        None
    );
}

#[test]
fn only_a_human_may_change_the_human_review_mark() {
    assert!(tasks::can_change_human_review(&human()));
    assert!(!tasks::can_change_human_review(&reviews_support::agent(
        "frontend"
    )));
}
