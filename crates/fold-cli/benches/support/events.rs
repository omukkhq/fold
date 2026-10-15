use std::time::Duration;

use http_body_util::{BodyExt, Empty};
use hyper::body::{Bytes, Incoming};
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use tokio::net::TcpStream;

use super::{BenchResult, fail};

/// The server-sent event stream of a project, read over HTTP.
pub struct EventStream {
    response: Response<Incoming>,
    pending: String,
}

impl EventStream {
    pub async fn open(port: u16, slug: &str) -> BenchResult<Self> {
        let stream = TcpStream::connect(("127.0.0.1", port)).await?;
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
        tokio::spawn(connection);
        let request = Request::get(format!("/api/v1/projects/{slug}/events"))
            .header("host", format!("127.0.0.1:{port}"))
            .header("accept", "text/event-stream")
            .body(Empty::<Bytes>::new())?;
        let response = sender.send_request(request).await?;
        if !response.status().is_success() {
            return fail(format!("the event stream answered {}", response.status()));
        }
        Ok(Self {
            response,
            pending: String::new(),
        })
    }

    /// The next frame as its event name and parsed `data`.
    pub async fn next_frame(&mut self) -> BenchResult<(String, Value)> {
        loop {
            if let Some(end) = self.pending.find("\n\n") {
                let frame: String = self.pending.drain(..end + 2).collect();
                if let Some(parsed) = parse_frame(&frame)? {
                    return Ok(parsed);
                }
                continue;
            }
            match self.response.body_mut().frame().await {
                Some(frame) => {
                    if let Ok(data) = frame?.into_data() {
                        self.pending
                            .push_str(&String::from_utf8_lossy(&data).replace("\r\n", "\n"));
                    }
                }
                None => return fail("the event stream ended"),
            }
        }
    }

    /// Reads frames until `matches` accepts one, or fails after `timeout`.
    pub async fn wait_for(
        &mut self,
        timeout: Duration,
        mut matches: impl FnMut(&str, &Value) -> bool,
    ) -> BenchResult<Value> {
        let wait = async {
            loop {
                let (name, data) = self.next_frame().await?;
                if matches(&name, &data) {
                    return Ok(data);
                }
            }
        };
        match tokio::time::timeout(timeout, wait).await {
            Ok(found) => found,
            Err(_) => fail(format!("no matching event within {timeout:?}")),
        }
    }
}

/// `None` for a comment-only frame such as a keep-alive.
fn parse_frame(frame: &str) -> BenchResult<Option<(String, Value)>> {
    let mut name = None;
    let mut data = String::new();
    for line in frame.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            name = Some(value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push_str(value.trim_start());
        }
    }
    match name {
        Some(name) => Ok(Some((name, serde_json::from_str(&data)?))),
        None => Ok(None),
    }
}
