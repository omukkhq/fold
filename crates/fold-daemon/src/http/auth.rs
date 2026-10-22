use axum::Json;
use axum::extract::{Request, State};
use axum::http::header::SET_COOKIE;
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{AppendHeaders, IntoResponse, Response};
use fold_core::{FoldError, HumanName, Timestamp, codes};
use fold_store::StoreError;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::actor::{HUMAN_HEADER, Human, cookie, refuse_worker_peer};
use super::error::{ApiError, ApiJson, ErrorEnvelope};
use super::host::web_origin;
use super::{SocketOnly, Transport};
use crate::daemon::AppState;
use crate::services::projects::{self, SignedIn};

/// The cookie that carries a session id. The id is random, the registry holds only its hash, and neither the token nor the sign-in link that opened the session ever goes into a cookie.
pub const SESSION_COOKIE: &str = "fold_session";
const SESSION_PATH: &str = "/api/v1/session";
/// The web route that opens a sign-in link. The secret goes in the fragment, which a browser never sends to a server, so it reaches no log and no `Referer`.
const SIGN_IN_ROUTE: &str = "/sign-in#link=";

/// The human a live `fold_session` cookie signs in. [`require_session`] adds it to the request, and the `Human` extractor takes the name from it before anything else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionHuman(pub HumanName);

/// The session routes. They stay outside [`require_session`], so a browser without a cookie can sign in.
pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_session, create_session, delete_session))
}

/// The routes that make a way to sign in: a web token and a one-time sign-in link. They are guarded like every other route and answer only over the socket, so only the host's user, outside every worker's process tree, gets one.
pub fn sign_in_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(create_token))
        .routes(routes!(create_link))
}

/// The layer on every guarded `/api` route. A request over the Unix socket comes from the host's user and passes. Every other request, on loopback too, needs the `fold_session` cookie of a live session: a missing cookie and an unknown, ended, expired or revoked session are all `auth_required`.
pub async fn require_session(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    if request.extensions().get::<Transport>() == Some(&Transport::Socket) {
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
            .with_hint(
                "open the sign-in link that `fld web` prints, or sign in with a token from `fld token create`",
            ),
        )
        .into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
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
            .with_hint("create a new token with `fld token create`"),
    )
}

fn invalid_link() -> ApiError {
    ApiError(
        FoldError::new(
            codes::INVALID_SIGN_IN_LINK,
            "the sign-in link is unknown, already used or expired",
        )
        .with_hint("run `fld web` for a new link"),
    )
}

/// What the sign-in screen needs to know.
#[derive(Debug, Serialize, ToSchema)]
pub struct SessionStatus {
    /// The name the request acts under; null before a sign-in gives one.
    #[schema(required = true)]
    pub human: Option<String>,
    /// False without the cookie of a live session: the sign-in screen then asks for a token. Over the socket it is always true.
    pub authenticated: bool,
}

/// A web token from `fld token create`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TokenSignIn {
    pub token: String,
}

/// The secret of a one-time sign-in link from `fld web`, which the sign-in screen reads from the link.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkSignIn {
    pub link: String,
}

/// A token, or the secret of a sign-in link.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum SessionRequest {
    Token(TokenSignIn),
    Link(LinkSignIn),
}

/// Reports who the request acts as. It is the only way the web application learns the name, because `fold_session` is HttpOnly.
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
        Some(Transport::Socket) => SessionStatus {
            human: parts
                .headers
                .get(HUMAN_HEADER)
                .and_then(|value| value.to_str().ok())
                .and_then(|raw| HumanName::parse(raw).ok())
                .map(|name| name.to_string()),
            authenticated: true,
        },
        None => {
            let human = signed_in(&state, &parts.headers).await?;
            SessionStatus {
                authenticated: human.is_some(),
                human: human.map(|name| name.to_string()),
            }
        }
    };
    Ok(Json(status))
}

/// Signs in with `{ token }`, a web token from `fld token create`, or `{ link }`, the secret of a sign-in link from `fld web`, which opens one session and then stops working. Either opens a session whose id `fold_session` carries.
#[utoipa::path(
    post,
    path = "/api/v1/session",
    operation_id = "create_session",
    request_body = SessionRequest,
    responses(
        (status = 204, description = "Signed in", headers(("Set-Cookie" = String, description = "`fold_session`"))),
        (status = 400, description = "`invalid_request`: a malformed body", body = ErrorEnvelope),
        (status = 401, description = "`invalid_token`: the registry holds no such token, or it is revoked; `invalid_sign_in_link`: the registry holds no such link, or it was used or has expired", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn create_session(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<SessionRequest>,
) -> Result<Response, ApiError> {
    let (opened, refused): (_, fn() -> ApiError) = match request {
        SessionRequest::Token(TokenSignIn { token }) => (
            projects::create_web_session(
                &state.registry,
                state.clock.as_ref(),
                state.session_lifetime,
                &token,
            )
            .await?,
            invalid_token,
        ),
        SessionRequest::Link(LinkSignIn { link }) => (
            projects::open_sign_in_link(
                &state.registry,
                state.clock.as_ref(),
                state.session_lifetime,
                &link,
            )
            .await?,
            invalid_link,
        ),
    };
    let Some(SignedIn { session, .. }) = opened else {
        let error = refused();
        tracing::warn!(
            route = SESSION_PATH,
            result = error.0.code(),
            "sign-in refused"
        );
        return Err(error);
    };
    Ok((
        StatusCode::NO_CONTENT,
        AppendHeaders([(
            SET_COOKIE,
            set_cookie(SESSION_COOKIE, &session, state.session_lifetime.as_secs()),
        )]),
    )
        .into_response())
}

/// Signs out: deletes the session that `fold_session` names, so the cookie stops working even where the browser keeps it, and expires `fold_session`.
#[utoipa::path(
    delete,
    path = "/api/v1/session",
    operation_id = "delete_session",
    responses(
        (status = 204, description = "Signed out", headers(("Set-Cookie" = String, description = "`fold_session` with `Max-Age=0`"))),
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
        AppendHeaders([(SET_COOKIE, set_cookie(SESSION_COOKIE, "", 0))]),
    )
        .into_response())
}

/// No `Secure` flag: the daemon serves plain HTTP. A session id is URL-safe base64, which a cookie value takes unquoted.
fn set_cookie(name: &str, value: &str, max_age_secs: u64) -> String {
    format!("{name}={value}; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age_secs}")
}

/// The human a new web token signs in.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateToken {
    /// The human the token signs in; created when absent.
    pub human: String,
    /// A note that tells this token apart from the human's other tokens.
    #[serde(default)]
    pub label: Option<String>,
}

/// A new web token. The plaintext is in this answer only: the registry keeps its hash.
#[derive(Debug, Serialize, ToSchema)]
pub struct CreatedToken {
    pub token: String,
}

/// Creates a web token for `fld token create`. Only over the socket, and never for a process in the tree of a worker that fold started or of a check command that fold runs: a token signs in as the human it names.
#[utoipa::path(
    post,
    path = "/api/v1/tokens",
    operation_id = "create_token",
    request_body = CreateToken,
    responses(
        (status = 201, description = "The new token", body = CreatedToken),
        (status = 400, description = "`invalid_name`: the human's name breaks the name pattern", body = ErrorEnvelope),
        (status = 403, description = "`worker_peer_refused`: the request comes from the process tree of a worker that fold started or of a check command that fold runs", body = ErrorEnvelope),
        (status = 404, description = "Not on the socket", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn create_token(
    _socket: SocketOnly,
    State(state): State<AppState>,
    parts: Parts,
    ApiJson(request): ApiJson<CreateToken>,
) -> Result<(StatusCode, Json<CreatedToken>), ApiError> {
    refuse_worker_peer(&parts, &state).await?;
    let human = HumanName::parse(&request.human).map_err(FoldError::from)?;
    let token = projects::create_web_token(
        &state.registry,
        &human,
        request.label.as_deref(),
        state.clock.now(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(CreatedToken { token })))
}

/// A new one-time sign-in link.
#[derive(Debug, Serialize, ToSchema)]
pub struct CreatedLink {
    /// The link to open in a browser on this machine. Its secret is in the fragment.
    pub url: String,
    /// UTC milliseconds after which the link no longer signs in.
    #[schema(value_type = i64)]
    pub expires_at: Timestamp,
    /// Seconds from now until `expires_at`, by the daemon's clock, so a client tells the person how long the link lasts without comparing clocks.
    pub expires_in_secs: u64,
}

/// Creates a one-time sign-in link for `fld web`, for the human that `x-fold-human` names. Only over the socket, where the `Human` extractor refuses a process in the tree of a worker that fold started or of a check command that fold runs. The link opens one session, within a few minutes.
#[utoipa::path(
    post,
    path = "/api/v1/sign-in-links",
    operation_id = "create_sign_in_link",
    responses(
        (status = 201, description = "The new link", body = CreatedLink),
        (status = 401, description = "`human_name_required`: the request names no human", body = ErrorEnvelope),
        (status = 404, description = "Not on the socket", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn create_link(
    _socket: SocketOnly,
    State(state): State<AppState>,
    human: Human,
) -> Result<(StatusCode, Json<CreatedLink>), ApiError> {
    let link =
        projects::create_sign_in_link(&state.registry, state.clock.as_ref(), &human.0).await?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedLink {
            url: format!("{}{SIGN_IN_ROUTE}{}", web_origin(state.listen), link.secret),
            expires_at: link.expires_at,
            expires_in_secs: projects::SIGN_IN_LINK_LIFETIME.as_secs(),
        }),
    ))
}
