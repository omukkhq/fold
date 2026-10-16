use serde_json::{Value, json};

use crate::services::agents::claude::{
    ClaudeInput, ClaudeMessage, ContentBlock, ControlRequest, ControlResponseBody, IncomingRequest,
    PermissionDecision, SystemMessage, Usage, UserContent,
};

/// What one stdout line of a worker changes. It holds counts, names and lengths, never text: the step goes into the live `worker.output` event and the change into the worker's row.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct LineEffect {
    pub step: Step,
    pub change: RowChange,
    /// The answer fold writes back on stdin, for a request Claude Code sent.
    pub reply: Option<ClaudeInput>,
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
    },
}

/// The answer to a permission question while no person can be asked: the turn goes on without the tool.
const NO_PERSON: &str = "fold cannot ask a person about this tool call now; continue without it";

pub(super) fn effect_of(message: &ClaudeMessage) -> LineEffect {
    let plain = |kind| LineEffect {
        step: Step::kind(kind),
        change: RowChange::None,
        reply: None,
    };
    match message {
        ClaudeMessage::System(SystemMessage::Init(init)) => LineEffect {
            step: Step::kind("init"),
            change: RowChange::TurnStarted {
                model: init.model.clone(),
            },
            reply: None,
        },
        ClaudeMessage::System(SystemMessage::PermissionDenied(denied)) => LineEffect {
            step: Step {
                kind: "permission_denied",
                tool: Some(denied.tool_name.clone()),
                text_length: None,
            },
            change: RowChange::None,
            reply: None,
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
            }
        }
        ClaudeMessage::User(user) => match &user.message.content {
            UserContent::Blocks(blocks) if !blocks.is_empty() => LineEffect {
                step: block_step(&blocks[0]),
                change: RowChange::None,
                reply: None,
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
            },
            reply: None,
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
                reply: Some(ClaudeInput::permission(
                    request_id.clone(),
                    PermissionDecision::Deny {
                        message: NO_PERSON.to_owned(),
                    },
                )),
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
            },
        },
        ClaudeMessage::ControlResponse(_) => plain("control_response"),
        ClaudeMessage::Other => plain("other"),
    }
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
        let data = call.step.data("web-coder");
        assert_eq!(
            data,
            json!({"agent": "web-coder", "step": "tool_use", "tool": "Read"})
        );
        assert!(all.iter().any(|effect| effect.step.kind == "tool_result"));
    }

    #[test]
    fn a_permission_question_is_denied_so_the_turn_goes_on() {
        let all = effects("permission_request");
        let asked = all
            .iter()
            .find(|effect| effect.step.kind == "permission_request")
            .unwrap();
        assert_eq!(asked.step.tool.as_deref(), Some("Bash"));
        match asked.reply.as_ref().unwrap() {
            ClaudeInput::ControlResponse {
                response: ControlResponseBody::Success { response, .. },
            } => assert_eq!(response["behavior"], "deny"),
            other => panic!("{other:?}"),
        }
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

    #[test]
    fn dollars_become_micro_dollars() {
        assert_eq!(micro_usd(0.00297035), 2970);
        assert_eq!(micro_usd(0.0), 0);
        assert_eq!(micro_usd(-1.0), 0);
        assert_eq!(micro_usd(f64::NAN), 0);
    }
}
