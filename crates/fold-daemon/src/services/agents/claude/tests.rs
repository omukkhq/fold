use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::*;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude")
}

/// The lines Claude Code wrote in one recorded step.
fn read(name: &str) -> Vec<ClaudeMessage> {
    let text = std::fs::read_to_string(fixture_dir().join(format!("{name}.jsonl"))).unwrap();
    text.lines()
        .map(|line| {
            ClaudeMessage::parse(line).unwrap_or_else(|error| panic!("{name}: {error}: {line}"))
        })
        .collect()
}

fn results(messages: &[ClaudeMessage]) -> Vec<&ResultMessage> {
    messages
        .iter()
        .filter_map(|message| match message {
            ClaudeMessage::Result(result) => Some(result),
            _ => None,
        })
        .collect()
}

fn init(messages: &[ClaudeMessage]) -> &Init {
    messages
        .iter()
        .find_map(|message| match message {
            ClaudeMessage::System(SystemMessage::Init(init)) => Some(init),
            _ => None,
        })
        .unwrap()
}

fn permission_requests(messages: &[ClaudeMessage]) -> Vec<(&str, &CanUseTool)> {
    messages
        .iter()
        .filter_map(|message| match message {
            ClaudeMessage::ControlRequest(ControlRequest {
                request_id,
                request: IncomingRequest::CanUseTool(request),
            }) => Some((request_id.as_str(), &**request)),
            _ => None,
        })
        .collect()
}

fn successes(messages: &[ClaudeMessage]) -> Vec<&Value> {
    messages
        .iter()
        .filter_map(|message| match message {
            ClaudeMessage::ControlResponse(ControlResponse {
                response: ControlResponseBody::Success { response, .. },
            }) => Some(response),
            _ => None,
        })
        .collect()
}

/// The variant a line of this `type` and `subtype` must parse to; `None` for lines fold does not read.
fn expected_kind(value: &Value) -> Option<&'static str> {
    let kind = match (value["type"].as_str()?, value["subtype"].as_str()) {
        ("system", Some("init")) => "init",
        ("system", Some("permission_denied")) => "permission_denied",
        ("system", Some("api_retry")) => "api_retry",
        ("system", _) => "system_other",
        ("assistant", _) => "assistant",
        ("user", _) => "user",
        ("stream_event", _) => "stream_event",
        ("result", _) => "result",
        ("control_request", _) => "control_request",
        ("control_response", _) => "control_response",
        _ => return None,
    };
    Some(kind)
}

fn kind(message: &ClaudeMessage) -> Option<&'static str> {
    let kind = match message {
        ClaudeMessage::System(SystemMessage::Init(_)) => "init",
        ClaudeMessage::System(SystemMessage::PermissionDenied(_)) => "permission_denied",
        ClaudeMessage::System(SystemMessage::ApiRetry(_)) => "api_retry",
        ClaudeMessage::System(SystemMessage::Other) => "system_other",
        ClaudeMessage::Assistant(_) => "assistant",
        ClaudeMessage::User(_) => "user",
        ClaudeMessage::StreamEvent(_) => "stream_event",
        ClaudeMessage::Result(_) => "result",
        ClaudeMessage::ControlRequest(_) => "control_request",
        ClaudeMessage::ControlResponse(_) => "control_response",
        ClaudeMessage::Other => return None,
    };
    Some(kind)
}

#[test]
fn every_recorded_line_parses_to_its_own_variant() {
    let mut files = 0;
    for entry in std::fs::read_dir(fixture_dir()).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !name.ends_with(".jsonl") || name.ends_with(".sent.jsonl") {
            continue;
        }
        files += 1;
        for line in std::fs::read_to_string(&path).unwrap().lines() {
            let value: Value = serde_json::from_str(line).unwrap();
            let message = ClaudeMessage::parse(line)
                .unwrap_or_else(|error| panic!("{name}: {error}: {line}"));
            assert_eq!(kind(&message), expected_kind(&value), "{name}: {line}");
        }
    }
    assert!(files >= 7, "only {files} fixtures");
}

#[test]
fn every_sent_line_round_trips_through_the_input_type() {
    for entry in std::fs::read_dir(fixture_dir()).unwrap() {
        let path = entry.unwrap().path();
        if !path.to_string_lossy().ends_with(".sent.jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(&path).unwrap().lines() {
            let input: ClaudeInput = serde_json::from_str(line).unwrap();
            let sent: Value = serde_json::from_str(line).unwrap();
            assert_eq!(serde_json::to_value(&input).unwrap(), sent, "{line}");
        }
    }
}

#[test]
fn unknown_types_subtypes_and_fields_are_ignored() {
    assert_eq!(
        ClaudeMessage::parse(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#
        )
        .unwrap(),
        ClaudeMessage::Other
    );
    assert_eq!(
        ClaudeMessage::parse(r#"{"type":"system","subtype":"brand_new","x":1}"#).unwrap(),
        ClaudeMessage::System(SystemMessage::Other)
    );
    let result = ClaudeMessage::parse(
        r#"{"type":"result","subtype":"error_max_turns","is_error":true,"session_id":"s","new_field":{"a":[1]},"usage":{"input_tokens":3,"future":true}}"#,
    )
    .unwrap();
    let ClaudeMessage::Result(result) = result else {
        panic!("{result:?}");
    };
    assert_eq!(result.subtype, ResultSubtype::Other);
    assert_eq!(result.usage.input_tokens, 3);
    let request = ClaudeMessage::parse(
        r#"{"type":"control_request","request_id":"r1","request":{"subtype":"hook_callback","callback_id":"c"}}"#,
    )
    .unwrap();
    assert_eq!(
        request,
        ClaudeMessage::ControlRequest(ControlRequest {
            request_id: "r1".to_owned(),
            request: IncomingRequest::Other,
        })
    );
    assert_eq!(
        ClaudeMessage::parse(
            r#"{"type":"control_response","response":{"subtype":"partial","request_id":"r2"}}"#
        )
        .unwrap(),
        ClaudeMessage::ControlResponse(ControlResponse {
            response: ControlResponseBody::Other,
        })
    );
    assert!(
        serde_json::to_value(ClaudeInput::ControlResponse {
            response: ControlResponseBody::Other,
        })
        .is_err()
    );
    let assistant = ClaudeMessage::parse(
        r#"{"type":"assistant","message":{"content":[{"type":"server_tool_use","id":"x"},{"type":"text","text":"hi","citations":null}]}}"#,
    )
    .unwrap();
    let ClaudeMessage::Assistant(assistant) = assistant else {
        panic!("{assistant:?}");
    };
    assert_eq!(
        assistant.message.content,
        [
            ContentBlock::Other,
            ContentBlock::Text {
                text: "hi".to_owned()
            }
        ]
    );
}

#[test]
fn init_names_the_session_model_servers_and_capabilities() {
    let messages = read("init");
    let init = init(&messages);
    assert_eq!(init.permission_mode.as_deref(), Some("auto"));
    assert_eq!(init.api_key_source.as_deref(), Some("none"));
    assert_eq!(init.claude_code_version.as_deref(), Some("2.1.301"));
    assert!(init.model.contains("haiku"), "{}", init.model);
    assert!(init.tools.iter().any(|tool| tool == "Bash"));
    assert_eq!(
        init.mcp_servers,
        [McpServerStatus {
            name: "fold".to_owned(),
            status: "connected".to_owned(),
            source: Some("dynamic".to_owned()),
        }]
    );
    assert!(
        init.capabilities
            .iter()
            .any(|name| name == "interrupt_receipt_v1")
    );
    let initialize: InitializeResponse =
        serde_json::from_value(successes(&messages)[0].clone()).unwrap();
    assert_eq!(initialize.current_permission_mode.as_deref(), Some("auto"));
    assert_eq!(initialize.session_state.as_deref(), Some("idle"));
}

#[test]
fn a_turn_ends_with_a_result_that_counts_the_one_hour_cache() {
    let messages = read("result");
    let [result] = results(&messages)[..] else {
        panic!("one result per turn");
    };
    assert_eq!(result.subtype, ResultSubtype::Success);
    assert!(!result.is_error);
    assert_eq!(result.num_turns, 2, "the Stop hook's block adds one turn");
    assert_eq!(result.result.as_deref(), Some("again"));
    assert!(result.total_cost_usd > 0.0);
    let cache = result.usage.cache_creation.unwrap();
    assert!(cache.ephemeral_1h_input_tokens > 0);
    assert_eq!(cache.ephemeral_5m_input_tokens, 0);
    assert!(result.usage.cache_read_input_tokens > 0);
    let texts: Vec<&str> = messages
        .iter()
        .filter_map(|message| match message {
            ClaudeMessage::Assistant(assistant) => Some(&assistant.message.content),
            _ => None,
        })
        .flatten()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["ok heron", "again"]);
}

#[test]
fn the_five_minute_run_streams_events_and_counts_the_five_minute_cache() {
    let messages = read("cache_5m");
    let usage = results(&messages)[0].usage;
    let cache = usage.cache_creation.unwrap();
    assert!(cache.ephemeral_5m_input_tokens > 0);
    assert_eq!(cache.ephemeral_1h_input_tokens, 0);
    let events: Vec<&ApiEvent> = messages
        .iter()
        .filter_map(|message| match message {
            ClaudeMessage::StreamEvent(event) => Some(&event.event),
            _ => None,
        })
        .collect();
    assert!(matches!(
        events.first(),
        Some(ApiEvent::MessageStart { .. })
    ));
    assert!(matches!(events.last(), Some(ApiEvent::MessageStop)));
    assert!(events.iter().any(|event| matches!(
        event,
        ApiEvent::ContentBlockDelta {
            delta: Delta::TextDelta { .. },
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        ApiEvent::MessageDelta { usage: Some(usage) } if usage.output_tokens > 0
    )));
}

#[test]
fn a_tool_call_and_its_result_pair_by_id() {
    let messages = read("tool_call");
    let used = messages.iter().find_map(|message| match message {
        ClaudeMessage::Assistant(assistant) => {
            assistant
                .message
                .content
                .iter()
                .find_map(|block| match block {
                    ContentBlock::ToolUse { id, name, input } => Some((id, name, input)),
                    _ => None,
                })
        }
        _ => None,
    });
    let (id, name, input) = used.unwrap();
    assert_eq!(name, "Read");
    assert!(input["file_path"].as_str().unwrap().ends_with("note.txt"));
    let answered = messages.iter().any(|message| match message {
        ClaudeMessage::User(user) => matches!(
            &user.message.content,
            UserContent::Blocks(blocks) if blocks.iter().any(|block| matches!(
                block,
                ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == id
            ))
        ),
        _ => false,
    });
    assert!(answered);
}

#[test]
fn an_ask_rule_sends_can_use_tool_and_an_allow_runs_the_tool() {
    let messages = read("permission_request");
    let [(request_id, request)] = permission_requests(&messages)[..] else {
        panic!("one permission request");
    };
    assert_eq!(request.tool_name, "Bash");
    assert_eq!(request.input["command"], "echo fold-probe-allow");
    assert_eq!(request.decision_reason_type.as_deref(), Some("rule"));
    assert_eq!(
        request.decision_reason_rule_source.as_deref(),
        Some("projectSettings")
    );
    let answer = ClaudeInput::permission(
        request_id,
        PermissionDecision::Allow {
            updated_input: request.input.clone(),
        },
    );
    let sent =
        std::fs::read_to_string(fixture_dir().join("permission_request.sent.jsonl")).unwrap();
    let recorded: Value = serde_json::from_str(sent.lines().nth(1).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(&answer).unwrap(), recorded);
    let result = results(&messages)[0];
    assert!(result.permission_denials.is_empty());
    assert!(
        result
            .result
            .as_deref()
            .unwrap()
            .contains("fold-probe-allow")
    );
}

#[test]
fn a_question_arrives_as_can_use_tool_and_the_answer_goes_back_as_updated_input() {
    let messages = read("ask_question");
    let [(request_id, request)] = permission_requests(&messages)[..] else {
        panic!("one permission request");
    };
    assert_eq!(request.tool_name, ASK_USER_QUESTION);
    assert!(request.requires_user_interaction);
    let questions = request.questions().unwrap();
    let [question] = &questions.questions[..] else {
        panic!("one question");
    };
    assert!(!question.multi_select);
    let labels: Vec<&str> = question
        .options
        .iter()
        .map(|option| option.label.as_str())
        .collect();
    assert_eq!(labels, ["Red", "Blue"]);
    let answer = ClaudeInput::permission(
        request_id,
        PermissionDecision::answer_questions(
            &request.input,
            [(question.question.as_str(), labels[0])],
        ),
    );
    let sent = std::fs::read_to_string(fixture_dir().join("ask_question.sent.jsonl")).unwrap();
    let recorded: Value = serde_json::from_str(sent.lines().nth(2).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(&answer).unwrap(), recorded);
    let told = messages.iter().any(|message| match message {
        ClaudeMessage::User(user) => matches!(
            &user.message.content,
            UserContent::Blocks(blocks) if blocks.iter().any(|block| matches!(
                block,
                ContentBlock::ToolResult { content, .. }
                    if content.as_str().is_some_and(|text| text.contains("\"Red\""))
            ))
        ),
        _ => false,
    });
    assert!(told);
    let replies: Vec<&str> = messages
        .iter()
        .filter_map(|message| match message {
            ClaudeMessage::Assistant(assistant) => Some(&assistant.message.content),
            _ => None,
        })
        .flatten()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(replies.first(), Some(&"Red"));
    let other = CanUseTool {
        tool_name: "Bash".to_owned(),
        ..request.clone()
    };
    assert_eq!(other.questions(), None);
}

#[test]
fn a_deny_from_fold_lands_in_the_permission_denials() {
    let messages = read("permission_deny");
    let [(request_id, _)] = permission_requests(&messages)[..] else {
        panic!("one permission request");
    };
    let answer = ClaudeInput::permission(
        request_id,
        PermissionDecision::Deny {
            message: "fold denies this action".to_owned(),
        },
    );
    let sent = std::fs::read_to_string(fixture_dir().join("permission_deny.sent.jsonl")).unwrap();
    let recorded: Value = serde_json::from_str(sent.lines().nth(1).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(&answer).unwrap(), recorded);
    assert!(!messages.iter().any(|message| matches!(
        message,
        ClaudeMessage::System(SystemMessage::PermissionDenied(_))
    )));
    let result = results(&messages)[0];
    assert!(!result.is_error);
    assert_eq!(result.permission_denials[0].tool_name, "Bash");
    assert_eq!(
        result.permission_denials[0].tool_input["command"],
        "echo fold-probe-deny"
    );
}

#[test]
fn a_deny_rule_sends_permission_denied_without_asking() {
    let messages = read("denied_by_rule");
    assert!(permission_requests(&messages).is_empty());
    let denied = messages
        .iter()
        .find_map(|message| match message {
            ClaudeMessage::System(SystemMessage::PermissionDenied(denied)) => Some(denied),
            _ => None,
        })
        .unwrap();
    assert_eq!(denied.tool_name, "Bash");
    assert_eq!(denied.decision_reason_type.as_deref(), Some("rule"));
    assert_eq!(
        results(&messages)[0].permission_denials[0].tool_use_id,
        denied.tool_use_id
    );
}

#[test]
fn auto_mode_ran_a_write_outside_the_project_without_asking() {
    let messages = read("auto_unsure");
    assert!(permission_requests(&messages).is_empty());
    let result = results(&messages)[0];
    assert!(result.permission_denials.is_empty());
    assert_eq!(result.subtype, ResultSubtype::Success);
}

#[test]
fn an_interrupt_ends_the_turn_with_an_error_result() {
    let messages = read("interrupt");
    let answer: InterruptResponse =
        serde_json::from_value(successes(&messages)[0].clone()).unwrap();
    assert!(answer.still_queued.is_empty());
    let result = results(&messages)[0];
    assert_eq!(result.subtype, ResultSubtype::ErrorDuringExecution);
    assert!(result.is_error);
    assert_eq!(result.result, None);
    let interrupted = messages.iter().any(|message| match message {
        ClaudeMessage::User(user) => matches!(
            &user.message.content,
            UserContent::Blocks(blocks) if blocks.contains(&ContentBlock::Text {
                text: "[Request interrupted by user]".to_owned()
            })
        ),
        _ => false,
    });
    assert!(interrupted);
    let sent = std::fs::read_to_string(fixture_dir().join("interrupt.sent.jsonl")).unwrap();
    let recorded: Value = serde_json::from_str(sent.lines().nth(1).unwrap()).unwrap();
    let interrupt = ClaudeInput::ControlRequest {
        request_id: recorded["request_id"].as_str().unwrap().to_owned(),
        request: OutgoingRequest::Interrupt,
    };
    assert_eq!(serde_json::to_value(&interrupt).unwrap(), recorded);
}

#[test]
fn a_resumed_session_keeps_its_id_and_drops_the_open_request() {
    let main = read("init");
    let pending = read("pending");
    assert_eq!(permission_requests(&pending).len(), 1);
    assert!(results(&pending).is_empty());
    let resumed = read("resumed");
    assert_eq!(init(&resumed).session_id, init(&main).session_id);
    assert!(permission_requests(&resumed).is_empty());
    assert_eq!(
        successes(&resumed)[0]["pending_permission_requests"],
        Value::Null
    );
    let result = results(&resumed)[0];
    assert_eq!(result.session_id, init(&main).session_id);
    assert!(result.result.as_deref().unwrap().starts_with("resumed"));
}

#[test]
fn a_resume_of_an_unknown_session_is_an_error_result() {
    let messages = read("error_result");
    let [result] = results(&messages)[..] else {
        panic!("one result");
    };
    assert_eq!(result.subtype, ResultSubtype::ErrorDuringExecution);
    assert!(result.is_error);
    assert_eq!(result.num_turns, 0);
    assert!(result.errors[0].starts_with("No conversation found"));
}

#[test]
fn api_retry_reads_the_documented_fields() {
    let message = ClaudeMessage::parse(
        r#"{"type":"system","subtype":"api_retry","attempt":2,"max_retries":10,"retry_delay_ms":1200,"error_status":529,"error":"overloaded","session_id":"s"}"#,
    )
    .unwrap();
    let ClaudeMessage::System(SystemMessage::ApiRetry(retry)) = message else {
        panic!("{message:?}");
    };
    assert_eq!(
        (retry.attempt, retry.error_status, retry.error.as_deref()),
        (Some(2), Some(529), Some("overloaded"))
    );
}

#[test]
fn inputs_are_single_json_lines() {
    let line = ClaudeInput::user("Reply with the word ok.")
        .to_line()
        .unwrap();
    assert!(line.ends_with('\n') && !line.trim_end().contains('\n'));
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap(),
        json!({
            "type": "user",
            "message": {"role": "user", "content": "Reply with the word ok."},
            "parent_tool_use_id": null,
            "session_id": "",
        })
    );
    let initialize = ClaudeInput::ControlRequest {
        request_id: "fold-1".to_owned(),
        request: OutgoingRequest::Initialize { hooks: None },
    };
    assert_eq!(
        serde_json::to_value(&initialize).unwrap(),
        json!({"type": "control_request", "request_id": "fold-1", "request": {"subtype": "initialize", "hooks": null}})
    );
    let refused = ClaudeInput::ControlResponse {
        response: ControlResponseBody::Error {
            request_id: "r1".to_owned(),
            error: "fold does not handle this request".to_owned(),
        },
    };
    assert_eq!(
        serde_json::to_value(&refused).unwrap()["response"]["subtype"],
        "error"
    );
}

#[test]
fn the_version_parser_reads_claude_version_output() {
    assert_eq!(
        ClaudeVersion::parse("2.1.301 (Claude Code)\n"),
        Some(ClaudeVersion::new(2, 1, 301))
    );
    assert_eq!(
        ClaudeVersion::parse("v3.0.12-beta.1"),
        Some(ClaudeVersion::new(3, 0, 12))
    );
    for bad in ["", "Claude Code", "2.1", "2.x.3", "2.1.x"] {
        assert_eq!(ClaudeVersion::parse(bad), None, "{bad}");
    }
    assert!(ClaudeVersion::new(2, 1, 242) < MIN_CLAUDE_VERSION);
    assert!(ClaudeVersion::new(2, 2, 0) > MIN_CLAUDE_VERSION);
    assert!(MIN_CLAUDE_VERSION >= ClaudeVersion::new(2, 1, 242));
    assert_eq!(MIN_CLAUDE_VERSION.to_string(), "2.1.301");
}

#[test]
fn auth_status_reads_only_the_sign_in_state() {
    let status = AuthStatus::parse(
        r#"{"loggedIn": true, "authMethod": "claude.ai", "email": "ana@example.com", "orgName": "Ana's Organization"}"#,
    )
    .unwrap();
    assert_eq!(
        status,
        AuthStatus {
            logged_in: true,
            auth_method: Some("claude.ai".to_owned()),
        }
    );
    assert!(!format!("{status:?}").contains("ana@example.com"));
    assert_eq!(
        AuthStatus::parse(r#"{"loggedIn": false}"#).map(|status| status.logged_in),
        Some(false)
    );
    assert_eq!(AuthStatus::parse("Not logged in"), None);
}
