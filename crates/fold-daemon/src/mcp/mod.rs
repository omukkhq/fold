pub mod agents;
pub mod error;
pub mod events;
pub mod footer;
pub mod handshake;
pub mod identity;
pub mod messages;
pub mod reviews;
pub mod tasks;
pub mod workspaces;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use fold_core::{FoldError, codes};
use fold_store::agents::WorkerRow;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::services::agents::{AgentService, SessionContext};
use crate::services::events::EventService;
use crate::services::messages::MessageService;
use crate::services::reviews::ReviewService;
use crate::services::tasks::TaskService;
use crate::services::workspaces::WorkspaceService;
use error::{from_agent, tool_error};
pub use footer::{FooterCounts, FooterPart, FooterSource, footer};

/// The short workflow every session receives in the `initialize` answer.
pub const INSTRUCTIONS: &str = "fold coordinates the agents of one project. The main agent is the session fld claude starts with the main tag: it reads the project, resolves choices and gets the user's OK to a plan, then publishes its tasks, dependencies, parent links and human review marks together with create_task_plan. It starts area leads and the review lead with start_worker. Area leads start task workers, and the start gives each the claim on its tasks; the review lead never reviews: it gives every review to a reviewer it starts, which gets its review claim or batch from the start. Workers never claim; only the main agent claims a review, and an area lead or a task worker messages the review lead instead. Task workers work only on the tasks they were given, edit only their own task workspace, run required checks and send proposal evidence; no_proposal tasks use report_task without a workspace. Reviewers fix what the task's agreed scope settles in their own review checkout and send it with submit_review_fix, send work that the task still misses back with request_changes, describe new work outside the task with create_request (the proposal can still be approved), ask the humans with asks_human for a product decision or a conflict with a settled decision, and approve only inspected versions with check results. fold integrates ready approvals when automatic integration is enabled. Use fold messages between agents, keep your assigned role and authority, and read current state before retrying uncertain writes. A session opened by hand calls register_agent first. Load the fold skill for tool details. A footer beginning with \"fold:\" names work waiting for you.";

/// The MCP server of one adapter session. Every tool call passes through [`FoldServer::call_tool`], which checks the agent, resolves `as`, records the call and adds the footer, so a tool only parses its input and calls its service.
pub struct FoldServer {
    agents: AgentService,
    events: EventService,
    workspaces: WorkspaceService,
    tasks: TaskService,
    messages: MessageService,
    /// The proposal tools exist only on a server that has it.
    reviews: Option<ReviewService>,
    /// Held across the `register_agent` command, which binds the session.
    session: Mutex<SessionContext>,
    footer: Vec<Arc<dyn FooterSource>>,
}

impl std::fmt::Debug for FoldServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FoldServer").finish_non_exhaustive()
    }
}

impl FoldServer {
    /// Every server counts the unread messages, the new events and the claims near stale of its agent in the footer.
    pub fn new(
        agents: AgentService,
        events: EventService,
        workspaces: WorkspaceService,
        tasks: TaskService,
        messages: MessageService,
        session: SessionContext,
    ) -> Self {
        let unread: Arc<dyn FooterSource> = Arc::new(messages::UnreadFooter::new(
            agents.clone(),
            messages.clone(),
        ));
        let new_events: Arc<dyn FooterSource> =
            Arc::new(events::NewEventsFooter::new(agents.clone(), events.clone()));
        let near_stale: Arc<dyn FooterSource> = Arc::new(tasks::ClaimsNearStaleFooter::new(
            agents.clone(),
            tasks.clone(),
        ));
        Self {
            agents,
            events,
            workspaces,
            tasks,
            messages,
            reviews: None,
            session: Mutex::new(session),
            footer: vec![unread, new_events, near_stale],
        }
    }

    /// Adds a count to the footer of every result.
    #[must_use]
    pub fn with_footer_source(mut self, source: Arc<dyn FooterSource>) -> Self {
        self.footer.push(source);
        self
    }

    /// Serves the proposal and review tools through `reviews`, which in the daemon is `ProjectHandle::reviews`.
    #[must_use]
    pub fn with_reviews(mut self, reviews: ReviewService) -> Self {
        self.reviews = Some(reviews);
        self
    }

    async fn session(&self) -> SessionContext {
        self.session.lock().await.clone()
    }

    async fn run(
        &self,
        request: &CallToolRequestParams,
        context: &RequestContext<RoleServer>,
    ) -> Result<Option<Value>, FoldError> {
        let tool = request.name.as_ref();
        let arguments = request.arguments.clone().unwrap_or_default();
        let before = self.session().await;
        // Every tool but the identity tools needs an agent, so its worker is known before it runs.
        let resolved = if identity::is_identity_tool(tool) {
            None
        } else {
            Some(self.resolve(&before).await?)
        };
        let call = Call {
            worker: resolved.clone(),
            actor: Arc::default(),
        };
        let value = CALL
            .scope(
                call,
                self.dispatch(tool, resolved.as_ref(), &arguments, context),
            )
            .await?;
        let Some(value) = value else {
            return Ok(None);
        };
        let after = self.session().await;
        if after.agent.is_some() {
            let worker = match resolved {
                Some(worker) => worker,
                None => self.resolve(&after).await?,
            };
            if let Err(error) = self.agents.record_call(&after, &worker, tool, None).await {
                tracing::warn!(
                    tool,
                    code = error.log_code(),
                    "could not record a tool call"
                );
            }
        }
        Ok(Some(value))
    }

    async fn dispatch(
        &self,
        tool: &str,
        resolved: Option<&WorkerRow>,
        arguments: &JsonObject,
        context: &RequestContext<RoleServer>,
    ) -> Result<Option<Value>, FoldError> {
        let value = match (tool, resolved) {
            (identity::REGISTER_AGENT, _) => identity::register_agent(self, arguments).await?,
            (agents::LIST_AGENTS, _) => agents::list_agents(self, arguments).await?,
            (agents::GET_ACTIVITY, Some(_)) => agents::get_activity(self, arguments).await?,
            (agents::DEFINE_AREA, Some(_)) => agents::define_area(self, arguments).await?,
            (agents::UPDATE_AREA, Some(_)) => agents::update_area(self, arguments).await?,
            (agents::START_WORKER, Some(_)) => agents::start_worker(self, arguments).await?,
            (agents::STOP_WORKER, Some(_)) => agents::stop_worker(self, arguments).await?,
            (agents::LIST_WORKERS, Some(_)) => agents::list_workers(self, arguments).await?,
            (agents::SET_WORKER_LIMITS, Some(_)) => {
                agents::set_worker_limits(self, arguments).await?
            }
            (agents::PAUSE_PROJECT, Some(_)) => agents::pause_project(self, arguments).await?,
            (agents::RESUME_PROJECT, Some(_)) => agents::resume_project(self, arguments).await?,
            (agents::WRITE_HANDOFF, Some(_)) => agents::write_handoff(self, arguments).await?,
            (agents::RESTART_WORKER, Some(_)) => agents::restart_worker(self, arguments).await?,
            (events::GET_EVENTS, Some(worker)) => {
                events::get_events(self, worker, arguments).await?
            }
            (events::WAIT_FOR_EVENTS, Some(worker)) => {
                events::wait_for_events(self, worker, arguments, context).await?
            }
            (messages::ACKNOWLEDGE_MESSAGE, Some(_)) => {
                messages::acknowledge_message(self, arguments).await?
            }
            (messages::GET_INBOX, Some(_)) => messages::get_inbox(self, arguments).await?,
            (messages::GET_THREAD, Some(_)) => messages::get_thread(self, arguments).await?,
            (messages::SEND_MESSAGE, Some(_)) => messages::send_message(self, arguments).await?,
            (messages::MARK_ANSWERED, Some(_)) => messages::mark_answered(self, arguments).await?,
            (workspaces::GET_REPOSITORY_STATE, Some(_)) => {
                workspaces::get_repository_state(self, arguments).await?
            }
            (workspaces::GET_REVISION, Some(_)) => {
                workspaces::get_revision(self, arguments).await?
            }
            (workspaces::COMPARE_REVISIONS, Some(_)) => {
                workspaces::compare_revisions(self, arguments).await?
            }
            (_, Some(_)) => {
                let value = match domain_of(tool) {
                    Some(Domain::Workspaces) => workspaces::call(self, tool, arguments).await?,
                    Some(Domain::Tasks) => tasks::call(self, tool, arguments).await?,
                    Some(Domain::Reviews) => reviews::call(self, tool, arguments).await?,
                    None => None,
                };
                match value {
                    Some(value) => value,
                    None => return Ok(None),
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(value))
    }

    /// The acting agent of the current call. The call resolved its worker before the tool ran, so only the agent's tags are read here, once per call.
    async fn actor(&self) -> Result<fold_core::Actor, FoldError> {
        let cached = CALL
            .try_with(|call| {
                call.worker
                    .clone()
                    .map(|worker| (worker, Arc::clone(&call.actor)))
            })
            .ok()
            .flatten();
        let Some((worker, actor)) = cached else {
            let session = self.session().await;
            return self.agents.actor(&session).await.map_err(from_agent);
        };
        actor
            .get_or_try_init(async || self.agents.actor_of(&worker).await)
            .await
            .cloned()
            .map_err(from_agent)
    }

    async fn resolve(&self, session: &SessionContext) -> Result<WorkerRow, FoldError> {
        self.agents
            .resolve_worker(session)
            .await
            .map_err(from_agent)
    }

    /// Adds to `counts` the sources taken before the call when `before_call` is true, the others when it is false.
    async fn counts(&self, before_call: bool, mut counts: FooterCounts) -> FooterCounts {
        let session = self.session().await;
        for source in self
            .footer
            .iter()
            .filter(|s| s.before_call() == before_call)
        {
            let part = source.part();
            let count = source.count(&session).await;
            counts = counts.with(part, counts.get(part) + count);
        }
        counts
    }
}

tokio::task_local! {
    /// The worker the current tool call acts as, scoped to the call's task instead of passed through every tool's signature.
    static CALL: Call;
}

struct Call {
    /// `None` for an identity tool.
    worker: Option<WorkerRow>,
    /// The agent's tags are read the first time a tool asks for its actor.
    actor: Arc<tokio::sync::OnceCell<fold_core::Actor>>,
}

/// The module that serves a tool which the dispatch does not name itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Domain {
    Workspaces,
    Tasks,
    Reviews,
}

/// Built once from the modules' tool lists, so a call looks its tool up instead of building every list.
fn domain_of(tool: &str) -> Option<Domain> {
    static DOMAINS: LazyLock<HashMap<String, Domain>> = LazyLock::new(|| {
        [
            (Domain::Workspaces, workspaces::tools()),
            (Domain::Tasks, tasks::tools()),
            (Domain::Tasks, tasks::requests::tools()),
            (Domain::Reviews, reviews::tools()),
        ]
        .into_iter()
        .flat_map(|(domain, tools)| {
            tools
                .into_iter()
                .map(move |tool| (tool.name.into_owned(), domain))
        })
        .collect()
    });
    DOMAINS.get(tool).copied()
}

/// The arguments as the tool's input type. The message never repeats the arguments.
pub(crate) fn parse<T: for<'de> Deserialize<'de>>(arguments: &JsonObject) -> Result<T, FoldError> {
    serde_json::from_value(Value::Object(arguments.clone())).map_err(|_| {
        FoldError::new(
            codes::INVALID_REQUEST,
            "the arguments do not match the tool's input schema",
        )
    })
}

/// The `initialize` instructions of [`OutsideProject`].
pub const OUTSIDE_INSTRUCTIONS: &str = "This folder is in no fold project, so fold offers no tools here. Run fld init in a folder to make it a fold project, then start Claude Code there.";

/// The MCP server of a session outside every fold project, which the user's Claude Code configuration starts in any folder: it answers `initialize` with no capabilities, so the session lists no fold tools and shows no error.
pub struct OutsideProject;

impl ServerHandler for OutsideProject {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::default())
            .with_server_info(Implementation::new("fold", env!("CARGO_PKG_VERSION")))
            .with_instructions(OUTSIDE_INSTRUCTIONS)
    }
}

impl OutsideProject {
    /// Serves the session on the two streams until the client closes its side.
    pub async fn serve<R, W>(read: R, write: W)
    where
        R: tokio::io::AsyncRead + Send + Unpin + 'static,
        W: tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        use rmcp::ServiceExt;
        if let Ok(running) = Self.serve((read, write)).await {
            let _ = running.waiting().await;
        }
    }
}

impl ServerHandler for FoldServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("fold", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut tools = identity::tools();
        tools.extend(agents::tools());
        tools.extend(events::tools());
        tools.extend(workspaces::tools());
        tools.extend(tasks::tools());
        tools.extend(tasks::requests::tools());
        tools.extend(messages::tools());
        if self.reviews.is_some() {
            tools.extend(reviews::tools());
        }
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let arrived = self.counts(true, FooterCounts::default()).await;
        let mut result = match self.run(&request, &context).await {
            Ok(Some(value)) => CallToolResult::success(vec![ContentBlock::text(value.to_string())]),
            // The specification answers an unknown tool with a JSON-RPC error; its data carries the hint a tool error would.
            Ok(None) => {
                return Err(ErrorData::invalid_params(
                    "there is no tool with this name",
                    Some(serde_json::json!({
                        "hint": "call tools/list to see the tools of this server"
                    })),
                ));
            }
            Err(error) => tool_error(&error),
        };
        if let Some(line) = footer(&self.counts(false, arrived).await) {
            result.content.push(ContentBlock::text(line));
        }
        Ok(result.into())
    }
}
