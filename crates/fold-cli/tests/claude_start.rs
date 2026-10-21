#![cfg(test)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use fold_cli::claude_user::{self, UserFiles};
use fold_core::{AgentName, ProjectPaths};

use crate::common::{DaemonGuard, Host, adapter, daemons, list_agents};

/// A stand-in for the user's `claude` at the front of `PATH`: it prints its arguments, its working directory and the fold variables, one per line. Like Claude Code, it keeps an empty transcript `<id>.jsonl` for the session in `projects/<working directory with - for every other character than a letter or digit>` of `CLAUDE_CONFIG_DIR` or `$HOME/.claude`, with a fixed id when no `--session-id` names it.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
for arg in "$@"; do printf 'arg %s\n' "$arg"; done
printf 'cwd %s\n' "$(pwd -P)"
printf 'FOLD_AGENT=%s\n' "$FOLD_AGENT"
printf 'FOLD_AGENT_TAGS=%s\n' "$FOLD_AGENT_TAGS"
session=99999999-9999-4999-8999-999999999999
while [ $# -gt 0 ]; do
  case "$1" in --session-id|--resume) session="$2" ;; esac
  shift
done
folder="${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects/$(pwd -P | sed 's/[^a-zA-Z0-9]/-/g')"
mkdir -p "$folder" && : > "$folder/$session.jsonl"
"#;

/// The session id a user's own `claude`, started without fld, gets from [`FAKE_CLAUDE`].
const OUTSIDE_SESSION: &str = "99999999-9999-4999-8999-999999999999";

/// [`FAKE_CLAUDE`] in a `bin` folder of the host.
fn fake_claude_bin(host: &Host) -> PathBuf {
    let bin = host.dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let program = bin.join("claude");
    std::fs::write(&program, FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

/// `fld <args>` in `cwd`, with the fake `claude` first on `PATH` and the human `ana`.
async fn fld(host: &Host, args: &[&str], cwd: &Path) -> Output {
    fld_with_config(host, args, cwd, None).await
}

/// [`fld`] with Claude Code's folder in `CLAUDE_CONFIG_DIR` when `config` names one.
async fn fld_with_config(host: &Host, args: &[&str], cwd: &Path, config: Option<&Path>) -> Output {
    let path = format!(
        "{}:{}",
        fake_claude_bin(host).display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut command = host.fold(args, cwd);
    command
        .env("PATH", path)
        .env("FOLD_HUMAN", "ana")
        .env_remove("FOLD_AGENT_TAGS")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CLAUDE_CODE_PROJECT_DIR_NAME");
    if let Some(config) = config {
        command.env("CLAUDE_CONFIG_DIR", config);
    }
    command.output().await.unwrap()
}

/// `fld claude` with `args`.
async fn fld_claude(host: &Host, args: &[&str], cwd: &Path) -> Output {
    let mut all = vec!["claude"];
    all.extend_from_slice(args);
    fld(host, &all, cwd).await
}

/// Every argument the fake `claude` got.
fn all_args(output: &Output) -> Vec<&str> {
    assert!(output.status.success(), "{}", text(&output.stderr));
    text(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("arg "))
        .collect()
}

/// The file the fake `claude` got as `--append-system-prompt-file`, when it got one.
fn prompt_arg(output: &Output) -> Option<&str> {
    let args = all_args(output);
    let at = args
        .iter()
        .position(|arg| *arg == "--append-system-prompt-file")?;
    args.get(at + 1).copied()
}

/// The arguments the fake `claude` got other than the main agent prompt, which [`prompt_arg`] reads, and the flags that give a user without the user setup its server, permission and hooks.
fn claude_args(output: &Output) -> Vec<&str> {
    let mut args = all_args(output);
    for flag in ["--append-system-prompt-file", "--mcp-config", "--settings"] {
        if let Some(at) = args.iter().position(|arg| *arg == flag) {
            args.drain(at..at + 2);
        }
    }
    args
}

/// The value the fake `claude` got for `flag`.
fn flag_arg<'a>(output: &'a Output, flag: &str) -> Option<&'a str> {
    let args = all_args(output);
    let at = args.iter().position(|arg| *arg == flag)?;
    args.get(at + 1).copied()
}

/// Claude Code's user files below the host's `HOME`.
fn user_files(host: &Host) -> UserFiles {
    UserFiles::from_vars(|name| (name == "HOME").then(|| host.dir.path().as_os_str().to_owned()))
        .unwrap()
}

/// The main session id that fold recorded for the agent `main` of the project.
fn recorded(host: &Host) -> String {
    recorded_for(host, "main")
}

/// The main session id that fold recorded for the agent `name` of the project.
fn recorded_for(host: &Host, name: &str) -> String {
    let agent = AgentName::parse(name).unwrap();
    std::fs::read_to_string(ProjectPaths::new(&host.root).main_session(&agent))
        .unwrap()
        .trim()
        .to_owned()
}

/// The transcript the fake `claude` keeps for a session of the project in `$HOME/.claude`.
fn transcript(host: &Host, id: &str) -> PathBuf {
    transcript_in(&host.dir.path().join(".claude"), host, id)
}

/// The transcript the fake `claude` keeps for a session of the project in Claude Code's folder `config`.
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
async fn fld_claude_starts_the_daemon_and_runs_claude_as_the_main_agent_in_the_project_root() {
    let host = Host::new().await;
    let below = host.root.join("src");
    std::fs::create_dir_all(&below).unwrap();
    assert!(daemons(&host.default_socket()).is_empty());

    claude_user::add(&user_files(&host)).unwrap();
    let output = fld_claude(&host, &["--", "--model", "opus"], &below).await;
    let guard = DaemonGuard(daemons(&host.default_socket()));
    assert!(output.status.success(), "{}", text(&output.stderr));
    let session = recorded(&host);
    let prompt = ProjectPaths::new(&host.root).main_prompt_file();
    assert_eq!(
        text(&output.stdout),
        format!(
            "arg --name\narg main\narg --session-id\narg {session}\narg --append-system-prompt-file\narg {}\narg --model\narg opus\ncwd {}\nFOLD_AGENT=main\nFOLD_AGENT_TAGS=main\n",
            prompt.display(),
            host.root.display()
        )
    );
    assert_eq!(
        std::fs::read_to_string(&prompt).unwrap(),
        fold_core::main_prompt()
    );
    assert_eq!(guard.0.len(), 1, "fld claude started the daemon");
    assert!(
        !host.root.join(".claude").exists(),
        "fld claude writes no Claude Code settings"
    );

    // The session's adapter inherits both variables and registers the main agent.
    let mut command = host.fold(&["mcp"], &host.root);
    command
        .env("FOLD_AGENT", "main")
        .env("FOLD_AGENT_TAGS", "main");
    let (client, mut child) = adapter(command).await;
    let agents = list_agents(&client).await;
    assert_eq!(agents["agents"][0]["name"], "main", "{agents}");
    assert_eq!(agents["agents"][0]["tags"], serde_json::json!(["main"]));
    client.cancel().await.unwrap();
    child.wait().await.unwrap();

    let again = fld_claude(&host, &[], &host.root).await;
    assert_eq!(
        claude_args(&again),
        ["--name", "main", "--resume", &session]
    );

    let other = fld_claude(&host, &["--name", "lead"], &host.root).await;
    assert_eq!(other.status.code(), Some(1));
    assert!(other.stdout.is_empty(), "claude did not run");
    let stderr = text(&other.stderr);
    assert!(
        stderr.contains("main_tag_denied: agent main is the main agent of this project\nhint: run `fld claude --name main` to continue as it"),
        "{stderr}"
    );
    guard.stop().await;
}

#[tokio::test]
async fn without_the_user_setup_fld_claude_names_the_command_and_gives_fold_by_flags() {
    let host = Host::new().await;
    let output = fld_claude(&host, &["--", "hello"], &host.root).await;
    let guard = DaemonGuard(daemons(&host.default_socket()));
    assert!(
        text(&output.stdout).starts_with(
            "fold is not in your Claude Code config; run fld setup --claude to add it\n"
        ),
        "{}",
        text(&output.stdout)
    );
    let files = user_files(&host);
    assert!(!files.config.exists() && !files.settings.exists());
    let mcp: serde_json::Value =
        serde_json::from_str(flag_arg(&output, "--mcp-config").unwrap()).unwrap();
    assert_eq!(
        mcp["mcpServers"]["fold"]["args"],
        serde_json::json!(["mcp"])
    );
    assert_eq!(mcp["mcpServers"]["fold"]["env"]["FOLD_AGENT"], "main");
    let settings: serde_json::Value =
        serde_json::from_str(flag_arg(&output, "--settings").unwrap()).unwrap();
    assert_eq!(settings, fold_core::layout::session_settings());
    let args = all_args(&output);
    assert_eq!(args[0], "--mcp-config");
    assert_eq!(args.last(), Some(&"hello"));

    // A user-scope fold server is not passed again; the settings the user lacks still are.
    std::fs::write(
        &files.config,
        r#"{"mcpServers": {"fold": {"type": "stdio", "command": "fld", "args": ["mcp"]}}}"#,
    )
    .unwrap();
    let server_only = fld_claude(&host, &[], &host.root).await;
    assert!(flag_arg(&server_only, "--mcp-config").is_none());
    assert_eq!(
        flag_arg(&server_only, "--settings"),
        Some(fold_core::layout::session_settings().to_string().as_str())
    );

    claude_user::add(&files).unwrap();
    let set_up = fld_claude(&host, &[], &host.root).await;
    assert!(flag_arg(&set_up, "--mcp-config").is_none());
    assert!(flag_arg(&set_up, "--settings").is_none());
    assert!(!text(&set_up.stdout).contains("fld setup --claude"));
    guard.stop().await;
}

#[tokio::test]
async fn fld_claude_resumes_the_main_session_fld_started_last_and_fld_new_starts_another() {
    let host = Host::new().await;
    // The user's own session, started without fld, is the newest of the folder but never resumed.
    let outside = std::process::Command::new(fake_claude_bin(&host).join("claude"))
        .current_dir(&host.root)
        .env("HOME", host.dir.path())
        .env_remove("CLAUDE_CONFIG_DIR")
        .output()
        .unwrap();
    assert!(outside.status.success());
    assert!(transcript(&host, OUTSIDE_SESSION).is_file());

    let first = fld_claude(&host, &[], &host.root).await;
    let guard = DaemonGuard(daemons(&host.default_socket()));
    let x = recorded(&host);
    assert_ne!(x, OUTSIDE_SESSION);
    assert_eq!(claude_args(&first), ["--name", "main", "--session-id", &x]);
    let resumed = fld_claude(&host, &["--", "--model", "opus"], &host.root).await;
    assert_eq!(
        claude_args(&resumed),
        ["--name", "main", "--resume", &x, "--model", "opus"]
    );

    let new = fld(&host, &["new"], &host.root).await;
    let y = recorded(&host);
    assert_ne!(y, x);
    assert_eq!(claude_args(&new), ["--name", "main", "--session-id", &y]);
    let resumed = fld_claude(&host, &[], &host.root).await;
    assert_eq!(claude_args(&resumed), ["--name", "main", "--resume", &y]);

    let new = fld(
        &host,
        &["new", "claude", "--", "--model", "opus"],
        &host.root,
    )
    .await;
    let z = recorded(&host);
    assert!(![&x, &y].contains(&&z));
    assert_eq!(
        claude_args(&new),
        ["--name", "main", "--session-id", &z, "--model", "opus"]
    );

    // Claude Code no longer has the session: a new one starts instead of a "No conversation found" error.
    std::fs::remove_file(transcript(&host, &z)).unwrap();
    let lost = fld_claude(&host, &[], &host.root).await;
    let w = recorded(&host);
    assert!(![&x, &y, &z].contains(&&w));
    assert_eq!(claude_args(&lost), ["--name", "main", "--session-id", &w]);
    let resumed = fld_claude(&host, &[], &host.root).await;
    assert_eq!(claude_args(&resumed), ["--name", "main", "--resume", &w]);

    let mut command = host.fold(&["mcp"], &host.root);
    command
        .env("FOLD_AGENT", "main")
        .env("FOLD_AGENT_TAGS", "main");
    let (client, mut child) = adapter(command).await;
    list_agents(&client).await;
    client.cancel().await.unwrap();
    child.wait().await.unwrap();
    let taken = fld(&host, &["new", "claude", "--name", "lead"], &host.root).await;
    assert_eq!(taken.status.code(), Some(1));
    assert!(taken.stdout.is_empty(), "claude did not run");
    assert!(text(&taken.stderr).contains("main_tag_denied"));
    assert_eq!(recorded(&host), w, "a refused start records nothing");
    guard.stop().await;
}

#[tokio::test]
async fn the_users_own_session_arguments_win_and_each_name_keeps_its_own_session() {
    let host = Host::new().await;
    let first = fld_claude(&host, &[], &host.root).await;
    let guard = DaemonGuard(daemons(&host.default_socket()));
    let x = recorded(&host);
    assert_eq!(claude_args(&first), ["--name", "main", "--session-id", &x]);

    let own = fld_claude(&host, &["--", "--resume", OUTSIDE_SESSION], &host.root).await;
    assert_eq!(
        claude_args(&own),
        ["--name", "main", "--resume", OUTSIDE_SESSION]
    );
    assert_eq!(recorded(&host), x, "fld keeps its record");
    let own = fld(&host, &["new", "--", "-c"], &host.root).await;
    assert_eq!(claude_args(&own), ["--name", "main", "-c"]);
    let own = fld_claude(&host, &["--", "--session-id", OUTSIDE_SESSION], &host.root).await;
    assert_eq!(
        claude_args(&own),
        ["--name", "main", "--session-id", OUTSIDE_SESSION]
    );
    assert_eq!(recorded(&host), x, "fld keeps its record");

    let lead = fld_claude(&host, &["--name", "lead"], &host.root).await;
    let l = recorded_for(&host, "lead");
    assert_ne!(l, x);
    assert_eq!(claude_args(&lead), ["--name", "lead", "--session-id", &l]);
    assert_eq!(recorded(&host), x);
    let resumed = fld_claude(&host, &[], &host.root).await;
    assert_eq!(claude_args(&resumed), ["--name", "main", "--resume", &x]);
    let resumed = fld_claude(&host, &["--name", "lead"], &host.root).await;
    assert_eq!(claude_args(&resumed), ["--name", "lead", "--resume", &l]);
    guard.stop().await;
}

#[tokio::test]
async fn every_start_and_resume_appends_the_main_prompt_unless_the_user_sets_one() {
    let host = Host::new().await;
    let paths = ProjectPaths::new(&host.root);
    let shipped = paths.main_prompt_file().display().to_string();
    let first = fld_claude(&host, &[], &host.root).await;
    let guard = DaemonGuard(daemons(&host.default_socket()));
    assert_eq!(prompt_arg(&first), Some(shipped.as_str()));
    let resumed = fld_claude(&host, &[], &host.root).await;
    assert_eq!(claude_args(&resumed)[2], "--resume");
    assert_eq!(prompt_arg(&resumed), Some(shipped.as_str()));
    let new = fld(&host, &["new"], &host.root).await;
    assert_eq!(prompt_arg(&new), Some(shipped.as_str()));
    let own_session = fld(&host, &["new", "--", "-c"], &host.root).await;
    assert_eq!(prompt_arg(&own_session), Some(shipped.as_str()));

    let own = paths.main_role_prompt();
    std::fs::create_dir_all(own.parent().unwrap()).unwrap();
    std::fs::write(&own, "# Our main agent\n").unwrap();
    let overridden = fld_claude(&host, &[], &host.root).await;
    assert_eq!(prompt_arg(&overridden), Some(shipped.as_str()));
    assert_eq!(
        std::fs::read_to_string(paths.main_prompt_file()).unwrap(),
        format!("# Our main agent\n\n{}", fold_core::shared_prompt())
    );
    let new = fld(&host, &["new"], &host.root).await;
    assert_eq!(prompt_arg(&new), Some(shipped.as_str()));

    for flag in [
        &["--system-prompt", "be brief"][..],
        &["--system-prompt-file", "p.md"],
        &["--append-system-prompt", "be brief"],
        &["--append-system-prompt-file=p.md"],
    ] {
        let mut args = vec!["--"];
        args.extend_from_slice(flag);
        let output = fld_claude(&host, &args, &host.root).await;
        let all = all_args(&output);
        assert_eq!(&all[all.len() - flag.len()..], flag, "{flag:?}");
        assert_eq!(
            all.iter()
                .filter(|arg| arg.contains("system-prompt"))
                .count(),
            1,
            "only the user's own flag: {all:?}"
        );
    }
    guard.stop().await;
}

#[tokio::test]
async fn the_transcript_is_looked_for_in_claude_config_dir() {
    let host = Host::new().await;
    let config = host.dir.path().join("claude-config");
    let first = fld_with_config(&host, &["claude"], &host.root, Some(&config)).await;
    let guard = DaemonGuard(daemons(&host.default_socket()));
    let x = recorded(&host);
    assert_eq!(claude_args(&first), ["--name", "main", "--session-id", &x]);
    assert!(transcript_in(&config, &host, &x).is_file());
    assert!(!transcript(&host, &x).exists());
    let resumed = fld_with_config(&host, &["claude"], &host.root, Some(&config)).await;
    assert_eq!(claude_args(&resumed), ["--name", "main", "--resume", &x]);

    // Without CLAUDE_CONFIG_DIR, Claude Code looks in $HOME/.claude, which does not have the session.
    let home = fld_claude(&host, &[], &host.root).await;
    let y = recorded(&host);
    assert_ne!(y, x);
    assert_eq!(claude_args(&home), ["--name", "main", "--session-id", &y]);
    guard.stop().await;
}

#[tokio::test]
async fn fld_claude_outside_a_project_fails_with_a_hint_and_runs_nothing() {
    let host = Host::new().await;
    let outside = host.dir.path().join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();

    let output = fld_claude(&host, &[], &outside).await;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty(), "claude did not run");
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("project_not_found: no registered project holds this directory\nhint: run `fld init` in the project folder first"),
        "{stderr}"
    );
    assert!(daemons(&host.default_socket()).is_empty());

    let bad = fld_claude(&host, &["--name", "Not A Name"], &host.root).await;
    assert_eq!(bad.status.code(), Some(1));
    assert!(
        text(&bad.stderr).contains("invalid_name"),
        "{}",
        text(&bad.stderr)
    );
    assert!(daemons(&host.default_socket()).is_empty());
}

#[tokio::test]
async fn fld_claude_brings_a_removed_project_back_before_claude_starts() {
    let host = Host::new().await;
    claude_user::add(&user_files(&host)).unwrap();
    let registry = fold_store::Store::open_registry(&host.data_home().join("fold/registry.db"))
        .await
        .unwrap();
    let now = fold_core::Timestamp::from_millis(5).unwrap();
    registry
        .write(async |tx| fold_store::projects::set_removed(tx, "app", now).await)
        .await
        .unwrap();

    let output = fld_claude(&host, &[], &host.root).await;
    let guard = DaemonGuard(daemons(&host.default_socket()));
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout).lines().next(),
        Some("project app is back in fold")
    );
    assert_eq!(claude_args(&output)[..2], ["--name", "main"]);
    let found = fold_store::projects::find_registration(registry.read(), "app")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.removed_at, None);
    registry.close().await.unwrap();
    guard.stop().await;
}

#[tokio::test]
async fn fld_claude_notes_that_a_person_uses_the_project() {
    let host = Host::new().await;
    claude_user::add(&user_files(&host)).unwrap();
    let registry = fold_store::Store::open_registry(&host.data_home().join("fold/registry.db"))
        .await
        .unwrap();
    let at = |millis| fold_core::Timestamp::from_millis(millis).unwrap();
    let far = at(i64::from(i32::MAX) * 1_000);
    assert!(
        registry
            .write(async |tx| fold_store::projects::touch_project(tx, "app", at(5), far).await)
            .await
            .unwrap()
    );
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();

    let output = fld_claude(&host, &[], &host.root).await;
    let guard = DaemonGuard(daemons(&host.default_socket()));
    assert!(output.status.success(), "{}", text(&output.stderr));
    let found = fold_store::projects::find_registration(registry.read(), "app")
        .await
        .unwrap()
        .unwrap();
    assert!(
        i128::from(found.last_used_at.as_millis()) >= before as i128,
        "{:?}",
        found.last_used_at
    );
    assert_eq!(found.removed_at, None);
    registry.close().await.unwrap();
    guard.stop().await;
}
