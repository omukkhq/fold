#![cfg(test)]

use crate::workspaces_support::{service, three_revisions};
use fold_core::tree::EntryKind;
use fold_core::{RevisionId, TaskId};
use fold_daemon::services::workspaces::{RevisionOrigin, RevisionSummary};
use fold_store::testing::TestProject;
use fold_vcs::compare::ChangeKind;

fn id(n: u32) -> RevisionId {
    RevisionId::from_number(n).unwrap()
}

fn numbers(revisions: &[RevisionSummary]) -> Vec<String> {
    revisions
        .iter()
        .map(|summary| summary.number.to_string())
        .collect()
}

#[tokio::test]
async fn the_state_names_the_head_the_main_directory_and_the_recent_revisions() {
    let project = TestProject::new().await;
    three_revisions(&project).await;
    let state = service(&project).get_repository_state().await.unwrap();
    assert_eq!(state.head, Some(id(3)));
    assert_eq!(state.main_path, project.paths().main_dir());
    assert_eq!(numbers(&state.recent), ["r3", "r2", "r1"]);

    let [r3, r2, r1] = &state.recent[..] else {
        panic!("three revisions");
    };
    assert_eq!(r1.origin, RevisionOrigin::Initial);
    assert_eq!(
        (r1.parent, r1.task, r1.author.as_deref()),
        (None, None, None)
    );
    assert_eq!(r2.origin.as_str(), "proposal");
    assert_eq!(r2.parent, Some(id(1)));
    assert_eq!(r2.task, Some(TaskId::from_number(3).unwrap()));
    assert_eq!(r2.proposal.map(|p| p.to_string()).as_deref(), Some("P-5"));
    assert_eq!(r2.title.as_deref(), Some("Add the guide"));
    assert_eq!(r2.author.as_deref(), Some("backend"));
    assert_eq!(r2.pull, None);
    assert_eq!(r3.origin, RevisionOrigin::Pull);
    assert_eq!(r3.pull.as_deref(), Some("abc123"));
    assert_eq!(r3.author.as_deref(), Some("human"));
    assert_eq!((r3.task, r3.proposal), (None, None));
}

#[tokio::test]
async fn an_empty_project_has_no_head() {
    let project = TestProject::new().await;
    let state = service(&project).get_repository_state().await.unwrap();
    assert_eq!(state.head, None);
    assert!(state.recent.is_empty());
}

#[tokio::test]
async fn revisions_page_newest_first() {
    let project = TestProject::new().await;
    three_revisions(&project).await;
    let workspaces = service(&project);
    let first = workspaces.list_revisions(Some(2), None).await.unwrap();
    assert_eq!(numbers(&first.revisions), ["r3", "r2"]);
    assert_eq!(first.before, Some(id(2)));
    let second = workspaces
        .list_revisions(Some(2), first.before)
        .await
        .unwrap();
    assert_eq!(numbers(&second.revisions), ["r1"]);
    assert_eq!(second.before, None);
    let all = workspaces.list_revisions(None, None).await.unwrap();
    assert_eq!(numbers(&all.revisions), ["r3", "r2", "r1"]);
    assert_eq!(all.before, None);
    let one = workspaces.list_revisions(Some(0), None).await.unwrap();
    assert_eq!(numbers(&one.revisions), ["r3"]);
    assert_eq!(one.before, Some(id(3)));
}

#[tokio::test]
async fn a_revision_lists_the_files_it_changed_against_its_parent() {
    let project = TestProject::new().await;
    three_revisions(&project).await;
    let workspaces = service(&project);
    let changes = |files: &[fold_vcs::compare::FileChange]| -> Vec<(String, ChangeKind)> {
        files
            .iter()
            .map(|file| (file.path.to_string(), file.change))
            .collect()
    };

    let r1 = workspaces.get_revision(id(1)).await.unwrap();
    assert_eq!(
        changes(&r1.files),
        [
            ("README.md".into(), ChangeKind::Added),
            ("logo.png".into(), ChangeKind::Added),
            ("src/lib.rs".into(), ChangeKind::Added),
        ]
    );
    assert!(r1.files[1].binary);
    assert_eq!((r1.totals.files, r1.totals.added_lines), (3, 2));

    let r2 = workspaces.get_revision(id(2)).await.unwrap();
    assert_eq!(r2.summary.number, id(2));
    assert_eq!(
        changes(&r2.files),
        [
            ("docs/guide.md".into(), ChangeKind::Added),
            ("src/lib.rs".into(), ChangeKind::Modified),
        ]
    );
    assert_eq!(r2.files[1].added_lines, 1);

    let r3 = workspaces.get_revision(id(3)).await.unwrap();
    assert_eq!(
        changes(&r3.files),
        [
            ("README.md".into(), ChangeKind::Deleted),
            ("src/main.rs".into(), ChangeKind::Added),
        ]
    );
    let missing = workspaces.get_revision(id(9)).await.unwrap_err();
    assert_eq!(missing.code(), Some("revision_not_found"));
}

#[tokio::test]
async fn trees_and_files_read_at_any_revision() {
    let project = TestProject::new().await;
    three_revisions(&project).await;
    let workspaces = service(&project);
    let root = workspaces.read_tree(id(1), "").await.unwrap();
    let names: Vec<(String, EntryKind)> = root
        .iter()
        .map(|entry| (String::from_utf8(entry.name.clone()).unwrap(), entry.kind))
        .collect();
    assert_eq!(
        names,
        [
            ("README.md".into(), EntryKind::File),
            ("logo.png".into(), EntryKind::File),
            ("src".into(), EntryKind::Directory),
        ]
    );
    let src = workspaces.read_tree(id(3), "src").await.unwrap();
    assert_eq!(src.len(), 2);
    assert_eq!(src[1].size, Some(13));

    let readme = workspaces.read_file(id(1), "README.md").await.unwrap();
    assert_eq!(readme.bytes, b"hello\n");
    assert!(!readme.binary);
    assert!(
        workspaces
            .read_file(id(1), "logo.png")
            .await
            .unwrap()
            .binary
    );
    let gone = workspaces.read_file(id(3), "README.md").await.unwrap_err();
    assert_eq!(gone.code(), Some("not_found"));
    for bad in ["", "../x"] {
        let error = workspaces.read_file(id(1), bad).await.unwrap_err();
        assert_eq!(error.code(), Some("invalid_path"), "{bad:?}");
    }
    let error = workspaces.read_tree(id(1), "src/../..").await.unwrap_err();
    assert_eq!(error.code(), Some("invalid_path"));
    let error = workspaces.read_tree(id(9), "").await.unwrap_err();
    assert_eq!(error.code(), Some("revision_not_found"));
}
