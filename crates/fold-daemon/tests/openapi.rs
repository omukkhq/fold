use std::collections::BTreeSet;

use fold_core::events::types;
use fold_core::{
    CheckpointId, Event, EventActor, EventType, ProposalId, RevisionId, TaskId, Timestamp,
    WorkspaceId,
};
use fold_daemon::http::events::{
    ChangeKind, ChangedFile, CheckpointSummary, EventEnvelope, LiveWorkspace, PresenceChange,
    Snapshot,
};
use fold_daemon::http::openapi::{base, check_operations, to_json};
use serde_json::{Value, json};
use utoipa::openapi::path::{HttpMethod, OperationBuilder, PathItem};
use utoipa::openapi::{
    ComponentsBuilder, Content, OpenApi, PathsBuilder, Ref, RefOr, Response, ResponseBuilder,
};

#[cfg(test)]
fn doc_json() -> Value {
    serde_json::to_value(fold_daemon::http::openapi()).unwrap()
}

#[cfg(test)]
fn schema<'a>(doc: &'a Value, name: &str) -> &'a Value {
    let schema = &doc["components"]["schemas"][name];
    assert!(schema.is_object(), "components.schemas.{name} is missing");
    schema
}

#[cfg(test)]
fn names(values: &Value) -> BTreeSet<String> {
    values
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect()
}

#[cfg(test)]
fn sample_event() -> Event {
    Event {
        seq: 41,
        event_type: types::TASK_CREATED,
        actor: EventActor::parse("frontend/ui").unwrap(),
        task: TaskId::from_number(12),
        proposal: None,
        workspace: WorkspaceId::from_number(9),
        revision: RevisionId::from_number(17),
        data: json!({ "title_len": 14 }),
        created_at: Timestamp::from_millis(1_760_000_000_000).unwrap(),
    }
}

#[cfg(test)]
fn sample_workspace() -> LiveWorkspace {
    LiveWorkspace {
        workspace: WorkspaceId::from_number(9).unwrap(),
        task: TaskId::from_number(12).unwrap(),
        worker: "frontend/ui".to_owned(),
        base: RevisionId::from_number(17).unwrap(),
        files: vec![ChangedFile {
            path: "src/board.ts".to_owned(),
            previous_path: Some("src/cards.ts".to_owned()),
            change: ChangeKind::Renamed,
            added_lines: 4,
            removed_lines: 1,
            binary: false,
        }],
        summary: "1 file, +4 -1".to_owned(),
        last_activity_at: Timestamp::from_millis(1_760_000_000_500).unwrap(),
        checkpoints: vec![CheckpointSummary {
            id: CheckpointId::from_number(88).unwrap(),
            label: None,
            automatic: true,
            created_at: Timestamp::from_millis(1_760_000_000_100).unwrap(),
        }],
        proposal: ProposalId::from_number(3),
        stalled: true,
    }
}

#[cfg(test)]
/// The `kind` value and the referenced schema of each `oneOf` entry of `EventEnvelope`.
fn envelope_variants(doc: &Value) -> Vec<(String, String)> {
    schema(doc, "EventEnvelope")["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|variant| {
            let parts = variant["allOf"].as_array().unwrap();
            let target = parts
                .iter()
                .find_map(|part| part["$ref"].as_str())
                .unwrap()
                .trim_start_matches("#/components/schemas/")
                .to_owned();
            let kind = parts
                .iter()
                .find_map(|part| part["properties"]["kind"]["enum"][0].as_str())
                .unwrap()
                .to_owned();
            (kind, target)
        })
        .collect()
}

#[test]
fn document_is_openapi_3_1_with_the_error_envelope() {
    let doc = doc_json();
    assert_eq!(doc["openapi"], "3.1.0");
    assert_eq!(doc["info"]["title"], "fold");
    assert_eq!(doc["info"]["version"], env!("CARGO_PKG_VERSION"));
    for path in doc["paths"].as_object().unwrap().keys() {
        assert!(path.starts_with("/api/v1/"), "{path} is outside /api/v1");
    }
    let envelope = schema(&doc, "ErrorEnvelope");
    assert_eq!(
        names(&envelope["required"]),
        BTreeSet::from(["error".to_owned()])
    );
    assert_eq!(
        envelope["properties"]["error"]["$ref"],
        "#/components/schemas/ErrorBody"
    );
    let body = schema(&doc, "ErrorBody");
    assert_eq!(
        names(&body["required"]),
        ["code", "message", "details"].map(str::to_owned).into()
    );
}

#[test]
fn base_document_holds_both_envelopes_without_any_route() {
    let doc = serde_json::to_value(base()).unwrap();
    assert!(
        doc["paths"]
            .as_object()
            .is_none_or(serde_json::Map::is_empty)
    );
    schema(&doc, "ErrorEnvelope");
    schema(&doc, "EventEnvelope");
    schema(&doc, "LiveWorkspace");
}

#[test]
fn document_declares_the_event_envelope() {
    let doc = doc_json();
    let variants = envelope_variants(&doc);
    assert_eq!(
        variants,
        [
            ("event", "DurableEvent"),
            ("workspace_changed", "LiveWorkspace"),
            ("agent_presence_changed", "PresenceChange"),
            ("worker_output", "WorkerOutput"),
            ("snapshot", "Snapshot"),
        ]
        .map(|(kind, target)| (kind.to_owned(), target.to_owned()))
    );

    let event = schema(&doc, "DurableEvent");
    assert_eq!(
        event["properties"]["type"]["$ref"],
        "#/components/schemas/DurableEventType"
    );
    let declared = names(&schema(&doc, "DurableEventType")["enum"]);
    let durable: BTreeSet<String> = EventType::ALL
        .iter()
        .filter(|event_type| !event_type.is_live())
        .map(|event_type| event_type.as_str().to_owned())
        .collect();
    assert_eq!(declared, durable);
    assert!(!declared.contains(types::WORKSPACE_CHANGED.as_str()));
    assert!(!declared.contains(types::AGENT_PRESENCE_CHANGED.as_str()));
    assert!(!declared.contains(types::WORKER_OUTPUT.as_str()));
    let output = schema(&doc, "WorkerOutput");
    assert_eq!(
        names(&output["required"]),
        ["agent", "step", "tool", "text_length"]
            .map(str::to_owned)
            .into()
    );

    let presence = schema(&doc, "PresenceChange");
    assert_eq!(
        names(&presence["required"]),
        ["agent", "presence"].map(str::to_owned).into()
    );
    assert_eq!(
        presence["properties"]["presence"]["$ref"],
        "#/components/schemas/PresenceState"
    );

    let snapshot = schema(&doc, "Snapshot");
    assert_eq!(
        names(&snapshot["required"]),
        ["seq", "workspaces"].map(str::to_owned).into()
    );
    assert_eq!(
        snapshot["properties"]["workspaces"]["items"]["$ref"],
        "#/components/schemas/LiveWorkspace"
    );
    let change = schema(&doc, "ChangeKind");
    assert_eq!(
        names(&change["enum"]),
        ["added", "modified", "deleted", "renamed"]
            .map(str::to_owned)
            .into()
    );
}

#[test]
fn serialised_event_reads_as_the_event_variant() {
    let event = sample_event();
    let mut wire = serde_json::to_value(&event).unwrap();
    let fields: BTreeSet<String> = wire.as_object().unwrap().keys().cloned().collect();
    wire.as_object_mut()
        .unwrap()
        .insert("kind".to_owned(), json!("event"));

    let read: EventEnvelope = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(read, EventEnvelope::Event(event.clone()));
    assert_eq!(serde_json::to_value(&read).unwrap(), wire);

    // The declared schema has exactly the fields the stored event serialises, all required.
    let doc = doc_json();
    let declared = schema(&doc, "DurableEvent");
    let properties: BTreeSet<String> = declared["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(properties, fields);
    assert_eq!(names(&declared["required"]), fields);
}

#[test]
fn live_variants_serialise_with_their_kind() {
    let changed = EventEnvelope::WorkspaceChanged(sample_workspace());
    let wire = serde_json::to_value(&changed).unwrap();
    assert_eq!(wire["kind"], "workspace_changed");
    assert_eq!(wire["workspace"], "ws-9");
    assert_eq!(wire["task"], "T-12");
    assert_eq!(wire["base"], "r17");
    assert_eq!(wire["proposal"], "P-3");
    assert_eq!(wire["files"][0]["change"], "renamed");
    assert_eq!(wire["checkpoints"][0]["id"], "ck-88");
    assert_eq!(wire["checkpoints"][0]["label"], Value::Null);
    assert_eq!(
        serde_json::from_value::<EventEnvelope>(wire).unwrap(),
        changed
    );

    let presence = EventEnvelope::AgentPresenceChanged(PresenceChange {
        agent: "frontend".to_owned(),
        presence: fold_core::Presence::Idle,
    });
    let wire = serde_json::to_value(&presence).unwrap();
    assert_eq!(
        wire,
        json!({"kind": "agent_presence_changed", "agent": "frontend", "presence": "idle"})
    );
    assert_eq!(
        serde_json::from_value::<EventEnvelope>(wire).unwrap(),
        presence
    );

    let snapshot = EventEnvelope::Snapshot(Snapshot {
        seq: 41,
        workspaces: vec![sample_workspace()],
    });
    let wire = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(wire["kind"], "snapshot");
    assert_eq!(wire["seq"], 41);
    assert_eq!(wire["workspaces"][0]["kind"], Value::Null);
    assert_eq!(
        serde_json::from_value::<EventEnvelope>(wire).unwrap(),
        snapshot
    );

    assert!(serde_json::from_value::<EventEnvelope>(json!({ "kind": "nope" })).is_err());
}

#[test]
fn every_live_workspace_field_is_declared_and_required() {
    let wire = serde_json::to_value(sample_workspace()).unwrap();
    let doc = doc_json();
    for (name, value) in [
        ("LiveWorkspace", &wire),
        ("ChangedFile", &wire["files"][0]),
        ("CheckpointSummary", &wire["checkpoints"][0]),
    ] {
        let fields: BTreeSet<String> = value.as_object().unwrap().keys().cloned().collect();
        let declared = schema(&doc, name);
        let properties: BTreeSet<String> = declared["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(properties, fields, "{name}");
        assert_eq!(names(&declared["required"]), fields, "{name}");
    }
}

#[test]
fn output_is_identical_on_two_runs() {
    let first = to_json(&fold_daemon::http::openapi()).unwrap();
    let second = to_json(&fold_daemon::http::openapi()).unwrap();
    assert_eq!(first, second);
    assert!(first.starts_with("{\n  \"components\""));
    assert!(!first.ends_with('\n'));
}

#[test]
fn every_operation_declares_success_and_error_responses() {
    assert_eq!(check_operations(&fold_daemon::http::openapi()), Ok(()));
}

#[cfg(test)]
fn json_response(schema_name: &str) -> Response {
    ResponseBuilder::new()
        .description("body")
        .content(
            "application/json",
            Content::new(Some(Ref::from_schema_name(schema_name))),
        )
        .build()
}

#[cfg(test)]
fn doc_with(method: HttpMethod, responses: Vec<(&str, RefOr<Response>)>) -> OpenApi {
    let operation = responses
        .into_iter()
        .fold(OperationBuilder::new(), |operation, (status, response)| {
            operation.response(status, response)
        });
    let mut doc = base();
    doc.paths = PathsBuilder::new()
        .path("/api/v1/things", PathItem::new(method, operation))
        .build();
    doc
}

#[test]
fn check_operations_rejects_an_operation_without_an_error_response() {
    let doc = doc_with(
        HttpMethod::Post,
        vec![("200", json_response("ProjectList").into())],
    );
    assert_eq!(
        check_operations(&doc),
        Err(vec![
            "POST /api/v1/things: no 4xx or 5xx response with the ErrorEnvelope body".to_owned()
        ])
    );
}

#[test]
fn check_operations_rejects_an_operation_without_a_success_response() {
    let doc = doc_with(
        HttpMethod::Get,
        vec![("404", json_response("ErrorEnvelope").into())],
    );
    assert_eq!(
        check_operations(&doc),
        Err(vec!["GET /api/v1/things: no 2xx response".to_owned()])
    );
}

#[test]
fn check_operations_wants_the_envelope_on_an_error_status() {
    let not_envelope = doc_with(
        HttpMethod::Delete,
        vec![
            ("204", Response::new("gone").into()),
            ("400", json_response("ProjectList").into()),
            ("302", json_response("ErrorEnvelope").into()),
        ],
    );
    assert_eq!(check_operations(&not_envelope).unwrap_err().len(), 1);

    let content_ref = ResponseBuilder::new()
        .description("body")
        .content_ref("application/json", Ref::new("#/components/contents/x"))
        .build();
    let doc = doc_with(
        HttpMethod::Patch,
        vec![
            ("200", Response::new("ok").into()),
            ("500", content_ref.into()),
        ],
    );
    assert_eq!(check_operations(&doc).unwrap_err().len(), 1);
}

#[test]
fn check_operations_resolves_a_shared_error_response() {
    let mut doc = doc_with(
        HttpMethod::Put,
        vec![
            ("200", Response::new("ok").into()),
            ("503", Ref::from_response_name("Unavailable").into()),
        ],
    );
    let components = doc.components.take().unwrap_or_default();
    doc.components = Some(
        ComponentsBuilder::from(components)
            .response("Unavailable", json_response("ErrorEnvelope"))
            .response("Chained", Ref::from_response_name("Unavailable"))
            .build(),
    );
    assert_eq!(check_operations(&doc), Ok(()));

    for target in ["Missing", "Chained"] {
        let mut broken = doc.clone();
        let item = broken.paths.paths.get_mut("/api/v1/things").unwrap();
        let operation = item.put.as_mut().unwrap();
        operation
            .responses
            .responses
            .insert("503".to_owned(), Ref::from_response_name(target).into());
        assert_eq!(check_operations(&broken).unwrap_err().len(), 1, "{target}");
    }

    let mut foreign = doc.clone();
    let operation = foreign
        .paths
        .paths
        .get_mut("/api/v1/things")
        .unwrap()
        .put
        .as_mut()
        .unwrap();
    operation
        .responses
        .responses
        .insert("503".to_owned(), Ref::new("other.json#/x").into());
    assert_eq!(check_operations(&foreign).unwrap_err().len(), 1);
}

#[test]
fn check_operations_checks_additional_operations() {
    let mut doc = doc_with(
        HttpMethod::Get,
        vec![
            ("200", Response::new("ok").into()),
            ("500", json_response("ErrorEnvelope").into()),
        ],
    );
    let item = doc.paths.paths.get_mut("/api/v1/things").unwrap();
    item.additional_operations
        .insert("COPY".to_owned(), OperationBuilder::new().build());
    assert_eq!(
        check_operations(&doc),
        Err(vec![
            "COPY /api/v1/things: no 2xx response".to_owned(),
            "COPY /api/v1/things: no 4xx or 5xx response with the ErrorEnvelope body".to_owned(),
        ])
    );
}
