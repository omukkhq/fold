use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use fold_daemon::services::agents::Launcher;
use tempfile::TempDir;

/// A stand-in for `claude -p` in stream-json mode, so no test needs Claude Code. It records its arguments, environment, working directory and every stdin line under `FOLD_FAKE_CLAUDE_DIR`, answers `initialize` and `interrupt` as Claude Code does, replays the fixture `FOLD_FAKE_CLAUDE_TURN` for each user turn, and exits when stdin closes. With `FOLD_FAKE_CLAUDE_STUBBORN` set it ignores SIGINT and a closed stdin, so only SIGTERM or SIGKILL ends it.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
dir="$FOLD_FAKE_CLAUDE_DIR"
for arg in "$@"; do printf '%s\n' "$arg"; done > "$dir/$$.args"
env > "$dir/$$.env"
pwd > "$dir/$$.cwd"
if [ -n "$FOLD_FAKE_CLAUDE_STUBBORN" ]; then trap '' INT; fi
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$dir/$$.stdin"
  id=$(printf '%s' "$line" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
  case "$line" in
    *'"subtype":"initialize"'*)
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{}}}\n' "$id" ;;
    *'"subtype":"interrupt"'*)
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":{"still_queued":[]}}}\n' "$id"
      printf '%s\n' '{"type":"result","subtype":"error_during_execution","is_error":true,"num_turns":1,"session_id":"fake","total_cost_usd":0}' ;;
    *'"type":"user"'*)
      cat "$FOLD_FAKE_CLAUDE_TURN" ;;
  esac
done
if [ -n "$FOLD_FAKE_CLAUDE_STUBBORN" ]; then
  while :; do sleep 1; done
fi
"#;

/// The recorded fixture a user turn replays.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/claude")
        .join(format!("{name}.jsonl"))
}

pub struct FakeClaude {
    dir: TempDir,
    pub launcher: Launcher,
}

/// One run of the fake: one worker process.
#[derive(Debug, Clone)]
pub struct Run {
    pub pid: u32,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: String,
    pub stdin: Vec<String>,
}

impl Run {
    /// The value after `flag`.
    pub fn flag(&self, flag: &str) -> Option<&str> {
        let at = self.args.iter().position(|arg| arg == flag)?;
        self.args.get(at + 1).map(String::as_str)
    }

    pub fn has(&self, flag: &str) -> bool {
        self.args.iter().any(|arg| arg == flag)
    }

    pub fn agent(&self) -> &str {
        self.env.get("FOLD_AGENT").map_or("", String::as_str)
    }
}

impl FakeClaude {
    /// Each user turn replays the fixture `turn`, such as `result` (a whole turn, so the worker ends idle) or `init` (a turn that never ends, so it stays working).
    pub fn new(turn: &str) -> Self {
        let dir = TempDir::new().unwrap();
        let program = dir.path().join("claude");
        std::fs::write(&program, FAKE_CLAUDE).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runs = dir.path().join("runs");
        std::fs::create_dir(&runs).unwrap();
        let launcher = Launcher {
            claude: program,
            fold: Some(PathBuf::from("/usr/local/bin/fld")),
            env: vec![
                (
                    "FOLD_FAKE_CLAUDE_DIR".to_owned(),
                    runs.to_string_lossy().into_owned(),
                ),
                (
                    "FOLD_FAKE_CLAUDE_TURN".to_owned(),
                    fixture(turn).to_string_lossy().into_owned(),
                ),
            ],
        };
        Self { dir, launcher }
    }

    #[must_use]
    pub fn stubborn(mut self) -> Self {
        self.launcher
            .env
            .push(("FOLD_FAKE_CLAUDE_STUBBORN".to_owned(), "1".to_owned()));
        self
    }

    /// Every run so far, in pid order, as far as each has written its files.
    pub fn runs(&self) -> Vec<Run> {
        let dir = self.dir.path().join("runs");
        let mut pids: Vec<u32> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| {
                let name = entry.unwrap().file_name().to_string_lossy().into_owned();
                name.strip_suffix(".cwd")?.parse().ok()
            })
            .collect();
        pids.sort_unstable();
        pids.into_iter()
            .map(|pid| {
                let read = |ext: &str| {
                    std::fs::read_to_string(dir.join(format!("{pid}.{ext}"))).unwrap_or_default()
                };
                Run {
                    pid,
                    args: read("args").lines().map(str::to_owned).collect(),
                    env: read("env")
                        .lines()
                        .filter_map(|line| line.split_once('='))
                        .map(|(key, value)| (key.to_owned(), value.to_owned()))
                        .collect(),
                    cwd: read("cwd").trim_end().to_owned(),
                    stdin: read("stdin").lines().map(str::to_owned).collect(),
                }
            })
            .collect()
    }

    /// The runs of one worker agent.
    pub fn runs_of(&self, agent: &str) -> Vec<Run> {
        self.runs()
            .into_iter()
            .filter(|run| run.agent() == agent)
            .collect()
    }
}
