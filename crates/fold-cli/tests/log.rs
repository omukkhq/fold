#![cfg(test)]

use crate::common::{DaemonGuard, Host, daemons};

#[tokio::test]
async fn fld_log_lists_a_new_projects_record_and_genesis_newest_first() {
    let host = Host::new().await;
    let root = host.dir.path().join("site");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("index.html"), "<p>hi</p>\n").unwrap();
    let init = host
        .fold(&["init"], &root)
        .env("FOLD_HUMAN", "ana")
        .output()
        .await
        .unwrap();
    assert!(init.status.success(), "{init:?}");

    let log = host
        .fold(&["log"], &root)
        .env("FOLD_HUMAN", "ana")
        .env("TZ", "UTC")
        .output()
        .await
        .unwrap();
    let guard = DaemonGuard(daemons(&host.default_socket()));
    assert!(log.status.success(), "{log:?}");
    let text = String::from_utf8(log.stdout).unwrap();
    assert!(!text.contains('\u{1b}'), "{text}");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 8, "{text}");
    assert_eq!(
        lines[..3],
        [
            "r1  Changes of the project folder",
            "  ↳ head of main",
            "  ↳ recorded from the project folder by @ana",
        ]
    );
    // The time is the local time to the minute, here in UTC: `23 Oct 2026, 17:11`.
    let time = lines[3].strip_prefix("  ↳ ").unwrap();
    assert!(
        time.len() >= 17 && time.contains(", ") && !time.contains('Z'),
        "{text}"
    );
    assert_eq!(
        lines[4..7],
        [
            "",
            "r0  Genesis revision",
            "  ↳ the start of main's history"
        ]
    );

    let oneline = host
        .fold(&["log", "--oneline", "-n", "1"], &root)
        .env("FOLD_HUMAN", "ana")
        .output()
        .await
        .unwrap();
    assert!(oneline.status.success(), "{oneline:?}");
    assert_eq!(
        String::from_utf8(oneline.stdout).unwrap(),
        "r1  Changes of the project folder (head of main)\nolder revisions are not listed; fld log -n 2 lists more\n"
    );
    guard.stop().await;
}
