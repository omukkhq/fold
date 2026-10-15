use axum::Json;
use axum::extract::{Request, State};
use axum::http::header::SET_COOKIE;
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{AppendHeaders, IntoResponse, Response};
use fold_core::{FoldError, HumanName, codes};
use fold_store::StoreError;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::Transport;
use super::actor::{HUMAN_COOKIE, HUMAN_HEADER, cookie};
use super::error::{ApiError, ApiJson, ErrorEnvelope};
use crate::daemon::AppState;
use crate::services::projects;

/// The cookie that carries a session id off loopback. The id is random, the registry holds only its hash, and the token that opened the session never goes into a cookie.
pub const SESSION_COOKIE: &str = "fold_session";
const SESSION_PATH: &str = "/api/v1/session";
const HUMAN_MAX_AGE_SECS: u64 = 365 * 24 * 60 * 60;

/// The human a live `fold_session` cookie signs in. [`require_session`] adds it to the request, and the `Human` extractor takes the name from it before anything else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionHuman(pub HumanName);

/// The session routes. They stay outside [`require_session`], so a browser without a cookie can sign in.
pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_session, create_session, delete_session))
}

/// The layer on every guarded `/api` route. A request over the Unix socket comes from the host's user and passes; so does every request on loopback. Off loopback the request needs the `fold_session` cookie of a live session: a missing cookie and an unknown, ended, expired or revoked session are all `auth_required`.
pub async fn require_session(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    if open(&state, request.extensions().get::<Transport>()) {
        return next.run(request).await;
    }
    match signed_in(&state, request.headers()).await {
        Ok(Some(name)) => {
            request.extensions_mut().insert(SessionHuman(name));
            next.run(request).await
        }
        Ok(None) => ApiError(
            FoldError::new(
                codes::AUTH_REQUIRED,
                "this route needs the cookie of a live session",
            )
            .with_hint("sign in with a token from `fold token create`"),
        )
        .into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

/// True when the request needs no token: it came over the socket, or the daemon listens on loopback.
fn open(state: &AppState, transport: Option<&Transport>) -> bool {
    transport == Some(&Transport::Socket) || !state.auth_required()
}

/// The human the live session in the `fold_session` cookie signs in.
async fn signed_in(state: &AppState, headers: &HeaderMap) -> Result<Option<HumanName>, StoreError> {
    let Some(session) = cookie(headers, SESSION_COOKIE) else {
        return Ok(None);
    };
    projects::check_web_session(&state.registry, state.clock.as_ref(), &session).await
}

fn invalid_token() -> ApiError {
    ApiError(
        FoldError::new(codes::INVALID_TOKEN, "the token is not valid")
            .with_hint("create a new token with `fold token create`"),
    )
}

/// What the sign-in screen needs to know.
#[derive(Debug, Serialize, ToSchema)]
pub struct SessionStatus {
    /// The name the request acts under; null before a name or a token gives one.
    #[schema(required = true)]
    pub human: Option<String>,
    /// False only off loopback without the cookie of a live session: the sign-in screen then asks for a token. True with a null `human` asks for a name.
    pub authenticated: bool,
}

/// Off loopback, a web token from `fold token create`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TokenSignIn {
    pub token: String,
}

/// On loopback, the name the person works under.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NameSignIn {
    pub name: String,
}

/// A token off loopback, a name on loopback.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum SessionRequest {
    Token(TokenSignIn),
    Name(NameSignIn),
}

/// Reports who the request acts as. It is the only way the web application learns the name, because `fold_human` is HttpOnly.
#[utoipa::path(
    get,
    path = "/api/v1/session",
    operation_id = "get_session",
    responses(
        (status = 200, description = "The session of this request", body = SessionStatus),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn get_session(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<SessionStatus>, ApiError> {
    let status = match parts.extensions.get::<Transport>() {
        Some(Transport::Socket) => opened(
            parts
                .headers
                .get(HUMAN_HEADER)
                .and_then(|value| value.to_str().ok()),
        ),
        None if !state.auth_required() => opened(cookie(&parts.headers, HUMAN_COOKIE).as_deref()),
        None => match signed_in(&state, &parts.headers).await? {
            Some(name) => SessionStatus {
                human: Some(name.to_string()),
                authenticated: true,
            },
            None => SessionStatus {
                human: None,
                authenticated: false,
            },
        },
    };
    Ok(Json(status))
}

/// An authenticated session, named by `raw` when that is a valid name.
fn opened(raw: Option<&str>) -> SessionStatus {
    SessionStatus {
        human: raw
            .and_then(|raw| HumanName::parse(raw).ok())
            .map(|name| name.to_string()),
        authenticated: true,
    }
}

/// Signs in. Off loopback the body is `{ token }` and a valid token opens a session whose id `fold_session` carries; on loopback it is `{ name }`, which registers the human on first use and sets `fold_human`.
#[utoipa::path(
    post,
    path = "/api/v1/session",
    operation_id = "create_session",
    request_body = SessionRequest,
    responses(
        (status = 204, description = "Signed in", headers(("Set-Cookie" = String, description = "`fold_session` off loopback, `fold_human` on loopback"))),
        (status = 400, description = "`invalid_request`: a name off loopback, a token on loopback, or a malformed body; `invalid_name`: the name breaks the name pattern", body = ErrorEnvelope),
        (status = 401, description = "`invalid_token`: the registry holds no such token, or it is revoked", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn create_session(
    State(state): State<AppState>,
    parts: Parts,
    ApiJson(request): ApiJson<SessionRequest>,
) -> Result<Response, ApiError> {
    let open = open(&state, parts.extensions.get::<Transport>());
    match (request, open) {
        (SessionRequest::Name(NameSignIn { name }), true) => {
            let name = HumanName::parse(&name).map_err(FoldError::from)?;
            projects::ensure_human(&state.registry, state.clock.as_ref(), &name).await?;
            Ok(signed_in_response(set_cookie(
                HUMAN_COOKIE,
                name.as_str(),
                HUMAN_MAX_AGE_SECS,
            )))
        }
        (SessionRequest::Token(TokenSignIn { token }), false) => {
            match projects::create_web_session(
                &state.registry,
                state.clock.as_ref(),
                state.session_lifetime,
                &token,
            )
            .await?
            {
                Some(signed_in) => Ok(signed_in_response(set_cookie(
                    SESSION_COOKIE,
                    &signed_in.session,
                    state.session_lifetime.as_secs(),
                ))),
                None => {
                    tracing::warn!(
                        route = SESSION_PATH,
                        result = codes::INVALID_TOKEN,
                        "sign-in refused"
                    );
                    Err(invalid_token())
                }
            }
        }
        (SessionRequest::Token(_), true) => Err(ApiError(
            FoldError::new(
                codes::INVALID_REQUEST,
                "this daemon listens on loopback and takes a name, not a token",
            )
            .with_hint("post { \"name\": \"...\" }"),
        )),
        (SessionRequest::Name(_), false) => Err(ApiError(
            FoldError::new(
                codes::INVALID_REQUEST,
                "this daemon listens off loopback and takes a token, not a name",
            )
            .with_hint(
                "create a token with `fold token create <human>` and post { \"token\": \"...\" }",
            ),
        )),
    }
}

/// Signs out: deletes the session that `fold_session` names, so the cookie stops working even where the browser keeps it, and expires `fold_session` and `fold_human`.
#[utoipa::path(
    delete,
    path = "/api/v1/session",
    operation_id = "delete_session",
    responses(
        (status = 204, description = "Signed out", headers(("Set-Cookie" = String, description = "`fold_session` and `fold_human` with `Max-Age=0`"))),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn delete_session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if let Some(session) = cookie(&headers, SESSION_COOKIE) {
        projects::end_web_session(&state.registry, &session).await?;
    }
    Ok((
        StatusCode::NO_CONTENT,
        AppendHeaders([
            (SET_COOKIE, set_cookie(SESSION_COOKIE, "", 0)),
            (SET_COOKIE, set_cookie(HUMAN_COOKIE, "", 0)),
        ]),
    )
        .into_response())
}

fn signed_in_response(cookie: String) -> Response {
    (
        StatusCode::NO_CONTENT,
        AppendHeaders([(SET_COOKIE, cookie)]),
    )
        .into_response()
}

/// No `Secure` flag: off loopback the daemon serves plain HTTP. Session id and name are URL-safe base64 and name characters, which a cookie value takes unquoted.
fn set_cookie(name: &str, value: &str, max_age_secs: u64) -> String {
    format!("{name}={value}; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age_secs}")
}
