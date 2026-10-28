use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Everything the daemon needs from the host. The daemon reads no environment variable; `fold-cli` builds this from flags, `daemon.toml` and the XDG directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonConfig {
    /// The HTTP listen address. On every address, loopback too, every guarded `/api` route needs the `fold_session` cookie of a live session.
    pub listen: SocketAddr,
    pub socket: PathBuf,
    /// Holds `registry.db`.
    pub data_dir: PathBuf,
    /// Holds `daemon.lock`; created with mode `0700`.
    pub runtime_dir: PathBuf,
    /// Serve the web files from this folder instead of the ones compiled into the binary.
    pub web_dir: Option<PathBuf>,
    /// How long a web sign-in lasts, counted from the sign-in. It is a host setting, the same for every project.
    pub session_lifetime: Duration,
}

impl DaemonConfig {
    pub fn registry_path(&self) -> PathBuf {
        self.data_dir.join("registry.db")
    }

    /// The kept inbox sockets of every project's agents, outside every project folder.
    pub fn push_dir(&self) -> PathBuf {
        self.data_dir.join("push")
    }

    /// The notice settings of the host and of every project, with their secrets, outside every project folder.
    pub fn notify_dir(&self) -> PathBuf {
        self.data_dir.join("notices")
    }

    pub fn lock_path(&self) -> PathBuf {
        self.runtime_dir.join(LOCK_FILE)
    }

    pub fn socket_dir(&self) -> Option<&Path> {
        self.socket.parent()
    }
}

/// The lock file in the runtime directory that the one running daemon holds.
pub const LOCK_FILE: &str = "daemon.lock";
/// The default socket in the runtime directory.
pub const SOCKET_FILE: &str = "fold.sock";

/// What the daemon that holds [`LOCK_FILE`] writes into it, so `fld restart` can stop that daemon and name the version it ran. The text is true only while the lock is held: a daemon that was killed leaves its lines behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockOwner {
    pub pid: u32,
    pub version: String,
}

impl LockOwner {
    /// This process, running this build.
    pub fn current() -> Self {
        Self {
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    /// The pid and the version, one per line.
    pub fn render(&self) -> String {
        format!("{}\n{}\n", self.pid, self.version)
    }

    /// `None` for text that is not two such lines, which is what a daemon older than the record leaves: an empty file.
    pub fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        let pid = lines.next()?.trim().parse().ok()?;
        let version = lines.next()?.trim();
        if version.is_empty() || lines.next().is_some() {
            return None;
        }
        Some(Self {
            pid,
            version: version.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(listen: &str) -> DaemonConfig {
        DaemonConfig {
            listen: listen.parse().unwrap(),
            socket: "/run/user/1/fold/fold.sock".into(),
            data_dir: "/data/fold".into(),
            runtime_dir: "/run/user/1/fold".into(),
            web_dir: None,
            session_lifetime: Duration::from_secs(60),
        }
    }

    #[test]
    fn derived_paths_hang_off_the_directories() {
        let config = config("127.0.0.1:7420");
        assert_eq!(config.registry_path(), Path::new("/data/fold/registry.db"));
        assert_eq!(
            config.lock_path(),
            Path::new("/run/user/1/fold/daemon.lock")
        );
        assert_eq!(config.socket_dir(), Some(Path::new("/run/user/1/fold")));
    }

    #[test]
    fn the_lock_owner_reads_back_what_it_writes_and_nothing_else() {
        let owner = LockOwner {
            pid: 4242,
            version: "0.3.0".to_owned(),
        };
        assert_eq!(owner.render(), "4242\n0.3.0\n");
        assert_eq!(LockOwner::parse(&owner.render()), Some(owner));
        for text in ["", "4242\n", "pid\n0.3.0\n", "4242\n\n", "1\n0.3.0\nmore\n"] {
            assert_eq!(LockOwner::parse(text), None, "{text:?}");
        }
        let current = LockOwner::current();
        assert_eq!(current.pid, std::process::id());
        assert_eq!(current.version, env!("CARGO_PKG_VERSION"));
    }
}
