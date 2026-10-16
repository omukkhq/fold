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

use fold_core::{FoldError, WorkerName, codes};
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
pub const INSTRUCTIONS: &str = "fold coordinates the agents of one project. Work in this order: 1. Call register_agent with your agent name once per session. 2. Read the work that is ready. 3. Claim one task before you change files for it. 4. Work only in your own task workspace. 5. Send a proposal when the work is ready for review. A line that starts with \"fold:\" at the end of a result tells you what waits for you.";

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
        let as_worker = as_worker(&arguments)?;
        let before = self.session().await;
        // Every tool but the identity tools needs an agent, so its worker is known before it runs. An identity tool on an unbound session has no agent to resolve `as` against: `register_agent` checks it as it binds, and the others need no worker.
        let resolved = if !identity::is_identity_tool(tool)
            || (before.agent.is_some() && as_worker.is_some())
        {
            Some(self.resolve(&before, as_worker.as_ref()).await?)
        } else {
            None
        };
        let call = Call {
            as_worker: as_worker.clone(),
            worker: resolved.clone(),
            actor: Arc::default(),
        };
        let value = CALL
            .scope(
                call,
                self.dispatch(
                    tool,
                    as_worker.as_ref(),
                    resolved.as_ref(),
                    &arguments,
                    context,
                ),
            )
            .await?;
        let Some(value) = value else {
            return Ok(None);
        };
        let after = self.session().await;
        if after.agent.is_some() {
            let worker = match resolved {
                Some(worker) => worker,
                None => self.resolve(&after, as_worker.as_ref()).await?,
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
        as_worker: Option<&WorkerName>,
        resolved: Option<&WorkerRow>,
        arguments: &JsonObject,
        context: &RequestContext<RoleServer>,
    ) -> Result<Option<Value>, FoldError> {
        let value = match (tool, resolved) {
            (identity::REGISTER_AGENT, _) => identity::register_agent(self, arguments).await?,
            (identity::REGISTER_WORKER, _) => identity::register_worker(self, arguments).await?,
            (agents::LIST_AGENTS, _) => agents::list_agents(self, arguments).await?,
            (agents::GET_ACTIVITY, Some(_)) => agents::get_activity(self, arguments).await?,
            (agents::DEFINE_AREA, Some(_)) => {
                agents::define_area(self, as_worker, arguments).await?
            }
            (agents::UPDATE_AREA, Some(_)) => {
                agents::update_area(self, as_worker, arguments).await?
            }
            (agents::START_WORKER, Some(_)) => {
                agents::start_worker(self, as_worker, arguments).await?
            }
            (agents::STOP_WORKER, Some(_)) => {
                agents::stop_worker(self, as_worker, arguments).await?
            }
            (agents::LIST_WORKERS, Some(_)) => agents::list_workers(self, arguments).await?,
            (agents::SET_WORKER_LIMITS, Some(_)) => {
                agents::set_worker_limits(self, as_worker, arguments).await?
            }
            (agents::WRITE_HANDOFF, Some(_)) => {
                agents::write_handoff(self, as_worker, arguments).await?
            }
            (events::GET_EVENTS, Some(worker)) => {
                events::get_events(self, worker, arguments).await?
            }
            (events::WAIT_FOR_EVENTS, Some(worker)) => {
                events::wait_for_events(self, worker, arguments, context).await?
            }
            (messages::ACKNOWLEDGE_MESSAGE, Some(worker)) => {
                messages::acknowledge_message(self, worker, arguments).await?
            }
            (messages::GET_INBOX, Some(worker)) => {
                messages::get_inbox(self, worker, arguments).await?
            }
            (messages::GET_THREAD, Some(_)) => messages::get_thread(self, arguments).await?,
            (messages::SEND_MESSAGE, Some(worker)) => {
                messages::send_message(self, worker, arguments).await?
            }
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
                    Some(Domain::Workspaces) => {
                        workspaces::call(self, tool, as_worker, arguments).await?
                    }
                    Some(Domain::Tasks) => tasks::call(self, tool, as_worker, arguments).await?,
                    Some(Domain::Reviews) => {
                        reviews::call(self, tool, as_worker, arguments).await?
                    }
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

    /// The acting agent of the current call, its worker resolved from `as`. The call resolved its worker before the tool ran, so only the agent's tags are read here, once per call.
    async fn actor(&self, as_worker: Option<&WorkerName>) -> Result<fold_core::Actor, FoldError> {
        let cached = CALL
            .try_with(|call| {
                call.worker
                    .clone()
                    .filter(|_| call.serves(as_worker))
                    .map(|worker| (worker, Arc::clone(&call.actor)))
            })
            .ok()
            .flatten();
        let Some((worker, actor)) = cached else {
            let session = self.session().await;
            return self
                .agents
                .actor(&session, as_worker)
                .await
                .map_err(from_agent);
        };
        actor
            .get_or_try_init(async || self.agents.actor_of(&worker).await)
            .await
            .cloned()
            .map_err(from_agent)
    }

    async fn resolve(
        &self,
        session: &SessionContext,
        as_worker: Option<&WorkerName>,
    ) -> Result<WorkerRow, FoldError> {
        self.agents
            .resolve_worker(session, as_worker)
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
            counts = counts.with(part, counts.get(part) + source.count(&session).await);
        }
        counts
    }
}

tokio::task_local! {
    /// The worker the current tool call acts as. The tool modules ask for their actor by `as`, so it is scoped to the call's task instead of passed through every tool's signature.
    static CALL: Call;
}

struct Call {
    as_worker: Option<WorkerName>,
    /// `None` for an identity tool on an unbound session.
    worker: Option<WorkerRow>,
    /// The agent's tags are read the first time a tool asks for its actor.
    actor: Arc<tokio::sync::OnceCell<fold_core::Actor>>,
}

impl Call {
    /// True when `as_worker` names the worker this call resolved: the same `as`, or that worker by its full name.
    fn serves(&self, as_worker: Option<&WorkerName>) -> bool {
        as_worker == self.as_worker.as_ref()
            || self
                .worker
                .as_ref()
                .is_some_and(|worker| as_worker == Some(&worker.name))
    }
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

fn as_worker(arguments: &JsonObject) -> Result<Option<WorkerName>, FoldError> {
    match arguments.get("as") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(name)) => Ok(Some(WorkerName::parse(name)?)),
        Some(_) => Err(FoldError::new(
            codes::INVALID_REQUEST,
            "as must be a worker name",
        )),
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
