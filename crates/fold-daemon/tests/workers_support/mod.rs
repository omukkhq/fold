use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use fold_daemon::services::agents::Launcher;
use tempfile::TempDir;

/// A stand-in for `claude -p` in stream-json mode, so no test needs Claude Code. It records its arguments, environment, working directory and every stdin line under `FOLD_FAKE_CLAUDE_DIR`, answers `initialize` and `interrupt` as Claude Code does, replays the fixture `FOLD_FAKE_CLAUDE_TURN` for each user turn, waits after a `can_use_tool` request until fold answers it, and exits when stdin closes. With `FOLD_FAKE_CLAUDE_STUBBORN` set it ignores SIGINT and a closed stdin, so only SIGTERM or SIGKILL ends it. With `FOLD_FAKE_CLAUDE_LOST` naming a fixture, a run with `--resume` answers its first user turn with that fixture and exits with code 1, as Claude Code does for a session it cannot find. With `FOLD_FAKE_CLAUDE_CHILD` set it starts a background `sleep` first, as a tool's command would, and records its pid; with `FOLD_FAKE_CLAUDE_NOISE` set it writes a line that is not UTF-8 and a line over the supervisor's length cap before each turn. Once its stdin closes it records that.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
dir="$FOLD_FAKE_CLAUDE_DIR"
for arg in "$@"; do printf '%s\n' "$arg"; done > "$dir/$$.args"
env > "$dir/$$.env"
pwd > "$dir/$$.cwd"
if [ -n "$FOLD_FAKE_CLAUDE_STUBBORN" ]; then trap '' INT; fi
if [ -n "$FOLD_FAKE_CLAUDE_CHILD" ]; then
  sleep 300 &
  printf '%s\n' "$!" > "$dir/$$.child"
fi
lost=""
if [ -n "$FOLD_FAKE_CLAUDE_LOST" ]; then
  for arg in "$@"; do [ "$arg" = "--resume" ] && lost=1; done
fi
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
      if [ -n "$lost" ]; then cat "$FOLD_FAKE_CLAUDE_LOST"; exit 1; fi
      if [ -n "$FOLD_FAKE_CLAUDE_NOISE" ]; then
        printf '\377\376\n'
        head -c 17000000 /dev/zero | tr '\0' a
        printf '\n'
      fi
      while IFS= read -r out <&3; do
        printf '%s\n' "$out"
        case "$out" in
          *'"subtype":"can_use_tool"'*)
            while :; do
              IFS= read -r reply || exit 0
              printf '%s\n' "$reply" >> "$dir/$$.stdin"
              case "$reply" in *'"type":"control_response"'*) break ;; esac
            done ;;
        esac
      done 3< "$FOLD_FAKE_CLAUDE_TURN" ;;
  esac
done
: > "$dir/$$.eof"
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
        // The fake never starts the fold server of its MCP configuration, but fold refuses a launch whose fld binary is not there.
        let fold = dir.path().join("fld");
        std::fs::write(&fold, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&fold, std::fs::Permissions::from_mode(0o755)).unwrap();
        let launcher = Launcher {
            claude: program,
            fold: Some(fold),
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

    /// Every `--resume` finds no session, so fold must start the worker in a new one.
    #[must_use]
    pub fn lost_sessions(mut self) -> Self {
        self.launcher.env.push((
            "FOLD_FAKE_CLAUDE_LOST".to_owned(),
            fixture("error_result").to_string_lossy().into_owned(),
        ));
        self
    }

    /// Each run starts a background child of its own.
    #[must_use]
    pub fn with_child(mut self) -> Self {
        self.launcher
            .env
            .push(("FOLD_FAKE_CLAUDE_CHILD".to_owned(), "1".to_owned()));
        self
    }

    /// Each turn starts with a line that is not UTF-8 and a line over the supervisor's length cap.
    #[must_use]
    pub fn noisy(mut self) -> Self {
        self.launcher
            .env
            .push(("FOLD_FAKE_CLAUDE_NOISE".to_owned(), "1".to_owned()));
        self
    }

    /// The pid of the background child `run` started, once it has recorded it.
    pub fn child_of(&self, run: &Run) -> Option<u32> {
        let path = self
            .dir
            .path()
            .join("runs")
            .join(format!("{}.child", run.pid));
        std::fs::read_to_string(path).ok()?.trim().parse().ok()
    }

    /// Whether `run` has seen its stdin close.
    pub fn saw_eof(&self, run: &Run) -> bool {
        self.dir
            .path()
            .join("runs")
            .join(format!("{}.eof", run.pid))
            .exists()
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
