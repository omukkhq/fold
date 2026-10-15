use std::io::{ErrorKind, Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use fold_core::time::Timestamp;
use serde::Serialize;

#[cfg(test)]
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(test)]
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(test)]
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Struct fields keep the documented key order on the wire, which a `serde_json::Value` map
/// does not promise.
#[cfg(test)]
#[derive(Serialize)]
struct AuthLine<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    token: &'a str,
}

#[cfg(test)]
#[derive(Serialize)]
struct NoticeLine<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    priority: &'a str,
    message: NoticeMessage<'a>,
}

#[cfg(test)]
#[derive(Serialize)]
struct NoticeMessage<'a> {
    role: &'a str,
    content: &'a str,
}

/// The probe runs inside the target session's own shell so the inbox token stays in that session.
/// Every printed frame is the redacted twin of the frame on the wire; the token itself is never
/// formatted, only its length and shape.
#[test]
#[ignore = "needs a live Claude Code session; run it from that session's own shell"]
fn inbox_probe() {
    let Ok(socket) = std::env::var("CLAUDE_CODE_MESSAGING_SOCKET") else {
        println!(
            "CLAUDE_CODE_MESSAGING_SOCKET is not set: nothing was sent. Run the probe from the Claude Code session's own shell"
        );
        return;
    };
    let token = match std::env::var("CLAUDE_CODE_MESSAGING_TOKEN") {
        Ok(token) => Some(token),
        Err(_) if std::env::var_os("FOLD_PROBE_ALLOW_NO_TOKEN").is_some() => {
            println!(
                "!! CLAUDE_CODE_MESSAGING_TOKEN is not set; FOLD_PROBE_ALLOW_NO_TOKEN is set, so the valid-token step is skipped"
            );
            None
        }
        Err(_) => {
            println!(
                "CLAUDE_CODE_MESSAGING_TOKEN is not set: nothing was sent. Run the probe from the Claude Code session's own shell, or set FOLD_PROBE_ALLOW_NO_TOKEN=1 to run the steps that need no token"
            );
            return;
        }
    };
    let label = std::env::var("FOLD_PROBE_LABEL").unwrap_or_else(|_| "unlabelled".to_owned());

    println!("== environment");
    println!("label: {label}");
    println!("socket: {socket}");
    match &token {
        Some(token) => println!(
            "token: {} (32 lowercase hex: {})",
            placeholder(token),
            is_lower_hex_32(token)
        ),
        None => println!("token: <absent>"),
    }
    let chain = ancestry(std::process::id());
    println!("probe pid: {}", std::process::id());
    println!("ancestry (ppid chain): {chain:?}");
    match socket_owner_pid(&socket) {
        Some(owner) => println!(
            "socket owner pid from the file name: {owner}; owner is an ancestor of the probe: {}",
            chain.contains(&owner)
        ),
        None => println!("socket file name is not <pid>.sock"),
    }

    println!("== socket file");
    describe_socket_file(Path::new(&socket));

    let steps = std::env::var("FOLD_PROBE_STEPS").ok();
    let runs = |step: &str| {
        steps
            .as_deref()
            .is_none_or(|list| list.split(',').any(|item| item.trim() == step))
    };
    println!(
        "steps: {}",
        steps
            .as_deref()
            .unwrap_or("all (FOLD_PROBE_STEPS is not set)")
    );

    let started = Timestamp::from_system_time(SystemTime::now()).expect("system time in range");
    if runs("valid-token")
        && let Some(token) = &token
    {
        println!("== step valid-token");
        post(
            &socket,
            Some(token),
            &notice(
                started,
                &label,
                "valid-token",
                "a test notice from the fold inbox spike. Reply with one short line to confirm you saw it; no other action is needed.",
            ),
        );
    }

    if runs("agent-form") {
        println!("== step agent-form");
        let from = std::env::var("FOLD_PROBE_FROM").ok();
        let name = std::env::var("FOLD_PROBE_NAME").unwrap_or_else(|_| "fold".to_owned());
        let mode = std::env::var("FOLD_PROBE_MODE").ok();
        let body = notice(
            started,
            &label,
            "agent-form",
            "a test notice from the fold inbox spike, sent in the cross-session envelope. Reply with one short line that names the sender you see; no other action is needed.",
        );
        let content = envelope(from.as_deref(), &name, mode.as_deref(), &body);
        post(&socket, token.as_deref(), &content);
    }

    if runs("wrong-token") {
        println!("== step wrong-token");
        let wrong = wrong_token(token.as_deref());
        post(
            &socket,
            Some(&wrong),
            &notice(
                started,
                &label,
                "wrong-token",
                "sent with a wrong token. If you can read this, the inbox accepts a wrong token. Reply with one short line; no other action is needed.",
            ),
        );
    }

    if runs("no-auth-line") {
        println!("== step no-auth-line");
        post(
            &socket,
            None,
            &notice(
                started,
                &label,
                "no-auth-line",
                "sent without an auth line. If you can read this, the inbox accepts a connection that does not authenticate. Reply with one short line; no other action is needed.",
            ),
        );
    }

    let scratch = tempfile::tempdir().expect("temporary directory");

    if runs("nonexistent-socket") {
        println!("== step nonexistent-socket");
        report_connect(&scratch.path().join("missing.sock"));
    }

    if runs("closed-socket") {
        println!("== step closed-socket (socket file left behind, no listener)");
        let closed = scratch.path().join("closed.sock");
        drop(UnixListener::bind(&closed).expect("bind a scratch socket"));
        report_connect(&closed);
    }

    println!("== done");
}

#[cfg(test)]
fn notice(at: Timestamp, label: &str, step: &str, text: &str) -> String {
    format!("[fold inbox probe] {at} label={label} step={step}: {text}")
}

/// The session reads the sender from this envelope only when the content round-trips exactly
/// through its own builder: attributes in the order `from`, `from-name`, `from-mode`, one space
/// apart, a newline after the opening tag and before the closing one. Anything else shows as an
/// anonymous peer message, so the probe refuses values the session would not round-trip.
#[cfg(test)]
fn envelope(from: Option<&str>, name: &str, mode: Option<&str>, body: &str) -> String {
    let mut attributes = Vec::new();
    if let Some(from) = from {
        assert!(
            !from.is_empty()
                && from
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "%:_/.\\-".contains(c)),
            "FOLD_PROBE_FROM may hold only A-Z a-z 0-9 % : _ / . \\ -"
        );
        attributes.push(format!("from=\"{from}\""));
    }
    assert!(
        !name.is_empty() && !name.chars().any(|c| c.is_control() || "\"<>".contains(c)),
        "FOLD_PROBE_NAME must be non-empty without quotes, angle brackets or control characters"
    );
    attributes.push(format!("from-name=\"{name}\""));
    if let Some(mode) = mode {
        assert!(
            mode == "bypass" || mode == "prompting",
            "FOLD_PROBE_MODE is bypass or prompting"
        );
        attributes.push(format!("from-mode=\"{mode}\""));
    }
    assert!(
        !body.contains("cross-session-message"),
        "the body must not name the envelope tag"
    );
    format!(
        "<cross-session-message {}>\n{body}\n</cross-session-message>",
        attributes.join(" ")
    )
}

#[cfg(test)]
fn placeholder(token: &str) -> String {
    format!("<token:len={}>", token.len())
}

#[cfg(test)]
fn is_lower_hex_32(token: &str) -> bool {
    token.len() == 32
        && token
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Same shape as a real token, so the inbox judges the value rather than the format.
#[cfg(test)]
fn wrong_token(real: Option<&str>) -> String {
    let zeros = "0".repeat(32);
    if real == Some(zeros.as_str()) {
        "f".repeat(32)
    } else {
        zeros
    }
}

#[cfg(test)]
fn socket_owner_pid(socket: &str) -> Option<u32> {
    Path::new(socket)
        .file_name()?
        .to_str()?
        .strip_suffix(".sock")?
        .parse()
        .ok()
}

#[cfg(test)]
fn ancestry(pid: u32) -> Vec<u32> {
    let mut chain = Vec::new();
    let mut current = pid;
    for _ in 0..12 {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{current}/stat")) else {
            break;
        };
        let Some(after_comm) = stat.rfind(')').map(|i| &stat[i + 2..]) else {
            break;
        };
        let Some(ppid) = after_comm
            .split(' ')
            .nth(1)
            .and_then(|field| field.parse::<u32>().ok())
        else {
            break;
        };
        if ppid == 0 {
            break;
        }
        chain.push(ppid);
        if ppid == 1 {
            break;
        }
        current = ppid;
    }
    chain
}

#[cfg(test)]
fn describe_socket_file(path: &Path) {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            println!("is a socket: {}", meta.file_type().is_socket());
            println!("mode: {:o}", meta.permissions().mode() & 0o7777);
        }
        Err(error) => println!("stat failed: {}", describe_error(&error)),
    }
    if let Some(dir) = path.parent() {
        match std::fs::metadata(dir) {
            Ok(meta) => println!(
                "directory {}: mode {:o}",
                dir.display(),
                meta.permissions().mode() & 0o7777
            ),
            Err(error) => println!("directory stat failed: {}", describe_error(&error)),
        }
    }
}

#[cfg(test)]
fn post(socket: &str, token: Option<&str>, content: &str) {
    let mut stream = match connect(Path::new(socket)) {
        Ok(stream) => {
            println!("connect: ok");
            stream
        }
        Err(error) => {
            println!("connect: {}", describe_error(&error));
            return;
        }
    };
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .expect("set the read timeout");
    stream
        .set_write_timeout(Some(WRITE_TIMEOUT))
        .expect("set the write timeout");

    if let Some(token) = token {
        let wire = frame(&AuthLine {
            kind: "auth",
            token,
        });
        let shown = frame(&AuthLine {
            kind: "auth",
            token: &placeholder(token),
        });
        send(&mut stream, "auth line", &wire, &shown);
    }
    let user = frame(&NoticeLine {
        kind: "user",
        priority: "next",
        message: NoticeMessage {
            role: "user",
            content,
        },
    });
    send(&mut stream, "notice line", &user, &user);

    match stream.shutdown(Shutdown::Write) {
        Ok(()) => println!("shutdown(write): ok"),
        Err(error) => println!("shutdown(write): {}", describe_error(&error)),
    }

    let started = Instant::now();
    let mut received = Vec::new();
    let mut buffer = [0_u8; 4096];
    let ending = loop {
        match stream.read(&mut buffer) {
            Ok(0) => break "EOF".to_owned(),
            Ok(n) => received.extend_from_slice(&buffer[..n]),
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                break format!("read timeout after {READ_TIMEOUT:?}");
            }
            Err(error) => break describe_error(&error),
        }
    };
    println!(
        "read: {} bytes {:?}, ended by {ending} after {} ms",
        received.len(),
        String::from_utf8_lossy(&received),
        started.elapsed().as_millis()
    );
}

#[cfg(test)]
fn frame(value: &impl Serialize) -> String {
    let mut line = serde_json::to_string(value).expect("serialize a frame");
    line.push('\n');
    line
}

#[cfg(test)]
fn send(stream: &mut UnixStream, what: &str, wire: &str, shown: &str) {
    match stream.write_all(wire.as_bytes()) {
        Ok(()) => println!("sent {what} ({} bytes): {shown:?}", wire.len()),
        Err(error) => println!(
            "sending {what} ({} bytes) failed: {}; frame: {shown:?}",
            wire.len(),
            describe_error(&error)
        ),
    }
}

#[cfg(test)]
/// A blocking connect to a Unix socket whose listen backlog is full waits for room, and std has
/// no connect timeout, so the connect runs on its own thread.
#[cfg(test)]
fn connect(path: &Path) -> std::io::Result<UnixStream> {
    let (sender, receiver) = mpsc::channel();
    let path: PathBuf = path.to_owned();
    std::thread::spawn(move || {
        let _ = sender.send(UnixStream::connect(path));
    });
    receiver.recv_timeout(CONNECT_TIMEOUT).unwrap_or_else(|_| {
        Err(std::io::Error::new(
            ErrorKind::TimedOut,
            format!("no connection within {CONNECT_TIMEOUT:?}"),
        ))
    })
}

#[cfg(test)]
fn report_connect(path: &Path) {
    match connect(path) {
        Ok(_) => println!("connect {}: ok", path.display()),
        Err(error) => println!("connect {}: {}", path.display(), describe_error(&error)),
    }
}

#[cfg(test)]
fn describe_error(error: &std::io::Error) -> String {
    format!(
        "error kind={:?} os={:?} message={error}",
        error.kind(),
        error.raw_os_error()
    )
}
