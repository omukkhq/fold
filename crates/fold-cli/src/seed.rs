mod agent;
mod sample;
mod scripts;
mod stub;

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::{Args, Subcommand};
use fold_cli::daemon_client::{ClientConfig, ClientError, DaemonClient, HandshakeError};
use fold_core::{ProjectPaths, codes};
use fold_daemon::services::agents::DEV_STUB_DIR;
use hyper::Method;
use serde_json::{Value, json};
use thiserror::Error;

use crate::init::{self, InitArgs, InitError};
use crate::output;
use crate::serve::{self, Dirs, ServeError};
use agent::Agent;
use scripts::{API_LEAD, LIMIT_WORKER, REVIEW_LEAD, ROUTES_WORKER, Tasks, WEB_LEAD};

/// The human the seed acts as; `make seed-dev` sets `FOLD_HUMAN` to the same name.
const HUMAN: &str = "dev";

/// The project settings of the demo. The review lead integrates each proposal at the step that needs it, so approved proposals stay open for the board; every worker the demo needs runs at once; and the model is one no Claude Code knows, so a daemon that runs `claude` instead of the stub fails each worker before it does any work.
const CONFIG: &str = "
[integration]
automatic = false

[workers]
task_worker = 8
reviewer = 6

[roles.area_lead]
model = \"fold-dev-stub\"

[roles.review_lead]
model = \"fold-dev-stub\"

[roles.task_worker]
model = \"fold-dev-stub\"

[roles.reviewer]
model = \"fold-dev-stub\"
";

/// The step of a failed worker that left no `stub.log`, as a worker that runs `claude` instead of the stub does.
const NO_STEP: &str = "it ran no step";

/// How long the seed waits for the agents' work.
const WAIT: Duration = Duration::from_secs(180);
const POLL: Duration = Duration::from_millis(100);

#[derive(Args, Debug)]
pub struct DevArgs {
    #[command(subcommand)]
    command: DevCommand,
}

#[derive(Subcommand, Debug)]
enum DevCommand {
    /// Build a throwaway demo project through the tools and routes a real client uses
    Seed(SeedArgs),
    /// Stand in for Claude Code as a worker of a project with a .fold/dev-stub folder: run the agent's script from it through the fold MCP tools
    Stub(stub::StubArgs),
}

#[derive(Args, Debug)]
struct SeedArgs {
    /// The project directory; it must not exist
    #[arg(long, default_value = ".dev/projects/demo")]
    dir: PathBuf,
    /// Unix socket of the running daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum SeedError {
    #[error("{} already exists", dir.display())]
    Exists { dir: PathBuf },
    #[error(transparent)]
    Serve(#[from] ServeError),
    #[error("no socket is known; pass --socket of a running daemon")]
    NoSocket,
    #[error(transparent)]
    Init(#[from] InitError),
    #[error("cannot start the async runtime: {0}")]
    Runtime(io::Error),
    #[error("cannot use the files of the demo project: {0}")]
    Files(#[from] io::Error),
    #[error("cannot connect to {}: {source}", path.display())]
    Socket { path: PathBuf, source: io::Error },
    #[error(transparent)]
    Handshake(HandshakeError),
    #[error("the daemon refused the session: {code}: {message}")]
    Refused { code: String, message: String },
    #[error("{tool} failed: {code}")]
    Tool { tool: &'static str, code: String },
    #[error("{tool} did not answer as expected")]
    Unexpected { tool: &'static str },
    #[error("the connection failed during {tool}")]
    Transport { tool: &'static str },
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("{method} {path} failed: {code}")]
    Route {
        method: Method,
        path: String,
        code: String,
    },
    #[error("cannot write the output: {0}")]
    Write(io::Error),
    #[error(transparent)]
    Stub(#[from] stub::StubError),
    #[error("the worker {agent} failed: {step}")]
    Worker { agent: String, step: String },
    #[error("the demo's agents did not reach {what} in time")]
    Timeout { what: &'static str },
}

impl From<HandshakeError> for SeedError {
    fn from(error: HandshakeError) -> Self {
        match error {
            HandshakeError::Refused { code, message, .. } => Self::Refused { code, message },
            other => Self::Handshake(other),
        }
    }
}

impl SeedError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Exists { .. } => Some(codes::SEED_EXISTS),
            Self::Init(error) => error.code(),
            Self::Refused { code, .. } | Self::Tool { code, .. } | Self::Route { code, .. } => {
                Some(code)
            }
            _ => None,
        }
    }

    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::Exists { .. } => Some(
                "stop `make dev-daemon`, run `make clean-keep-cargo`, start `make dev-daemon` again, then run `make seed-dev`",
            ),
            Self::Worker { step, .. } if step == NO_STEP => Some(
                "the daemon must run the stub workers, which only a build with the `dev-seed` feature has: stop the daemon, run `make clean-keep-cargo`, start `make dev-daemon`, which builds fld with that feature, then run `make seed-dev`",
            ),
            Self::Worker { .. } | Self::Timeout { .. } => Some(
                "each worker's steps are in `.fold/workers/<agent>/stub.log`; to build the demo again, stop `make dev-daemon`, run `make clean-keep-cargo`, start `make dev-daemon` again, then run `make seed-dev`",
            ),
            _ => None,
        }
    }
}

pub fn run(args: &DevArgs, dirs: &Dirs, out: &mut impl Write) -> Result<(), SeedError> {
    match &args.command {
        DevCommand::Seed(args) => seed(args, dirs, out),
        DevCommand::Stub(args) => Ok(stub::run(args)?),
    }
}

fn seed(args: &SeedArgs, dirs: &Dirs, out: &mut impl Write) -> Result<(), SeedError> {
    let dir = std::path::absolute(&args.dir)?;
    if dir.exists() {
        return Err(SeedError::Exists { dir });
    }
    let socket = serve::socket_path(args.socket.as_deref(), dirs)?.ok_or(SeedError::NoSocket)?;
    let socket = std::path::absolute(socket)?;

    // The sample's files are in the folder before init, so they become r1.
    sample::write(&dir)?;
    let init = InitArgs {
        dir: Some(dir.clone()),
        name: Some("Demo".to_owned()),
        with_git: true,
        socket: Some(socket.clone()),
        refresh: false,
        git_name: Some("Demo".to_owned()),
        git_email: Some("demo@example.com".to_owned()),
    };
    let slug = init::initialise(&dir, &init, init::Kind::New, dirs, &mut io::sink())?;
    let root = dir.canonicalize()?;
    let paths = ProjectPaths::new(&root);
    let mut text = std::fs::read_to_string(paths.config())?;
    text.push_str(CONFIG);
    std::fs::write(paths.config(), text)?;
    // The folder makes the daemon run every worker of the project as the stub, which follows the agent's script from it.
    std::fs::create_dir_all(paths.fold_dir().join(DEV_STUB_DIR))?;

    let runtime = serve::current_thread_runtime().map_err(SeedError::Runtime)?;
    let built = runtime.block_on(build(&socket, &root, &slug))?;

    let mut write = |text: String| output::line(out, &text).map_err(SeedError::Write);
    write(format!("seeded project {slug} at {}", root.display()))?;
    write(format!(
        "{} tasks, {} proposals and {} workers",
        built.tasks, built.proposals, built.workers
    ))?;
    for (column, total) in &built.columns {
        write(format!("{column}: {total}"))?;
    }
    let listen = dirs
        .config_file()
        .and_then(|path| serve::read_file_config(&path))
        .ok()
        .and_then(|file| file.listen)
        .unwrap_or(serve::DEFAULT_LISTEN);
    write(format!("board: http://{listen}/p/{slug}/board"))
}

struct Built {
    tasks: u32,
    proposals: usize,
    workers: usize,
    columns: Vec<(String, u64)>,
}

async fn build(socket: &Path, root: &Path, slug: &str) -> Result<Built, SeedError> {
    let config = ClientConfig::running(socket.to_owned());
    let mut seeder = Seeder {
        root: root.to_owned(),
        slug: slug.to_owned(),
        human: DaemonClient::new(config, Some(HUMAN.to_owned())),
        // The user's own session, which `fld claude` opens as the project's main agent; every other agent is a worker that fold starts.
        main: Agent::connect(socket, root, "main", &["main"]).await?,
        tasks: 0,
    };
    let outcome = seeder.scenario().await;
    let built = match outcome {
        Ok(()) => seeder.built().await,
        Err(error) => Err(error),
    };
    seeder.main.close().await;
    built
}

/// The tasks the main agent adds after the scripts are written.
struct Later {
    pagination: String,
}

struct Seeder {
    root: PathBuf,
    slug: String,
    human: DaemonClient,
    main: Agent,
    tasks: u32,
}

impl Seeder {
    /// The main agent plans the work and starts the leads; the leads start the task workers and reviewers, which follow their scripts. The main agent then waits for their work at the points where it acts itself.
    async fn scenario(&mut self) -> Result<(), SeedError> {
        let (tasks, later) = self.plan().await?;
        for (name, label) in [("api", "backend"), ("web", "frontend")] {
            self.main
                .call("define_area", json!({"name": name, "label": label}))
                .await?;
        }
        self.write_scripts(&tasks).await?;
        self.main
            .call(
                "start_worker",
                json!({"role": "review_lead", "name": REVIEW_LEAD}),
            )
            .await?;
        for (name, area) in [(API_LEAD, "api"), (WEB_LEAD, "web")] {
            self.main
                .call(
                    "start_worker",
                    json!({"role": "area_lead", "name": name, "area": area}),
                )
                .await?;
        }

        // The limit task already has a workspace when it gets a prerequisite, so its card shows the blocked indicator.
        self.wait_task(&tasks.limit, "the limit task's workspace", |task| {
            !task["workspace"].is_null()
        })
        .await?;
        self.main
            .call(
                "add_dependency",
                json!({"task": tasks.limit, "needs": later.pagination}),
            )
            .await?;

        // The feedback goes to the worker that holds the task.
        self.wait_worker(ROUTES_WORKER, "the work on the user routes", done)
            .await?;
        self.route(
            Method::POST,
            "/feedback",
            Some(json!({
                "body": "Keep the handlers small; one file per resource is fine.",
                "task": tasks.routes,
            })),
        )
        .await?;

        for task in [&tasks.schema, &tasks.local_dates] {
            self.wait_task(task, "the integrations", |task| {
                !task["integration"].is_null()
            })
            .await?;
        }
        self.container(&tasks).await?;
        self.settled(&tasks).await
    }

    /// Labels, and every task with its edges. Each column is reached later by the work done on the tasks.
    async fn plan(&mut self) -> Result<(Tasks, Later), SeedError> {
        for (name, color) in [
            ("backend", "sage"),
            ("frontend", "slate"),
            ("infra", "teal"),
        ] {
            self.main
                .call("create_label", json!({"name": name, "color": color}))
                .await?;
        }
        let schema = self
            .task(
                "Create the database schema",
                "Define the users table and the helper that opens it.",
                &["backend"],
                &[],
            )
            .await?;
        let routes = self
            .task(
                "Add the user routes",
                "List, create and read users over HTTP.",
                &["backend"],
                &[&schema],
            )
            .await?;
        let local_dates = self
            .task(
                "Show dates in the local time zone",
                "The list shows dates as UTC today.",
                &["frontend"],
                &[],
            )
            .await?;
        let relative_dates = self
            .task(
                "Show dates as relative text",
                "Show \"2 hours ago\" instead of a date for recent items.",
                &["frontend"],
                &[],
            )
            .await?;
        let counts = self
            .task(
                "Format counts with separators",
                "Large counts are hard to read.",
                &["frontend"],
                &[],
            )
            .await?;
        let login = self
            .task(
                "Add the login form",
                "A form with a name and a password field.",
                &["frontend"],
                &[],
            )
            .await?;
        let audit = self
            .task(
                "Record changes in an audit log",
                "Every write leaves one line with who and when.",
                &["backend"],
                &[],
            )
            .await?;
        let limit = self
            .task(
                "Limit requests per client",
                "Refuse a client that sends too many requests.",
                &["backend", "infra"],
                &[],
            )
            .await?;
        let pagination = self
            .task(
                "Choose the pagination style",
                "Pick between pages and a cursor before the list grows.",
                &["frontend"],
                &[],
            )
            .await?;
        // The human adds one task on the board, which also makes them known to the project, so the agents can write to them.
        self.route(
            Method::POST,
            "/tasks",
            Some(json!({
                "title": "Write the deployment guide",
                "description": "Describe how to run the service on a small server.",
                "labels": ["infra"],
            })),
        )
        .await?;
        self.tasks += 1;
        let export = self
            .task(
                "Add the admin export",
                "Export all users as a CSV file.",
                &["backend"],
                &[&routes],
            )
            .await?;
        let search = self
            .task(
                "Add full text search",
                "Search the notes by their text.",
                &["backend"],
                &[&export],
            )
            .await?;
        self.main
            .call(
                "add_dependency",
                json!({"task": search, "needs": pagination}),
            )
            .await?;
        Ok((
            Tasks {
                schema,
                routes,
                local_dates,
                relative_dates,
                counts,
                login,
                audit,
                limit,
            },
            Later { pagination },
        ))
    }

    async fn write_scripts(&self, tasks: &Tasks) -> Result<(), SeedError> {
        let dir = ProjectPaths::new(&self.root).fold_dir().join(DEV_STUB_DIR);
        let files: Vec<(PathBuf, Vec<u8>)> = scripts::scripts(tasks)
            .into_iter()
            .map(|(agent, script)| {
                let text = serde_json::to_vec_pretty(&script).map_err(io::Error::other)?;
                Ok((dir.join(format!("{agent}.json")), text))
            })
            .collect::<io::Result<_>>()?;
        blocking(move || {
            for (path, text) in files {
                std::fs::write(path, text)?;
            }
            Ok(())
        })
        .await
    }

    /// A parent task whose checklist links two Done tasks and one step. The last tick completes it by the container rule, which is why the step is added first and ticked last. Nobody holds the parent, so the main agent changes its checklist.
    async fn container(&mut self, tasks: &Tasks) -> Result<(), SeedError> {
        let parent = self
            .task(
                "Ship the user dashboard",
                "The dashboard shows users with formatted dates.",
                &["frontend"],
                &[],
            )
            .await?;
        let step = self
            .main
            .call(
                "add_checklist_item",
                json!({"task": parent, "text": "Check the empty state"}),
            )
            .await?;
        for child in [&tasks.schema, &tasks.local_dates] {
            self.main
                .call(
                    "add_checklist_item",
                    json!({"task": parent, "linked_task": child}),
                )
                .await?;
        }
        self.main
            .call(
                "update_checklist_item",
                json!({"id": text(&step["item"], "add_checklist_item")?, "done": true}),
            )
            .await?;
        Ok(())
    }

    /// Waits until the review lead has tried its last integration, the counts proposal has its second version and no worker has a turn under way.
    async fn settled(&self, tasks: &Tasks) -> Result<(), SeedError> {
        self.wait_worker(REVIEW_LEAD, "the end of the review round", done)
            .await?;
        self.wait_task(
            &tasks.counts,
            "the second version of the counts proposal",
            |task| task["proposal"]["version"] == 2,
        )
        .await?;
        for name in [API_LEAD, WEB_LEAD, LIMIT_WORKER] {
            self.wait_worker(name, "the end of the leads' turns", done)
                .await?;
        }
        self.wait("the end of every worker's turn", async || {
            let workers = self.main.call("list_workers", json!({})).await?;
            let busy = workers["workers"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|worker| !matches!(worker["state"].as_str(), Some("idle" | "stopped")));
            Ok(!busy)
        })
        .await
    }

    async fn task(
        &mut self,
        title: &str,
        description: &str,
        labels: &[&str],
        needs: &[&String],
    ) -> Result<String, SeedError> {
        let made = self
            .main
            .call(
                "create_task",
                json!({"title": title, "description": description, "labels": labels, "needs": needs}),
            )
            .await?;
        self.tasks += 1;
        Ok(text(&made["id"], "create_task")?.to_owned())
    }

    async fn wait_task(
        &self,
        task: &str,
        what: &'static str,
        ready: impl Fn(&Value) -> bool,
    ) -> Result<(), SeedError> {
        self.wait(what, async || {
            let task = self.main.call("get_task", json!({"id": task})).await?;
            Ok(ready(&task))
        })
        .await
    }

    async fn wait_worker(
        &self,
        name: &str,
        what: &'static str,
        ready: impl Fn(&Value) -> bool,
    ) -> Result<(), SeedError> {
        self.wait(what, async || {
            let workers = self.main.call("list_workers", json!({})).await?;
            Ok(workers["workers"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|worker| worker["name"] == name && ready(worker)))
        })
        .await
    }

    /// Polls `ready` until it holds. A failed worker ends the wait with the step its stub could not do.
    async fn wait(
        &self,
        what: &'static str,
        ready: impl AsyncFn() -> Result<bool, SeedError>,
    ) -> Result<(), SeedError> {
        let started = Instant::now();
        loop {
            if ready().await? {
                return Ok(());
            }
            self.no_failed_worker().await?;
            if started.elapsed() > WAIT {
                return Err(SeedError::Timeout { what });
            }
            tokio::time::sleep(POLL).await;
        }
    }

    async fn no_failed_worker(&self) -> Result<(), SeedError> {
        let workers = self.main.call("list_workers", json!({})).await?;
        let failed = workers["workers"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|worker| worker["state"] == "failed")
            .and_then(|worker| worker["name"].as_str());
        let Some(agent) = failed else {
            return Ok(());
        };
        let log = ProjectPaths::new(&self.root)
            .fold_dir()
            .join("workers")
            .join(agent)
            .join("stub.log");
        let step = tokio::fs::read_to_string(log)
            .await
            .ok()
            .and_then(|text| text.lines().last().map(str::to_owned))
            .unwrap_or_else(|| NO_STEP.to_owned());
        Err(SeedError::Worker {
            agent: agent.to_owned(),
            step,
        })
    }

    /// A route of this project, as the human `dev`.
    async fn route(
        &self,
        method: Method,
        suffix: &str,
        body: Option<Value>,
    ) -> Result<Value, SeedError> {
        let path = format!("/api/v1/projects/{}{suffix}", self.slug);
        let response = self
            .human
            .request(method.clone(), &path, body.as_ref())
            .await?;
        if response.status >= 400 {
            let code = response
                .refusal()
                .code
                .unwrap_or_else(|| "unknown".to_owned());
            return Err(SeedError::Route { method, path, code });
        }
        Ok(response.body)
    }

    /// What the seed made: the tasks, the proposals, the workers and the name and size of each board column.
    async fn built(&self) -> Result<Built, SeedError> {
        let board = self.route(Method::GET, "/board", None).await?;
        let columns = board["columns"]
            .as_array()
            .ok_or(SeedError::Unexpected { tool: "get_board" })?;
        let proposals = self.route(Method::GET, "/proposals", None).await?;
        let workers = self
            .main
            .call("list_workers", json!({"include_stopped": true}))
            .await?;
        let length = |value: &Value| value.as_array().map_or(0, Vec::len);
        Ok(Built {
            tasks: self.tasks,
            proposals: length(&proposals["proposals"]),
            workers: length(&workers["workers"]),
            columns: columns
                .iter()
                .map(|column| {
                    (
                        column["status"].as_str().unwrap_or("unknown").to_owned(),
                        column["total"].as_u64().unwrap_or(0),
                    )
                })
                .collect(),
        })
    }
}

/// A worker whose turn ended and which has one behind it.
fn done(worker: &Value) -> bool {
    worker["state"] == "idle" && worker["turns"].as_u64().is_some_and(|turns| turns >= 1)
}

fn text<'a>(value: &'a Value, tool: &'static str) -> Result<&'a str, SeedError> {
    value.as_str().ok_or(SeedError::Unexpected { tool })
}

/// `text` with the 1-based `line` replaced; the text may hold several lines. A line the text does not have changes nothing.
fn replace_line(text: &str, line: usize, with: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    if let Some(slot) = line.checked_sub(1).and_then(|index| lines.get_mut(index)) {
        *slot = with;
    }
    let mut joined = lines.join("\n");
    joined.push('\n');
    joined
}

/// `text` with `added` after the 1-based `line`, or at the end when the text has fewer lines.
fn insert_after(text: &str, line: usize, added: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    lines.insert(line.min(lines.len()), added.trim_end_matches('\n'));
    let mut joined = lines.join("\n");
    joined.push('\n');
    joined
}

async fn blocking(work: impl FnOnce() -> io::Result<()> + Send + 'static) -> Result<(), SeedError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| SeedError::Files(io::Error::other(error)))?
        .map_err(SeedError::Files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_replaced_and_inserted_by_their_one_based_number() {
        assert_eq!(replace_line("a\nb\nc\n", 2, "x\ny"), "a\nx\ny\nc\n");
        assert_eq!(insert_after("a\nb\n", 1, "x\n"), "a\nx\nb\n");
        assert_eq!(insert_after("a\n", 1, "x\n"), "a\nx\n");
    }

    #[test]
    fn a_line_outside_the_text_never_panics() {
        assert_eq!(replace_line("a\n", 0, "x"), "a\n");
        assert_eq!(replace_line("a\n", 9, "x"), "a\n");
        assert_eq!(insert_after("a\n", 9, "x\n"), "a\nx\n");
        assert_eq!(insert_after("", 0, "x\n"), "x\n");
    }

    #[test]
    fn the_demo_settings_are_a_valid_project_configuration() {
        let config = fold_core::ProjectConfig::from_toml(CONFIG).unwrap();
        assert_eq!(config.workers.limits().task_worker, 8);
        assert_eq!(
            config.role(fold_core::Role::Reviewer).model.as_deref(),
            Some("fold-dev-stub")
        );
    }

    #[test]
    fn errors_have_their_codes_and_hints() {
        let error = SeedError::Exists {
            dir: PathBuf::from(".dev/projects/demo"),
        };
        assert_eq!(error.code(), Some("seed_exists"));
        assert_eq!(
            error.hint(),
            Some(
                "stop `make dev-daemon`, run `make clean-keep-cargo`, start `make dev-daemon` again, then run `make seed-dev`"
            )
        );
        assert_eq!(SeedError::NoSocket.code(), None);
        assert_eq!(SeedError::NoSocket.hint(), None);
        let refused = SeedError::Tool {
            tool: "claim_task",
            code: "already_claimed".to_owned(),
        };
        assert_eq!(refused.code(), Some("already_claimed"));
        let failed = SeedError::Worker {
            agent: "api-lead".to_owned(),
            step: "turn 0, step 1: get_task failed: task_not_found".to_owned(),
        };
        assert!(failed.hint().unwrap().contains("stub.log"));
        let without_stub = SeedError::Worker {
            agent: "api-lead".to_owned(),
            step: NO_STEP.to_owned(),
        };
        assert!(without_stub.hint().unwrap().contains("`dev-seed`"));
        assert!(failed.to_string().contains("api-lead"));
        assert!(SeedError::Timeout { what: "x" }.hint().is_some());
    }
}
