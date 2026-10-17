use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::serve::Dirs;

/// What the host offers. Tests build one by hand so every "not applicable" path runs on any host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Platform {
    pub(crate) linux: bool,
    pub(crate) macos: bool,
    pub(crate) file_modes: bool,
}

impl Platform {
    pub(crate) const fn current() -> Self {
        Self {
            linux: cfg!(target_os = "linux"),
            macos: cfg!(target_os = "macos"),
            file_modes: cfg!(unix),
        }
    }
}

/// The launchd label of fold's agent, which also names its file in `~/Library/LaunchAgents`.
pub(crate) const LAUNCH_AGENT_LABEL: &str = "dev.omukk.fold";

/// Reads the host files outside the project (`/proc`, `/run/systemd`, `/var/lib/systemd`) below a root; on macOS none of them exists. The root is `/` in production and a temporary directory of real files in tests.
#[derive(Debug, Clone)]
pub(crate) struct Probe {
    root: PathBuf,
}

impl Probe {
    pub(crate) fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self, absolute: &str) -> PathBuf {
        self.root.join(absolute.trim_start_matches('/'))
    }

    pub(crate) fn read(&self, absolute: &str) -> Option<String> {
        std::fs::read_to_string(self.path(absolute)).ok()
    }

    pub(crate) fn exists(&self, absolute: &str) -> bool {
        self.path(absolute).exists()
    }

    pub(crate) fn inotify_max_watches(&self) -> Option<u64> {
        self.read("/proc/sys/fs/inotify/max_user_watches")?
            .trim()
            .parse()
            .ok()
    }

    pub(crate) fn systemd_running(&self) -> bool {
        self.exists("/run/systemd/system")
    }

    pub(crate) fn linger_enabled(&self, user: &str) -> bool {
        self.exists(&format!("/var/lib/systemd/linger/{user}"))
    }
}

/// The environment the host commands read once, so the rules stay pure.
#[derive(Debug, Clone)]
pub(crate) struct Host {
    pub(crate) dirs: Dirs,
    pub(crate) user: Option<String>,
    /// The real user id, which names the launchd domain `gui/<uid>` of the user's login session.
    pub(crate) uid: u32,
    pub(crate) path: Option<OsString>,
    pub(crate) platform: Platform,
    pub(crate) probe: Probe,
    /// The variables that decide the human's fold name.
    pub(crate) names: crate::fold_name::Environment,
}

impl Host {
    pub(crate) fn from_env() -> Self {
        let user = ["USER", "LOGNAME"].iter().find_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.is_empty() && !value.contains('/'))
        });
        Self {
            dirs: Dirs::from_env(),
            user,
            uid: rustix::process::getuid().as_raw(),
            path: std::env::var_os("PATH"),
            platform: Platform::current(),
            probe: Probe::new("/"),
            names: crate::fold_name::Environment::from_env(),
        }
    }

    /// `systemctl` on Linux when it is on `PATH`.
    pub(crate) fn systemctl(&self) -> Option<PathBuf> {
        if self.platform.linux {
            on_path("systemctl", self.path.as_deref())
        } else {
            None
        }
    }

    /// `launchctl` on macOS when it is on `PATH`.
    pub(crate) fn launchctl(&self) -> Option<PathBuf> {
        if self.platform.macos {
            on_path("launchctl", self.path.as_deref())
        } else {
            None
        }
    }

    /// `~/Library/LaunchAgents/<label>.plist`, where launchd finds the agents it starts at login. launchd reads no XDG variable, so only the home directory decides.
    pub(crate) fn agent_path(&self) -> Option<PathBuf> {
        let home = self.dirs.home.as_ref()?;
        Some(
            home.join("Library/LaunchAgents")
                .join(format!("{LAUNCH_AGENT_LABEL}.plist")),
        )
    }

    /// `$XDG_CONFIG_HOME/systemd/user/fold.service`, or the same below `~/.config`.
    pub(crate) fn unit_path(&self) -> Option<PathBuf> {
        let base = self.dirs.config_base().ok()?;
        Some(base.join("systemd").join("user").join("fold.service"))
    }
}

/// The first executable file called `name` in a directory of `path`.
pub(crate) fn on_path(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    std::env::split_paths(path?)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::TempDir::new().unwrap()
    }

    #[test]
    fn the_probe_reads_files_below_its_root() {
        let root = tmp();
        let dir = root.path().join("proc/sys/fs/inotify");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("max_user_watches"), "8192\n").unwrap();
        std::fs::create_dir_all(root.path().join("run/systemd/system")).unwrap();
        std::fs::create_dir_all(root.path().join("var/lib/systemd/linger")).unwrap();
        std::fs::write(root.path().join("var/lib/systemd/linger/ana"), "").unwrap();
        let probe = Probe::new(root.path());
        assert_eq!(probe.inotify_max_watches(), Some(8192));
        assert!(probe.systemd_running());
        assert!(probe.linger_enabled("ana"));
        assert!(!probe.linger_enabled("bob"));
        assert_eq!(probe.read("/proc/missing"), None);
        std::fs::write(dir.join("max_user_watches"), "many").unwrap();
        assert_eq!(probe.inotify_max_watches(), None);
    }

    #[test]
    fn an_empty_root_offers_nothing() {
        let root = tmp();
        let probe = Probe::new(root.path());
        assert_eq!(probe.inotify_max_watches(), None);
        assert!(!probe.systemd_running());
        assert!(!probe.linger_enabled("ana"));
    }

    #[test]
    fn on_path_finds_executables_only() {
        let dir = tmp();
        let tool = dir.path().join("tool");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        let path = std::env::join_paths([dir.path()]).unwrap();
        assert_eq!(on_path("tool", Some(&path)), None);
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(on_path("tool", Some(&path)), Some(tool));
        assert_eq!(on_path("other", Some(&path)), None);
        assert_eq!(on_path("tool", None), None);
    }

    fn host(config_home: Option<&str>, home: Option<&str>, linux: bool) -> Host {
        Host {
            dirs: Dirs {
                home: home.map(Into::into),
                config_home: config_home.map(Into::into),
                ..Dirs::default()
            },
            user: None,
            uid: 501,
            path: None,
            platform: Platform {
                linux,
                macos: !linux,
                file_modes: true,
            },
            probe: Probe::new("/nonexistent-root"),
            names: crate::fold_name::Environment::default(),
        }
    }

    #[test]
    fn the_unit_path_follows_the_config_directory() {
        assert_eq!(
            host(Some("/cfg"), Some("/home/ana"), true).unit_path(),
            Some(PathBuf::from("/cfg/systemd/user/fold.service"))
        );
        assert_eq!(
            host(None, Some("/home/ana"), true).unit_path(),
            Some(PathBuf::from("/home/ana/.config/systemd/user/fold.service"))
        );
        assert_eq!(host(None, None, true).unit_path(), None);
    }

    #[test]
    fn systemctl_needs_linux_and_the_path() {
        let dir = tmp();
        let tool = dir.path().join("systemctl");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut linux = host(None, None, true);
        linux.path = Some(std::env::join_paths([dir.path()]).unwrap());
        assert_eq!(linux.systemctl(), Some(tool));
        let mut other = linux.clone();
        other.platform.linux = false;
        assert_eq!(other.systemctl(), None);
        assert_eq!(host(None, None, true).systemctl(), None);
    }

    #[test]
    fn launchctl_needs_macos_and_the_path_and_the_agent_lives_in_the_home_library() {
        let dir = tmp();
        let tool = dir.path().join("launchctl");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut macos = host(Some("/cfg"), Some("/Users/ana"), false);
        macos.path = Some(std::env::join_paths([dir.path()]).unwrap());
        assert_eq!(macos.launchctl(), Some(tool));
        assert_eq!(macos.systemctl(), None);
        assert_eq!(
            macos.agent_path(),
            Some(PathBuf::from(
                "/Users/ana/Library/LaunchAgents/dev.omukk.fold.plist"
            ))
        );
        let mut linux = macos.clone();
        linux.platform.macos = false;
        linux.platform.linux = true;
        assert_eq!(linux.launchctl(), None);
        assert_eq!(host(None, None, false).agent_path(), None);
    }

    #[test]
    fn the_current_platform_matches_the_build() {
        let platform = Platform::current();
        assert_eq!(platform.linux, cfg!(target_os = "linux"));
        assert_eq!(platform.macos, cfg!(target_os = "macos"));
        assert_eq!(Host::from_env().uid, rustix::process::getuid().as_raw());
    }
}
