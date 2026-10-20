use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{CONNECT_TIMEOUT, Check, Registered};

const NAME: &str = "session push";

/// Whether every connected agent of the project gets fold's notices, as the running daemon knows it. `socket` is the daemon's socket, `slug` the project's registered slug.
pub(super) fn push_check(socket: Option<&Path>, slug: Option<&str>) -> Check {
    let Some(slug) = slug else {
        return Check::ok(
            NAME,
            "the project is not registered, so no agent is connected",
        );
    };
    push_verdict(
        socket
            .and_then(|socket| listed_agents(socket, slug))
            .as_ref(),
    )
}

/// The slug of the registered project whose root is `root`.
pub(super) fn slug_of(projects: &[Registered], root: &Path) -> Option<String> {
    let canonical =
        |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let root: PathBuf = canonical(root);
    projects
        .iter()
        .find(|(_, registered)| canonical(registered) == root)
        .map(|(slug, _)| slug.clone())
}

/// `agents` is the daemon's agent list, `None` when no daemon answered. An agent with a connected session and no way for a notice to reach it sees a message only when it reads its inbox.
pub(super) fn push_verdict(agents: Option<&Value>) -> Check {
    let Some(agents) = agents else {
        return Check::ok(NAME, "the daemon does not answer, so no agent is connected");
    };
    let missing: Vec<&str> = agents["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|agent| agent["sessions"].as_u64().is_some_and(|count| count > 0))
        .filter(|agent| matches!(agent["notices"].as_str(), Some("unreachable" | "failing")))
        .filter_map(|agent| agent["name"].as_str())
        .collect();
    if missing.is_empty() {
        return Check::ok(NAME, "every connected agent gets fold's notices");
    }
    Check::warn(
        NAME,
        format!(
            "fold cannot post notices into the session of {}; it sees new messages only when it calls get_inbox",
            missing.join(", ")
        ),
        restart_fix(&missing),
    )
}

/// The main agent's session starts with `fld claude`; any other agent's session starts the way it started before.
fn restart_fix(agents: &[&str]) -> String {
    let others: Vec<&str> = agents
        .iter()
        .copied()
        .filter(|agent| *agent != "main")
        .collect();
    let mut steps = Vec::new();
    if others.len() < agents.len() {
        steps.push(
            "end the main agent's Claude Code session and start it again with `fld claude`"
                .to_owned(),
        );
    }
    if !others.is_empty() {
        steps.push(format!(
            "end the Claude Code session of {} and start it again in the same way",
            others.join(", ")
        ));
    }
    format!(
        "{}, so the start hook of each session gives fold its inbox",
        steps.join("; ")
    )
}

/// The daemon's answer to the project's agent list, read to its end over a connection of its own.
fn listed_agents(socket: &Path, slug: &str) -> Option<Value> {
    let mut stream = UnixStream::connect(socket).ok()?;
    let _ = stream.set_read_timeout(Some(CONNECT_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CONNECT_TIMEOUT));
    let request = format!(
        "GET /api/v1/projects/{slug}/agents HTTP/1.1\r\nHost: fold\r\nConnection: close\r\n\r\n"
    );
    let mut reply = Vec::new();
    stream.write_all(request.as_bytes()).ok()?;
    stream.read_to_end(&mut reply).ok()?;
    if !reply.starts_with(b"HTTP/1.1 200") {
        return None;
    }
    let start = reply.windows(4).position(|window| window == b"\r\n\r\n")? + 4;
    serde_json::from_slice(&reply[start..]).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::Status;
    use super::*;

    #[test]
    fn a_connected_agent_without_a_push_target_is_a_warning() {
        assert_eq!(push_verdict(None).status, Status::Ok);
        let agents = json!({"agents": [
            {"name": "main", "sessions": 1, "notices": "socket"},
            {"name": "web-lead", "sessions": 1, "notices": "stdin"},
            {"name": "gone", "sessions": 0, "notices": "unreachable"},
        ]});
        let check = push_verdict(Some(&agents));
        assert_eq!(check.status, Status::Ok, "{}", check.message);
        let agents = json!({"agents": [
            {"name": "main", "sessions": 1, "notices": "unreachable"},
            {"name": "docs", "sessions": 2, "notices": "failing"},
            {"name": "gone", "sessions": 0, "notices": "unreachable"},
        ]});
        let check = push_verdict(Some(&agents));
        assert_eq!(check.status, Status::Warn);
        assert!(check.message.contains("main, docs"), "{}", check.message);
        let fix = check.fix.unwrap();
        assert!(fix.contains("main agent's Claude Code session"), "{fix}");
        assert!(fix.contains("`fld claude`"), "{fix}");
        assert!(fix.contains("session of docs"), "{fix}");
        let agents = json!({"agents": [{"name": "docs", "sessions": 1, "notices": "failing"}]});
        let fix = push_verdict(Some(&agents)).fix.unwrap();
        assert!(!fix.contains("fld claude"), "{fix}");
        assert!(fix.contains("session of docs"), "{fix}");
    }

    /// Answers one request with `status` and `body`, and hands back the request line.
    fn daemon(
        socket: &Path,
        status: &'static str,
        body: String,
    ) -> std::thread::JoinHandle<String> {
        let listener = std::os::unix::net::UnixListener::bind(socket).unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0_u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            let reply = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(reply.as_bytes()).unwrap();
            String::from_utf8(request)
                .unwrap()
                .lines()
                .next()
                .unwrap()
                .to_owned()
        })
    }

    #[test]
    fn the_check_reads_the_agents_from_the_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("fold.sock");
        let body = json!({"agents": [{"name": "main", "sessions": 1, "notices": "unreachable"}]});
        let answered = daemon(&socket, "200 OK", body.to_string());
        let check = push_check(Some(&socket), Some("app"));
        assert_eq!(
            answered.join().unwrap(),
            "GET /api/v1/projects/app/agents HTTP/1.1"
        );
        assert_eq!(check.status, Status::Warn);
        assert!(check.message.contains("main"), "{}", check.message);

        std::fs::remove_file(&socket).unwrap();
        let refused = daemon(&socket, "404 Not Found", "{}".to_owned());
        assert_eq!(push_check(Some(&socket), Some("app")).status, Status::Ok);
        refused.join().unwrap();
    }

    #[test]
    fn the_slug_is_the_registered_project_with_the_same_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir_all(&root).unwrap();
        let projects = vec![
            ("other".to_owned(), dir.path().join("other")),
            ("app".to_owned(), root.clone()),
        ];
        assert_eq!(
            slug_of(&projects, &root.join(".").join("..").join("app")),
            Some("app".to_owned())
        );
        assert_eq!(slug_of(&projects, dir.path()), None);
        assert_eq!(push_check(None, None).status, Status::Ok);
        assert_eq!(
            push_check(Some(&dir.path().join("no.sock")), Some("app")).status,
            Status::Ok
        );
    }
}
