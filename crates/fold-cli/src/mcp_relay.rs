use std::collections::HashSet;
use std::io;
use std::time::Duration;

use fold_core::{FoldError, codes};
use fold_daemon::mcp::error::{ErrorBody, tool_error};
use serde::Deserialize;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::time::Instant;

use crate::daemon_client::{self, ClientConfig, HandshakeError, HelloLine};

/// The halves of a daemon connection after its handshake.
pub type Connection = (BufReader<OwnedReadHalf>, OwnedWriteHalf);

/// How long the daemon of the replayed `initialize` may take to answer it.
const REPLAY_TIMEOUT: Duration = Duration::from_secs(10);

/// The JSON-RPC code of a request answered while no daemon serves the session: the start of the range JSON-RPC leaves to servers.
const RESTARTING_RPC_CODE: i64 = -32000;

#[derive(Debug, Error)]
pub enum RelayError {
    #[error("the daemon closed the session; start a new session to reconnect")]
    DaemonClosed,
    #[error(transparent)]
    Refused(HandshakeError),
    #[error("the connection to the daemon failed: {0}")]
    Stdio(io::Error),
}

/// The waits between reconnect attempts after the daemon closed the session, and when the adapter gives up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconnectPolicy {
    pub first_delay: Duration,
    pub max_delay: Duration,
    pub give_up_after: Duration,
}

impl Default for ReconnectPolicy {
    /// A restart takes seconds, an upgrade of the binary up to a minute or two; five minutes covers both and still ends a session whose daemon was stopped for good.
    fn default() -> Self {
        Self {
            first_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(5),
            give_up_after: Duration::from_secs(300),
        }
    }
}

impl ReconnectPolicy {
    /// The wait before attempt `attempt`, counted from 0, of a gap that started `elapsed` ago, or `None` when the adapter gives up. The wait doubles up to `max_delay` and never ends past `give_up_after`, so the last attempt comes at the limit.
    pub fn next_delay(&self, attempt: u32, elapsed: Duration) -> Option<Duration> {
        let left = self.give_up_after.checked_sub(elapsed)?;
        if left.is_zero() {
            return None;
        }
        let delay = self
            .first_delay
            .saturating_mul(1_u32 << attempt.min(16))
            .min(self.max_delay);
        Some(delay.min(left))
    }
}

/// What the relay needs to know of one JSON-RPC line.
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    Request { id: Value, method: String },
    Notification { method: String },
    Response { id: Value },
    Other,
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    id: Option<Value>,
    #[serde(default)]
    method: Option<String>,
}

/// Reads only `id` and `method`; a line that is not a JSON-RPC object is [`Line::Other`] and passes through as it is.
pub fn classify(line: &[u8]) -> Line {
    match serde_json::from_slice::<Envelope>(line) {
        Ok(Envelope {
            id: Some(id),
            method: Some(method),
        }) => Line::Request { id, method },
        Ok(Envelope {
            id: None,
            method: Some(method),
        }) => Line::Notification { method },
        Ok(Envelope {
            id: Some(id),
            method: None,
        }) => Line::Response { id },
        _ => Line::Other,
    }
}

pub fn restarting_error() -> FoldError {
    FoldError::new(
        codes::DAEMON_RESTARTING,
        "the fold daemon is not reachable now; fld mcp reconnects to it by itself",
    )
    .with_hint(
        "call the tool again in a few seconds; a call that was running when the daemon stopped may have finished, so read its result before you repeat a write",
    )
}

/// The answer to a request that no daemon can serve, as one line. A `tools/call` gets a tool result with the error, as the daemon's own refusals do, because Claude Code shows that to the agent; `ping` gets its empty result; any other request gets a JSON-RPC error that carries the same error body.
pub fn restarting_reply(id: &Value, method: &str) -> Vec<u8> {
    let error = restarting_error();
    let reply = match method {
        "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
        "tools/call" => {
            let result = serde_json::to_value(tool_error(&error)).unwrap_or(Value::Null);
            json!({"jsonrpc": "2.0", "id": id, "result": result})
        }
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": RESTARTING_RPC_CODE,
                "message": error.message(),
                "data": ErrorBody::from(&error),
            },
        }),
    };
    let mut line = reply.to_string().into_bytes();
    line.push(b'\n');
    line
}

/// What the relay keeps of the client's session: the lines that open MCP on a new daemon connection, and the requests the daemon has not answered yet.
#[derive(Debug, Default)]
pub struct SessionLines {
    initialize: Option<(Value, Vec<u8>)>,
    initialized: Option<Vec<u8>>,
    pending: HashSet<String>,
    methods: Vec<(String, Value, String)>,
}

impl SessionLines {
    /// Notes a line from the client before it goes to the daemon.
    pub fn from_client(&mut self, line: &[u8]) {
        match classify(line) {
            Line::Request { id, method } => {
                if method == "initialize" {
                    self.initialize = Some((id.clone(), line.to_vec()));
                }
                let key = id.to_string();
                if self.pending.insert(key.clone()) {
                    self.methods.push((key, id, method));
                }
            }
            Line::Notification { method } if method == "notifications/initialized" => {
                self.initialized = Some(line.to_vec());
            }
            _ => {}
        }
    }

    /// Notes a line from the daemon before it goes to the client.
    pub fn from_daemon(&mut self, line: &[u8]) {
        if let Line::Response { id } = classify(line) {
            let key = id.to_string();
            if self.pending.remove(&key) {
                self.methods.retain(|(pending, _, _)| *pending != key);
            }
        }
    }

    /// The answers to every request the closed connection left without one, in the order the client sent them.
    pub fn abandon_pending(&mut self) -> Vec<Vec<u8>> {
        self.pending.clear();
        std::mem::take(&mut self.methods)
            .into_iter()
            .map(|(_, id, method)| restarting_reply(&id, &method))
            .collect()
    }
}

/// Carries the MCP session between the client's stdio and the daemon, and moves it to a new daemon connection when the daemon closes this one. `hello` is the line of the first connect; every reconnect sends its [`HelloLine::replay`]. `reconnect` must never start a daemon: a daemon the user stopped stays stopped, and the adapter gives up as `policy` says. Ends cleanly when the client closes its input.
pub async fn relay<I, O>(
    stdin: I,
    mut stdout: O,
    connection: Connection,
    reconnect: &ClientConfig,
    hello: &HelloLine,
    policy: &ReconnectPolicy,
) -> Result<(), RelayError>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
{
    let mut input = BufReader::new(stdin);
    let mut input_line = Vec::new();
    let mut session = SessionLines::default();
    let mut connection = connection;
    let replay = hello.replay();
    loop {
        if serve(
            &mut input,
            &mut input_line,
            &mut stdout,
            connection,
            &mut session,
        )
        .await?
            == Served::ClientClosed
        {
            return Ok(());
        }
        for reply in session.abandon_pending() {
            write_line(&mut stdout, &reply).await?;
        }
        match bridge_gap(
            &mut input,
            &mut input_line,
            &mut stdout,
            &mut session,
            reconnect,
            &replay,
            policy,
        )
        .await?
        {
            Some(next) => connection = next,
            None => return Ok(()),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Served {
    ClientClosed,
    DaemonClosed,
}

/// Copies whole lines both ways until one side closes. `read_until` keeps a partial line in its buffer when the other branch wins, so no byte is lost; a partial line of a daemon that went away is dropped, never written to the client.
async fn serve<I, O>(
    input: &mut BufReader<I>,
    input_line: &mut Vec<u8>,
    stdout: &mut O,
    connection: Connection,
    session: &mut SessionLines,
) -> Result<Served, RelayError>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
{
    let (mut read, mut write) = connection;
    let mut daemon_line = Vec::new();
    loop {
        tokio::select! {
            read_input = input.read_until(b'\n', input_line) => {
                if read_input.map_err(RelayError::Stdio)? == 0 {
                    return Ok(Served::ClientClosed);
                }
                let line = std::mem::take(input_line);
                session.from_client(&line);
                if write.write_all(&line).await.is_err() {
                    return Ok(Served::DaemonClosed);
                }
            }
            read_daemon = read.read_until(b'\n', &mut daemon_line) => {
                match read_daemon {
                    Ok(read) if read > 0 && daemon_line.ends_with(b"\n") => {
                        session.from_daemon(&daemon_line);
                        write_line(stdout, &daemon_line).await?;
                        daemon_line.clear();
                    }
                    _ => return Ok(Served::DaemonClosed),
                }
            }
        }
    }
}

/// Answers the client while no daemon serves it and tries to reconnect on the policy's schedule: a new connection, `None` when the client closed its input, or the error that ends the adapter.
async fn bridge_gap<I, O>(
    input: &mut BufReader<I>,
    input_line: &mut Vec<u8>,
    stdout: &mut O,
    session: &mut SessionLines,
    config: &ClientConfig,
    hello: &HelloLine,
    policy: &ReconnectPolicy,
) -> Result<Option<Connection>, RelayError>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
{
    let started = Instant::now();
    let mut attempt = 0_u32;
    loop {
        let Some(delay) = policy.next_delay(attempt, started.elapsed()) else {
            return Err(RelayError::DaemonClosed);
        };
        let wait = tokio::time::sleep(delay);
        tokio::pin!(wait);
        loop {
            tokio::select! {
                () = &mut wait => break,
                read_input = input.read_until(b'\n', input_line) => {
                    if read_input.map_err(RelayError::Stdio)? == 0 {
                        return Ok(None);
                    }
                    let line = std::mem::take(input_line);
                    if let Line::Request { id, method } = classify(&line) {
                        write_line(stdout, &restarting_reply(&id, &method)).await?;
                    }
                }
            }
        }
        attempt = attempt.saturating_add(1);
        match reopen(config, hello, session).await {
            Reopened::Connected(connection) => return Ok(Some(connection)),
            Reopened::Retry => {}
            Reopened::Refused(error) => return Err(RelayError::Refused(error)),
        }
    }
}

enum Reopened {
    Connected(Connection),
    Retry,
    Refused(HandshakeError),
}

/// Connects without starting a daemon, repeats the handshake with the replay line, so the daemon binds the same agent and never takes over a main session on it, and replays the client's `initialize` and `notifications/initialized`, which the new daemon session needs before any other request. The daemon's answer to the replayed `initialize` is the client's no longer, so it is dropped. A refused handshake is final: the daemon answered and will answer the same again.
async fn reopen(config: &ClientConfig, hello: &HelloLine, session: &SessionLines) -> Reopened {
    let Ok(stream) = daemon_client::connect(config).await else {
        return Reopened::Retry;
    };
    let (mut read, mut write) = match daemon_client::handshake(stream, hello).await {
        Ok(halves) => halves,
        Err(error @ HandshakeError::Refused { .. }) => return Reopened::Refused(error),
        Err(_) => return Reopened::Retry,
    };
    if let Some((id, line)) = &session.initialize {
        if write.write_all(line).await.is_err() {
            return Reopened::Retry;
        }
        let answered = tokio::time::timeout(REPLAY_TIMEOUT, async {
            let mut answer = Vec::new();
            loop {
                answer.clear();
                match read.read_until(b'\n', &mut answer).await {
                    Ok(read) if read > 0 && answer.ends_with(b"\n") => {
                        if classify(&answer) == (Line::Response { id: id.clone() }) {
                            return true;
                        }
                    }
                    _ => return false,
                }
            }
        })
        .await;
        if answered != Ok(true) {
            return Reopened::Retry;
        }
    }
    if let Some(line) = &session.initialized
        && write.write_all(line).await.is_err()
    {
        return Reopened::Retry;
    }
    Reopened::Connected((read, write))
}

async fn write_line<O: AsyncWrite + Unpin>(stdout: &mut O, line: &[u8]) -> Result<(), RelayError> {
    stdout.write_all(line).await.map_err(RelayError::Stdio)?;
    stdout.flush().await.map_err(RelayError::Stdio)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> ReconnectPolicy {
        ReconnectPolicy {
            first_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(5),
            give_up_after: Duration::from_secs(300),
        }
    }

    #[test]
    fn the_wait_doubles_up_to_the_cap() {
        let policy = policy();
        let waits: Vec<_> = (0..8)
            .map(|attempt| policy.next_delay(attempt, Duration::ZERO).unwrap())
            .collect();
        assert_eq!(
            waits,
            [100, 200, 400, 800, 1600, 3200, 5000, 5000].map(Duration::from_millis)
        );
        assert_eq!(
            policy.next_delay(u32::MAX, Duration::ZERO),
            Some(Duration::from_secs(5))
        );
    }

    #[test]
    fn the_adapter_gives_up_at_the_limit_after_a_last_attempt() {
        let policy = policy();
        assert_eq!(
            policy.next_delay(20, Duration::from_millis(298_500)),
            Some(Duration::from_millis(1_500)),
            "the last wait ends at the limit"
        );
        assert_eq!(policy.next_delay(21, Duration::from_secs(300)), None);
        assert_eq!(policy.next_delay(0, Duration::from_secs(301)), None);
    }

    #[test]
    fn lines_are_requests_notifications_responses_or_other() {
        assert_eq!(
            classify(br#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{}}"#),
            Line::Request {
                id: json!(3),
                method: "tools/call".into()
            }
        );
        assert_eq!(
            classify(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            Line::Notification {
                method: "notifications/initialized".into()
            }
        );
        assert_eq!(
            classify(br#"{"jsonrpc":"2.0","id":"a","result":{}}"#),
            Line::Response { id: json!("a") }
        );
        assert_eq!(classify(b"not json\n"), Line::Other);
        assert_eq!(classify(b"[1,2]\n"), Line::Other);
    }

    #[test]
    fn a_tool_call_in_the_gap_gets_a_tool_error_with_the_hint() {
        let reply: Value =
            serde_json::from_slice(&restarting_reply(&json!(7), "tools/call")).unwrap();
        assert_eq!(reply["id"], 7);
        assert_eq!(reply["result"]["isError"], true);
        let text = reply["result"]["content"][0]["text"].as_str().unwrap();
        let body: Value = serde_json::from_str(text).unwrap();
        assert_eq!(body["error"]["code"], "daemon_restarting");
        assert!(
            body["error"]["hint"]
                .as_str()
                .unwrap()
                .starts_with("call the tool again")
        );
    }

    #[test]
    fn other_requests_in_the_gap_get_a_json_rpc_error_and_ping_its_result() {
        let reply: Value =
            serde_json::from_slice(&restarting_reply(&json!("x"), "tools/list")).unwrap();
        assert_eq!(reply["id"], "x");
        assert_eq!(reply["error"]["code"], RESTARTING_RPC_CODE);
        assert_eq!(reply["error"]["data"]["code"], "daemon_restarting");
        let ping: Value = serde_json::from_slice(&restarting_reply(&json!(1), "ping")).unwrap();
        assert_eq!(ping["result"], json!({}));
        assert!(restarting_reply(&json!(1), "ping").ends_with(b"\n"));
    }

    #[test]
    fn the_session_keeps_its_opening_lines_and_answers_what_is_pending() {
        let mut session = SessionLines::default();
        let init = br#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}
"#;
        session.from_client(init);
        session.from_daemon(
            br#"{"jsonrpc":"2.0","id":0,"result":{}}
"#,
        );
        session.from_client(
            br#"{"jsonrpc":"2.0","method":"notifications/initialized"}
"#,
        );
        session.from_client(
            br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}
"#,
        );
        session.from_client(
            br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{}}
"#,
        );
        session.from_daemon(
            br#"{"jsonrpc":"2.0","id":1,"result":{}}
"#,
        );
        assert_eq!(session.initialize, Some((json!(0), init.to_vec())));
        assert!(session.initialized.is_some());
        let replies = session.abandon_pending();
        assert_eq!(replies.len(), 1);
        let reply: Value = serde_json::from_slice(&replies[0]).unwrap();
        assert_eq!(reply["id"], 2);
        assert_eq!(reply["result"]["isError"], true);
        assert!(session.abandon_pending().is_empty());
    }
}
