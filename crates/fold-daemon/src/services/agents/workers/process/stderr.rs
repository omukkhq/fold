use std::sync::{Arc, Mutex};
use std::time::Duration;

use fold_core::turn_errors;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::watch;

use crate::support::lock;

/// The most stderr bytes kept, well above what [`turn_errors::STDERR_LINES`] lines of an error take.
const KEPT_BYTES: usize = 16 * 1024;

/// The end of a worker process's stderr. The reader drains the pipe to its end, so a full pipe never blocks the process, and keeps only the last [`KEPT_BYTES`]: what Claude Code writes before it exits with an error says why. Its text goes onto the worker's row only, never into a log.
#[derive(Clone)]
pub(super) struct StderrTail {
    bytes: Arc<Mutex<Vec<u8>>>,
    closed: watch::Receiver<bool>,
}

impl StderrTail {
    /// Starts the reader of `stderr` in a task of its own.
    pub(super) fn read(stderr: impl AsyncRead + Unpin + Send + 'static) -> Self {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (close, closed) = watch::channel(false);
        tokio::spawn(keep_tail(stderr, Arc::clone(&bytes), close));
        Self { bytes, closed }
    }

    /// The kept lines as [`turn_errors::stderr_text`] cleans them, once the pipe closed or `wait` passed: a child of the process can keep the pipe open after the process ended. `None` when nothing is left.
    pub(super) async fn text(&self, wait: Duration) -> Option<String> {
        let mut closed = self.closed.clone();
        let _ = tokio::time::timeout(wait, closed.wait_for(|closed| *closed)).await;
        let bytes = lock(&self.bytes).clone();
        turn_errors::stderr_text(&bytes)
    }
}

async fn keep_tail(
    mut stderr: impl AsyncRead + Unpin,
    bytes: Arc<Mutex<Vec<u8>>>,
    close: watch::Sender<bool>,
) {
    let mut buffer = [0_u8; 4096];
    while let Ok(read) = stderr.read(&mut buffer).await {
        if read == 0 {
            break;
        }
        let mut kept = lock(&bytes);
        kept.extend_from_slice(&buffer[..read]);
        if kept.len() > 2 * KEPT_BYTES {
            let cut = kept.len() - KEPT_BYTES;
            kept.drain(..cut);
        }
    }
    let _ = close.send(true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_tail_keeps_the_last_lines_of_the_pipe() {
        let mut text = String::new();
        for n in 0..5000 {
            text.push_str(&format!("line {n}\n"));
        }
        let tail = StderrTail::read(std::io::Cursor::new(text.into_bytes()));
        let kept = tail.text(Duration::from_secs(5)).await.unwrap();
        assert_eq!(kept.lines().count(), turn_errors::STDERR_LINES);
        assert!(kept.ends_with("line 4999"));
        let empty = StderrTail::read(std::io::Cursor::new(Vec::new()));
        assert_eq!(empty.text(Duration::from_secs(5)).await, None);
    }

    #[tokio::test]
    async fn an_open_pipe_gives_what_it_wrote_once_the_wait_passed() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let tail = StderrTail::read(reader);
        tokio::io::AsyncWriteExt::write_all(&mut writer, b"Error: no access\n")
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::flush(&mut writer).await.unwrap();
        let mut seen = None;
        for _ in 0..50 {
            seen = tail.text(Duration::from_millis(20)).await;
            if seen.is_some() {
                break;
            }
        }
        assert_eq!(seen.as_deref(), Some("Error: no access"));
        drop(writer);
    }
}
