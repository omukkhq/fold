use utoipa::OpenApi as _;
use utoipa::openapi::path::{Operation, PathItem};
use utoipa::openapi::{Content, Info, OpenApi, OpenApiVersion, Ref, RefOr, Response};

use super::error::ErrorEnvelope;
use super::events::EventEnvelope;

const ERROR_ENVELOPE_REF: &str = "#/components/schemas/ErrorEnvelope";

/// The two envelopes are declared here, not by a route, so the web types hold them before any route or the stream refers to them.
#[derive(utoipa::OpenApi)]
#[openapi(components(schemas(ErrorEnvelope, EventEnvelope)))]
struct ApiDoc;

/// The document every route is added to.
pub fn base() -> OpenApi {
    let mut doc = ApiDoc::openapi();
    doc.openapi = OpenApiVersion::Version31;
    doc.info = Info::new("fold", env!("CARGO_PKG_VERSION"));
    doc
}

const HOST_NOT_ALLOWED: &str = "`host_not_allowed`: the `Host`, or the `Origin` of a write, names a host this daemon does not listen on";
const CLIENT_HEADER_REQUIRED: &str =
    "`client_header_required`: a write over TCP lacks the `x-fold-client` header";
const SESSION_REFUSED: &str = "`auth_required`: off loopback, the request has no `fold_session` cookie, or its session is unknown, ended, expired or revoked";

/// Declares the errors of the route layers, which answer before any handler and so are in no route's own declaration: `host_not_allowed` on every operation, `client_header_required` on every write, the session errors on every operation of `guarded`.
pub fn declare_layer_errors(doc: &mut OpenApi, guarded: &OpenApi) {
    for (path, item) in &mut doc.paths.paths {
        for operation in [
            &mut item.put,
            &mut item.post,
            &mut item.delete,
            &mut item.patch,
        ]
        .into_iter()
        .filter_map(Option::as_mut)
        {
            add_error(operation, "403", CLIENT_HEADER_REQUIRED);
        }
        let session = guarded.paths.paths.contains_key(path);
        for operation in operations_mut(item) {
            add_error(operation, "403", HOST_NOT_ALLOWED);
            if session {
                add_error(operation, "401", SESSION_REFUSED);
            }
        }
    }
}

fn operations_mut(item: &mut PathItem) -> impl Iterator<Item = &mut Operation> {
    [
        &mut item.get,
        &mut item.put,
        &mut item.post,
        &mut item.delete,
        &mut item.options,
        &mut item.head,
        &mut item.patch,
        &mut item.trace,
        &mut item.query,
    ]
    .into_iter()
    .filter_map(Option::as_mut)
    .chain(item.additional_operations.values_mut())
}

/// A status the route already declares keeps its body and gains the layer's codes in its description.
fn add_error(operation: &mut Operation, status: &str, description: &str) {
    match operation.responses.responses.get_mut(status) {
        Some(RefOr::T(response)) => {
            response.description = format!("{}; {description}", response.description);
        }
        Some(RefOr::Ref(_)) => {}
        None => {
            let response = Response::builder()
                .description(description)
                .content(
                    "application/json",
                    Content::new(Some(Ref::new(ERROR_ENVELOPE_REF))),
                )
                .build();
            operation
                .responses
                .responses
                .insert(status.to_owned(), RefOr::T(response));
        }
    }
}

/// The document as pretty JSON without a trailing newline. Every map serialises sorted, so equal documents give equal text.
pub fn to_json(doc: &OpenApi) -> Result<String, serde_json::Error> {
    let mut value = serde_json::to_value(doc)?;
    value.sort_all_objects();
    serde_json::to_string_pretty(&value)
}

/// Lists every operation that has no 2xx response, or no 4xx or 5xx response whose body is the `ErrorEnvelope`.
pub fn check_operations(doc: &OpenApi) -> Result<(), Vec<String>> {
    let mut problems = Vec::new();
    for (path, item) in &doc.paths.paths {
        let operations = [
            ("GET", &item.get),
            ("PUT", &item.put),
            ("POST", &item.post),
            ("DELETE", &item.delete),
            ("OPTIONS", &item.options),
            ("HEAD", &item.head),
            ("PATCH", &item.patch),
            ("TRACE", &item.trace),
            ("QUERY", &item.query),
        ];
        let additional = item
            .additional_operations
            .iter()
            .map(|(method, operation)| (method.as_str(), operation));
        let present = operations
            .into_iter()
            .filter_map(|(method, operation)| {
                operation.as_ref().map(|operation| (method, operation))
            })
            .chain(additional);
        for (method, operation) in present {
            problems.extend(
                operation_problems(doc, operation)
                    .into_iter()
                    .map(|problem| format!("{method} {path}: {problem}")),
            );
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

fn operation_problems(doc: &OpenApi, operation: &Operation) -> Vec<&'static str> {
    let responses = &operation.responses.responses;
    let mut problems = Vec::new();
    if !responses.keys().any(|status| status.starts_with('2')) {
        problems.push("no 2xx response");
    }
    let has_error = responses.iter().any(|(status, response)| {
        (status.starts_with('4') || status.starts_with('5'))
            && resolve(doc, response).is_some_and(is_error_envelope)
    });
    if !has_error {
        problems.push("no 4xx or 5xx response with the ErrorEnvelope body");
    }
    problems
}

fn resolve<'a>(doc: &'a OpenApi, response: &'a RefOr<Response>) -> Option<&'a Response> {
    match response {
        RefOr::T(response) => Some(response),
        RefOr::Ref(reference) => {
            let name = reference
                .ref_location
                .strip_prefix("#/components/responses/")?;
            match doc.components.as_ref()?.responses.get(name)? {
                RefOr::T(response) => Some(response),
                RefOr::Ref(_) => None,
            }
        }
    }
}

fn is_error_envelope(response: &Response) -> bool {
    response.content.values().any(|content| match content {
        RefOr::T(content) => matches!(
            &content.schema,
            Some(RefOr::Ref(schema)) if schema.ref_location == ERROR_ENVELOPE_REF
        ),
        RefOr::Ref(_) => false,
    })
}
