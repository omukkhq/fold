use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

struct Sandbox {
    dir: TempDir,
}

impl Sandbox {
    fn new() -> io::Result<Self> {
        Ok(Self {
            dir: TempDir::new()?,
        })
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// The environment holds no real home and a `PATH` of the sandbox only, so no run reaches the user's service manager.
    fn fold(&self, args: &[&str], path: &Path, xdg_config: Option<&Path>) -> io::Result<Output> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fld"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.path("home"))
            .env("USER", "fold-test-user")
            .env("PATH", path);
        if let Some(config) = xdg_config {
            command.env("XDG_CONFIG_HOME", config);
        }
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        command.output()
    }

    #[cfg(target_os = "linux")]
    fn fake_systemctl(&self, exit: i32) -> io::Result<(PathBuf, PathBuf)> {
        self.fake_tool("systemctl", exit)
    }

    /// A script called `name` in the sandbox's `bin` that logs its arguments to `<name>.log` and exits with `exit`.
    fn fake_tool(&self, name: &str, exit: i32) -> io::Result<(PathBuf, PathBuf)> {
        let bin = self.path("bin");
        std::fs::create_dir_all(&bin)?;
        let log = self.path(&format!("{name}.log"));
        let script = bin.join(name);
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$@\" >> '{}'\nexit {exit}\n",
                log.display()
            ),
        )?;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
        Ok((bin, log))
    }
}

#[cfg(target_os = "linux")]
fn golden(exe: &str) -> String {
    format!(
        "[Unit]\nDescription=fold daemon\nAfter=default.target\n\n[Service]\nType=simple\nExecStart={exe} serve\nRestart=on-failure\nRestartSec=2\n\n[Install]\nWantedBy=default.target\n"
    )
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[cfg(target_os = "linux")]
#[test]
fn no_start_writes_the_unit_under_the_default_config_directory() {
    let sandbox = Sandbox::new().unwrap();
    let output = sandbox
        .fold(&["setup", "--no-start"], Path::new(""), None)
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let unit = sandbox.path("home/.config/systemd/user/fold.service");
    let exe = env!("CARGO_BIN_EXE_fld");
    if exe.contains([' ', '%', '"', '\\']) {
        return;
    }
    assert_eq!(std::fs::read_to_string(&unit).unwrap(), golden(exe));
    let text = stdout(&output);
    assert!(text.starts_with(&format!("wrote {}\n", unit.display())));
    assert!(text.contains("loginctl enable-linger fold-test-user"));

    let before = std::fs::read(&unit).unwrap();
    let again = sandbox
        .fold(&["setup", "--no-start"], Path::new(""), None)
        .unwrap();
    assert!(again.status.success());
    assert_eq!(std::fs::read(&unit).unwrap(), before);
}

#[cfg(target_os = "linux")]
#[test]
fn xdg_config_home_decides_where_the_unit_goes() {
    let sandbox = Sandbox::new().unwrap();
    let config = sandbox.path("cfg");
    let output = sandbox
        .fold(&["setup", "--no-start"], Path::new(""), Some(&config))
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(config.join("systemd/user/fold.service").is_file());
    assert!(!sandbox.path("home/.config").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn install_enables_and_starts_through_systemctl() {
    let sandbox = Sandbox::new().unwrap();
    let (bin, log) = sandbox.fake_systemctl(0).unwrap();
    let output = sandbox.fold(&["setup"], &bin, None).unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "--user daemon-reload\n--user enable --now fold.service\n"
    );
    assert!(stdout(&output).contains("enabled and started fold.service"));

    let broken = Sandbox::new().unwrap();
    let (bin, _) = broken.fake_systemctl(1).unwrap();
    let failing = broken.fold(&["setup"], &bin, None).unwrap();
    assert_eq!(failing.status.code(), Some(1));
    assert!(stderr(&failing).contains("service_install_failed"));
}

#[cfg(target_os = "linux")]
#[test]
fn a_host_without_systemctl_is_refused_with_invalid_request() {
    let sandbox = Sandbox::new().unwrap();
    let output = sandbox.fold(&["setup"], Path::new(""), None).unwrap();
    assert_eq!(output.status.code(), Some(1));
    let text = stderr(&output);
    assert!(
        text.contains("invalid_request: no service manager support"),
        "{text}"
    );
    assert!(text.contains("hint: run `fld serve` under the host's own supervisor"));
    assert!(!sandbox.path("home/.config").exists());
}

#[cfg(target_os = "macos")]
#[test]
fn no_start_on_macos_writes_the_launch_agent_and_runs_no_launchctl() {
    let sandbox = Sandbox::new().unwrap();
    let (bin, log) = sandbox.fake_tool("launchctl", 0).unwrap();
    let output = sandbox.fold(&["setup", "--no-start"], &bin, None).unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let agent = sandbox.path("home/Library/LaunchAgents/dev.omukk.fold.plist");
    let text = std::fs::read_to_string(&agent).unwrap();
    assert!(text.contains("<string>dev.omukk.fold</string>"));
    assert!(text.contains("<string>serve</string>"));
    assert!(text.contains(&format!(
        "<key>PATH</key>\n\t\t<string>{}</string>",
        bin.display()
    )));
    assert_eq!(
        stdout(&output),
        format!(
            "wrote {}\nfold is not in your Claude Code config; run fld setup --claude to add it\n",
            agent.display()
        )
    );
    assert!(!log.exists());
    // The plist parser of macOS accepts the file.
    let lint = Command::new("/usr/bin/plutil")
        .arg("-lint")
        .arg(&agent)
        .output()
        .unwrap();
    assert!(
        lint.status.success(),
        "{}",
        String::from_utf8_lossy(&lint.stdout)
    );
    assert!(!sandbox.path("home/.config/systemd").exists());
}

#[cfg(target_os = "macos")]
#[test]
fn setup_on_macos_loads_the_agent_through_launchctl() {
    let sandbox = Sandbox::new().unwrap();
    let (bin, log) = sandbox.fake_tool("launchctl", 0).unwrap();
    let output = sandbox.fold(&["setup"], &bin, None).unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let uid = rustix::process::getuid().as_raw();
    let agent = sandbox.path("home/Library/LaunchAgents/dev.omukk.fold.plist");
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        format!(
            "bootout gui/{uid}/dev.omukk.fold\nenable gui/{uid}/dev.omukk.fold\nbootstrap gui/{uid} {}\n",
            agent.display()
        )
    );
    assert!(stdout(&output).contains("loaded dev.omukk.fold"));

    let broken = Sandbox::new().unwrap();
    let (bin, _) = broken.fake_tool("launchctl", 1).unwrap();
    let failing = broken.fold(&["setup"], &bin, None).unwrap();
    assert_eq!(failing.status.code(), Some(1));
    assert!(stderr(&failing).contains("service_install_failed"));

    let missing = Sandbox::new().unwrap();
    let refused = missing.fold(&["setup"], Path::new(""), None).unwrap();
    assert_eq!(refused.status.code(), Some(1));
    assert!(stderr(&refused).contains("invalid_request: no service manager support"));
    assert!(!missing.path("home/Library").exists());
}

/// The Claude Code files of the sandbox's home, as JSON.
#[cfg(test)]
fn user_file(sandbox: &Sandbox, relative: &str) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(sandbox.path(relative)).unwrap()).unwrap()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn setup_without_a_terminal_leaves_the_claude_config_and_names_the_command() {
    let sandbox = Sandbox::new().unwrap();
    let output = sandbox
        .fold(&["setup", "--no-start"], Path::new(""), None)
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).ends_with(
            "fold is not in your Claude Code config; run fld setup --claude to add it\n"
        )
    );
    assert!(!sandbox.path("home/.claude.json").exists());
    assert!(!sandbox.path("home/.claude").exists());
}

#[test]
fn setup_claude_merges_into_the_user_files_and_remove_takes_fold_out_again() {
    let sandbox = Sandbox::new().unwrap();
    let home = sandbox.path("home");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    let config =
        r#"{"numStartups": 3, "mcpServers": {"pencil": {"command": "pencil"}}, "projects": {}}"#;
    std::fs::write(home.join(".claude.json"), config).unwrap();
    std::fs::set_permissions(
        home.join(".claude.json"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let settings = r#"{"permissions": {"allow": ["mcp__pencil"]}, "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "say done"}]}]}, "theme": "light"}"#;
    std::fs::write(home.join(".claude/settings.json"), settings).unwrap();

    let output = sandbox
        .fold(&["setup", "--claude"], Path::new(""), None)
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).starts_with("added fold to Claude Code in "));
    assert!(!sandbox.path("home/.config/systemd").exists(), "no service");
    let merged = user_file(&sandbox, "home/.claude.json");
    let keys: Vec<_> = merged.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["numStartups", "mcpServers", "projects"]);
    let servers: Vec<_> = merged["mcpServers"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(servers, ["pencil", "fold"]);
    assert_eq!(
        merged["mcpServers"]["fold"],
        serde_json::json!({"type": "stdio", "command": "fld", "args": ["mcp"], "env": {}})
    );
    let mode = std::fs::metadata(home.join(".claude.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "the file keeps its mode");
    let user_settings = user_file(&sandbox, "home/.claude/settings.json");
    assert_eq!(
        user_settings["permissions"]["allow"],
        serde_json::json!(["mcp__pencil", "mcp__fold"])
    );
    assert_eq!(user_settings["theme"], "light");
    assert_eq!(user_settings["hooks"]["Stop"].as_array().unwrap().len(), 2);
    assert_eq!(
        user_settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        "fld hook pre-tool-use"
    );
    let bytes = std::fs::read(home.join(".claude/settings.json")).unwrap();
    let again = sandbox
        .fold(&["setup", "--claude"], Path::new(""), None)
        .unwrap();
    assert!(again.status.success());
    assert_eq!(
        std::fs::read(home.join(".claude/settings.json")).unwrap(),
        bytes
    );

    let removed = sandbox
        .fold(&["setup", "--remove"], Path::new(""), None)
        .unwrap();
    assert!(removed.status.success(), "{}", stderr(&removed));
    assert!(stdout(&removed).starts_with("removed fold from Claude Code in "));
    assert_eq!(
        user_file(&sandbox, "home/.claude.json"),
        serde_json::from_str::<serde_json::Value>(config).unwrap()
    );
    assert_eq!(
        user_file(&sandbox, "home/.claude/settings.json"),
        serde_json::from_str::<serde_json::Value>(settings).unwrap()
    );
    let declined = std::fs::read_to_string(sandbox.path("home/.config/fold/claude.toml")).unwrap();
    assert_eq!(declined, "declined = true\n");
}

#[test]
fn setup_claude_refuses_a_malformed_user_file_and_changes_neither() {
    let sandbox = Sandbox::new().unwrap();
    let home = sandbox.path("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join(".claude.json"), "[1]").unwrap();
    let output = sandbox
        .fold(&["setup", "--claude"], Path::new(""), None)
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let text = stderr(&output);
    assert!(
        text.contains(&format!(
            "invalid_request: {} does not hold a JSON object\nhint: correct or move the file, then run `fld setup --claude`",
            home.join(".claude.json").display()
        )),
        "{text}"
    );
    assert_eq!(
        std::fs::read_to_string(home.join(".claude.json")).unwrap(),
        "[1]"
    );
    assert!(!home.join(".claude").exists());
    let conflict = sandbox
        .fold(&["setup", "--claude", "--remove"], Path::new(""), None)
        .unwrap();
    assert_eq!(conflict.status.code(), Some(2));
}
