use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One line Claude Code writes on stdout when fold runs it as `claude -p --input-format stream-json --output-format stream-json --verbose --session-id <uuid> --model <name> --permission-mode auto --permission-prompt-tool stdio --mcp-config <file> --append-system-prompt-file <file>`, with `--strict-mcp-config` only for a role that limits its MCP servers.
///
/// Facts from the probe against Claude Code 2.1.301: `--permission-prompt-tool stdio` is accepted although `--help` lists only `--permission-prompts <host|none>` (default `host`); `--permission-mode auto` works under `-p`; `system/init` comes again at the start of every user turn, not once per process; hook lines (`system/hook_started`, `system/hook_response`) appear only with `--include-hook-events`, and `stream_event` lines only with `--include-partial-messages`. Lines of a type or subtype fold does not read become `Other`, and unknown fields are ignored, so a newer Claude Code never breaks the reader.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClaudeMessage {
    System(SystemMessage),
    Assistant(AssistantMessage),
    User(UserMessage),
    StreamEvent(StreamEvent),
    Result(ResultMessage),
    ControlRequest(ControlRequest),
    ControlResponse(ControlResponse),
    #[serde(other)]
    Other,
}

impl ClaudeMessage {
    pub fn parse(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }
}

/// Hooks under `-p`: the project's SessionStart hook runs with `source` `startup`, or `resume` after `--resume`, and its `additionalContext` reaches the model. A Stop hook's `decision: block` gives the model one more turn with the reason; the next Stop carries `stop_hook_active: true`, so a hook that lets that stop pass, as `fld hook stop` does, ends the turn without a loop and the `result` comes once, after the extra turn.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum SystemMessage {
    Init(Init),
    ApiRetry(ApiRetry),
    PermissionDenied(PermissionDenied),
    #[serde(other)]
    Other,
}

/// `apiKeySource` is `none` when Claude Code signs in with the user's claude.ai login: under `-p` without `--bare` it uses that login with no API key in the environment.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Init {
    pub session_id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerStatus>,
    /// Feature names such as `interrupt_receipt_v1`; they say which control requests this Claude Code understands.
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(rename = "permissionMode", default)]
    pub permission_mode: Option<String>,
    #[serde(rename = "apiKeySource", default)]
    pub api_key_source: Option<String>,
    #[serde(default)]
    pub claude_code_version: Option<String>,
}

/// `status` is `connected` once the server answered `initialize`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    pub status: String,
    #[serde(default)]
    pub source: Option<String>,
}

/// Claude Code retries a failed API request by itself and reports each attempt. The probe could not cause a retry; the fields follow the Agent SDK reference.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ApiRetry {
    #[serde(default)]
    pub attempt: Option<u32>,
    #[serde(default)]
    pub max_retries: Option<u32>,
    #[serde(default)]
    pub retry_delay_ms: Option<u64>,
    #[serde(default)]
    pub error_status: Option<u16>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

/// Comes when a `deny` rule (`decision_reason_type` `rule`) or auto mode refuses a tool call without asking. A refusal by the host through `can_use_tool` sends no such line; both kinds are listed in the result's `permission_denials`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PermissionDenied {
    pub tool_name: String,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub decision_reason_type: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AssistantMessage {
    pub message: ApiMessage,
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ApiMessage {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    /// Claude Code sends the thinking text empty; only its signature is filled.
    Thinking {
        #[serde(default)]
        thinking: String,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: Value,
        #[serde(default)]
        is_error: Option<bool>,
    },
    #[serde(other)]
    Other,
}

/// A user turn as Claude Code echoes it: tool results, and the `[Request interrupted by user]` text after an interrupt.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct UserMessage {
    pub message: UserTurn,
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct UserTurn {
    pub content: UserContent,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum UserContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct StreamEvent {
    pub event: ApiEvent,
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

/// The Messages API streaming events, passed through by Claude Code.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApiEvent {
    MessageStart {
        message: ApiMessage,
    },
    ContentBlockStart {
        index: u32,
        content_block: ContentBlock,
    },
    ContentBlockDelta {
        index: u32,
        delta: Delta,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        #[serde(default)]
        usage: Option<Usage>,
    },
    MessageStop,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Delta {
    TextDelta {
        text: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
    #[serde(other)]
    Other,
}

/// The end of a turn. An interrupted turn ends with subtype `error_during_execution` and `is_error`; so does a start that fails, such as `--resume` of an unknown session, which names the cause in `errors` and exits with code 1.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ResultMessage {
    pub subtype: ResultSubtype,
    pub is_error: bool,
    #[serde(default)]
    pub num_turns: u32,
    pub session_id: String,
    #[serde(default)]
    pub total_cost_usd: f64,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub usage: Usage,
    /// The final text; absent when the turn ended in an error.
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub permission_denials: Vec<PermissionDenial>,
    #[serde(default)]
    pub errors: Vec<String>,
    /// Each model the process used, by name, with its context window.
    #[serde(default, rename = "modelUsage")]
    pub model_usage: std::collections::BTreeMap<String, ModelUsage>,
}

/// What a result reports for one model. The probe saw `contextWindow` 1000000 for Haiku.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct ModelUsage {
    #[serde(default, rename = "contextWindow")]
    pub context_window: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultSubtype {
    Success,
    ErrorDuringExecution,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PermissionDenial {
    pub tool_name: String,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub tool_input: Value,
}

/// Token counts of a request or a turn. `CLAUDE_CODE_PROMPT_CACHE_TTL=1h` puts the written cache in `cache_creation.ephemeral_1h_input_tokens` and `5m` in `ephemeral_5m_input_tokens`; the probe saw both on Haiku.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation: Option<CacheCreation>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct CacheCreation {
    #[serde(default)]
    pub ephemeral_1h_input_tokens: u64,
    #[serde(default)]
    pub ephemeral_5m_input_tokens: u64,
}

/// A request from Claude Code that fold must answer with a `ControlResponseBody` carrying the same `request_id`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ControlRequest {
    pub request_id: String,
    pub request: IncomingRequest,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum IncomingRequest {
    CanUseTool(Box<CanUseTool>),
    /// A request fold does not handle; it answers with `ControlResponseBody::Error`.
    #[serde(other)]
    Other,
}

/// Claude Code asks the host about a tool call that a permission rule marks `ask` (`decision_reason_type` `rule`, `decision_reason_rule_source` such as `projectSettings`), also under auto mode. Auto mode's classifier decides other calls itself: the probe's write outside the project ran without a question. A process stopped with a request open (SIGTERM, exit code 143) loses it: a resumed session sends it again neither in the `initialize` answer nor as a request, and the model sees the tool call as interrupted.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CanUseTool {
    pub tool_name: String,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub decision_reason_type: Option<String>,
    #[serde(default)]
    pub decision_reason_rule_source: Option<String>,
    /// `true` for a tool that only a person can answer, such as `AskUserQuestion`.
    #[serde(default)]
    pub requires_user_interaction: bool,
}

/// The tool a worker uses to ask a multiple-choice question.
pub const ASK_USER_QUESTION: &str = "AskUserQuestion";

impl CanUseTool {
    /// The questions of an `AskUserQuestion` request; `None` for any other tool or an input fold cannot read.
    pub fn questions(&self) -> Option<AskUserQuestion> {
        if self.tool_name != ASK_USER_QUESTION {
            return None;
        }
        serde_json::from_value(self.input.clone()).ok()
    }
}

/// Under `-p` an `AskUserQuestion` call arrives as `can_use_tool` with `requires_user_interaction`, also in auto mode. Allowing it with the request's input plus `answers`, each question's text mapped to the chosen option's label, gives the model the tool result `Your questions have been answered: "<question>"="<label>"`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AskUserQuestion {
    pub questions: Vec<Question>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Question {
    pub question: String,
    #[serde(default)]
    pub header: Option<String>,
    #[serde(rename = "multiSelect", default)]
    pub multi_select: bool,
    #[serde(default)]
    pub options: Vec<QuestionOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Claude Code's answer to a request fold sent.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ControlResponse {
    pub response: ControlResponseBody,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum ControlResponseBody {
    Success {
        request_id: String,
        #[serde(default)]
        response: Value,
    },
    Error {
        request_id: String,
        error: String,
    },
    /// An answer of a subtype fold does not read. fold never writes one.
    #[serde(other, skip_serializing)]
    Other,
}

/// The `response` of a successful `initialize`. The answer also names the signed-in account; it is never deserialized. Claude Code 2.1.301 sends no `pending_permission_requests`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct InitializeResponse {
    #[serde(default)]
    pub claude_code_version: Option<String>,
    #[serde(default)]
    pub current_permission_mode: Option<String>,
    #[serde(default)]
    pub session_state: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub pid: Option<u32>,
}

/// The `response` of a successful `interrupt`. The interrupted turn then ends with a `result` of subtype `error_during_execution`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct InterruptResponse {
    #[serde(default)]
    pub still_queued: Vec<Value>,
}

/// One line fold writes on Claude Code's stdin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClaudeInput {
    User {
        message: UserInput,
        parent_tool_use_id: Option<String>,
        /// Empty: Claude Code fills in the session it runs.
        session_id: String,
    },
    ControlRequest {
        request_id: String,
        request: OutgoingRequest,
    },
    ControlResponse {
        response: ControlResponseBody,
    },
}

impl ClaudeInput {
    pub fn user(text: impl Into<String>) -> Self {
        Self::User {
            message: UserInput {
                role: UserRole::User,
                content: text.into(),
            },
            parent_tool_use_id: None,
            session_id: String::new(),
        }
    }

    pub fn permission(request_id: impl Into<String>, decision: PermissionDecision) -> Self {
        Self::ControlResponse {
            response: ControlResponseBody::Success {
                request_id: request_id.into(),
                response: serde_json::to_value(decision).unwrap_or_default(),
            },
        }
    }

    /// One JSON line with its newline, ready for stdin.
    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        let mut line = serde_json::to_string(self)?;
        line.push('\n');
        Ok(line)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserInput {
    pub role: UserRole,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    User,
}

/// `initialize` is the first line fold sends; Claude Code answers it before the first user turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum OutgoingRequest {
    Initialize { hooks: Option<Value> },
    Interrupt,
}

/// The answer to `can_use_tool`. `Allow` runs the tool with `updated_input`, which may be the request's own input; `Deny` gives the model `message` as the tool's error and the turn goes on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "behavior", rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow {
        #[serde(rename = "updatedInput")]
        updated_input: Value,
    },
    Deny {
        message: String,
    },
}

impl PermissionDecision {
    /// Allows an `AskUserQuestion` call with `answers` (question text, chosen label) added to its own `input`, which keeps every field Claude Code sent.
    pub fn answer_questions<'a>(
        input: &Value,
        answers: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Self {
        let answers: serde_json::Map<String, Value> = answers
            .into_iter()
            .map(|(question, label)| (question.to_owned(), Value::from(label)))
            .collect();
        let mut updated_input = input.clone();
        if let Some(fields) = updated_input.as_object_mut() {
            fields.insert("answers".to_owned(), Value::Object(answers));
        }
        Self::Allow { updated_input }
    }
}
