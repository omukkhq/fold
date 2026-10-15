use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[cfg(test)]
const START_TIMEOUT: Duration = Duration::from_secs(60);
#[cfg(test)]
const TURN_TIMEOUT: Duration = Duration::from_secs(180);
#[cfg(test)]
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// The quiet time after a resume before the first user turn, in which a re-sent permission request would arrive.
#[cfg(test)]
const RESUME_QUIET: Duration = Duration::from_secs(5);

/// The words the probe puts into the project, so a reply shows which input reached the model.
#[cfg(test)]
const SESSION_WORD: &str = "heron";
#[cfg(test)]
const NOTE_WORD: &str = "lantern";

/// Starts the installed Claude Code headless the way fold starts a worker and records every line it reads and sends as fixtures in `tests/fixtures/claude/`. Each step records what happened and goes on, so one run answers every question; only a missing login or binary stops it.
///
/// It runs in a fresh temporary project with harmless prompts, never passes a flag that skips permissions, removes every `CLAUDE*` variable and `ANTHROPIC_API_KEY` from the child's environment (so only the user's existing login can sign it in), and stops every process it starts. The fixtures replace paths below the temporary directory and the home directory, the account, the process id and every UUID, and they keep only Claude Code's own slash commands: the user's skills, plugins, agents and output styles become neutral examples.
///
/// `FOLD_PROBE_CLAUDE` names the binary (default `claude` on `PATH`), `FOLD_PROBE_MODEL` the model (default `haiku`), `FOLD_PROBE_OUT` the fixture directory, and `FOLD_PROBE_RUNS` a comma-separated choice of the runs `main`, `resume`, `error`, `5m` and `question` (default all; `resume` resumes the session of `main`). Only the fixtures of the runs that ran are written.
#[test]
#[ignore = "starts the installed Claude Code with the user's login; run it by hand"]
fn claude_probe() {
    let claude = std::env::var_os("FOLD_PROBE_CLAUDE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("claude"));
    let model = std::env::var("FOLD_PROBE_MODEL").unwrap_or_else(|_| "haiku".to_owned());
    let out = std::env::var_os("FOLD_PROBE_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude"));
    let scratch = tempfile::tempdir().expect("temporary directory");
    let mut probe = Probe::new(claude, model, scratch.path());

    println!("== version");
    let version = probe.plain(&["--version"]);
    println!("claude --version: {version:?}");
    println!("== auth");
    let auth = probe.plain(&["auth", "status", "--json"]);
    let logged_in = serde_json::from_str::<Value>(&auth)
        .ok()
        .and_then(|value| value["loggedIn"].as_bool());
    println!("claude auth status --json: loggedIn={logged_in:?}");
    if logged_in != Some(true) {
        println!("not signed in: nothing else runs");
        return;
    }

    let runs = std::env::var("FOLD_PROBE_RUNS").ok();
    let runs = |run: &str| {
        runs.as_deref()
            .is_none_or(|list| list.split(',').any(|item| item.trim() == run))
    };
    let session = new_uuid();
    if runs("main") {
        probe.run_main(&session);
    }
    if runs("resume") {
        probe.run_resume(&session);
    }
    if runs("error") {
        probe.run_error();
    }
    if runs("5m") {
        probe.run_five_minutes();
    }
    if runs("question") {
        probe.run_question();
    }

    println!("== hooks");
    for name in ["session-start", "stop"] {
        let log =
            std::fs::read_to_string(probe.dir.join(format!("logs/{name}.log"))).unwrap_or_default();
        println!("{name} hook ran {} times", log.lines().count());
        for line in log.lines() {
            let input: Value = serde_json::from_str(line).unwrap_or_default();
            println!(
                "  {name}: hook_event_name={} source={} stop_hook_active={} session_id={}",
                input["hook_event_name"],
                input["source"],
                input["stop_hook_active"],
                input["session_id"]
            );
        }
    }

    probe.write_fixtures(&out);
    println!("== findings");
    for finding in &probe.findings {
        println!("- {finding}");
    }
    println!("fixtures: {}", out.display());
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Answer {
    Allow,
    Deny,
    /// Leave the request unanswered and end the turn at once.
    Hold,
    /// Answer nothing and send `interrupt` instead.
    Interrupt,
    /// Answer an `AskUserQuestion` request with the first option of every question; deny any other tool.
    Choose,
}

#[cfg(test)]
#[derive(Default)]
struct Turn {
    permission_requests: Vec<Value>,
    result: Option<Value>,
    interrupt_response: Option<Value>,
    closed: bool,
}

#[cfg(test)]
struct Probe {
    claude: PathBuf,
    model: String,
    dir: PathBuf,
    project: PathBuf,
    project_5m: PathBuf,
    outside: PathBuf,
    mcp_config: PathBuf,
    fixtures: BTreeMap<String, Vec<String>>,
    findings: Vec<String>,
    /// A flag the main run settles: `--permission-prompt-tool stdio` when Claude Code accepts it.
    prompt_flags: Vec<String>,
    next_request: u32,
}

#[cfg(test)]
impl Probe {
    fn new(claude: PathBuf, model: String, scratch: &Path) -> Self {
        let dir = scratch.canonicalize().expect("canonical scratch");
        let project = dir.join("project");
        let project_5m = dir.join("project-5m");
        let outside = dir.join("outside");
        for path in [
            &project,
            &project_5m,
            &outside,
            &dir.join("logs"),
            &dir.join("bin"),
        ] {
            std::fs::create_dir_all(path).expect("probe directory");
        }
        let stub = dir.join("bin/fold");
        std::fs::write(&stub, MCP_STUB).expect("write the MCP stub");
        executable(&stub);
        let mcp_config = dir.join("mcp.json");
        std::fs::write(
            &mcp_config,
            json!({"mcpServers": {"fold": {"command": stub, "args": ["mcp"]}}}).to_string(),
        )
        .expect("write the MCP config");
        for root in [&project, &project_5m] {
            write_project(root, &dir);
        }
        Self {
            claude,
            model,
            dir,
            project,
            project_5m,
            outside,
            mcp_config,
            fixtures: BTreeMap::new(),
            findings: Vec::new(),
            prompt_flags: vec!["--permission-prompt-tool".to_owned(), "stdio".to_owned()],
            next_request: 0,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.claude);
        for (name, _) in std::env::vars_os() {
            let text = name.to_string_lossy();
            if text.starts_with("CLAUDE") || text == "ANTHROPIC_API_KEY" {
                command.env_remove(&name);
            }
        }
        command
    }

    /// Runs a command that needs no model and returns its stdout.
    fn plain(&self, args: &[&str]) -> String {
        let output = self
            .command()
            .args(args)
            .current_dir(&self.dir)
            .stdin(Stdio::null())
            .output()
            .expect("run claude");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn headless(&self, cwd: &Path, ttl: &str, extra: &[String]) -> Command {
        let mut command = self.command();
        command
            .args([
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
                "--model",
                &self.model,
                "--permission-mode",
                "auto",
            ])
            .args(&self.prompt_flags)
            .arg("--mcp-config")
            .arg(&self.mcp_config)
            .arg("--strict-mcp-config")
            .args(extra)
            .current_dir(cwd)
            .env("CLAUDE_CODE_PROMPT_CACHE_TTL", ttl)
            .env("CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL", ttl);
        command
    }

    fn request_id(&mut self) -> String {
        self.next_request += 1;
        format!("fold-probe-{}", self.next_request)
    }

    fn note(&mut self, finding: String) {
        println!("finding: {finding}");
        self.findings.push(finding);
    }

    /// Starts a worker and sends `initialize`; when Claude Code refuses `--permission-prompt-tool stdio` the run starts again without it and the default `--permission-prompts host` answers instead.
    fn start(&mut self, cwd: &Path, ttl: &str, extra: &[String], fixture: &str) -> Option<Worker> {
        loop {
            let mut worker = Worker::spawn(self.headless(cwd, ttl, extra), fixture);
            let id = self.request_id();
            worker.send(&json!({
                "type": "control_request",
                "request_id": id,
                "request": {"subtype": "initialize", "hooks": null},
            }));
            let response = worker.until(START_TIMEOUT, |line| {
                line["type"] == "control_response" && line["response"]["request_id"] == id
            });
            match response {
                Some(response) => {
                    let body = &response["response"];
                    self.note(format!(
                        "initialize answered with subtype {} and keys {:?}; pending_permission_requests: {}",
                        body["subtype"],
                        keys(&body["response"]),
                        body["response"]["pending_permission_requests"]
                    ));
                    return Some(worker);
                }
                None if !self.prompt_flags.is_empty() => {
                    let (status, stderr) = worker.stop();
                    self.note(format!(
                        "with {:?} the worker ended ({status:?}) before answering initialize; stderr: {stderr:?}; starting again without it",
                        self.prompt_flags
                    ));
                    self.absorb(&mut worker);
                    self.prompt_flags.clear();
                }
                None => {
                    let (status, stderr) = worker.stop();
                    self.note(format!(
                        "the worker ended ({status:?}) before answering initialize; stderr: {stderr:?}"
                    ));
                    self.absorb(&mut worker);
                    return None;
                }
            }
        }
    }

    fn turn(&mut self, worker: &mut Worker, text: &str, answer: Answer) -> Turn {
        worker.send(&json!({
            "type": "user",
            "session_id": "",
            "message": {"role": "user", "content": text},
            "parent_tool_use_id": null,
        }));
        let mut turn = Turn::default();
        let mut interrupt_sent = false;
        let deadline = Instant::now() + TURN_TIMEOUT;
        loop {
            let Some(line) = worker.next(deadline.saturating_duration_since(Instant::now())) else {
                turn.closed = worker.closed;
                break;
            };
            if line["type"] == "system" && line["subtype"] == "init" {
                self.note(describe_init(&line));
                worker.fixture = worker.fixture.replace("init", "result");
            }
            if line["type"] == "result" {
                turn.result = Some(line);
                break;
            }
            if line["type"] == "control_response" {
                turn.interrupt_response = Some(line);
                continue;
            }
            let wants_interrupt = answer == Answer::Interrupt
                && line["type"] == "assistant"
                && line["message"]["content"]
                    .as_array()
                    .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_use"));
            if line["type"] == "control_request" {
                let request = &line["request"];
                let id = line["request_id"].clone();
                if request["subtype"] != "can_use_tool" {
                    self.note(format!(
                        "an unexpected control request {}; answered with an error",
                        request["subtype"]
                    ));
                    worker.send(&json!({
                        "type": "control_response",
                        "response": {"subtype": "error", "request_id": id, "error": "the fold probe does not handle this request"},
                    }));
                    continue;
                }
                turn.permission_requests.push(line.clone());
                match answer {
                    Answer::Allow => worker.send(&json!({
                        "type": "control_response",
                        "response": {"subtype": "success", "request_id": id, "response": {"behavior": "allow", "updatedInput": request["input"]}},
                    })),
                    Answer::Deny => worker.send(&json!({
                        "type": "control_response",
                        "response": {"subtype": "success", "request_id": id, "response": {"behavior": "deny", "message": "fold denies this action"}},
                    })),
                    Answer::Choose if request["tool_name"] == "AskUserQuestion" => {
                        let mut input = request["input"].clone();
                        input["answers"] = first_choices(&request["input"]);
                        worker.send(&json!({
                            "type": "control_response",
                            "response": {"subtype": "success", "request_id": id, "response": {"behavior": "allow", "updatedInput": input}},
                        }));
                    }
                    Answer::Choose => worker.send(&json!({
                        "type": "control_response",
                        "response": {"subtype": "success", "request_id": id, "response": {"behavior": "deny", "message": "fold denies this action"}},
                    })),
                    Answer::Hold => break,
                    Answer::Interrupt => {}
                }
            }
            if (wants_interrupt
                || (answer == Answer::Interrupt && line["type"] == "control_request"))
                && !interrupt_sent
            {
                interrupt_sent = true;
                let id = self.request_id();
                worker.send(&json!({
                    "type": "control_request",
                    "request_id": id,
                    "request": {"subtype": "interrupt"},
                }));
            }
        }
        if answer == Answer::Interrupt && turn.interrupt_response.is_none() {
            turn.interrupt_response = worker.until(Duration::from_secs(5), |line| {
                line["type"] == "control_response"
            });
        }
        turn
    }

    fn report(&mut self, step: &str, turn: &Turn) {
        let requests: Vec<String> = turn
            .permission_requests
            .iter()
            .map(|line| {
                format!(
                    "tool {} keys {:?}",
                    line["request"]["tool_name"],
                    keys(&line["request"])
                )
            })
            .collect();
        let result = match &turn.result {
            Some(result) => format!(
                "result subtype {} is_error {} num_turns {} result {} permission_denials {} usage {}",
                result["subtype"],
                result["is_error"],
                result["num_turns"],
                result["result"],
                result["permission_denials"],
                result["usage"]
            ),
            None if turn.closed => "no result: the worker closed stdout".to_owned(),
            None => "no result before the step ended".to_owned(),
        };
        self.note(format!(
            "{step}: permission requests {requests:?}; {result}; interrupt response {}",
            turn.interrupt_response
                .as_ref()
                .map_or(Value::Null, |line| line["response"].clone())
        ));
    }

    fn absorb(&mut self, worker: &mut Worker) {
        for (fixture, line) in worker.log.drain(..) {
            self.fixtures.entry(fixture).or_default().push(line);
        }
    }

    fn run_main(&mut self, session: &str) {
        println!("== main run (cache 1h)");
        let extra = vec![
            "--session-id".to_owned(),
            session.to_owned(),
            "--include-hook-events".to_owned(),
        ];
        let project = self.project.clone();
        let Some(mut worker) = self.start(&project, "1h", &extra, "init") else {
            return;
        };
        let steps: [(&str, String, Answer); 7] = [
            (
                "result",
                "Reply with the word ok, then the fold probe word from your session context if you were given one.".to_owned(),
                Answer::Deny,
            ),
            (
                "tool_call",
                "Use the Read tool to read note.txt in the current directory, then reply with the word it names.".to_owned(),
                Answer::Deny,
            ),
            (
                "permission_request",
                "Use the Bash tool to run exactly this command: echo fold-probe-allow".to_owned(),
                Answer::Allow,
            ),
            (
                "permission_deny",
                "Use the Bash tool to run exactly this command: echo fold-probe-deny".to_owned(),
                Answer::Deny,
            ),
            (
                "denied_by_rule",
                "Use the Bash tool to run exactly this command: echo fold-probe-refused".to_owned(),
                Answer::Deny,
            ),
            (
                "auto_unsure",
                format!(
                    "Use the Write tool to create the file {} containing the word outside.",
                    self.outside.join("probe.txt").display()
                ),
                Answer::Deny,
            ),
            (
                "interrupt",
                "Use the Bash tool to run exactly this command: sleep 30".to_owned(),
                Answer::Interrupt,
            ),
        ];
        for (step, text, answer) in steps {
            if step != "result" {
                worker.fixture = step.to_owned();
            }
            let turn = self.turn(&mut worker, &text, answer);
            self.report(step, &turn);
            if turn.closed {
                break;
            }
        }
        self.note(format!(
            "outside file written: {}",
            self.outside.join("probe.txt").exists()
        ));

        worker.fixture = "pending".to_owned();
        let turn = self.turn(
            &mut worker,
            "Use the Bash tool to run exactly this command: echo fold-probe-pending",
            Answer::Hold,
        );
        self.report("pending", &turn);
        let (status, stderr) = worker.terminate();
        self.note(format!(
            "SIGTERM with a permission request open: exit {status:?}, stderr {stderr:?}"
        ));
        self.absorb(&mut worker);
    }

    fn run_resume(&mut self, session: &str) {
        println!("== resume");
        let extra = vec!["--resume".to_owned(), session.to_owned()];
        let project = self.project.clone();
        let Some(mut worker) = self.start(&project, "1h", &extra, "resumed") else {
            return;
        };
        let mut spontaneous = Vec::new();
        while let Some(line) = worker.next(RESUME_QUIET) {
            if line["type"] == "control_request" {
                worker.send(&json!({
                    "type": "control_response",
                    "response": {"subtype": "success", "request_id": line["request_id"], "response": {"behavior": "deny", "message": "fold denies this action"}},
                }));
            }
            spontaneous.push(format!("{} {}", line["type"], line["subtype"]));
        }
        self.note(format!(
            "after resume, before any user turn: {spontaneous:?}"
        ));
        let turn = self.turn(
            &mut worker,
            "Reply with the word resumed, then the last bash command you were asked to run.",
            Answer::Deny,
        );
        self.report("resumed", &turn);
        worker.close_stdin();
        let (status, stderr) = worker.stop();
        self.note(format!(
            "stdin closed after the result: exit {status:?}, stderr {stderr:?}"
        ));
        self.absorb(&mut worker);
    }

    fn run_error(&mut self) {
        println!("== error result");
        let missing = new_uuid();
        let mut worker = Worker::spawn(
            self.headless(
                &self.project.clone(),
                "1h",
                &["--resume".to_owned(), missing],
            ),
            "error_result",
        );
        worker.send(&json!({
            "type": "user",
            "session_id": "",
            "message": {"role": "user", "content": "Reply with the word ok."},
            "parent_tool_use_id": null,
        }));
        let result = worker.until(START_TIMEOUT, |line| line["type"] == "result");
        worker.close_stdin();
        let (status, stderr) = worker.stop();
        self.note(format!(
            "--resume of an unknown session: result {}, exit {status:?}, stderr {stderr:?}",
            result.map_or(Value::Null, |line| json!({
                "subtype": line["subtype"],
                "is_error": line["is_error"],
                "errors": line["errors"],
                "result": line["result"],
            }))
        ));
        self.absorb(&mut worker);
    }

    fn run_five_minutes(&mut self) {
        println!("== cache 5m, partial messages");
        let extra = vec![
            "--session-id".to_owned(),
            new_uuid(),
            "--include-partial-messages".to_owned(),
        ];
        let project = self.project_5m.clone();
        let Some(mut worker) = self.start(&project, "5m", &extra, "cache_5m") else {
            return;
        };
        let turn = self.turn(&mut worker, "Reply with the word ok.", Answer::Deny);
        self.report("cache_5m", &turn);
        worker.close_stdin();
        let (status, _) = worker.stop();
        self.note(format!("5m run: exit {status:?}"));
        self.absorb(&mut worker);
    }

    fn run_question(&mut self) {
        println!("== AskUserQuestion");
        let extra = vec!["--session-id".to_owned(), new_uuid()];
        let project = self.project.clone();
        let Some(mut worker) = self.start(&project, "1h", &extra, "ask_question") else {
            return;
        };
        let turn = self.turn(
            &mut worker,
            "Use the AskUserQuestion tool to ask me one multiple-choice question: which colour the fold probe should use, with the options red and blue. Then reply with only the colour I chose.",
            Answer::Choose,
        );
        self.report("ask_question", &turn);
        for line in &turn.permission_requests {
            self.note(format!(
                "AskUserQuestion request input: {}",
                line["request"]["input"]
            ));
        }
        worker.close_stdin();
        let (status, stderr) = worker.stop();
        self.note(format!("question run: exit {status:?}, stderr {stderr:?}"));
        self.absorb(&mut worker);
    }

    fn write_fixtures(&self, out: &Path) {
        std::fs::create_dir_all(out).expect("fixture directory");
        let home = std::env::var("HOME").unwrap_or_default();
        let dir = self.dir.display().to_string();
        let mut replacements = vec![
            (dir.clone(), "/tmp/fold-probe".to_owned()),
            (dir.replace(['/', '.'], "-"), "-tmp-fold-probe".to_owned()),
        ];
        if !home.is_empty() {
            replacements.push((home, "/home/user".to_owned()));
        }
        // The initialize answer names the signed-in account; its organisation contains the email, so it goes first.
        for line in self.fixtures.values().flatten() {
            let value: Value = serde_json::from_str(line).unwrap_or_default();
            let account = &value["response"]["response"]["account"];
            for (key, placeholder) in [
                ("organization", "Example Organization"),
                ("email", "user@example.com"),
            ] {
                if let Some(text) = account[key].as_str().filter(|text| !text.is_empty()) {
                    replacements.push((text.to_owned(), placeholder.to_owned()));
                }
            }
        }
        let mut uuids = BTreeMap::new();
        for (name, lines) in &self.fixtures {
            let mut text = String::new();
            for line in lines {
                let mut line = line.clone();
                for (from, to) in &replacements {
                    line = line.replace(from.as_str(), to);
                }
                if let Ok(mut value) = serde_json::from_str::<Value>(&line) {
                    neutralise(&mut value);
                    line = value.to_string();
                }
                text.push_str(&fake_uuids(&line, &mut uuids));
                text.push('\n');
            }
            std::fs::write(out.join(format!("{name}.jsonl")), text).expect("write a fixture");
        }
    }
}

/// The slash commands of Claude Code itself that the fixtures keep.
#[cfg(test)]
const BUILTIN_COMMANDS: [&str; 10] = [
    "clear",
    "color",
    "compact",
    "config",
    "doctor",
    "focus",
    "init",
    "mcp",
    "model",
    "reload-plugins",
];
#[cfg(test)]
const EXAMPLE_SKILL: &str = "example-skill";
#[cfg(test)]
const EXAMPLE_AGENT: &str = "example-plugin:example-agent";
#[cfg(test)]
const EXAMPLE_PID: u64 = 12345;

/// Replaces what a recording learns about the user's own setup and account with neutral values of the same shape.
#[cfg(test)]
fn neutralise(value: &mut Value) {
    if value["type"] == "system" && value["subtype"] == "init" {
        let commands = value["slash_commands"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|name| {
                name.as_str()
                    .is_some_and(|name| BUILTIN_COMMANDS.contains(&name))
            })
            .cloned()
            .chain([json!(EXAMPLE_SKILL)])
            .collect();
        value["slash_commands"] = Value::Array(commands);
        value["skills"] = json!([EXAMPLE_SKILL]);
        value["agents"] = json!(["general-purpose", EXAMPLE_AGENT]);
        value["plugins"] = json!([{
            "name": "example-plugin",
            "path": "/home/user/.claude/plugins/cache/example-marketplace/example-plugin/1.0.0",
            "source": "example-plugin@example-marketplace",
            "version": "1.0.0",
        }]);
        if value.get("messaging_socket_path").is_some() {
            value["messaging_socket_path"] =
                json!(format!("/run/user/1000/cc-socks/{EXAMPLE_PID}.sock"));
        }
    }
    if value["type"] == "control_response"
        && let Some(response) = value
            .pointer_mut("/response/response")
            .and_then(Value::as_object_mut)
    {
        if let Some(commands) = response.get_mut("commands").and_then(Value::as_array_mut) {
            commands.retain(|command| {
                command["name"]
                    .as_str()
                    .is_some_and(|name| BUILTIN_COMMANDS.contains(&name))
            });
            commands.push(json!({"name": EXAMPLE_SKILL, "description": "An example skill.", "argumentHint": ""}));
        }
        if response.contains_key("agents") {
            response.insert(
                "agents".to_owned(),
                json!([
                    {"name": "general-purpose", "description": "An example built-in agent."},
                    {"name": EXAMPLE_AGENT, "description": "An example plugin agent.", "model": "inherit"},
                ]),
            );
        }
        if let Some(styles) = response
            .get_mut("available_output_styles")
            .and_then(Value::as_array_mut)
        {
            styles.retain(|style| {
                style
                    .as_str()
                    .is_some_and(|style| ["default", "Explanatory", "Learning"].contains(&style))
            });
        }
        if response.contains_key("pid") {
            response.insert("pid".to_owned(), json!(EXAMPLE_PID));
        }
        if let Some(account) = response.get_mut("account").and_then(Value::as_object_mut) {
            account.insert("email".to_owned(), json!("user@example.com"));
            account.insert("organization".to_owned(), json!("Example Organization"));
            account.insert("subscriptionType".to_owned(), json!("Example Subscription"));
        }
    }
    if value["type"] == "rate_limit_event"
        && let Some(info) = value.get_mut("rate_limit_info")
    {
        neutralise_limits(info);
    }
}

/// The account's usage, reset times and overage policy.
#[cfg(test)]
fn neutralise_limits(value: &mut Value) {
    let Some(fields) = value.as_object_mut() else {
        return;
    };
    for (key, field) in fields.iter_mut() {
        match key.as_str() {
            "utilization" => *field = json!(0.1),
            "resetsAt" => *field = json!(1_800_000_000),
            "overageDisabledReason" => *field = json!("example"),
            _ => neutralise_limits(field),
        }
    }
}

/// Replaces every UUID in `line` with a fake one, the same fake for the same UUID in every fixture, so sessions and requests still pair up.
#[cfg(test)]
fn fake_uuids(line: &str, seen: &mut BTreeMap<String, String>) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let (mut copied, mut at) = (0, 0);
    while at + 36 <= bytes.len() {
        if is_uuid(&bytes[at..at + 36]) {
            out.push_str(&line[copied..at]);
            let next = seen.len() + 1;
            let fake = seen
                .entry(line[at..at + 36].to_owned())
                .or_insert_with(|| format!("00000000-0000-4000-8000-{next:012}"));
            out.push_str(fake);
            at += 36;
            copied = at;
        } else {
            at += 1;
        }
    }
    out.push_str(&line[copied..]);
    out
}

#[cfg(test)]
fn is_uuid(bytes: &[u8]) -> bool {
    bytes.iter().enumerate().all(|(index, byte)| match index {
        8 | 13 | 18 | 23 => *byte == b'-',
        _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(byte),
    })
}

#[cfg(test)]
struct Worker {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    stderr: Receiver<String>,
    /// The fixture the next line goes to; a sent line goes to `<fixture>.sent`.
    fixture: String,
    log: Vec<(String, String)>,
    closed: bool,
}

#[cfg(test)]
impl Worker {
    fn spawn(mut command: Command, fixture: &str) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start claude");
        let stdout = child.stdout.take().expect("stdout");
        let (line_tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line_tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut stderr_pipe = child.stderr.take().expect("stderr");
        let (err_tx, stderr) = mpsc::channel();
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr_pipe.read_to_string(&mut text);
            let _ = err_tx.send(text);
        });
        Self {
            stdin: child.stdin.take(),
            child,
            lines,
            stderr,
            fixture: fixture.to_owned(),
            log: Vec::new(),
            closed: false,
        }
    }

    fn send(&mut self, value: &Value) {
        let line = value.to_string();
        self.log
            .push((format!("{}.sent", self.fixture), line.clone()));
        if let Some(stdin) = &mut self.stdin
            && (stdin.write_all(line.as_bytes()).is_err()
                || stdin.write_all(b"\n").is_err()
                || stdin.flush().is_err())
        {
            println!("!! writing to the worker failed");
        }
    }

    /// The next JSON line within `timeout`; a line that is not JSON is printed and skipped.
    fn next(&mut self, timeout: Duration) -> Option<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            match self
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(line) => match serde_json::from_str::<Value>(&line) {
                    Ok(value) => {
                        println!(
                            "<- {} {}",
                            value["type"],
                            value.get("subtype").unwrap_or(&Value::Null)
                        );
                        self.log.push((self.fixture.clone(), line));
                        return Some(value);
                    }
                    Err(_) => println!("<- not JSON ({} bytes)", line.len()),
                },
                Err(RecvTimeoutError::Timeout) => return None,
                Err(RecvTimeoutError::Disconnected) => {
                    self.closed = true;
                    return None;
                }
            }
        }
    }

    fn until(&mut self, timeout: Duration, matches: impl Fn(&Value) -> bool) -> Option<Value> {
        let deadline = Instant::now() + timeout;
        while let Some(line) = self.next(deadline.saturating_duration_since(Instant::now())) {
            if matches(&line) {
                return Some(line);
            }
        }
        None
    }

    fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// Waits for the worker to end, killing it after `STOP_TIMEOUT`, and returns its exit status and stderr.
    fn stop(&mut self) -> (Option<ExitStatus>, String) {
        self.close_stdin();
        let deadline = Instant::now() + STOP_TIMEOUT;
        let status = loop {
            while self.next(Duration::from_millis(100)).is_some() {}
            match self.child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => {}
                _ => {
                    println!("!! the worker did not end within {STOP_TIMEOUT:?}; killing it");
                    let _ = self.child.kill();
                    break self.child.wait().ok();
                }
            }
        };
        let stderr = self.stderr.recv_timeout(STOP_TIMEOUT).unwrap_or_default();
        (status, stderr.trim().to_owned())
    }

    fn terminate(&mut self) -> (Option<ExitStatus>, String) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status();
        let deadline = Instant::now() + STOP_TIMEOUT;
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                break;
            }
            while self.next(Duration::from_millis(100)).is_some() {}
        }
        self.stop()
    }
}

#[cfg(test)]
impl Drop for Worker {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
fn describe_init(line: &Value) -> String {
    format!(
        "system/init: claude_code_version {} model {} permissionMode {} apiKeySource {} mcp_servers {} capabilities {} keys {:?}",
        line["claude_code_version"],
        line["model"],
        line["permissionMode"],
        line["apiKeySource"],
        line["mcp_servers"],
        line["capabilities"],
        keys(line)
    )
}

/// The answers of an `AskUserQuestion` input as the Agent SDK documents them: each question's text mapped to the label of the option chosen, here the first.
#[cfg(test)]
fn first_choices(input: &Value) -> Value {
    let mut answers = serde_json::Map::new();
    for question in input["questions"].as_array().into_iter().flatten() {
        if let (Some(text), Some(label)) = (
            question["question"].as_str(),
            question["options"][0]["label"].as_str(),
        ) {
            answers.insert(text.to_owned(), Value::from(label));
        }
    }
    Value::Object(answers)
}

#[cfg(test)]
fn keys(value: &Value) -> Vec<String> {
    value
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

#[cfg(test)]
fn new_uuid() -> String {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).expect("random bytes");
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
fn executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("make the file executable");
}

/// A project with a note to read, `ask` rules that force a permission request, a `deny` rule, and SessionStart and Stop hooks that answer the way `fold hook` does: the Stop hook blocks once and lets every stop with `stop_hook_active` pass.
#[cfg(test)]
fn write_project(root: &Path, dir: &Path) {
    std::fs::write(
        root.join("note.txt"),
        format!("fold probe note: the word is {NOTE_WORD}.\n"),
    )
    .expect("write the note");
    let hooks = root.join(".claude/hooks");
    std::fs::create_dir_all(&hooks).expect("hooks directory");
    let logs = dir.join("logs");
    let session_start = hooks.join("session-start.sh");
    std::fs::write(
        &session_start,
        format!(
            "#!/bin/sh\ncat >> '{logs}/session-start.log'\necho >> '{logs}/session-start.log'\nprintf '%s\\n' '{{\"hookSpecificOutput\":{{\"hookEventName\":\"SessionStart\",\"additionalContext\":\"The fold probe word is {SESSION_WORD}.\"}}}}'\n",
            logs = logs.display()
        ),
    )
    .expect("write the session-start hook");
    executable(&session_start);
    let stop = hooks.join("stop.sh");
    std::fs::write(
        &stop,
        format!(
            "#!/bin/sh\ninput=$(cat)\nprintf '%s\\n' \"$input\" >> '{logs}/stop.log'\nif printf '%s' \"$input\" | grep -Eq '\"stop_hook_active\": ?true'; then exit 0; fi\nif [ -e '{logs}/stop.blocked' ]; then exit 0; fi\n: > '{logs}/stop.blocked'\nprintf '%s\\n' '{{\"decision\":\"block\",\"reason\":\"The fold probe stop hook asks for one more line: reply with the word again.\"}}'\n",
            logs = logs.display()
        ),
    )
    .expect("write the stop hook");
    executable(&stop);
    let settings = json!({
        "permissions": {
            "ask": [
                "Bash(echo fold-probe-allow)",
                "Bash(echo fold-probe-deny)",
                "Bash(echo fold-probe-pending)",
            ],
            "deny": ["Bash(echo fold-probe-refused)"],
        },
        "hooks": {
            "SessionStart": [{"hooks": [{"type": "command", "command": session_start}]}],
            "Stop": [{"hooks": [{"type": "command", "command": stop}]}],
        },
    });
    std::fs::write(
        root.join(".claude/settings.json"),
        serde_json::to_string_pretty(&settings).expect("settings JSON"),
    )
    .expect("write the settings");
}

/// An MCP server named `fold` that answers `initialize` and lists no tools, so the probe needs no daemon and starts none.
#[cfg(test)]
const MCP_STUB: &str = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  version=$(printf '%s' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"fold","version":"probe"}}}\n' "$id" "$version" ;;
    *'"method":"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[]}}\n' "$id" ;;
    *'"id":'*) [ -n "$id" ] && printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
  esac
done
"#;
