use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Everything the daemon needs from the host. The daemon reads no environment variable; `fold-cli` builds this from flags, `daemon.toml` and the XDG directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonConfig {
    /// The HTTP listen address. Off loopback the daemon refuses to start until the session cookie layer exists.
    pub listen: SocketAddr,
    pub socket: PathBuf,
    /// Holds `registry.db`.
    pub data_dir: PathBuf,
    /// Holds `daemon.lock`; created with mode `0700`.
    pub runtime_dir: PathBuf,
    /// Serve the web files from this folder instead of the ones compiled into the binary.
    pub web_dir: Option<PathBuf>,
}

impl DaemonConfig {
    pub fn registry_path(&self) -> PathBuf {
        self.data_dir.join("registry.db")
    }

    pub fn lock_path(&self) -> PathBuf {
        self.runtime_dir.join(LOCK_FILE)
    }

    pub fn is_loopback(&self) -> bool {
        self.listen.ip().is_loopback()
    }

    pub fn socket_dir(&self) -> Option<&Path> {
        self.socket.parent()
    }
}

const LOCK_FILE: &str = "daemon.lock";

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
    fn loopback_addresses_are_recognised() {
        assert!(config("127.0.0.1:7420").is_loopback());
        assert!(config("[::1]:7420").is_loopback());
        assert!(!config("0.0.0.0:7420").is_loopback());
        assert!(!config("192.168.1.5:7420").is_loopback());
    }
}
