use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use fold_core::projects::notify::{SmtpSecurity, shown_url};
use fold_core::{FoldError, NotifyTargetId, Timestamp, codes};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::super::Project;
use super::super::actor::Human;
use super::super::error::{ApiError, ApiJson, ErrorEnvelope};
use crate::daemon::AppState;
use crate::services::projects::notify::{
    self, Channel, NewChannel, NewTarget, NotifySettings, Scope, Target,
};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(show_host_notify, set_host_notify))
        .routes(routes!(add_host_target))
        .routes(routes!(remove_host_target))
        .routes(routes!(test_host_target))
        .routes(routes!(show_project_notify))
        .routes(routes!(add_project_target))
        .routes(routes!(remove_project_target))
        .routes(routes!(test_project_target))
}

fn settings(state: &AppState) -> Result<&NotifySettings, ApiError> {
    state.projects.notify().ok_or_else(|| {
        FoldError::new(
            codes::NOTIFY_SETTINGS_IO,
            "this daemon keeps no notification settings",
        )
        .into()
    })
}

fn target_id(id: &str) -> Result<NotifyTargetId, ApiError> {
    Ok(id.parse::<NotifyTargetId>().map_err(FoldError::from)?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotifyTargetKind {
    Webhook,
    Email,
}

/// How an email target reaches its SMTP server: `tls` from the first byte (port 465 by default), `starttls` on a plain connection that is upgraded first (port 587), `none` for a relay on a trusted network (port 25), which takes no password.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotifySecurity {
    Tls,
    Starttls,
    None,
}

impl From<NotifySecurity> for SmtpSecurity {
    fn from(security: NotifySecurity) -> Self {
        match security {
            NotifySecurity::Tls => SmtpSecurity::Tls,
            NotifySecurity::Starttls => SmtpSecurity::Starttls,
            NotifySecurity::None => SmtpSecurity::None,
        }
    }
}

impl From<SmtpSecurity> for NotifySecurity {
    fn from(security: SmtpSecurity) -> Self {
        match security {
            SmtpSecurity::Tls => NotifySecurity::Tls,
            SmtpSecurity::Starttls => NotifySecurity::Starttls,
            SmtpSecurity::None => NotifySecurity::None,
        }
    }
}

/// One notification target. Its signing secret or SMTP password never leaves the daemon, and the webhook URL shows without its path, which can hold a token.
#[derive(Debug, Serialize, ToSchema)]
pub struct NotifyTargetEntry {
    #[schema(example = "nt-3")]
    pub id: String,
    pub kind: NotifyTargetKind,
    /// The shortest time between two sends to the target.
    #[schema(example = 10)]
    pub interval_minutes: u32,
    /// UTC milliseconds of when a human added the target.
    #[schema(value_type = i64)]
    pub created_at: Timestamp,
    #[schema(example = "tanim")]
    pub created_by: String,
    /// A webhook's scheme, host and port, with `/…` when a path or query follows; null for an email target.
    #[schema(required = true)]
    pub url: Option<String>,
    /// The address an email target sends to.
    #[schema(required = true, example = "team@example.com")]
    pub to: Option<String>,
    /// The sender address of an email target.
    #[schema(required = true, example = "fold@example.com")]
    pub from: Option<String>,
    #[schema(required = true, example = "smtp.example.com")]
    pub smtp_host: Option<String>,
    #[schema(required = true, example = 587)]
    pub smtp_port: Option<u16>,
    #[schema(required = true)]
    pub security: Option<NotifySecurity>,
    /// The SMTP user name; null when the target signs in to no SMTP server.
    #[schema(required = true)]
    pub username: Option<String>,
    /// True when fold keeps the webhook's signing secret or the email target's SMTP password.
    pub secret_set: bool,
}

impl From<Target> for NotifyTargetEntry {
    fn from(target: Target) -> Self {
        let mut entry = Self {
            id: target.id.to_string(),
            kind: NotifyTargetKind::Webhook,
            interval_minutes: target.interval_minutes,
            created_at: target.created_at,
            created_by: target.created_by,
            url: None,
            to: None,
            from: None,
            smtp_host: None,
            smtp_port: None,
            security: None,
            username: None,
            secret_set: false,
        };
        match target.channel {
            Channel::Webhook { url, .. } => {
                entry.url = Some(shown_url(&url));
                entry.secret_set = true;
            }
            Channel::Email(email) => {
                entry.kind = NotifyTargetKind::Email;
                entry.to = Some(email.to);
                entry.from = Some(email.from);
                entry.smtp_host = Some(email.smtp_host);
                entry.smtp_port = Some(email.smtp_port);
                entry.security = Some(email.security.into());
                entry.username = email.username;
                entry.secret_set = email.password.is_some();
            }
        }
        entry
    }
}

fn entries(targets: Vec<Target>) -> Vec<NotifyTargetEntry> {
    targets.into_iter().map(Into::into).collect()
}

/// A new target. A webhook needs `url` and `secret`; an email target needs `to`, `from` and `smtp_host`, and takes `smtp_port`, `security` (`starttls` by default), `username` and `password`. The fields of the other kind are refused.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewNotifyTargetBody {
    pub kind: NotifyTargetKind,
    /// 1 to 1440 minutes; 10 when left out.
    #[schema(example = 10)]
    pub interval_minutes: Option<u32>,
    pub url: Option<String>,
    /// The key of the HMAC-SHA256 signature in each post's `x-fold-signature-256` header: 16 to 512 bytes without spaces. fold never shows it again.
    pub secret: Option<String>,
    #[schema(example = "team@example.com")]
    pub to: Option<String>,
    #[schema(example = "fold@example.com")]
    pub from: Option<String>,
    #[schema(example = "smtp.example.com")]
    pub smtp_host: Option<String>,
    #[schema(example = 587)]
    pub smtp_port: Option<u16>,
    pub security: Option<NotifySecurity>,
    pub username: Option<String>,
    /// fold never shows it again.
    pub password: Option<String>,
}

fn needs(field: Option<String>, name: &str, kind: &str) -> Result<String, FoldError> {
    field.ok_or_else(|| {
        FoldError::new(
            codes::INVALID_NOTIFY_TARGET,
            format!("a {kind} target needs {name}"),
        )
    })
}

impl TryFrom<NewNotifyTargetBody> for NewTarget {
    type Error = FoldError;

    fn try_from(body: NewNotifyTargetBody) -> Result<Self, FoldError> {
        let channel = match body.kind {
            NotifyTargetKind::Webhook => {
                if body.to.is_some()
                    || body.from.is_some()
                    || body.smtp_host.is_some()
                    || body.smtp_port.is_some()
                    || body.security.is_some()
                    || body.username.is_some()
                    || body.password.is_some()
                {
                    return Err(FoldError::new(
                        codes::INVALID_NOTIFY_TARGET,
                        "a webhook target takes url and secret, not email fields",
                    ));
                }
                NewChannel::Webhook {
                    url: needs(body.url, "url", "webhook")?,
                    secret: needs(body.secret, "secret", "webhook")?,
                }
            }
            NotifyTargetKind::Email => {
                if body.url.is_some() || body.secret.is_some() {
                    return Err(FoldError::new(
                        codes::INVALID_NOTIFY_TARGET,
                        "an email target takes no url and no secret",
                    ));
                }
                NewChannel::Email {
                    to: needs(body.to, "to", "email")?,
                    from: needs(body.from, "from", "email")?,
                    smtp_host: needs(body.smtp_host, "smtp_host", "email")?,
                    smtp_port: body.smtp_port,
                    security: body.security.unwrap_or(NotifySecurity::Starttls).into(),
                    username: body.username,
                    password: body.password,
                }
            }
        };
        Ok(NewTarget {
            interval_minutes: body.interval_minutes,
            channel,
        })
    }
}

/// The host's notification settings: the targets every project without targets of its own uses, and the web address of links.
#[derive(Debug, Serialize, ToSchema)]
pub struct HostNotify {
    /// Oldest first.
    pub targets: Vec<NotifyTargetEntry>,
    /// The web address a human set for links; null when links follow the daemon's listen address.
    #[schema(required = true)]
    pub web_url: Option<String>,
    /// The address every link of a notice starts with now.
    pub link_base: String,
}

async fn host_notify(state: &AppState) -> Result<HostNotify, ApiError> {
    let settings = settings(state)?;
    Ok(HostNotify {
        targets: entries(settings.targets(&Scope::Host).await?),
        web_url: settings.web_url().await?,
        link_base: notify::link_base(settings, state.listen).await?,
    })
}

/// The host's notification targets and the web address of links.
#[utoipa::path(
    get,
    path = "/api/v1/notify",
    operation_id = "show_host_notify",
    responses(
        (status = 200, description = "The host's settings", body = HostNotify),
        (status = 500, description = "`notify_settings_io` or internal error", body = ErrorEnvelope),
    )
)]
pub async fn show_host_notify(State(state): State<AppState>) -> Result<Json<HostNotify>, ApiError> {
    Ok(Json(host_notify(&state).await?))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetHostNotifyBody {
    /// The full address people open fold at, with its http or https scheme; null or empty to let links follow the daemon's listen address.
    pub web_url: Option<String>,
}

/// A named human sets the web address that links in notices start with.
#[utoipa::path(
    put,
    path = "/api/v1/notify",
    operation_id = "set_host_notify",
    request_body = SetHostNotifyBody,
    responses(
        (status = 200, description = "The host's settings after the change", body = HostNotify),
        (status = 400, description = "`invalid_request`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 500, description = "`notify_settings_io` or internal error", body = ErrorEnvelope),
    )
)]
pub async fn set_host_notify(
    State(state): State<AppState>,
    human: Human,
    ApiJson(body): ApiJson<SetHostNotifyBody>,
) -> Result<Json<HostNotify>, ApiError> {
    notify::set_web_url(settings(&state)?, &human.actor(), body.web_url).await?;
    Ok(Json(host_notify(&state).await?))
}

/// A named human adds a host target, which gets the notices of every project without targets of its own.
#[utoipa::path(
    post,
    path = "/api/v1/notify/targets",
    operation_id = "add_host_notify_target",
    request_body = NewNotifyTargetBody,
    responses(
        (status = 201, description = "The new target", body = NotifyTargetEntry),
        (status = 400, description = "`invalid_notify_target` or `invalid_request`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 500, description = "`notify_settings_io` or internal error", body = ErrorEnvelope),
    )
)]
pub async fn add_host_target(
    State(state): State<AppState>,
    human: Human,
    ApiJson(body): ApiJson<NewNotifyTargetBody>,
) -> Result<(StatusCode, Json<NotifyTargetEntry>), ApiError> {
    let new = NewTarget::try_from(body)?;
    let target = notify::add_target(
        settings(&state)?,
        &human.actor(),
        &Scope::Host,
        new,
        state.clock.now(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(target.into())))
}

/// A named human removes a host target.
#[utoipa::path(
    delete,
    path = "/api/v1/notify/targets/{id}",
    operation_id = "remove_host_notify_target",
    params(("id" = String, Path, description = "The target, such as `nt-3`")),
    responses(
        (status = 204, description = "The target is gone"),
        (status = 400, description = "`invalid_id`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`notify_target_not_found`", body = ErrorEnvelope),
        (status = 500, description = "`notify_settings_io` or internal error", body = ErrorEnvelope),
    )
)]
pub async fn remove_host_target(
    State(state): State<AppState>,
    human: Human,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    notify::remove_target(
        settings(&state)?,
        &human.actor(),
        &Scope::Host,
        target_id(&id)?,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A named human sends a test notice through a host target and waits until its receiver or SMTP server took it.
#[utoipa::path(
    post,
    path = "/api/v1/notify/targets/{id}/test",
    operation_id = "test_host_notify_target",
    params(("id" = String, Path, description = "The target, such as `nt-3`")),
    responses(
        (status = 204, description = "The receiver took the test notice"),
        (status = 400, description = "`invalid_id`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`notify_target_not_found`", body = ErrorEnvelope),
        (status = 502, description = "`notify_send_failed`: the message says what failed", body = ErrorEnvelope),
        (status = 500, description = "`notify_settings_io` or internal error", body = ErrorEnvelope),
    )
)]
pub async fn test_host_target(
    State(state): State<AppState>,
    human: Human,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    notify::test_target(
        settings(&state)?,
        &human.actor(),
        &Scope::Host,
        target_id(&id)?,
        None,
        state.clock.now(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A project's notification settings.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectNotify {
    /// The project's own targets, oldest first.
    pub targets: Vec<NotifyTargetEntry>,
    /// The host's targets, oldest first.
    pub host_targets: Vec<NotifyTargetEntry>,
    /// True while the project has no targets of its own, so the host's targets get its notices.
    pub uses_host: bool,
    /// The address every link of a notice starts with now.
    pub link_base: String,
}

async fn project_notify(state: &AppState, slug: &str) -> Result<ProjectNotify, ApiError> {
    let settings = settings(state)?;
    let targets = settings.targets(&Scope::Project(slug.to_owned())).await?;
    Ok(ProjectNotify {
        uses_host: targets.is_empty(),
        targets: entries(targets),
        host_targets: entries(settings.targets(&Scope::Host).await?),
        link_base: notify::link_base(settings, state.listen).await?,
    })
}

/// Where the project's notices go: one notice when an item comes into Needs you, at most one digest per target in each of its intervals, and nothing for an item a human closes before the send.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/notify",
    operation_id = "show_project_notify",
    params(("slug" = String, Path, description = "The project slug")),
    responses(
        (status = 200, description = "The project's settings", body = ProjectNotify),
        (status = 404, description = "`project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "`notify_settings_io` or internal error", body = ErrorEnvelope),
    )
)]
pub async fn show_project_notify(
    State(state): State<AppState>,
    project: Project,
) -> Result<Json<ProjectNotify>, ApiError> {
    Ok(Json(project_notify(&state, &project.0.slug).await?))
}

/// A named human adds a target of the project. From then on the project's own targets get its notices instead of the host's.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/notify/targets",
    operation_id = "add_project_notify_target",
    params(("slug" = String, Path, description = "The project slug")),
    request_body = NewNotifyTargetBody,
    responses(
        (status = 201, description = "The new target", body = NotifyTargetEntry),
        (status = 400, description = "`invalid_notify_target` or `invalid_request`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "`notify_settings_io` or internal error", body = ErrorEnvelope),
    )
)]
pub async fn add_project_target(
    State(state): State<AppState>,
    project: Project,
    human: Human,
    ApiJson(body): ApiJson<NewNotifyTargetBody>,
) -> Result<(StatusCode, Json<NotifyTargetEntry>), ApiError> {
    let new = NewTarget::try_from(body)?;
    let target = notify::add_target(
        settings(&state)?,
        &human.actor(),
        &Scope::Project(project.0.slug.clone()),
        new,
        project.0.clock.now(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(target.into())))
}

/// A named human removes a target of the project.
#[utoipa::path(
    delete,
    path = "/api/v1/projects/{slug}/notify/targets/{id}",
    operation_id = "remove_project_notify_target",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The target, such as `nt-3`"),
    ),
    responses(
        (status = 204, description = "The target is gone"),
        (status = 400, description = "`invalid_id`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`notify_target_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "`notify_settings_io` or internal error", body = ErrorEnvelope),
    )
)]
pub async fn remove_project_target(
    State(state): State<AppState>,
    project: Project,
    human: Human,
    Path((_slug, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    notify::remove_target(
        settings(&state)?,
        &human.actor(),
        &Scope::Project(project.0.slug.clone()),
        target_id(&id)?,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A named human sends a test notice that names the project through one of its targets, or one of the host's, and waits until the receiver or SMTP server took it.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/notify/targets/{id}/test",
    operation_id = "test_project_notify_target",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The target, such as `nt-3`"),
    ),
    responses(
        (status = 204, description = "The receiver took the test notice"),
        (status = 400, description = "`invalid_id`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`notify_target_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 502, description = "`notify_send_failed`: the message says what failed", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "`notify_settings_io` or internal error", body = ErrorEnvelope),
    )
)]
pub async fn test_project_target(
    State(state): State<AppState>,
    project: Project,
    human: Human,
    Path((_slug, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let settings = settings(&state)?;
    let base = notify::link_base(settings, state.listen).await?;
    let handle = &project.0;
    notify::test_target(
        settings,
        &human.actor(),
        &Scope::Project(handle.slug.clone()),
        target_id(&id)?,
        Some(notify::notice_project(&base, &handle.slug, &handle.name)),
        handle.clock.now(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
