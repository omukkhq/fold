use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

/// The longest stdout line the supervisor reads. A stream-json line carries one message, and a tool result with a large file or image can run to megabytes, so the cap sits well above that and only stops a process that writes without end.
pub(super) const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// The lines of a worker's stdout, each without its `\n` or `\r\n`. A line longer than [`MAX_LINE_BYTES`] or one that is not UTF-8 is read to its end and dropped, so the pipe keeps draining and a process never blocks on a full pipe.
pub(super) struct Lines<R> {
    reader: BufReader<R>,
    line: Vec<u8>,
    /// The current line passed the cap; its bytes are dropped until its end.
    skipping: bool,
}

impl<R: AsyncRead + Unpin> Lines<R> {
    pub(super) fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
            line: Vec::new(),
            skipping: false,
        }
    }

    /// The next line, or `None` at the end of the output. It keeps what it read in `self` between awaits, so a caller that drops the future in a `select!` loses no bytes.
    pub(super) async fn next_line(&mut self) -> std::io::Result<Option<String>> {
        loop {
            let available = self.reader.fill_buf().await?;
            if available.is_empty() {
                let last = std::mem::take(&mut self.line);
                let skipped = std::mem::take(&mut self.skipping);
                if last.is_empty() || skipped {
                    return Ok(None);
                }
                match decode(last) {
                    Some(line) => return Ok(Some(line)),
                    None => return Ok(None),
                }
            }
            let end = available.iter().position(|byte| *byte == b'\n');
            let chunk = &available[..end.unwrap_or(available.len())];
            if !self.skipping {
                if self.line.len() + chunk.len() > MAX_LINE_BYTES {
                    self.skipping = true;
                    self.line = Vec::new();
                } else {
                    self.line.extend_from_slice(chunk);
                }
            }
            let used = end.map_or(available.len(), |end| end + 1);
            self.reader.consume(used);
            if end.is_none() {
                continue;
            }
            let line = std::mem::take(&mut self.line);
            if std::mem::take(&mut self.skipping) {
                tracing::debug!("a worker wrote a line over the length cap");
                continue;
            }
            match decode(line) {
                Some(line) => return Ok(Some(line)),
                None => tracing::debug!("a worker wrote a line that is not UTF-8"),
            }
        }
    }
}

fn decode(mut line: Vec<u8>) -> Option<String> {
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn read_all(bytes: Vec<u8>) -> Vec<String> {
        let mut lines = Lines::new(std::io::Cursor::new(bytes));
        let mut read = Vec::new();
        while let Some(line) = lines.next_line().await.unwrap() {
            read.push(line);
        }
        read
    }

    #[tokio::test]
    async fn lines_lose_their_ends_and_the_last_needs_none() {
        assert_eq!(
            read_all(b"one\ntwo\r\nthree".to_vec()).await,
            ["one", "two", "three"]
        );
        assert!(read_all(Vec::new()).await.is_empty());
    }

    #[tokio::test]
    async fn a_line_that_is_not_utf8_is_dropped_and_reading_goes_on() {
        assert_eq!(
            read_all(b"before\n\xff\xfe\nafter\n".to_vec()).await,
            ["before", "after"]
        );
    }

    #[tokio::test]
    async fn a_line_over_the_cap_is_dropped_to_its_end() {
        let mut bytes = b"before\n".to_vec();
        bytes.extend(std::iter::repeat_n(b'a', MAX_LINE_BYTES + 1));
        bytes.extend_from_slice(b"\nafter\n");
        assert_eq!(read_all(bytes).await, ["before", "after"]);
        let mut exact = vec![b'b'; MAX_LINE_BYTES];
        exact.push(b'\n');
        assert_eq!(read_all(exact).await[0].len(), MAX_LINE_BYTES);
    }
}
