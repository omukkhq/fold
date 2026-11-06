#![cfg(test)]

use std::path::{Path, PathBuf};

use fold_core::WorkerState;
use fold_core::transcript::{ProjectFolder, claude_project_folder};
use serde_json::json;

use crate::common::acceptance::read_tree;
use crate::worker_resume::{TASK_WORKER, task_worker_for, until_runs, until_state, worker};
use crate::workers_support::FakeClaude;

/// The start folder fold gives the worker `agent` of the project at `root`.
fn start_of(root: &str, agent: &str) -> PathBuf {
    Path::new(root)
        .join(".fold/workers")
        .join(agent)
        .join("start")
}

/// A worker's shell command writes a file by a relative path, as the zsh loop of the incident did with its unquoted redirect target. The worker starts in its own start folder, so the file lands there and the project folder stays as main left it; once the worker has its workspace, a command that changes into it by its absolute path writes there.
#[tokio::test]
async fn acceptance_34_a_workers_relative_write_never_lands_in_the_project_folder() {
    let config = tempfile::TempDir::new().unwrap();
    let fake = FakeClaude::new("result")
        .writing_relative("plan.md")
        .with_config_dir(config.path());
    let harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    let root = harness.root().to_owned();
    let main_before = read_tree(Path::new(&root));

    for agent in ["web-lead", TASK_WORKER] {
        let run = until_runs(&fake, agent, 1).await.remove(0);
        let start = start_of(&root, agent);
        assert_eq!(Path::new(&run.cwd), start, "{agent}");
        assert!(start.join("plan.md").is_file(), "{agent}");
        // Write and Edit may still reach every workspace, and only the workspaces.
        assert_eq!(
            run.flag("--add-dir").map(PathBuf::from),
            Some(Path::new(&root).join(".fold/ws"))
        );
    }
    assert!(!Path::new(&root).join("plan.md").exists());
    assert_eq!(read_tree(Path::new(&root)), main_before);

    // The worker's workspace is where a command that changes into it writes.
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    let command = format!("cd '{}' && : > notes.txt", work.dir.display());
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(&command)
        .current_dir(start_of(&root, TASK_WORKER))
        .status()
        .unwrap();
    assert!(status.success());
    assert!(work.dir.join("notes.txt").is_file());
    assert!(!Path::new(&root).join("notes.txt").exists());
    let changes = harness
        .call(
            TASK_WORKER,
            "get_workspace_changes",
            json!({"ws": work.workspace}),
        )
        .await;
    assert!(changes.to_string().contains("notes.txt"), "{changes}");

    // Claude Code names the worker's transcript folder after the folder it runs in.
    let row = until_state(&harness, TASK_WORKER, WorkerState::Idle).await;
    let transcripts = harness
        .handle
        .agents()
        .worker_transcripts(std::slice::from_ref(&row))
        .await;
    let start = start_of(&root, TASK_WORKER).canonicalize().unwrap();
    let ProjectFolder::Named(folder) = claude_project_folder(&start) else {
        panic!("the test's start folder has a short path");
    };
    let expected = config.path().join("projects").join(folder).join(format!(
        "{}.jsonl",
        worker(&harness, TASK_WORKER).await.session_id
    ));
    assert_eq!(
        transcripts,
        vec![Some(expected.to_string_lossy().into_owned())]
    );
}

/// The start folder holds copies of the project's Claude Code settings, servers and fold's skills, so a worker there reads the same ones as in the project folder; a write to a copy never reaches the project folder.
#[tokio::test]
async fn acceptance_34_a_workers_start_folder_holds_copies_of_the_projects_claude_files() {
    let fake = FakeClaude::new("result");
    let mut harness =
        crate::common::acceptance::Harness::start_with_launcher(fake.launcher.clone()).await;
    let root = PathBuf::from(harness.root());
    std::fs::create_dir_all(root.join(".claude/skills/fold-area-lead")).unwrap();
    std::fs::write(root.join(".claude/settings.json"), "{\"env\":{}}\n").unwrap();
    std::fs::write(root.join(".mcp.json"), "{\"mcpServers\":{}}\n").unwrap();
    std::fs::write(
        root.join(".claude/skills/fold-area-lead/SKILL.md"),
        "---\nname: fold-area-lead\n---\n",
    )
    .unwrap();
    harness.make_main("frontend").await;
    harness
        .call(
            "frontend",
            "create_label",
            json!({"name": "ui", "color": "sage"}),
        )
        .await;
    harness
        .call(
            "frontend",
            "define_area",
            json!({"name": "web", "label": "ui"}),
        )
        .await;
    harness
        .call(
            "frontend",
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    until_state(&harness, "web-lead", WorkerState::Idle).await;
    harness.connect("web-lead").await;

    let start = start_of(harness.root(), "web-lead");
    for path in [
        ".claude/settings.json",
        ".mcp.json",
        ".claude/skills/fold-area-lead/SKILL.md",
    ] {
        assert_eq!(
            std::fs::read(start.join(path)).unwrap(),
            std::fs::read(root.join(path)).unwrap(),
            "{path}"
        );
        assert!(!start.join(path).is_symlink(), "{path}");
    }
    assert!(!start.join(".claude/settings.local.json").exists());
}
