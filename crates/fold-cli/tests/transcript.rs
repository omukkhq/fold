#![cfg(test)]

use std::path::{Path, PathBuf};
use std::process::Output;

use fold_core::{AgentName, ProjectPaths};

use crate::common::{DaemonGuard, Host, daemons};

const ID: &str = "0f8e2c1a-5b7d-4c3e-9a1f-2d4b6c8e0a13";

/// `fld transcript <args>` in the project root, with Claude Code's folder `config` when one is given.
async fn fld_transcript(host: &Host, args: &[&str], config: Option<&Path>) -> Output {
    let mut all = vec!["transcript"];
    all.extend_from_slice(args);
    let mut command = host.fold(&all, &host.root);
    command
        .env("FOLD_HUMAN", "ana")
        .env_remove("CLAUDE_CODE_PROJECT_DIR_NAME");
    if let Some(config) = config {
        command.env("CLAUDE_CONFIG_DIR", config);
    }
    command.output().await.unwrap()
}

fn record(host: &Host, name: &str, id: &str) {
    let path = ProjectPaths::new(&host.root).main_session(&AgentName::parse(name).unwrap());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("{id}\n")).unwrap();
}

fn transcript_in(config: &Path, host: &Host, id: &str) -> PathBuf {
    let folder: String = host
        .root
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    config
        .join("projects")
        .join(folder)
        .join(format!("{id}.jsonl"))
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

#[tokio::test]
async fn the_main_agents_transcript_is_its_recorded_session_in_claude_codes_folder() {
    let host = Host::new().await;
    let config = host.dir.path().join("claude-config");
    record(&host, "main", ID);
    let file = transcript_in(&config, &host, ID);

    let missing = fld_transcript(&host, &["main"], Some(&config)).await;
    assert_eq!(missing.status.code(), Some(1));
    let stderr = text(&missing.stderr);
    assert!(
        stderr.contains(&format!(
            "transcript_not_found: {} does not exist\nhint: Claude Code writes the transcript after the first message of the session",
            file.display()
        )),
        "{stderr}"
    );

    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{\"n\":1}\n{\"n\":2}\n{\"n\":3}\n").unwrap();
    let path = fld_transcript(&host, &["main"], Some(&config)).await;
    assert!(path.status.success(), "{path:?}");
    assert_eq!(text(&path.stdout), format!("{}\n", file.display()));
    let tail = fld_transcript(&host, &["main", "--tail", "2"], Some(&config)).await;
    assert!(tail.status.success(), "{tail:?}");
    assert_eq!(text(&tail.stdout), "{\"n\":2}\n{\"n\":3}\n");

    let home = fld_transcript(&host, &["main"], None).await;
    assert_eq!(home.status.code(), Some(1));
    let home_file = transcript_in(&host.dir.path().join(".claude"), &host, ID);
    assert!(
        text(&home.stderr).contains(&home_file.display().to_string()),
        "without CLAUDE_CONFIG_DIR the transcript is below $HOME/.claude: {}",
        text(&home.stderr)
    );
    assert!(
        daemons(&host.default_socket()).is_empty(),
        "a recorded main session needs no daemon"
    );
}

#[tokio::test]
async fn a_renamed_project_folder_hides_the_main_agents_transcript() {
    let host = Host::new().await;
    record(&host, "main", ID);
    let output = host
        .fold(&["transcript", "main"], &host.root)
        .env("FOLD_HUMAN", "ana")
        .env("CLAUDE_CODE_PROJECT_DIR_NAME", "app")
        .output()
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "transcript_not_found: fold cannot tell where Claude Code keeps the transcript of session {ID}\nhint: run `find ~/.claude/projects -name {ID}.jsonl`"
        )),
        "{stderr}"
    );
    assert!(output.stdout.is_empty());
}

#[tokio::test]
async fn an_agent_without_a_session_is_refused_with_a_hint() {
    let host = Host::new().await;
    let output = fld_transcript(&host, &["ghost"], None).await;
    let guard = DaemonGuard(daemons(&host.default_socket()));
    assert_eq!(output.status.code(), Some(1));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains(
            "agent_session_not_found: fold knows no Claude Code session of ghost\nhint: "
        ),
        "{stderr}"
    );
    assert!(output.stdout.is_empty());
    guard.stop().await;
}
