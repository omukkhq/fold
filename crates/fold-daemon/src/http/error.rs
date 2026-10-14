use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use fold_core::{FoldError, codes};
use fold_store::StoreError;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use utoipa::ToSchema;

use crate::services::projects::ProjectError;

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
    #[schema(value_type = Object)]
    pub details: Value,
}

/// The status of each code that has one. Every other code is an internal failure: it answers 500 `internal_error` and the code goes to the log only.
fn status_of(code: &str) -> Option<StatusCode> {
    match code {
        codes::NOT_FOUND | codes::PROJECT_NOT_FOUND => Some(StatusCode::NOT_FOUND),
        codes::INVALID_REQUEST => Some(StatusCode::BAD_REQUEST),
        codes::PROJECT_UNAVAILABLE => Some(StatusCode::SERVICE_UNAVAILABLE),
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
            Some(status) => (
                status,
                ErrorBody {
                    code: error.code().to_owned(),
                    message: error.message().to_owned(),
                    details: error.details().cloned().unwrap_or_else(|| json!({})),
                },
            ),
            None => {
                tracing::error!(code = error.code(), "request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ErrorBody {
                        code: codes::INTERNAL_ERROR.to_owned(),
                        message: "internal error".to_owned(),
                        details: json!({}),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapped_codes_keep_their_code_and_unmapped_codes_become_internal() {
        for (code, status) in [
            (codes::NOT_FOUND, StatusCode::NOT_FOUND),
            (codes::PROJECT_NOT_FOUND, StatusCode::NOT_FOUND),
            (codes::INVALID_REQUEST, StatusCode::BAD_REQUEST),
            (codes::PROJECT_UNAVAILABLE, StatusCode::SERVICE_UNAVAILABLE),
        ] {
            assert_eq!(status_of(code), Some(status));
        }
        assert_eq!(status_of(codes::ALREADY_CLAIMED), None);
        assert_eq!(status_of(codes::INTERNAL_ERROR), None);
    }

    #[test]
    fn store_errors_keep_a_code_only_when_they_have_one() {
        let ApiError(error) = StoreError::SchemaTooNew { version: 9 }.into();
        assert_eq!(error.code(), codes::SCHEMA_TOO_NEW);
        let ApiError(error) = StoreError::Corrupt("events.seq").into();
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
