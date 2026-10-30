#![cfg(test)]

use std::sync::Arc;
use std::time::Duration;

use crate::git_support::{
    approved, assert_map_matches_rows, commit_of, git, start_job, wait_for_rows,
};
use crate::reviews_support::{Fixture, p, t};
use fold_core::events::types;
use fold_core::{Event, RevisionId};
use fold_daemon::services::events::BusMessage;
use fold_vcs::git::Git;

fn r(number: u32) -> RevisionId {
    RevisionId::from_number(number).unwrap()
}

async fn next_event(
    bus: &mut tokio::sync::broadcast::Receiver<BusMessage>,
    kind: fold_core::EventType,
) -> Arc<Event> {
    tokio::time::timeout(Duration::from_secs(30), async {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proposal_integration_becomes_one_commit_with_the_git_identity_and_trailers() {
    let fx = Fixture::new().await;
    let root = fx.project.paths().root();
    git(root, &["config", "user.name", "Ana Lima"]);
    git(root, &["config", "user.email", "ana@example.com"]);
    let _job = start_job(&fx, Git::default());
    wait_for_rows(&fx, 1).await;
    let review = approved(&fx).await;
    fx.reviews
        .integrate_proposal(&review, p(1), Some(1))
        .await
        .unwrap();
    wait_for_rows(&fx, 2).await;

    let mirror = fx.project.paths().git_dir();
    let (first, second) = (commit_of(&fx, 1).await, commit_of(&fx, 2).await);
    assert_eq!(git(&mirror, &["rev-parse", "main"]).trim(), second);
    assert_eq!(git(&mirror, &["rev-list", "--count", "main"]).trim(), "2");
    assert_eq!(
        git(&mirror, &["log", "-1", "--format=%P", &second]).trim(),
        first
    );
    assert_eq!(
        git(&mirror, &["log", "-1", "--format=%B", &second]),
        "Add the report\n\nAdds the report.\n\nIt does not change the schema.\n\nReport: Adds z and renames x to y.\nDocs link: Links the readme from docs.\n\nFold-Proposal: P-1\nFold-Version: 1\nFold-Task: T-1\nFold-Revision: r2\n\n"
    );
    let created = fx
        .count("SELECT created_at FROM revisions WHERE number = 2")
        .await
        / 1000;
    assert_eq!(
        git(
            &mirror,
            &["log", "-1", "--format=%an <%ae>|%cn <%ce>|%at|%ct", &second]
        ),
        format!("Ana Lima <ana@example.com>|Ana Lima <ana@example.com>|{created}|{created}\n")
    );
    // The trailers are Git trailers, so Git reads them back.
    assert_eq!(
        git(
            &mirror,
            &[
                "log",
                "-1",
                "--format=%(trailers:key=Fold-Task,valueonly)",
                &second
            ]
        )
        .trim(),
        "T-1"
    );
    assert_map_matches_rows(&fx, 2).await;
    assert_eq!(
        fx.text("SELECT DISTINCT branch FROM revision_commits")
            .await
            .as_deref(),
        Some("main")
    );

    let detail = fx.workspaces.get_revision(r(2)).await.unwrap();
    assert_eq!(detail.summary.commit.as_deref(), Some(second.as_str()));
    let task = fx.tasks.get_view(t(1)).await.unwrap();
    assert_eq!(
        task.integration.unwrap().commit.as_deref(),
        Some(second.as_str())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_commit_is_recorded_and_the_next_start_writes_it_on_the_configured_branch() {
    let fx = Fixture::new().await;
    std::fs::write(fx.project.paths().config(), "[git]\nbranch = \"trunk\"\n").unwrap();
    let mut bus = fx.bus.subscribe();
    let missing = Git::new(fx.project.paths().root().join("bin/git"));
    let job = start_job(&fx, missing);
    let failed = next_event(&mut bus, types::GIT_COMMIT_FAILED).await;
    assert_eq!(failed.actor.as_str(), "fold");
    assert_eq!(failed.revision, Some(r(1)));
    assert_eq!(failed.data, serde_json::json!({"code": "git_not_found"}));
    assert_eq!(fx.count("SELECT COUNT(*) FROM revision_commits").await, 0);
    drop(job);

    let _job = start_job(&fx, Git::default());
    wait_for_rows(&fx, 1).await;
    let mirror = fx.project.paths().git_dir();
    assert_eq!(
        git(
            &mirror,
            &["for-each-ref", "--format=%(refname)", "refs/heads"]
        ),
        "refs/heads/trunk\n"
    );
    assert_eq!(
        git(&mirror, &["rev-parse", "refs/heads/trunk"]).trim(),
        commit_of(&fx, 1).await
    );
    assert_eq!(
        fx.text("SELECT branch FROM revision_commits")
            .await
            .as_deref(),
        Some("trunk")
    );
}

/// `git` that reads no global or system config, so only the repository's own config can name an identity.
fn isolated_git(dir: &std::path::Path) -> Git {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join("git-isolated");
    std::fs::write(
        &script,
        "#!/bin/sh\nGIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null exec git \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    Git::new(script)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_an_identity_fold_signs_and_a_later_identity_keeps_the_written_commits() {
    let fx = Fixture::new().await;
    let bin = tempfile::tempdir().unwrap();
    let job = start_job(&fx, isolated_git(bin.path()));
    wait_for_rows(&fx, 1).await;
    let mirror = fx.project.paths().git_dir();
    let first = commit_of(&fx, 1).await;
    let people = |commit: &str| {
        git(
            &mirror,
            &["log", "-1", "--format=%an <%ae>|%cn <%ce>", commit],
        )
    };
    assert_eq!(
        people(&first),
        "fold <fold@localhost>|fold <fold@localhost>\n"
    );
    drop(job);

    let root = fx.project.paths().root();
    git(root, &["config", "user.name", "Ana Lima"]);
    git(root, &["config", "user.email", "ana@example.com"]);
    let _job = start_job(&fx, isolated_git(bin.path()));
    let review = approved(&fx).await;
    fx.reviews
        .integrate_proposal(&review, p(1), Some(1))
        .await
        .unwrap();
    wait_for_rows(&fx, 2).await;
    let second = commit_of(&fx, 2).await;
    assert_eq!(
        people(&second),
        "Ana Lima <ana@example.com>|Ana Lima <ana@example.com>\n"
    );
    assert_eq!(commit_of(&fx, 1).await, first);
    assert_eq!(
        git(&mirror, &["log", "-1", "--format=%P", &second]).trim(),
        first
    );
    assert_map_matches_rows(&fx, 2).await;
}
