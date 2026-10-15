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
        let mut command = Command::new(env!("CARGO_BIN_EXE_fold"));
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

    fn fake_systemctl(&self, exit: i32) -> io::Result<(PathBuf, PathBuf)> {
        let bin = self.path("bin");
        std::fs::create_dir_all(&bin)?;
        let log = self.path("systemctl.log");
        let script = bin.join("systemctl");
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
    let exe = env!("CARGO_BIN_EXE_fold");
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
    assert!(text.contains("hint: run `fold serve` under the host's own supervisor"));
    assert!(!sandbox.path("home/.config").exists());
}
