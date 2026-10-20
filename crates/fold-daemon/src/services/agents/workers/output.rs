use fold_core::layout::MCP_SERVER_NAME;
use fold_core::turn_errors;
use serde_json::{Value, json};

use crate::services::agents::claude::{
    CanUseTool, ClaudeInput, ClaudeMessage, ContentBlock, ControlRequest, ControlResponseBody,
    IncomingRequest, ResultMessage, SystemMessage, Usage, UserContent,
};

/// What one stdout line of a worker changes. It holds counts, names and lengths, never text: the step goes into the live `worker.output` event and the change into the worker's row.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct LineEffect {
    pub step: Step,
    pub change: RowChange,
    /// The answer fold writes back on stdin, for a request Claude Code sent.
    pub reply: Option<ClaudeInput>,
    /// A `can_use_tool` request, which waits for a human's answer or a stored one.
    pub permission: Option<ToolRequest>,
}

/// A `can_use_tool` request and the id its answer must carry.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ToolRequest {
    pub request_id: String,
    pub call: Box<CanUseTool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Step {
    pub kind: &'static str,
    pub tool: Option<String>,
    pub text_length: Option<usize>,
}

impl Step {
    fn kind(kind: &'static str) -> Self {
        Self {
            kind,
            tool: None,
            text_length: None,
        }
    }

    /// The `data` of the live event: the worker's agent and the step's kind, tool name and text length.
    pub fn data(&self, agent: &str) -> Value {
        let mut data = json!({"agent": agent, "step": self.kind});
        if let Some(tool) = &self.tool {
            data["tool"] = json!(tool);
        }
        if let Some(length) = self.text_length {
            data["text_length"] = json!(length);
        }
        data
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum RowChange {
    None,
    /// `system/init` starts every user turn and names the model in use.
    TurnStarted {
        model: String,
    },
    /// The tokens one request sent, which is the size of the worker's context, and the model that answered it.
    Context {
        tokens: u64,
        model: Option<String>,
    },
    /// The end of a turn. `total_cost_usd` is the running total of the process: the probe's turns of one process report 0.00297, 0.00355, … up to 0.00613 for an interrupted turn that cost nothing more. The usage is the turn's own.
    TurnEnded {
        total_cost_usd: f64,
        usage: Usage,
        /// The context window of each model the process used, by model name.
        windows: Vec<(String, u64)>,
        /// The error a turn ended in; `None` for a turn that ended well.
        error: Option<TurnError>,
    },
}

/// The error a turn ended in: Claude Code's text, bounded by [`turn_errors::error_text`], and whether any token of the turn reached the model. The text goes onto the worker's row only, never into a log or an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TurnError {
    pub text: String,
    pub reached_model: bool,
}

/// The last error of a turn whose result names none.
const NO_ERROR_TEXT: &str = "Claude Code ended the turn in an error without a message";

/// The result's final text when it has one, else its `errors`. `usage` is the turn's own, so a turn that made no model call has 0 in every count; the running cost and `modelUsage` count the whole process.
fn turn_error(result: &ResultMessage) -> TurnError {
    let named = result
        .result
        .as_deref()
        .and_then(turn_errors::error_text)
        .or_else(|| turn_errors::error_text(&result.errors.join("\n")));
    let usage = &result.usage;
    let tokens = usage
        .input_tokens
        .saturating_add(usage.output_tokens)
        .saturating_add(usage.cache_creation_input_tokens)
        .saturating_add(usage.cache_read_input_tokens);
    TurnError {
        text: named.unwrap_or_else(|| NO_ERROR_TEXT.to_owned()),
        reached_model: tokens > 0,
    }
}

pub(super) fn effect_of(message: &ClaudeMessage) -> LineEffect {
    let plain = |kind| LineEffect {
        step: Step::kind(kind),
        change: RowChange::None,
        reply: None,
        permission: None,
    };
    match message {
        ClaudeMessage::System(SystemMessage::Init(init)) => LineEffect {
            step: Step::kind("init"),
            change: RowChange::TurnStarted {
                model: init.model.clone(),
            },
            reply: None,
            permission: None,
        },
        ClaudeMessage::System(SystemMessage::PermissionDenied(denied)) => LineEffect {
            step: Step {
                kind: "permission_denied",
                tool: Some(denied.tool_name.clone()),
                text_length: None,
            },
            change: RowChange::None,
            reply: None,
            permission: None,
        },
        ClaudeMessage::System(_) => plain("system"),
        ClaudeMessage::Assistant(assistant) => {
            let step = assistant
                .message
                .content
                .first()
                .map_or_else(|| Step::kind("assistant"), block_step);
            let change =
                assistant
                    .message
                    .usage
                    .map_or(RowChange::None, |usage| RowChange::Context {
                        tokens: context(&usage),
                        model: assistant.message.model.clone(),
                    });
            LineEffect {
                step,
                change,
                reply: None,
                permission: None,
            }
        }
        ClaudeMessage::User(user) => match &user.message.content {
            UserContent::Blocks(blocks) if !blocks.is_empty() => LineEffect {
                step: block_step(&blocks[0]),
                change: RowChange::None,
                reply: None,
                permission: None,
            },
            UserContent::Blocks(_) | UserContent::Text(_) => plain("user"),
        },
        ClaudeMessage::StreamEvent(_) => plain("stream"),
        ClaudeMessage::Result(result) => LineEffect {
            step: Step {
                kind: if result.is_error {
                    "result_error"
                } else {
                    "result"
                },
                tool: None,
                text_length: result.result.as_ref().map(String::len),
            },
            change: RowChange::TurnEnded {
                total_cost_usd: result.total_cost_usd,
                usage: result.usage,
                windows: result
                    .model_usage
                    .iter()
                    .map(|(model, usage)| (model.clone(), usage.context_window))
                    .collect(),
                error: result.is_error.then(|| turn_error(result)),
            },
            reply: None,
            permission: None,
        },
        ClaudeMessage::ControlRequest(ControlRequest {
            request_id,
            request,
        }) => match request {
            IncomingRequest::CanUseTool(request) => LineEffect {
                step: Step {
                    kind: "permission_request",
                    tool: Some(request.tool_name.clone()),
                    text_length: None,
                },
                change: RowChange::None,
                reply: None,
                permission: Some(ToolRequest {
                    request_id: request_id.clone(),
                    call: request.clone(),
                }),
            },
            IncomingRequest::Other => LineEffect {
                step: Step::kind("control_request"),
                change: RowChange::None,
                reply: Some(ClaudeInput::ControlResponse {
                    response: ControlResponseBody::Error {
                        request_id: request_id.clone(),
                        error: "fold does not handle this request".to_owned(),
                    },
                }),
                permission: None,
            },
        },
        ClaudeMessage::ControlResponse(_) => plain("control_response"),
        ClaudeMessage::Other => plain("other"),
    }
}

/// Whether the line is a `system/init` whose `fold` MCP server failed to connect, so the turn it starts has no fold tool. Only `failed` counts: a server that is still `pending` can connect during the turn, and an init without a `fold` entry names no server fold configured.
pub(super) fn fold_unreachable(message: &ClaudeMessage) -> bool {
    let ClaudeMessage::System(SystemMessage::Init(init)) = message else {
        return false;
    };
    init.mcp_servers
        .iter()
        .any(|server| server.name == MCP_SERVER_NAME && server.status == "failed")
}

fn block_step(block: &ContentBlock) -> Step {
    match block {
        ContentBlock::Text { text } => Step {
            kind: "text",
            tool: None,
            text_length: Some(text.len()),
        },
        ContentBlock::Thinking { .. } => Step::kind("thinking"),
        ContentBlock::ToolUse { name, .. } => Step {
            kind: "tool_use",
            tool: Some(name.clone()),
            text_length: None,
        },
        ContentBlock::ToolResult { .. } => Step::kind("tool_result"),
        ContentBlock::Other => Step::kind("other"),
    }
}

fn context(usage: &Usage) -> u64 {
    usage
        .input_tokens
        .saturating_add(usage.cache_creation_input_tokens)
        .saturating_add(usage.cache_read_input_tokens)
}

/// Dollars as whole millionths, the unit of `worker_processes.cost_micro_usd`.
pub(super) fn micro_usd(dollars: f64) -> u64 {
    let micro = (dollars * 1_000_000.0).round();
    if micro.is_finite() && micro > 0.0 {
        // The value is finite, positive and rounded, so the conversion only saturates.
        micro as u64
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn lines(name: &str) -> Vec<ClaudeMessage> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/claude")
            .join(format!("{name}.jsonl"));
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| ClaudeMessage::parse(line).unwrap())
            .collect()
    }

    fn effects(name: &str) -> Vec<LineEffect> {
        lines(name).iter().map(effect_of).collect()
    }

    #[test]
    fn only_an_init_whose_fold_server_failed_counts_as_unreachable() {
        let failed: Vec<bool> = lines("mcp_failed").iter().map(fold_unreachable).collect();
        assert_eq!(failed, [false, false, false, true]);
        assert!(!lines("init").iter().any(fold_unreachable));
        let init = |servers: &str| {
            ClaudeMessage::parse(&format!(
                r#"{{"type":"system","subtype":"init","session_id":"s","mcp_servers":{servers}}}"#
            ))
            .unwrap()
        };
        assert!(!fold_unreachable(&init("[]")));
        assert!(!fold_unreachable(&init(
            r#"[{"name":"fold","status":"pending"}]"#
        )));
        assert!(!fold_unreachable(&init(
            r#"[{"name":"pencil","status":"failed"},{"name":"fold","status":"connected"}]"#
        )));
        assert!(fold_unreachable(&init(
            r#"[{"name":"pencil","status":"connected"},{"name":"fold","status":"failed"}]"#
        )));
    }

    #[test]
    fn a_turn_reports_its_steps_its_context_and_its_end() {
        let all = effects("init")
            .into_iter()
            .chain(effects("result"))
            .collect::<Vec<_>>();
        assert!(all.iter().any(|effect| matches!(
            &effect.change,
            RowChange::TurnStarted { model } if !model.is_empty()
        )));
        assert!(
            all.iter().any(|effect| matches!(
                &effect.change,
                RowChange::Context { tokens, model: Some(model) } if *tokens > 0 && model == "claude-haiku-5-5"
            ))
        );
        let text = all
            .iter()
            .find(|effect| effect.step.kind == "text")
            .unwrap();
        assert_eq!(text.step.text_length, Some("ok heron".len()));
        let end = all
            .iter()
            .find_map(|effect| match effect.change {
                RowChange::TurnEnded {
                    total_cost_usd,
                    usage,
                    ..
                } => Some((total_cost_usd, usage)),
                _ => None,
            })
            .unwrap();
        assert!(end.0 > 0.0);
        assert!(end.1.cache_read_input_tokens > 0);
        assert!(all.iter().all(|effect| effect.reply.is_none()));
        assert!(all.iter().any(|effect| effect.step.kind == "system"));
        assert!(all.iter().any(|effect| effect.step.kind == "thinking"));
        assert!(
            all.iter()
                .any(|effect| effect.step.kind == "control_response")
        );
        assert!(all.iter().any(|effect| effect.step.kind == "other"));
    }

    #[test]
    fn a_tool_call_names_the_tool_and_never_its_input() {
        let all = effects("tool_call");
        let call = all
            .iter()
            .find(|effect| effect.step.kind == "tool_use")
            .unwrap();
        assert_eq!(call.step.tool.as_deref(), Some("Read"));
        let data = call.step.data("web-worker");
        assert_eq!(
            data,
            json!({"agent": "web-worker", "step": "tool_use", "tool": "Read"})
        );
        assert!(all.iter().any(|effect| effect.step.kind == "tool_result"));
    }

    #[test]
    fn a_permission_question_waits_for_an_answer() {
        let all = effects("permission_request");
        let asked = all
            .iter()
            .find(|effect| effect.step.kind == "permission_request")
            .unwrap();
        assert_eq!(asked.step.tool.as_deref(), Some("Bash"));
        assert!(asked.reply.is_none());
        let request = asked.permission.as_ref().unwrap();
        assert_eq!(request.request_id, "00000000-0000-4000-8000-000000000103");
        assert_eq!(request.call.input["command"], "echo fold-probe-allow");
        assert!(
            all.iter()
                .filter(|effect| effect.step.kind != "permission_request")
                .all(|effect| effect.permission.is_none())
        );
        let denied = effects("denied_by_rule");
        assert!(
            denied
                .iter()
                .any(|effect| effect.step.kind == "permission_denied")
        );
        let unknown = effect_of(
            &ClaudeMessage::parse(
                r#"{"type":"control_request","request_id":"r1","request":{"subtype":"mcp_message"}}"#,
            )
            .unwrap(),
        );
        assert!(matches!(
            unknown.reply,
            Some(ClaudeInput::ControlResponse {
                response: ControlResponseBody::Error { .. }
            })
        ));
    }

    #[test]
    fn an_interrupted_turn_ends_in_an_error_result() {
        let all = effects("interrupt");
        assert!(all.iter().any(|effect| effect.step.kind == "result_error"));
        let text = effect_of(
            &ClaudeMessage::parse(r#"{"type":"user","message":{"content":"go on"}}"#).unwrap(),
        );
        assert_eq!(text.step.kind, "user");
        let empty = effect_of(
            &ClaudeMessage::parse(r#"{"type":"assistant","message":{"content":[]}}"#).unwrap(),
        );
        assert_eq!(empty.step.kind, "assistant");
        assert_eq!(empty.change, RowChange::None);
        let blocks = effect_of(
            &ClaudeMessage::parse(r#"{"type":"user","message":{"content":[]}}"#).unwrap(),
        );
        assert_eq!(blocks.step.kind, "user");
        let stream = effect_of(
            &ClaudeMessage::parse(r#"{"type":"stream_event","event":{"type":"message_stop"}}"#)
                .unwrap(),
        );
        assert_eq!(stream.step.kind, "stream");
        let other_block = effect_of(
            &ClaudeMessage::parse(
                r#"{"type":"assistant","message":{"content":[{"type":"image"}]}}"#,
            )
            .unwrap(),
        );
        assert_eq!(other_block.step.kind, "other");
    }

    fn turn_error_of(effects: &[LineEffect]) -> Option<Option<TurnError>> {
        effects.iter().find_map(|effect| match &effect.change {
            RowChange::TurnEnded { error, .. } => Some(error.clone()),
            _ => None,
        })
    }

    #[test]
    fn an_error_result_keeps_claude_codes_text_and_whether_the_turn_reached_the_model() {
        assert_eq!(
            turn_error_of(&effects("error_result")),
            Some(Some(TurnError {
                text: "No conversation found with session ID: 00000000-0000-4000-8000-000000000074"
                    .to_owned(),
                reached_model: false,
            }))
        );
        assert_eq!(turn_error_of(&effects("result")), Some(None));
        let interrupted = turn_error_of(&effects("interrupt")).flatten().unwrap();
        assert!(
            !interrupted.reached_model,
            "an interrupted turn reports no tokens of its own"
        );
        let result = |line: &str| turn_error_of(&[effect_of(&ClaudeMessage::parse(line).unwrap())]);
        assert_eq!(
            result(
                r#"{"type":"result","subtype":"success","is_error":true,"session_id":"s","result":"\u001b[31mNo access\u001b[0m","errors":["other"],"usage":{"output_tokens":5}}"#
            ),
            Some(Some(TurnError {
                text: "No access".to_owned(),
                reached_model: true,
            }))
        );
        assert_eq!(
            result(
                r#"{"type":"result","subtype":"error_max_turns","is_error":true,"session_id":"s","result":" ","errors":["first","second"]}"#
            ),
            Some(Some(TurnError {
                text: "first\nsecond".to_owned(),
                reached_model: false,
            }))
        );
        assert_eq!(
            result(
                r#"{"type":"result","subtype":"error_during_execution","is_error":true,"session_id":"s"}"#
            ),
            Some(Some(TurnError {
                text: NO_ERROR_TEXT.to_owned(),
                reached_model: false,
            }))
        );
    }

    #[test]
    fn dollars_become_micro_dollars() {
        assert_eq!(micro_usd(0.00297035), 2970);
        assert_eq!(micro_usd(0.0), 0);
        assert_eq!(micro_usd(-1.0), 0);
        assert_eq!(micro_usd(f64::NAN), 0);
    }
}
