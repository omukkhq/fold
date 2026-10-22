use std::collections::VecDeque;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fold_daemon::daemon::AppState;
use fold_daemon::http::events::{EventEnvelope, LiveWorkspace};
use http_body_util::BodyExt;
use tower::ServiceExt as _;

use super::acceptance::{SLUG, WAIT};

/// How many received frames a failure message prints.
const KEPT: usize = 8;

/// One SSE frame: its `event` and `id` fields and its `data` read as the shared envelope.
#[derive(Debug)]
pub struct Frame {
    pub event: Option<String>,
    pub id: Option<String>,
    pub envelope: EventEnvelope,
}

impl Frame {
    /// The workspace of a `workspace.changed` frame; `None` for any other frame.
    pub fn workspace(&self) -> Option<&LiveWorkspace> {
        match &self.envelope {
            EventEnvelope::WorkspaceChanged(view) => Some(view),
            _ => None,
        }
    }

    /// The workspaces of a `snapshot` frame; `None` for any other frame.
    pub fn snapshot(&self) -> Option<&[LiveWorkspace]> {
        match &self.envelope {
            EventEnvelope::Snapshot(snapshot) => Some(&snapshot.workspaces),
            _ => None,
        }
    }
}

/// One open event stream of the project, driven through the router the way the browser's `EventSource` reads it: one request, then frame after frame on the same body. Every wait has a timeout, and a failed wait prints the last frames received.
pub struct Sse {
    body: Body,
    /// Bytes, so a chunk that ends inside a UTF-8 sequence waits for the rest of it.
    buffer: Vec<u8>,
    received: VecDeque<String>,
}

impl Sse {
    /// Opens the stream; panics unless the daemon answers 200 with an event stream.
    pub async fn open(state: &AppState, last_event_id: Option<&str>) -> Self {
        let mut request = Request::builder()
            .uri(format!("/api/v1/projects/{SLUG}/events"))
            .header("accept", "text/event-stream");
        if let Some(id) = last_event_id {
            request = request.header("last-event-id", id);
        }
        let response = crate::common::signed_in_router(state, "ana")
            .await
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"].to_str().unwrap(),
            "text/event-stream"
        );
        Self {
            body: response.into_body(),
            buffer: Vec::new(),
            received: VecDeque::new(),
        }
    }

    /// The next frame within [`WAIT`]; keep-alive comments are skipped.
    pub async fn next(&mut self) -> Frame {
        let frame = tokio::time::timeout(WAIT, self.read())
            .await
            .unwrap_or_else(|_| panic!("no frame arrived in time; {}", self.last_frames()))
            .unwrap_or_else(|| panic!("the stream ended; {}", self.last_frames()));
        if self.received.len() == KEPT {
            self.received.pop_front();
        }
        self.received.push_back(format!("{frame:?}"));
        frame
    }

    /// Reads frames until `pick` returns a value for one, and returns it. The frames before it are dropped. Panics with the last frames when [`WAIT`] passes first.
    pub async fn until<T>(&mut self, what: &str, mut pick: impl FnMut(&Frame) -> Option<T>) -> T {
        let found = tokio::time::timeout(WAIT, async {
            loop {
                let frame = self.next().await;
                if let Some(value) = pick(&frame) {
                    return value;
                }
            }
        })
        .await;
        found.unwrap_or_else(|_| {
            panic!(
                "no frame with {what} arrived in time; {}",
                self.last_frames()
            )
        })
    }

    fn last_frames(&self) -> String {
        format!("last frames: {:#?}", self.received)
    }

    async fn read(&mut self) -> Option<Frame> {
        loop {
            while let Some(end) = self.buffer.windows(2).position(|pair| pair == b"\n\n") {
                let block: Vec<u8> = self.buffer.drain(..end + 2).collect();
                if let Some(frame) = parse(std::str::from_utf8(&block).unwrap()) {
                    return Some(frame);
                }
            }
            match self.body.frame().await {
                None => return None,
                Some(chunk) => {
                    if let Ok(data) = chunk.unwrap().into_data() {
                        self.buffer.extend_from_slice(&data);
                    }
                }
            }
        }
    }
}

/// A frame of `event`, `id` and `data` lines; `None` for a block of comments only, such as a keep-alive.
fn parse(block: &str) -> Option<Frame> {
    let mut event = None;
    let mut id = None;
    let mut data = Vec::new();
    for line in block.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            event = Some(value.trim_start().to_owned());
        } else if let Some(value) = line.strip_prefix("id:") {
            id = Some(value.trim_start().to_owned());
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.trim_start().to_owned());
        }
    }
    if data.is_empty() {
        return None;
    }
    let envelope = serde_json::from_str(&data.join("\n"))
        .unwrap_or_else(|error| panic!("a frame is not an EventEnvelope: {error}"));
    Some(Frame {
        event,
        id,
        envelope,
    })
}
