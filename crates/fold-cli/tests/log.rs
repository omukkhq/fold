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
        .output()
        .await
        .unwrap();
    let guard = DaemonGuard(daemons(&host.default_socket()));
    assert!(log.status.success(), "{log:?}");
    let text = String::from_utf8(log.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4, "{text}");
    assert_eq!(
        lines[0],
        "r1 (head of main) record · changes of the project folder"
    );
    assert!(
        lines[1].starts_with("  @ana · ") && lines[1].ends_with('Z'),
        "{text}"
    );
    assert_eq!(lines[2], "r0 Genesis revision");
    assert!(lines[3].starts_with("  unknown · "), "{text}");

    let oneline = host
        .fold(&["log", "--oneline", "-n", "1"], &root)
        .env("FOLD_HUMAN", "ana")
        .output()
        .await
        .unwrap();
    assert!(oneline.status.success(), "{oneline:?}");
    assert_eq!(
        String::from_utf8(oneline.stdout).unwrap(),
        "r1 (head of main) record · changes of the project folder\nolder revisions are not listed; fld log -n 2 lists more\n"
    );
    guard.stop().await;
}
