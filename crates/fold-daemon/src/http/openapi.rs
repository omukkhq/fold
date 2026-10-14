use utoipa::OpenApi as _;
use utoipa::openapi::path::Operation;
use utoipa::openapi::{Info, OpenApi, OpenApiVersion, RefOr, Response};

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

/// The document as pretty JSON without a trailing newline. Every map serialises sorted, so equal documents give equal text.
pub fn to_json(doc: &OpenApi) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&serde_json::to_value(doc)?)
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
