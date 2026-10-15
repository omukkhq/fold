use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, FromRequestParts, Query, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use fold_core::{FoldError, codes};
use fold_store::StoreError;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use utoipa::ToSchema;

use crate::services::agents::AgentError;
use crate::services::events::EventsError;
use crate::services::messages::MessageError;
use crate::services::projects::ProjectError;
use crate::services::reviews::ReviewError;
use crate::services::tasks::TaskError;
use crate::services::workspaces::WorkspaceError;

/// The one error body of every route.
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorBody {
    /// A stable code; branch on it, never on the message.
    pub code: String,
    pub message: String,
    /// The error's named fields. `hint`, when present, names the next step to take.
    #[schema(value_type = std::collections::BTreeMap<String, serde_json::Value>)]
    pub details: Map<String, Value>,
}

/// The status of each code that has one. Every other code is an internal failure: it answers 500 `internal_error` and the code goes to the log only.
fn status_of(code: &str) -> Option<StatusCode> {
    match code {
        codes::NOT_FOUND
        | codes::PROJECT_NOT_FOUND
        | codes::WEB_NOT_BUILT
        | codes::AGENT_NOT_FOUND
        | codes::REVISION_NOT_FOUND
        | codes::WORKSPACE_NOT_FOUND
        | codes::PATH_NOT_IN_COMPARISON
        | codes::TASK_NOT_FOUND
        | codes::LABEL_NOT_FOUND
        | codes::DEPENDENCY_NOT_FOUND
        | codes::CHECKLIST_ITEM_NOT_FOUND
        | codes::CLAIM_NOT_FOUND
        | codes::NOT_CLAIMED
        | codes::WORKER_NOT_FOUND
        | codes::PROPOSAL_NOT_FOUND
        | codes::MESSAGE_NOT_FOUND
        | codes::RECIPIENT_NOT_FOUND
        | codes::UNKNOWN_HUMAN => Some(StatusCode::NOT_FOUND),
        codes::INVALID_REQUEST
        | codes::INVALID_NAME
        | codes::INVALID_ID
        | codes::INVALID_PATH
        | codes::INVALID_STATE_REF
        | codes::INVALID_TASK_TITLE
        | codes::INVALID_LABEL_NAME
        | codes::INVALID_LABEL_COLOR
        | codes::INVALID_TASK_STATUS
        | codes::INVALID_CHECKLIST_NAME
        | codes::INVALID_STEP_TEXT
        | codes::INVALID_LAST_EVENT_ID => Some(StatusCode::BAD_REQUEST),
        codes::HUMAN_NAME_REQUIRED | codes::AUTH_REQUIRED | codes::INVALID_TOKEN => {
            Some(StatusCode::UNAUTHORIZED)
        }
        codes::HUMAN_REQUIRED
        | codes::ACTOR_CANNOT_CLAIM
        | codes::HOST_NOT_ALLOWED
        | codes::AUTHOR_CANNOT_INTEGRATE => Some(StatusCode::FORBIDDEN),
        codes::PROJECT_UNAVAILABLE | codes::GIT_NOT_FOUND => Some(StatusCode::SERVICE_UNAVAILABLE),
        codes::GC_IN_PROGRESS
        | codes::REMOTE_NOT_SET
        | codes::REMOTE_AHEAD
        | codes::EXPORT_DIVERGED
        | codes::GIT_SYNC_MAP_INVALID
        | codes::INVALID_CONFIG
        | codes::TASK_CLOSED
        | codes::LABEL_EXISTS
        | codes::DEPENDENCY_EXISTS
        | codes::DEPENDENCY_CYCLE
        | codes::ALREADY_CLAIMED
        | codes::ALREADY_HOLDER
        | codes::CLAIM_NOT_STALE
        | codes::NOT_CLAIM_HOLDER
        | codes::WORKSPACE_NOT_ACTIVE
        | codes::HUMAN_REVIEW_ALREADY_SET
        | codes::STALE_VERSION
        | codes::CHANGES_REQUESTED
        | codes::NOT_APPROVED
        | codes::HUMAN_APPROVAL_MISSING
        | codes::PREREQUISITES_NOT_DONE => Some(StatusCode::CONFLICT),
        codes::GIT_FAILED => Some(StatusCode::BAD_GATEWAY),
        codes::WORKSPACE_IO => Some(StatusCode::INTERNAL_SERVER_ERROR),
        codes::HEAD_MOVED => Some(StatusCode::CONFLICT),
        codes::INTEGRATION_FAILED => Some(StatusCode::SERVICE_UNAVAILABLE),
        _ => None,
    }
}

#[derive(Debug)]
pub struct ApiError(pub FoldError);

impl From<FoldError> for ApiError {
    fn from(error: FoldError) -> Self {
        Self(error)
    }
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        let fold = match error.code() {
            Some(code) => FoldError::new(code, error.to_string()),
            None => FoldError::new(codes::INTERNAL_ERROR, error.to_string()),
        };
        Self(fold)
    }
}

impl From<ProjectError> for ApiError {
    fn from(error: ProjectError) -> Self {
        match error {
            ProjectError::Fold(error) => error.into(),
            ProjectError::Store(error) => error.into(),
        }
    }
}

impl From<AgentError> for ApiError {
    fn from(error: AgentError) -> Self {
        match error {
            AgentError::Fold(error) => error.into(),
            AgentError::Store(error) => error.into(),
            AgentError::Event(error) => {
                Self(FoldError::new(codes::INTERNAL_ERROR, error.to_string()))
            }
        }
    }
}

impl From<EventsError> for ApiError {
    fn from(error: EventsError) -> Self {
        match error {
            EventsError::Fold(error) => error.into(),
            EventsError::Store(error) => error.into(),
        }
    }
}

impl From<TaskError> for ApiError {
    fn from(error: TaskError) -> Self {
        match error {
            TaskError::Fold(error) => error.into(),
            TaskError::Store(error) => error.into(),
            TaskError::Event(error) => {
                Self(FoldError::new(codes::INTERNAL_ERROR, error.to_string()))
            }
        }
    }
}

impl From<ReviewError> for ApiError {
    fn from(error: ReviewError) -> Self {
        match error {
            ReviewError::Fold(error) => error.into(),
            ReviewError::Store(error) => error.into(),
            ReviewError::Task(error) => error.into(),
            ReviewError::Event(error) => {
                Self(FoldError::new(codes::INTERNAL_ERROR, error.to_string()))
            }
        }
    }
}

impl From<WorkspaceError> for ApiError {
    fn from(error: WorkspaceError) -> Self {
        match error {
            WorkspaceError::Fold(error) => error.into(),
            WorkspaceError::Store(error) => error.into(),
            WorkspaceError::Task(error) => error.into(),
            WorkspaceError::Event(error) => {
                Self(FoldError::new(codes::INTERNAL_ERROR, error.to_string()))
            }
        }
    }
}

impl From<MessageError> for ApiError {
    fn from(error: MessageError) -> Self {
        match error {
            MessageError::Fold(error) => error.into(),
            MessageError::Store(error) => error.into(),
            MessageError::Event(error) => {
                Self(FoldError::new(codes::INTERNAL_ERROR, error.to_string()))
            }
        }
    }
}

impl ApiError {
    pub fn not_found() -> Self {
        Self(FoldError::new(codes::NOT_FOUND, "no such route"))
    }

    pub fn invalid_request(message: &str) -> Self {
        Self(FoldError::new(codes::INVALID_REQUEST, message))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let error = self.0;
        let (status, body) = match status_of(error.code()) {
            Some(status) => {
                let mut details = error.details().cloned().unwrap_or_default();
                if let Some(hint) = error.hint() {
                    details.insert("hint".to_owned(), Value::from(hint));
                }
                (
                    status,
                    ErrorBody {
                        code: error.code().to_owned(),
                        message: error.message().to_owned(),
                        details,
                    },
                )
            }
            None => {
                tracing::error!(code = error.code(), "request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ErrorBody {
                        code: codes::INTERNAL_ERROR.to_owned(),
                        message: "internal error".to_owned(),
                        details: Map::new(),
                    },
                )
            }
        };
        (status, Json(ErrorEnvelope { error: body })).into_response()
    }
}

/// `Json` with the rejection mapped to the envelope. The message never repeats the body.
pub struct ApiJson<T>(pub T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, ApiError> {
        match Json::<T>::from_request(request, state).await {
            Ok(Json(value)) => Ok(Self(value)),
            Err(JsonRejection::MissingJsonContentType(_)) => Err(ApiError::invalid_request(
                "the request body must be application/json",
            )),
            Err(_) => Err(ApiError::invalid_request(
                "the request body is not valid for this route",
            )),
        }
    }
}

/// `Query` with the rejection mapped to the envelope. The message never repeats the query string.
pub struct ApiQuery<T>(pub T);

impl<S, T> FromRequestParts<S> for ApiQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(value)| Self(value))
            .map_err(|_| ApiError::invalid_request("the query string is not valid for this route"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapped_codes_keep_their_code_and_unmapped_codes_become_internal() {
        for (code, status) in [
            (codes::NOT_FOUND, StatusCode::NOT_FOUND),
            (codes::PROJECT_NOT_FOUND, StatusCode::NOT_FOUND),
            (codes::WEB_NOT_BUILT, StatusCode::NOT_FOUND),
            (codes::INVALID_REQUEST, StatusCode::BAD_REQUEST),
            (codes::REVISION_NOT_FOUND, StatusCode::NOT_FOUND),
            (codes::WORKSPACE_NOT_FOUND, StatusCode::NOT_FOUND),
            (codes::PATH_NOT_IN_COMPARISON, StatusCode::NOT_FOUND),
            (codes::INVALID_ID, StatusCode::BAD_REQUEST),
            (codes::INVALID_PATH, StatusCode::BAD_REQUEST),
            (codes::INVALID_STATE_REF, StatusCode::BAD_REQUEST),
            (codes::PROJECT_UNAVAILABLE, StatusCode::SERVICE_UNAVAILABLE),
            (codes::GC_IN_PROGRESS, StatusCode::CONFLICT),
            (codes::WORKSPACE_NOT_ACTIVE, StatusCode::CONFLICT),
            (codes::WORKSPACE_IO, StatusCode::INTERNAL_SERVER_ERROR),
            (codes::AGENT_NOT_FOUND, StatusCode::NOT_FOUND),
            (codes::INVALID_NAME, StatusCode::BAD_REQUEST),
            (codes::HUMAN_NAME_REQUIRED, StatusCode::UNAUTHORIZED),
            (codes::AUTH_REQUIRED, StatusCode::UNAUTHORIZED),
            (codes::INVALID_TOKEN, StatusCode::UNAUTHORIZED),
            (codes::HOST_NOT_ALLOWED, StatusCode::FORBIDDEN),
            (codes::HUMAN_REQUIRED, StatusCode::FORBIDDEN),
            (codes::ACTOR_CANNOT_CLAIM, StatusCode::FORBIDDEN),
            (codes::GIT_NOT_FOUND, StatusCode::SERVICE_UNAVAILABLE),
            (codes::REMOTE_NOT_SET, StatusCode::CONFLICT),
            (codes::REMOTE_AHEAD, StatusCode::CONFLICT),
            (codes::EXPORT_DIVERGED, StatusCode::CONFLICT),
            (codes::GIT_SYNC_MAP_INVALID, StatusCode::CONFLICT),
            (codes::INVALID_CONFIG, StatusCode::CONFLICT),
            (codes::GIT_FAILED, StatusCode::BAD_GATEWAY),
            (codes::HEAD_MOVED, StatusCode::CONFLICT),
            (codes::INTEGRATION_FAILED, StatusCode::SERVICE_UNAVAILABLE),
            (codes::TASK_NOT_FOUND, StatusCode::NOT_FOUND),
            (codes::INVALID_ID, StatusCode::BAD_REQUEST),
            (codes::DEPENDENCY_CYCLE, StatusCode::CONFLICT),
            (codes::ALREADY_CLAIMED, StatusCode::CONFLICT),
            (codes::MESSAGE_NOT_FOUND, StatusCode::NOT_FOUND),
            (codes::RECIPIENT_NOT_FOUND, StatusCode::NOT_FOUND),
            (codes::PROPOSAL_NOT_FOUND, StatusCode::NOT_FOUND),
            (codes::UNKNOWN_HUMAN, StatusCode::NOT_FOUND),
            (codes::HUMAN_REVIEW_ALREADY_SET, StatusCode::CONFLICT),
            (codes::STALE_VERSION, StatusCode::CONFLICT),
            (codes::AUTHOR_CANNOT_INTEGRATE, StatusCode::FORBIDDEN),
            (codes::CHANGES_REQUESTED, StatusCode::CONFLICT),
            (codes::NOT_APPROVED, StatusCode::CONFLICT),
            (codes::HUMAN_APPROVAL_MISSING, StatusCode::CONFLICT),
            (codes::PREREQUISITES_NOT_DONE, StatusCode::CONFLICT),
        ] {
            assert_eq!(status_of(code), Some(status));
        }
        assert_eq!(status_of(codes::OBJECT_CORRUPT), None);
        assert_eq!(status_of(codes::INTERNAL_ERROR), None);
    }

    async fn body_of(error: FoldError) -> Value {
        let response = ApiError(error).into_response();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn a_hint_travels_in_the_details_and_never_with_an_internal_error() {
        let body = body_of(
            FoldError::new(codes::PROJECT_NOT_FOUND, "absent")
                .with_hint("call init")
                .with_details([("project", serde_json::json!("web"))]),
        )
        .await;
        assert_eq!(
            body["error"]["details"],
            serde_json::json!({"project": "web", "hint": "call init"})
        );
        let body = body_of(FoldError::new(codes::NOT_FOUND, "absent")).await;
        assert_eq!(body["error"]["details"], serde_json::json!({}));
        let body = body_of(FoldError::new(codes::OBJECT_CORRUPT, "T-1").with_hint("wait")).await;
        assert_eq!(body["error"]["details"], serde_json::json!({}));
    }

    #[test]
    fn store_errors_keep_a_code_only_when_they_have_one() {
        let ApiError(error) = StoreError::SchemaTooNew { version: 9 }.into();
        assert_eq!(error.code(), codes::SCHEMA_TOO_NEW);
        let ApiError(error) = StoreError::Corrupt("events.seq").into();
        assert_eq!(error.code(), codes::INTERNAL_ERROR);
    }

    #[test]
    fn agent_errors_keep_their_code_and_a_bad_event_is_internal() {
        let ApiError(error) =
            AgentError::Fold(FoldError::new(codes::AGENT_NOT_FOUND, "absent")).into();
        assert_eq!(error.code(), codes::AGENT_NOT_FOUND);
        let ApiError(error) = AgentError::Store(StoreError::SchemaTooNew { version: 9 }).into();
        assert_eq!(error.code(), codes::SCHEMA_TOO_NEW);
        let ApiError(error) = AgentError::Event(fold_core::EventError::UnknownType).into();
        assert_eq!(error.code(), codes::INTERNAL_ERROR);
    }

    #[test]
    fn workspace_errors_keep_their_code() {
        let ApiError(error) = WorkspaceError::Store(StoreError::Corrupt("revisions.hash")).into();
        assert_eq!(error.code(), codes::INTERNAL_ERROR);
        let ApiError(error) =
            WorkspaceError::Fold(FoldError::new(codes::INVALID_PATH, "bad")).into();
        assert_eq!(error.code(), codes::INVALID_PATH);
        let ApiError(error) = WorkspaceError::Task(TaskError::Fold(FoldError::new(
            codes::ALREADY_CLAIMED,
            "T-1",
        )))
        .into();
        assert_eq!(error.code(), codes::ALREADY_CLAIMED);
        let ApiError(error) = WorkspaceError::Event(fold_core::EventError::UnknownType).into();
        assert_eq!(error.code(), codes::INTERNAL_ERROR);
    }

    #[test]
    fn review_errors_keep_their_code_and_a_bad_event_is_internal() {
        let ApiError(error) =
            ReviewError::Fold(FoldError::new(codes::HUMAN_REVIEW_ALREADY_SET, "set")).into();
        assert_eq!(error.code(), codes::HUMAN_REVIEW_ALREADY_SET);
        let ApiError(error) = ReviewError::Store(StoreError::Corrupt("tasks.number")).into();
        assert_eq!(error.code(), codes::INTERNAL_ERROR);
        let ApiError(error) =
            ReviewError::Task(TaskError::Fold(FoldError::new(codes::TASK_CLOSED, "done"))).into();
        assert_eq!(error.code(), codes::TASK_CLOSED);
        let ApiError(error) = ReviewError::Event(fold_core::EventError::UnknownType).into();
        assert_eq!(error.code(), codes::INTERNAL_ERROR);
    }

    #[test]
    fn task_errors_keep_their_code_and_a_bad_event_is_internal() {
        let ApiError(error) =
            TaskError::Fold(FoldError::new(codes::TASK_NOT_FOUND, "absent")).into();
        assert_eq!(error.code(), codes::TASK_NOT_FOUND);
        let ApiError(error) = TaskError::Store(StoreError::Corrupt("tasks.number")).into();
        assert_eq!(error.code(), codes::INTERNAL_ERROR);
        let ApiError(error) = TaskError::Event(fold_core::EventError::UnknownType).into();
        assert_eq!(error.code(), codes::INTERNAL_ERROR);
    }

    #[test]
    fn message_errors_keep_their_code_and_a_bad_event_is_internal() {
        let ApiError(error) =
            MessageError::Fold(FoldError::new(codes::MESSAGE_NOT_FOUND, "absent")).into();
        assert_eq!(error.code(), codes::MESSAGE_NOT_FOUND);
        let ApiError(error) = MessageError::Store(StoreError::Corrupt("messages.number")).into();
        assert_eq!(error.code(), codes::INTERNAL_ERROR);
        let ApiError(error) = MessageError::Event(fold_core::EventError::UnknownType).into();
        assert_eq!(error.code(), codes::INTERNAL_ERROR);
    }

    #[test]
    fn project_errors_keep_their_code() {
        let ApiError(error) = ProjectError::Store(StoreError::Corrupt("projects.slug")).into();
        assert_eq!(error.code(), codes::INTERNAL_ERROR);
        let ApiError(error) =
            ProjectError::Fold(FoldError::new(codes::PROJECT_NOT_FOUND, "absent")).into();
        assert_eq!(error.code(), codes::PROJECT_NOT_FOUND);
    }
}
