#![cfg(test)]

use std::fs;

use crate::workspaces_support::{
    R1, changeset, checkpoint, exec, objects, service, three_revisions, workspace_dir,
};
use fold_core::{ChangesetId, CheckpointId, RevisionId, WorkspaceId};
use fold_daemon::services::workspaces::{
    StateComparison, StateKind, StateRef, StateSummary, WorkspaceError,
};
use fold_store::testing::TestProject;
use fold_vcs::compare::ChangeKind;
use fold_vcs::diff::LineKind;

fn r(n: u32) -> StateRef {
    StateRef::Revision(RevisionId::from_number(n).unwrap())
}

fn c(n: u32) -> StateRef {
    StateRef::Changeset(ChangesetId::from_number(n).unwrap())
}

fn ck(n: u32) -> StateRef {
    StateRef::Checkpoint(CheckpointId::from_number(n).unwrap())
}

fn ws(n: u32) -> StateRef {
    StateRef::Workspace(WorkspaceId::from_number(n).unwrap())
}

fn files(comparison: &StateComparison) -> Vec<(String, ChangeKind, u32, u32)> {
    comparison
        .files
        .iter()
        .map(|file| {
            (
                file.path.to_string(),
                file.change,
                file.added_lines,
                file.removed_lines,
            )
        })
        .collect()
}

fn code(error: WorkspaceError) -> &'static str {
    error.code().unwrap()
}

const C1: &[(&str, &[u8])] = &[
    ("README.md", b"hello\n"),
    ("logo.png", b"\x89PNG\0\x01"),
    ("src/lib.rs", b"fn a() {}\nfn x() {}\n"),
];

const C2: &[(&str, &[u8])] = &[
    ("README.md", b"hello\n"),
    ("logo.png", b"\x89PNG\0\x01"),
    ("src/lib.rs", b"fn a() {}\nfn x() {}\n"),
    ("src/new.rs", b"pub mod x;\n"),
];

const CK1: &[(&str, &[u8])] = &[
    ("README.md", b"hello\nworld\n"),
    ("logo.png", b"\x89PNG\0\x01"),
];

#[test]
fn state_refs_parse_their_four_public_forms_only() {
    for (text, expected) in [
        ("r17", r(17)),
        ("C-40", c(40)),
        ("ck-88", ck(88)),
        ("ws-9", ws(9)),
    ] {
        let parsed = StateRef::parse(text).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(parsed.to_string(), text);
    }
    assert_eq!(r(1).kind(), StateKind::Revision);
    assert_eq!(ws(1).kind().as_str(), "workspace");
    for bad in [
        "", "T-3", "P-1", "r0", "r01", "C-", "ck-x", "ws--1", "17", "R1",
    ] {
        let error = StateRef::parse(bad).unwrap_err();
        assert_eq!(error.code(), "invalid_state_ref", "{bad}");
        assert!(error.hint().is_some());
    }
}

#[tokio::test]
async fn changesets_checkpoints_and_working_files_compare() {
    let project = TestProject::new().await;
    three_revisions(&project).await;
    changeset(&project, 1, C1).await;
    changeset(&project, 2, C2).await;
    checkpoint(&project, 1, CK1).await;
    let root = workspace_dir(
        &project,
        &[
            ("README.md", b"hello\n"),
            ("logo.png", b"\x89PNG\0\x01"),
            ("src/lib.rs", b"fn a() {}\nfn w() {}\n"),
            ("notes.txt", b"todo\n"),
        ],
    );
    let workspaces = service(&project);

    let between = workspaces.compare_states(c(1), c(2)).await.unwrap();
    assert_eq!(
        files(&between),
        [("src/new.rs".into(), ChangeKind::Added, 1, 0)]
    );
    assert_eq!(
        between.from,
        StateSummary {
            kind: StateKind::Changeset,
            label: "C-1".into(),
            base: None,
            task: Some("T-3".parse().unwrap()),
            worker: None,
        }
    );

    let checkpointed = workspaces.compare_states(r(1), ck(1)).await.unwrap();
    assert_eq!(
        files(&checkpointed),
        [
            ("README.md".into(), ChangeKind::Modified, 1, 0),
            ("src/lib.rs".into(), ChangeKind::Deleted, 0, 1),
        ]
    );
    assert_eq!(checkpointed.to.label, "ck-1");
    assert_eq!(checkpointed.to.task, Some("T-3".parse().unwrap()));

    let working = workspaces.compare_states(r(1), ws(1)).await.unwrap();
    assert_eq!(
        files(&working),
        [
            ("notes.txt".into(), ChangeKind::Added, 1, 0),
            ("src/lib.rs".into(), ChangeKind::Modified, 1, 0),
        ]
    );
    assert_eq!(
        working.to,
        StateSummary {
            kind: StateKind::Workspace,
            label: "ws-1".into(),
            base: Some(RevisionId::from_number(1).unwrap()),
            task: Some("T-3".parse().unwrap()),
            worker: Some("backend/api".into()),
        }
    );
    let diff = workspaces
        .diff_file(c(1), ws(1), "src/lib.rs")
        .await
        .unwrap();
    let changed: Vec<(LineKind, &str)> = diff.hunks[0]
        .lines
        .iter()
        .map(|line| (line.kind, line.text.as_str()))
        .collect();
    assert_eq!(
        changed,
        [
            (LineKind::Context, "fn a() {}\n"),
            (LineKind::Removed, "fn x() {}\n"),
            (LineKind::Added, "fn w() {}\n"),
        ]
    );

    fs::write(
        root.join("src/lib.rs"),
        b"fn a() {}\nfn w() {}\nfn v() {}\n",
    )
    .unwrap();
    let edited = workspaces.compare_states(r(1), ws(1)).await.unwrap();
    assert_eq!(
        files(&edited),
        [
            ("notes.txt".into(), ChangeKind::Added, 1, 0),
            ("src/lib.rs".into(), ChangeKind::Modified, 2, 0),
        ]
    );
    assert_eq!(
        workspaces.compare_states(c(1), c(2)).await.unwrap(),
        between
    );
    assert_eq!(
        workspaces.compare_states(r(1), ck(1)).await.unwrap(),
        checkpointed
    );
}

#[tokio::test]
async fn reading_working_files_writes_no_object_and_no_index() {
    let project = TestProject::new().await;
    three_revisions(&project).await;
    workspace_dir(
        &project,
        &[
            ("README.md", b"hello\n"),
            ("logo.png", b"\x89PNG\0\x01"),
            ("src/lib.rs", b"fn only_here() {}\n"),
        ],
    );
    let before = objects(&project).entries().unwrap().len();
    let workspaces = service(&project);
    let resolved = workspaces.resolve_state(ws(1)).await.unwrap();
    assert_eq!(resolved.tree, None);
    assert!(resolved.state.is_working());
    assert_eq!(resolved.state.entries().len(), 3);
    let compared = workspaces.compare_states(ws(1), r(1)).await.unwrap();
    assert_eq!(compared.files.len(), 1);
    assert_eq!(objects(&project).entries().unwrap().len(), before);
    assert!(!project.paths().index().join("ws-1.idx").exists());
    let recorded = workspaces.resolve_state(r(1)).await.unwrap();
    assert!(recorded.tree.is_some());
    assert!(!recorded.state.is_working());
}

#[tokio::test]
async fn a_missing_workspace_has_no_working_side_but_its_checkpoints_compare() {
    let project = TestProject::new().await;
    three_revisions(&project).await;
    checkpoint(&project, 1, CK1).await;
    let workspaces = service(&project);
    let missing = |error: WorkspaceError| {
        let WorkspaceError::Fold(error) = error else {
            panic!("a missing workspace is a refusal");
        };
        assert_eq!(error.code(), "workspace_not_active");
        assert_eq!(error.details().unwrap()["state"], "missing");
    };
    // The row is still active, but its directory is gone.
    missing(workspaces.compare_states(r(1), ws(1)).await.unwrap_err());
    missing(
        workspaces
            .diff_file(ws(1), r(1), "README.md")
            .await
            .unwrap_err(),
    );
    workspace_dir(&project, R1);
    exec(
        project.store(),
        "UPDATE workspaces SET state = 'missing' WHERE number = 1",
    )
    .await;
    missing(workspaces.compare_states(ws(1), r(1)).await.unwrap_err());
    let checkpointed = workspaces.compare_states(r(1), ck(1)).await.unwrap();
    assert_eq!(checkpointed.to.label, "ck-1");
    assert!(!checkpointed.files.is_empty());
}

#[tokio::test]
async fn unknown_states_and_paths_return_their_codes() {
    let project = TestProject::new().await;
    three_revisions(&project).await;
    let workspaces = service(&project);
    assert_eq!(
        code(workspaces.compare_states(r(9), r(1)).await.unwrap_err()),
        "revision_not_found"
    );
    assert_eq!(
        code(workspaces.compare_states(r(1), ws(9)).await.unwrap_err()),
        "workspace_not_found"
    );
    for (state, key, label) in [(c(9), "changeset", "C-9"), (ck(9), "checkpoint", "ck-9")] {
        let WorkspaceError::Fold(error) = workspaces.compare_states(state, r(1)).await.unwrap_err()
        else {
            panic!("a missing {key} is a refusal");
        };
        assert_eq!(error.code(), "not_found");
        assert_eq!(error.details().unwrap()[key], label);
    }
    for path in ["logo.png", "nowhere.txt"] {
        assert_eq!(
            code(workspaces.diff_file(r(2), r(3), path).await.unwrap_err()),
            "path_not_in_comparison"
        );
    }
    assert_eq!(
        code(workspaces.diff_file(r(1), r(2), "../x").await.unwrap_err()),
        "invalid_path"
    );
    let deleted = workspaces.diff_file(r(2), r(3), "README.md").await.unwrap();
    assert_eq!(deleted.change, ChangeKind::Deleted);
}

#[tokio::test]
async fn a_tree_state_compares_by_its_hash() {
    let project = TestProject::new().await;
    three_revisions(&project).await;
    let workspaces = service(&project);
    let r2 = workspaces.resolve_state(r(2)).await.unwrap();
    let tree = StateRef::Tree(r2.tree.unwrap());
    assert_eq!(tree.kind(), StateKind::Tree);
    assert_eq!(tree.to_string(), r2.tree.unwrap().to_string());
    let same = workspaces.compare_states(r(2), tree).await.unwrap();
    assert!(same.files.is_empty());
    assert_eq!(same.to.kind.as_str(), "tree");
}

#[tokio::test]
async fn a_closed_workspace_has_no_working_side_but_its_checkpoints_compare() {
    let project = TestProject::new().await;
    three_revisions(&project).await;
    checkpoint(&project, 1, CK1).await;
    workspace_dir(&project, R1);
    exec(
        project.store(),
        "UPDATE workspaces SET state = 'archived', closed_at = 1 WHERE number = 1",
    )
    .await;
    let workspaces = service(&project);
    let WorkspaceError::Fold(error) = workspaces.compare_states(r(1), ws(1)).await.unwrap_err()
    else {
        panic!("a closed workspace is a refusal");
    };
    assert_eq!(error.code(), "workspace_not_active");
    assert_eq!(error.details().unwrap()["state"], "archived");
    assert_eq!(
        code(
            workspaces
                .diff_file(ws(1), r(1), "README.md")
                .await
                .unwrap_err()
        ),
        "workspace_not_active"
    );
    let checkpointed = workspaces.compare_states(r(1), ck(1)).await.unwrap();
    assert_eq!(checkpointed.to.task, Some("T-3".parse().unwrap()));
}
