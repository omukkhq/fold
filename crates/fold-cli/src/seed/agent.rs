use std::path::Path;

use fold_daemon::mcp::handshake::{Answer, Hello, MAX_LINE, PROTOCOL_VERSION};
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use super::SeedError;

/// What a refused tool call reports.
pub struct Refusal {
    pub code: String,
}

/// An MCP session over the daemon socket, opened with the same handshake as `fld mcp`.
pub struct Agent {
    name: &'static str,
    client: RunningService<RoleClient, ()>,
}

impl Agent {
    /// Connects as `name` from `root` with `tags` in the handshake line, as `fld mcp` sends `FOLD_AGENT_TAGS`, then registers the agent. Only a handshake gives the `main` tag.
    pub async fn connect(
        socket: &Path,
        root: &Path,
        name: &'static str,
        tags: &[&str],
    ) -> Result<Self, SeedError> {
        let stream = UnixStream::connect(socket)
            .await
            .map_err(|source| SeedError::Socket {
                path: socket.to_owned(),
                source,
            })?;
        let (read, mut write) = stream.into_split();
        let hello = Hello {
            fold: PROTOCOL_VERSION,
            cwd: root.to_owned(),
            pid: std::process::id(),
            agent: Some(name.to_owned()),
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        };
        let mut line = serde_json::to_vec(&hello).map_err(|_| SeedError::Handshake)?;
        line.push(b'\n');
        write
            .write_all(&line)
            .await
            .map_err(|_| SeedError::Handshake)?;
        let mut read = BufReader::new(read);
        let mut answer = Vec::new();
        (&mut read)
            .take(MAX_LINE as u64)
            .read_until(b'\n', &mut answer)
            .await
            .map_err(|_| SeedError::Handshake)?;
        let answer: Answer = serde_json::from_slice(&answer).map_err(|_| SeedError::Handshake)?;
        if !answer.ok {
            return Err(SeedError::Refused {
                code: answer.error.map(|error| error.code).unwrap_or_default(),
            });
        }
        let client = ().serve((read, write)).await.map_err(|_| SeedError::Handshake)?;
        let agent = Self { name, client };
        agent.call("register_agent", json!({"name": name})).await?;
        Ok(agent)
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The JSON of a successful call.
    pub async fn call(&self, tool: &'static str, arguments: Value) -> Result<Value, SeedError> {
        let (failed, body) = self.raw(tool, arguments).await?;
        if failed {
            return Err(SeedError::Tool {
                tool,
                code: error_code(&body),
            });
        }
        Ok(body)
    }

    /// A call that must fail: the error code, or an error when the call succeeds.
    pub async fn refused(
        &self,
        tool: &'static str,
        arguments: Value,
    ) -> Result<Refusal, SeedError> {
        let (failed, body) = self.raw(tool, arguments).await?;
        if failed {
            Ok(Refusal {
                code: error_code(&body),
            })
        } else {
            Err(SeedError::Unexpected { tool })
        }
    }

    async fn raw(&self, tool: &'static str, arguments: Value) -> Result<(bool, Value), SeedError> {
        let Value::Object(arguments) = arguments else {
            return Err(SeedError::Unexpected { tool });
        };
        let result = self
            .client
            .call_tool(CallToolRequestParams::new(tool).with_arguments(arguments))
            .await
            .map_err(|_| SeedError::Transport { tool })?;
        let text = result
            .content
            .first()
            .and_then(|block| block.as_text())
            .ok_or(SeedError::Unexpected { tool })?;
        let body = serde_json::from_str(&text.text).map_err(|_| SeedError::Unexpected { tool })?;
        Ok((result.is_error == Some(true), body))
    }

    /// Ends the session and waits for the daemon to see it close.
    pub async fn close(self) {
        let _ = self.client.cancel().await;
    }
}

fn error_code(body: &Value) -> String {
    body["error"]["code"]
        .as_str()
        .unwrap_or("unknown")
        .to_owned()
}
