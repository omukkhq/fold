use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use clap::Args;
use fold_cli::daemon_client::Response;
use fold_core::{AgentName, FoldError, ProjectPaths, codes};
use fold_daemon::services::agents::claude::transcript_path;
use hyper::Method;

use super::remote::{RemoteError, project_paths, refusal, request_project};
use crate::{claude, output};

#[derive(Args, Debug)]
pub struct TranscriptArgs {
    /// The agent's name, such as main or web-lead
    agent: String,
    /// Print the transcript's last N lines instead of its path; fld reads them from the file itself, never through the daemon
    #[arg(long, value_name = "N")]
    tail: Option<usize>,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

/// Prints the path of the agent's Claude Code transcript, or its last lines. An agent has one current session: the one `fld claude` or `fld new` recorded for a main agent, which follows `/clear` and `/resume` in that session, else the one fold runs or resumes for a worker, which the daemon names. The main agent's path comes from this process's environment, where its `claude` runs; a worker's comes from the daemon, whose environment its workers get.
pub fn run_transcript(args: &TranscriptArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let agent = AgentName::parse(&args.agent).map_err(FoldError::from)?;
    let paths = project_paths(args.project.as_deref())?;
    let path = match claude::recorded(&paths, &agent) {
        Some(id) => main_transcript(&paths, &id)?,
        None => {
            let response = request_project(
                args.socket.as_deref(),
                args.project.as_deref(),
                Method::GET,
                "workers",
            )?;
            worker_transcript(&response, &agent)?
        }
    };
    if !path.is_file() {
        return Err(missing(&path).into());
    }
    match args.tail {
        None => output::line(out, &path.to_string_lossy())?,
        Some(lines) => {
            let mut file = std::fs::File::open(&path)?;
            let text = last_lines(&mut file, lines)?;
            if !text.is_empty() {
                let text = String::from_utf8_lossy(&text);
                output::line(out, text.strip_suffix('\n').unwrap_or(&text))?;
            }
        }
    }
    Ok(())
}

/// The transcript of the main agent's recorded session `id`, in the folder Claude Code uses with this process's environment.
fn main_transcript(paths: &ProjectPaths, id: &str) -> Result<PathBuf, FoldError> {
    let root = paths
        .root()
        .canonicalize()
        .unwrap_or_else(|_| paths.root().to_owned());
    claude::local_config_dir(&root)
        .and_then(|dir| transcript_path(&dir, &root, id))
        .ok_or_else(|| unknown_location(id))
}

/// The transcript the daemon names for the worker `agent` in its answer to the worker list.
fn worker_transcript(response: &Response, agent: &AgentName) -> Result<PathBuf, RemoteError> {
    refusal(response)?;
    let workers = response.body["workers"]
        .as_array()
        .ok_or(RemoteError::Answer)?;
    let Some(worker) = workers
        .iter()
        .find(|worker| worker["name"].as_str() == Some(agent.as_str()))
    else {
        return Err(FoldError::new(
            codes::AGENT_SESSION_NOT_FOUND,
            format!("fold knows no Claude Code session of {agent}"),
        )
        .with_hint("run `fld claude` to record the main agent's session; fold records a worker's session when it starts the worker")
        .into());
    };
    let id = worker["session_id"].as_str().ok_or(RemoteError::Answer)?;
    match worker["transcript"].as_str() {
        Some(path) => Ok(PathBuf::from(path)),
        None => Err(unknown_location(id).into()),
    }
}

fn unknown_location(id: &str) -> FoldError {
    FoldError::new(
        codes::TRANSCRIPT_NOT_FOUND,
        format!("fold cannot tell where Claude Code keeps the transcript of session {id}"),
    )
    .with_hint(format!(
        "run `find ~/.claude/projects -name {id}.jsonl`, or search the projects folder of your CLAUDE_CONFIG_DIR; CLAUDE_CODE_PROJECT_DIR_NAME, an empty CLAUDE_CONFIG_DIR or a very long project path hide it from fold"
    ))
}

fn missing(path: &Path) -> FoldError {
    FoldError::new(
        codes::TRANSCRIPT_NOT_FOUND,
        format!("{} does not exist", path.display()),
    )
    .with_hint("Claude Code writes the transcript after the first message of the session")
}

/// The last `lines` lines of `file`, read from its end in chunks, so a long transcript is not read whole. A final newline does not start another line.
fn last_lines(file: &mut (impl Read + Seek), lines: usize) -> io::Result<Vec<u8>> {
    const CHUNK: u64 = 64 * 1024;
    if lines == 0 {
        return Ok(Vec::new());
    }
    let mut end = file.seek(SeekFrom::End(0))?;
    let mut tail = Vec::new();
    loop {
        let start = end.saturating_sub(CHUNK);
        let size = usize::try_from(end - start).map_err(io::Error::other)?;
        let mut chunk = vec![0; size];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut chunk)?;
        chunk.extend_from_slice(&tail);
        tail = chunk;
        end = start;
        if start == 0 || line_start(&tail, lines).is_some() {
            break;
        }
    }
    let from = line_start(&tail, lines).unwrap_or(0);
    Ok(tail.split_off(from))
}

/// Where the last `lines` lines of `text` start, when `text` holds the newline before them.
fn line_start(text: &[u8], lines: usize) -> Option<usize> {
    let body = text.strip_suffix(b"\n").unwrap_or(text);
    body.iter()
        .enumerate()
        .rev()
        .filter(|(_, byte)| **byte == b'\n')
        .nth(lines - 1)
        .map(|(at, _)| at + 1)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use serde_json::json;

    use super::*;

    const ID: &str = "0f8e2c1a-5b7d-4c3e-9a1f-2d4b6c8e0a13";

    fn tail_of(text: &str, lines: usize) -> String {
        String::from_utf8(last_lines(&mut Cursor::new(text.as_bytes()), lines).unwrap()).unwrap()
    }

    #[test]
    fn the_tail_is_the_last_lines_with_or_without_a_final_newline() {
        assert_eq!(tail_of("a\nb\nc\n", 2), "b\nc\n");
        assert_eq!(tail_of("a\nb\nc", 2), "b\nc");
        assert_eq!(tail_of("a\nb\nc\n", 5), "a\nb\nc\n");
        assert_eq!(tail_of("a\nb\nc\n", 0), "");
        assert_eq!(tail_of("", 3), "");
    }

    #[test]
    fn a_tail_across_chunks_is_whole() {
        let line = format!("{}\n", "x".repeat(50_000));
        let text = format!("first\n{line}{line}last\n");
        assert_eq!(tail_of(&text, 3), format!("{line}{line}last\n"));
        assert_eq!(tail_of(&text, 4), text);
    }

    #[test]
    fn a_last_line_longer_than_a_chunk_is_whole() {
        let long = "y".repeat(300_000);
        assert_eq!(tail_of(&format!("a\n{long}\n"), 1), format!("{long}\n"));
        assert_eq!(tail_of(&format!("a\n{long}"), 2), format!("a\n{long}"));
    }

    fn answer(body: serde_json::Value) -> Response {
        Response { status: 200, body }
    }

    fn agent(name: &str) -> AgentName {
        AgentName::parse(name).unwrap()
    }

    #[test]
    fn a_worker_transcript_is_the_path_the_daemon_names() {
        let response = answer(json!({"workers": [
            {"name": "web-lead", "session_id": ID, "transcript": "/c/projects/-p/x.jsonl"},
            {"name": "api-lead", "session_id": ID, "transcript": null},
        ]}));
        assert_eq!(
            worker_transcript(&response, &agent("web-lead")).unwrap(),
            PathBuf::from("/c/projects/-p/x.jsonl")
        );
        let unknown = worker_transcript(&response, &agent("api-lead")).unwrap_err();
        assert_eq!(unknown.code(), Some(codes::TRANSCRIPT_NOT_FOUND));
        assert!(unknown.to_string().contains(ID), "{unknown}");
        let none = worker_transcript(&response, &agent("ghost")).unwrap_err();
        assert_eq!(none.code(), Some(codes::AGENT_SESSION_NOT_FOUND));
        assert!(none.hint().is_some());
    }

    #[test]
    fn a_refused_or_strange_answer_is_an_error() {
        let refused = Response {
            status: 404,
            body: json!({"error": {"code": "project_not_found", "message": "no project"}}),
        };
        assert_eq!(
            worker_transcript(&refused, &agent("web-lead"))
                .unwrap_err()
                .code(),
            Some("project_not_found")
        );
        assert!(matches!(
            worker_transcript(&answer(json!({})), &agent("web-lead")),
            Err(RemoteError::Answer)
        ));
        assert!(matches!(
            worker_transcript(
                &answer(json!({"workers": [{"name": "web-lead"}]})),
                &agent("web-lead")
            ),
            Err(RemoteError::Answer)
        ));
    }
}
