#![cfg(test)]

use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

use fold_store::testing::eventually;

use crate::common::{DaemonGuard, Host, daemons};

const WAIT: Duration = Duration::from_secs(20);

async fn bare(host: &Host, cwd: &Path) -> Output {
    let output = host
        .fold(&[], cwd)
        .stdin(Stdio::null())
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    output
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[tokio::test]
async fn a_bare_fld_without_a_daemon_says_so_and_starts_none() {
    let host = Host::new().await;
    let socket = host.default_socket();
    let registry = host.data_home().join("fold").join("registry.db");
    let before = std::fs::read(&registry).unwrap();
    let output = bare(&host, &host.root).await;
    assert_eq!(
        std::fs::read(&registry).unwrap(),
        before,
        "a bare fld never writes the registry"
    );
    assert_eq!(
        stdout(&output),
        format!(
            "fld {}\ndaemon: not running; fld claude, fld serve or fld setup starts it\nsocket: {}, where fld looks for the daemon\nservice: none; fld setup installs a service that starts the daemon at login\nproject: App (app) in {}\nRun fld --help for commands\n",
            env!("CARGO_PKG_VERSION"),
            socket.display(),
            host.root.display()
        )
    );
    assert!(daemons(&socket).is_empty(), "a bare fld starts no daemon");
    assert!(!socket.exists());
}

#[tokio::test]
async fn a_bare_fld_without_a_registry_names_no_project_and_creates_no_registry() {
    let host = Host::new().await;
    let fold_data = host.data_home().join("fold");
    std::fs::remove_dir_all(&fold_data).unwrap();
    let text = stdout(&bare(&host, &host.root).await);
    assert!(!text.contains("project:"), "{text}");
    assert!(text.ends_with("Run fld --help for commands\n"), "{text}");
    assert!(!fold_data.exists(), "a bare fld creates no registry");
}

#[tokio::test]
async fn a_bare_fld_names_the_port_the_running_daemon_got() {
    let host = Host::new().await;
    let socket = host.default_socket();
    let mut serve = host.fold(&["serve"], &host.root);
    serve
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = serve.spawn().unwrap();
    eventually(WAIT, async || {
        tokio::net::UnixStream::connect(&socket)
            .await
            .map(drop)
            .map_err(|error| error.kind())
    })
    .await;
    let guard = DaemonGuard(
        child
            .id()
            .map(|pid| i32::try_from(pid).unwrap())
            .into_iter()
            .collect(),
    );

    let outside = host.dir.path().join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    let text = stdout(&bare(&host, &outside).await);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 6, "{text}");
    assert_eq!(lines[0], format!("fld {}", env!("CARGO_PKG_VERSION")));
    let prefix = format!(
        "daemon: running fold {}, web interface http://127.0.0.1:",
        env!("CARGO_PKG_VERSION")
    );
    let port: u16 = lines[1]
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("{text}"))
        .parse()
        .unwrap();
    assert_ne!(
        port, 0,
        "the port the daemon got, not the 0 that daemon.toml asks for"
    );
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_ok(),
        "{text}"
    );
    assert_eq!(lines[2], format!("socket: {}", socket.display()));
    assert_eq!(
        lines[3],
        "service: started by hand; fld setup installs a service that starts the daemon at login"
    );
    assert_eq!(
        lines[4],
        "project: none; this folder is in no fold project, and fld init makes one"
    );
    assert_eq!(lines[5], "Run fld --help for commands");

    guard.stop().await;
    let _ = child.wait().await;
}
