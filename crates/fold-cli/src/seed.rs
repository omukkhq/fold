mod agent;
mod sample;

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use fold_cli::daemon_client::{ClientConfig, ClientError, DaemonClient, HandshakeError};
use fold_core::{EventActor, ProjectPaths, codes};
use hyper::Method;
use serde_json::{Value, json};
use thiserror::Error;

use crate::init::{self, InitArgs, InitError};
use crate::output;
use crate::serve::{self, Dirs, ServeError};
use agent::Agent;

/// The human the seed acts as; `make seed-dev` sets `FOLD_HUMAN` to the same name.
const HUMAN: &str = "dev";

#[derive(Args, Debug)]
pub struct DevArgs {
    #[command(subcommand)]
    command: DevCommand,
}

#[derive(Subcommand, Debug)]
enum DevCommand {
    /// Build a throwaway demo project through the tools and routes a real client uses
    Seed(SeedArgs),
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
                "stop make dev-daemon, run make clean-keep-cargo, start make dev-daemon again, then run make seed-dev",
            ),
            _ => None,
        }
    }
}

pub fn run(args: &DevArgs, dirs: &Dirs, out: &mut impl Write) -> Result<(), SeedError> {
    match &args.command {
        DevCommand::Seed(args) => seed(args, dirs, out),
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
        from_git: false,
        with_git: true,
        socket: Some(socket.clone()),
        refresh: false,
        git_name: Some("Demo".to_owned()),
        git_email: Some("demo@example.com".to_owned()),
    };
    let slug = init::initialise(&dir, &init, dirs, &mut io::sink())?;
    let root = dir.canonicalize()?;
    // The scenario integrates each proposal at the step that needs it and keeps approved proposals open for the board.
    let config = ProjectPaths::new(&root).config();
    let mut text = std::fs::read_to_string(&config)?;
    text.push_str("\n[integration]\nautomatic = false\n");
    std::fs::write(&config, text)?;

    let runtime = serve::current_thread_runtime().map_err(SeedError::Runtime)?;
    let built = runtime.block_on(build(&socket, &root, &slug))?;

    let mut write = |text: String| output::line(out, &text).map_err(SeedError::Write);
    write(format!("seeded project {slug} at {}", root.display()))?;
    write(format!(
        "{} tasks, {} workspaces, {} proposals, {} messages, 3 agents and 1 worker",
        built.counts.tasks, built.counts.workspaces, built.counts.proposals, built.counts.messages
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

#[derive(Debug, Default)]
struct Counts {
    tasks: u32,
    workspaces: u32,
    proposals: u32,
    messages: u32,
}

struct Built {
    counts: Counts,
    columns: Vec<(String, u64)>,
}

async fn build(socket: &Path, root: &Path, slug: &str) -> Result<Built, SeedError> {
    let config = ClientConfig::running(socket.to_owned());
    let mut seeder = Seeder {
        root: root.to_owned(),
        slug: slug.to_owned(),
        human: DaemonClient::new(config, Some(HUMAN.to_owned())),
        frontend: Agent::connect(socket, root, "frontend", &["frontend"]).await?,
        backend: Agent::connect(socket, root, "backend", &["backend"]).await?,
        // `review` is the project's main agent, the only session opened without a role that may approve and integrate.
        review: Agent::connect(socket, root, "review", &["main", "review"]).await?,
        counts: Counts::default(),
    };
    let outcome = seeder.scenario().await;
    let columns = match outcome {
        Ok(()) => seeder.columns().await,
        Err(error) => Err(error),
    };
    let Seeder {
        frontend,
        backend,
        review,
        counts,
        ..
    } = seeder;
    frontend.close().await;
    backend.close().await;
    review.close().await;
    Ok(Built {
        counts,
        columns: columns?,
    })
}

/// A task workspace and the directory its worker edits.
struct Workspace {
    id: String,
    dir: PathBuf,
}

/// The ids the later steps need.
struct Tasks {
    schema: String,
    routes: String,
    local_dates: String,
    relative_dates: String,
    counts: String,
    login: String,
    audit: String,
    limit: String,
    pagination: String,
}

struct Seeder {
    root: PathBuf,
    slug: String,
    human: DaemonClient,
    frontend: Agent,
    backend: Agent,
    review: Agent,
    counts: Counts,
}

impl Seeder {
    async fn scenario(&mut self) -> Result<(), SeedError> {
        self.frontend
            .call("register_worker", json!({"name": "ui"}))
            .await?;
        let tasks = self.plan().await?;
        self.finish_schema(&tasks).await?;
        self.work_on_routes(&tasks).await?;
        self.review_round(&tasks).await?;
        self.container(&tasks).await?;
        self.talk(&tasks).await
    }

    /// Labels, and every task with its edges. Each column is reached later by the work done on the tasks.
    async fn plan(&mut self) -> Result<Tasks, SeedError> {
        for (name, color) in [
            ("backend", "sage"),
            ("frontend", "slate"),
            ("infra", "teal"),
        ] {
            self.frontend
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
        self.task(
            "Write the deployment guide",
            "Describe how to run the service on a small server.",
            &["infra"],
            &[],
        )
        .await?;
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
                &["backend", "frontend"],
                &[&export],
            )
            .await?;
        self.frontend
            .call(
                "add_dependency",
                json!({"task": search, "needs": pagination}),
            )
            .await?;
        Ok(Tasks {
            schema,
            routes,
            local_dates,
            relative_dates,
            counts,
            login,
            audit,
            limit,
            pagination,
        })
    }

    /// The first task is reviewed and integrated, which unblocks the routes.
    async fn finish_schema(&mut self, tasks: &Tasks) -> Result<(), SeedError> {
        let backend = self.backend.name();
        let workspace = self.start(backend, &tasks.schema, None).await?;
        edit(&workspace, "src/db.js", |text| {
            insert_after(&text, 6, "  tables.set(\"users\", []);\n")
        })
        .await?;
        let proposal = self
            .propose(
                backend,
                &workspace,
                &tasks.schema,
                "Create the users table",
                "The service starts and the users table exists.",
                None,
            )
            .await?;
        self.approve(&proposal, 1, "The table and the helper look right.")
            .await?;
        self.integrate(&proposal, 1).await
    }

    /// The routes task stays In Progress with recorded and unrecorded changes in its workspace.
    async fn work_on_routes(&mut self, tasks: &Tasks) -> Result<(), SeedError> {
        let backend = self.backend.name();
        let workspace = self.start(backend, &tasks.routes, None).await?;
        edit(&workspace, "src/users.js", |_| {
            "export function listUsers(db) {\n  return db.all(\"users\");\n}\n\nexport function createUser(db, user) {\n  return db.insert(\"users\", user);\n}\n".to_owned()
        })
        .await?;
        edit(&workspace, "src/routes.js", |text| {
            insert_after(
                &text,
                4,
                "    \"GET /users\": async () => ({ status: 200, body: await db.all(\"users\") }),\n",
            )
        })
        .await?;
        remove(&workspace, "docs/changelog.md").await?;
        self.backend
            .call(
                "record_checkpoint",
                json!({"ws": workspace.id, "label": "Handlers listed"}),
            )
            .await?;
        edit(&workspace, "src/users.js", |text| {
            format!("{text}\nexport function removeUser(db, id) {{\n  return db.remove(\"users\", id);\n}}\n")
        })
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
        self.counts.messages += 1;
        Ok(())
    }

    /// Four open proposals, an integrated one and a conflicted one, all from the same base.
    async fn review_round(&mut self, tasks: &Tasks) -> Result<(), SeedError> {
        let frontend = self.frontend.name();
        let backend = self.backend.name();
        let local = self.start(frontend, &tasks.local_dates, None).await?;
        let relative = self.start(backend, &tasks.relative_dates, None).await?;
        let counts = self.start(frontend, &tasks.counts, None).await?;
        let login = self
            .start(frontend, &tasks.login, Some("frontend/ui"))
            .await?;
        let audit = self.start(backend, &tasks.audit, None).await?;
        let limit = self.start(backend, &tasks.limit, None).await?;

        edit(&local, "src/format.js", |text| {
            replace_line(
                &text,
                4,
                "  return new Date(value).toLocaleDateString(\"en-GB\");",
            )
        })
        .await?;
        edit(&relative, "src/format.js", |text| {
            replace_line(
                &text,
                4,
                "  return relativeTime(Date.now() - new Date(value));",
            )
        })
        .await?;
        edit(&counts, "src/format.js", |text| {
            replace_line(&text, 8, "  return count.toLocaleString(\"en-US\");")
        })
        .await?;
        edit(&login, "src/app.js", |text| {
            replace_line(
                &text,
                6,
                "  const handler = routes(db);\n  const form = {\n    fields: [\"name\", \"password\"],\n    submit: (values) => db.insert(\"sessions\", values),\n  };\n  handler.login = form;",
            )
        })
        .await?;
        edit(&audit, "src/audit.js", |_| {
            "export function record(log, who, what) {\n  log.push({ who, what });\n}\n".to_owned()
        })
        .await?;
        edit(&limit, "src/routes.js", |text| {
            format!("{text}\nexport function limited(counts, client, max) {{\n  return (counts.get(client) ?? 0) >= max;\n}}\n")
        })
        .await?;

        let local_proposal = self
            .propose(
                frontend,
                &local,
                &tasks.local_dates,
                "Show dates in the local time zone",
                "Checked in the list view with two time zones.",
                None,
            )
            .await?;
        let relative_proposal = self
            .propose(
                backend,
                &relative,
                &tasks.relative_dates,
                "Show dates as relative text",
                "Checked with items from the last day.",
                None,
            )
            .await?;
        let counts_proposal = self
            .propose(
                frontend,
                &counts,
                &tasks.counts,
                "Format counts with separators",
                "Checked with counts above one thousand.",
                None,
            )
            .await?;
        let login_proposal = self
            .propose(
                frontend,
                &login,
                &tasks.login,
                "Add the login form",
                "The form renders and submits.",
                Some("frontend/ui"),
            )
            .await?;
        let audit_proposal = self
            .propose(
                backend,
                &audit,
                &tasks.audit,
                "Record changes in an audit log",
                "Each write adds one entry.",
                None,
            )
            .await?;
        // The limit task already has a workspace when it gets a prerequisite, so its card shows the blocked indicator.
        self.frontend
            .call(
                "add_dependency",
                json!({"task": tasks.limit, "needs": tasks.pagination}),
            )
            .await?;

        // Two proposals changed the same line of one file. The first integrates; the second gets its conflict report.
        self.approve(&local_proposal, 1, "Reads well.").await?;
        self.approve(&relative_proposal, 1, "Reads well.").await?;
        self.integrate(&local_proposal, 1).await?;
        let refused = self
            .review
            .refused(
                "integrate_proposal",
                json!({"id": relative_proposal, "expected_version": 1}),
            )
            .await?;
        if refused.code != "integration_conflicted" {
            return Err(SeedError::Unexpected {
                tool: "integrate_proposal",
            });
        }

        // The counts proposal gets a second version after a change request.
        self.review
            .call("claim_review", json!({"id": counts_proposal}))
            .await?;
        self.review
            .call(
                "submit_review",
                json!({
                    "id": counts_proposal,
                    "version": 1,
                    "verdict": "request_changes",
                    "body": "Keep the separator for small counts too.",
                    "comments": [{
                        "path": "src/format.js",
                        "start_line": 8,
                        "end_line": 8,
                        "side": "new",
                        "body": "This is right for large counts. What about counts below one thousand?"
                    }],
                }),
            )
            .await?;
        edit(&counts, "src/format.js", |text| {
            insert_after(&text, 6, "// Counts use a separator at every size.\n")
        })
        .await?;
        self.update(
            frontend,
            &counts_proposal,
            "Checked with counts from 1 to 10000000.",
        )
        .await?;

        // The login form stays with its author after a review with one range comment and two single-line comments.
        self.review
            .call("claim_review", json!({"id": login_proposal}))
            .await?;
        self.review
            .call(
                "submit_review",
                json!({
                    "id": login_proposal,
                    "version": 1,
                    "verdict": "request_changes",
                    "body": "The form needs a failure path and a clearer name for the handler.",
                    "comments": [
                        {
                            "path": "src/app.js",
                            "start_line": 6,
                            "end_line": 10,
                            "side": "new",
                            "body": "The submit function writes the password into the session row. Store a hash instead."
                        },
                        {
                            "path": "src/app.js",
                            "start_line": 12,
                            "end_line": 12,
                            "side": "new",
                            "body": "Name the server after what it serves."
                        },
                        {
                            "path": "src/app.js",
                            "start_line": 7,
                            "end_line": 7,
                            "side": "old",
                            "body": "Keep creating the server after the handler is built."
                        }
                    ],
                }),
            )
            .await?;

        // A human has to look at the audit log before it integrates. The agent approves it, which puts it on the human's screen, and the human's approval stays missing.
        self.review
            .call(
                "require_human_review",
                json!({
                    "task": tasks.audit,
                    "reviewer": HUMAN,
                    "reason": "The audit log records personal data.",
                }),
            )
            .await?;
        self.approve(
            &audit_proposal,
            1,
            "Each write adds one entry, as the evidence says.",
        )
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
            .review
            .call(
                "add_checklist_item",
                json!({"task": parent, "text": "Check the empty state"}),
            )
            .await?;
        for child in [&tasks.schema, &tasks.local_dates] {
            self.review
                .call(
                    "add_checklist_item",
                    json!({"task": parent, "linked_task": child}),
                )
                .await?;
        }
        self.review
            .call(
                "update_checklist_item",
                json!({"id": text(&step["item"], "add_checklist_item")?, "done": true}),
            )
            .await?;
        Ok(())
    }

    /// Two messages between agents, and a question to the human that gives the limit task its `question` item.
    async fn talk(&mut self, tasks: &Tasks) -> Result<(), SeedError> {
        self.backend
            .call(
                "send_message",
                json!({
                    "to": ["frontend"],
                    "body": "The user routes are in progress; the form can post to /users.",
                    "task": tasks.routes,
                }),
            )
            .await?;
        self.frontend
            .call(
                "send_message",
                json!({
                    "to": ["backend"],
                    "body": "Thanks. I will connect the login form once the first change lands.",
                    "task": tasks.login,
                }),
            )
            .await?;
        self.backend
            .call(
                "send_message",
                json!({
                    "to": [format!("{}{HUMAN}", EventActor::HUMAN_PREFIX)],
                    "body": "Should the limit apply per client address or per token?",
                    "task": tasks.limit,
                }),
            )
            .await?;
        self.counts.messages += 3;
        Ok(())
    }

    async fn task(
        &mut self,
        title: &str,
        description: &str,
        labels: &[&str],
        needs: &[&String],
    ) -> Result<String, SeedError> {
        let made = self
            .frontend
            .call(
                "create_task",
                json!({"title": title, "description": description, "labels": labels, "needs": needs}),
            )
            .await?;
        self.counts.tasks += 1;
        Ok(text(&made["id"], "create_task")?.to_owned())
    }

    async fn start(
        &mut self,
        who: &str,
        task: &str,
        as_worker: Option<&str>,
    ) -> Result<Workspace, SeedError> {
        let made = self
            .agent(who)
            .call(
                "create_workspace",
                with_worker(json!({"task": task}), as_worker),
            )
            .await?;
        self.counts.workspaces += 1;
        let id = text(&made["workspace"], "create_workspace")?.to_owned();
        let dir = self.root.join(text(&made["path"], "create_workspace")?);
        Ok(Workspace { id, dir })
    }

    async fn propose(
        &mut self,
        who: &str,
        workspace: &Workspace,
        task: &str,
        title: &str,
        evidence: &str,
        as_worker: Option<&str>,
    ) -> Result<String, SeedError> {
        let agent = self.agent(who);
        agent
            .call(
                "record_changes",
                with_worker(json!({"ws": workspace.id, "message": title}), as_worker),
            )
            .await?;
        let made = agent
            .call(
                "create_proposal",
                with_worker(
                    json!({"task": task, "title": title, "evidence": evidence}),
                    as_worker,
                ),
            )
            .await?;
        self.counts.proposals += 1;
        Ok(text(&made["id"], "create_proposal")?.to_owned())
    }

    async fn update(&self, who: &str, proposal: &str, evidence: &str) -> Result<(), SeedError> {
        self.agent(who)
            .call(
                "update_proposal",
                json!({"id": proposal, "evidence": evidence}),
            )
            .await?;
        Ok(())
    }

    async fn approve(&self, proposal: &str, version: u32, body: &str) -> Result<(), SeedError> {
        self.review
            .call("claim_review", json!({"id": proposal}))
            .await?;
        self.review
            .call(
                "submit_review",
                json!({"id": proposal, "version": version, "verdict": "approve", "body": body}),
            )
            .await?;
        Ok(())
    }

    async fn integrate(&self, proposal: &str, version: u32) -> Result<(), SeedError> {
        self.review
            .call(
                "integrate_proposal",
                json!({"id": proposal, "expected_version": version}),
            )
            .await?;
        Ok(())
    }

    fn agent(&self, name: &str) -> &Agent {
        if name == self.backend.name() {
            &self.backend
        } else {
            &self.frontend
        }
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

    /// The name and size of each board column.
    async fn columns(&self) -> Result<Vec<(String, u64)>, SeedError> {
        let board = self.route(Method::GET, "/board", None).await?;
        let columns = board["columns"]
            .as_array()
            .ok_or(SeedError::Unexpected { tool: "get_board" })?;
        Ok(columns
            .iter()
            .map(|column| {
                (
                    column["status"].as_str().unwrap_or("unknown").to_owned(),
                    column["total"].as_u64().unwrap_or(0),
                )
            })
            .collect())
    }
}

fn with_worker(mut arguments: Value, as_worker: Option<&str>) -> Value {
    if let Some(worker) = as_worker {
        arguments["as"] = json!(worker);
    }
    arguments
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

/// Rewrites a file of the workspace the way an agent's editor would, creating it when it is new.
async fn edit(
    workspace: &Workspace,
    path: &str,
    change: impl FnOnce(String) -> String + Send + 'static,
) -> Result<(), SeedError> {
    let file = workspace.dir.join(path);
    blocking(move || {
        let current = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error),
        };
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&file, change(current))
    })
    .await
}

async fn remove(workspace: &Workspace, path: &str) -> Result<(), SeedError> {
    let file = workspace.dir.join(path);
    blocking(move || std::fs::remove_file(file)).await
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

    #[tokio::test]
    async fn an_edit_creates_a_missing_file_and_stops_at_an_unreadable_one() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace {
            id: "ws-1".to_owned(),
            dir: dir.path().to_owned(),
        };
        edit(&workspace, "new/a.txt", |text| format!("{text}x"))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("new/a.txt")).unwrap(),
            "x"
        );
        let error = edit(&workspace, "new", |text| text).await.unwrap_err();
        assert!(matches!(error, SeedError::Files(_)));
        assert!(dir.path().join("new/a.txt").is_file());
    }

    #[test]
    fn a_line_outside_the_text_never_panics() {
        assert_eq!(replace_line("a\n", 0, "x"), "a\n");
        assert_eq!(replace_line("a\n", 9, "x"), "a\n");
        assert_eq!(insert_after("a\n", 9, "x\n"), "a\nx\n");
        assert_eq!(insert_after("", 0, "x\n"), "x\n");
    }

    #[test]
    fn a_tool_argument_names_the_worker_only_when_one_is_given() {
        assert_eq!(with_worker(json!({"id": 1}), None), json!({"id": 1}));
        assert_eq!(
            with_worker(json!({"id": 1}), Some("frontend/ui")),
            json!({"id": 1, "as": "frontend/ui"})
        );
    }

    #[test]
    fn an_existing_directory_has_a_code_and_a_hint() {
        let error = SeedError::Exists {
            dir: PathBuf::from(".dev/projects/demo"),
        };
        assert_eq!(error.code(), Some("seed_exists"));
        assert_eq!(
            error.hint(),
            Some(
                "stop make dev-daemon, run make clean-keep-cargo, start make dev-daemon again, then run make seed-dev"
            )
        );
        assert_eq!(SeedError::NoSocket.code(), None);
        assert_eq!(SeedError::NoSocket.hint(), None);
        let refused = SeedError::Tool {
            tool: "claim_task",
            code: "already_claimed".to_owned(),
        };
        assert_eq!(refused.code(), Some("already_claimed"));
    }
}
