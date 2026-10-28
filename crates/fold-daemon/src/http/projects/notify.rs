use std::collections::BTreeSet;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use fold_core::projects::notify::{NoticeType, Preset, TargetKind, shown_url};
use fold_core::{FoldError, NotifySendId, NotifyTargetId, Timestamp, codes};
use fold_store::projects::notify::{self as outbox, SendRow, SendState};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::super::Project;
use super::super::actor::Human;
use super::super::error::{ApiError, ApiJson, ErrorEnvelope};
use super::super::tasks::TaskStatus;
use crate::daemon::AppState;
use crate::services::error::from_store;
use crate::services::projects::notify::{
    self, NotifySettings, OffReason, Scope, Target, TargetFields,
};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(show_host_notify, set_host_notify))
        .routes(routes!(add_host_target))
        .routes(routes!(edit_host_target, remove_host_target))
        .routes(routes!(rotate_host_target))
        .routes(routes!(show_project_notify, set_project_notify))
        .routes(routes!(add_project_target))
        .routes(routes!(edit_project_target, remove_project_target))
        .routes(routes!(rotate_project_target))
        .routes(routes!(test_notify_target))
        .routes(routes!(list_notify_sends))
        .routes(routes!(send_notify_again))
}

fn settings(state: &AppState) -> Result<&NotifySettings, ApiError> {
    state.projects.notify().ok_or_else(|| {
        FoldError::new(
            codes::NOTIFY_SETTINGS_IO,
            "this daemon keeps no notice settings",
        )
        .into()
    })
}

fn target_id(id: &str) -> Result<NotifyTargetId, ApiError> {
    Ok(id.parse::<NotifyTargetId>().map_err(FoldError::from)?)
}

/// What a target sends to: `webhook` gets the signed generic JSON, `slack` and `discord` a message for their incoming webhook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotifyTargetKind {
    Webhook,
    Slack,
    Discord,
}

impl From<NotifyTargetKind> for TargetKind {
    fn from(kind: NotifyTargetKind) -> Self {
        match kind {
            NotifyTargetKind::Webhook => TargetKind::Webhook,
            NotifyTargetKind::Slack => TargetKind::Slack,
            NotifyTargetKind::Discord => TargetKind::Discord,
        }
    }
}

impl From<TargetKind> for NotifyTargetKind {
    fn from(kind: TargetKind) -> Self {
        match kind {
            TargetKind::Webhook => NotifyTargetKind::Webhook,
            TargetKind::Slack => NotifyTargetKind::Slack,
            TargetKind::Discord => NotifyTargetKind::Discord,
        }
    }
}

/// A notice type a target selects. `webhook.test` is the type of a test send, which every target gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
pub enum NotifyEventName {
    #[serde(rename = "needs_you.opened")]
    NeedsYouOpened,
    #[serde(rename = "needs_you.closed")]
    NeedsYouClosed,
    #[serde(rename = "task.created")]
    TaskCreated,
    #[serde(rename = "task.moved")]
    TaskMoved,
    #[serde(rename = "task.blocked")]
    TaskBlocked,
    #[serde(rename = "task.sent_back")]
    TaskSentBack,
    #[serde(rename = "proposal.opened")]
    ProposalOpened,
    #[serde(rename = "review.approved")]
    ReviewApproved,
    #[serde(rename = "review.changes_requested")]
    ReviewChangesRequested,
    #[serde(rename = "proposal.integrated")]
    ProposalIntegrated,
    #[serde(rename = "proposal.closed")]
    ProposalClosed,
    #[serde(rename = "integration.failed")]
    IntegrationFailed,
    #[serde(rename = "checks.failed")]
    ChecksFailed,
    #[serde(rename = "worker.failed")]
    WorkerFailed,
    #[serde(rename = "git.failed")]
    GitFailed,
    #[serde(rename = "message.to_human")]
    MessageToHuman,
    #[serde(rename = "request.approved")]
    RequestApproved,
    #[serde(rename = "project.paused")]
    ProjectPaused,
    #[serde(rename = "webhook.test")]
    WebhookTest,
}

impl From<NoticeType> for NotifyEventName {
    fn from(kind: NoticeType) -> Self {
        match kind {
            NoticeType::NeedsYouOpened => Self::NeedsYouOpened,
            NoticeType::NeedsYouClosed => Self::NeedsYouClosed,
            NoticeType::TaskCreated => Self::TaskCreated,
            NoticeType::TaskMoved => Self::TaskMoved,
            NoticeType::TaskBlocked => Self::TaskBlocked,
            NoticeType::TaskSentBack => Self::TaskSentBack,
            NoticeType::ProposalOpened => Self::ProposalOpened,
            NoticeType::ReviewApproved => Self::ReviewApproved,
            NoticeType::ReviewChangesRequested => Self::ReviewChangesRequested,
            NoticeType::ProposalIntegrated => Self::ProposalIntegrated,
            NoticeType::ProposalClosed => Self::ProposalClosed,
            NoticeType::IntegrationFailed => Self::IntegrationFailed,
            NoticeType::ChecksFailed => Self::ChecksFailed,
            NoticeType::WorkerFailed => Self::WorkerFailed,
            NoticeType::GitFailed => Self::GitFailed,
            NoticeType::MessageToHuman => Self::MessageToHuman,
            NoticeType::RequestApproved => Self::RequestApproved,
            NoticeType::ProjectPaused => Self::ProjectPaused,
            NoticeType::WebhookTest => Self::WebhookTest,
        }
    }
}

impl From<NotifyEventName> for NoticeType {
    fn from(name: NotifyEventName) -> Self {
        match name {
            NotifyEventName::NeedsYouOpened => Self::NeedsYouOpened,
            NotifyEventName::NeedsYouClosed => Self::NeedsYouClosed,
            NotifyEventName::TaskCreated => Self::TaskCreated,
            NotifyEventName::TaskMoved => Self::TaskMoved,
            NotifyEventName::TaskBlocked => Self::TaskBlocked,
            NotifyEventName::TaskSentBack => Self::TaskSentBack,
            NotifyEventName::ProposalOpened => Self::ProposalOpened,
            NotifyEventName::ReviewApproved => Self::ReviewApproved,
            NotifyEventName::ReviewChangesRequested => Self::ReviewChangesRequested,
            NotifyEventName::ProposalIntegrated => Self::ProposalIntegrated,
            NotifyEventName::ProposalClosed => Self::ProposalClosed,
            NotifyEventName::IntegrationFailed => Self::IntegrationFailed,
            NotifyEventName::ChecksFailed => Self::ChecksFailed,
            NotifyEventName::WorkerFailed => Self::WorkerFailed,
            NotifyEventName::GitFailed => Self::GitFailed,
            NotifyEventName::MessageToHuman => Self::MessageToHuman,
            NotifyEventName::RequestApproved => Self::RequestApproved,
            NotifyEventName::ProjectPaused => Self::ProjectPaused,
            NotifyEventName::WebhookTest => Self::WebhookTest,
        }
    }
}

/// A named selection of events: `needs_you_only`, `recommended`, `all`, or `custom` for a selection made event by event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotifyPreset {
    NeedsYouOnly,
    Recommended,
    All,
    Custom,
}

impl From<Preset> for NotifyPreset {
    fn from(preset: Preset) -> Self {
        match preset {
            Preset::NeedsYouOnly => Self::NeedsYouOnly,
            Preset::Recommended => Self::Recommended,
            Preset::All => Self::All,
            Preset::Custom => Self::Custom,
        }
    }
}

impl From<NotifyPreset> for Preset {
    fn from(preset: NotifyPreset) -> Self {
        match preset {
            NotifyPreset::NeedsYouOnly => Self::NeedsYouOnly,
            NotifyPreset::Recommended => Self::Recommended,
            NotifyPreset::All => Self::All,
            NotifyPreset::Custom => Self::Custom,
        }
    }
}

/// Whether a target gets notices: `on`; `off`, which a person chose; `gone`, as its receiver answered 410 Gone; or `failing`, as fold turned it off after 20 failed tries in a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotifyTargetState {
    On,
    Off,
    Gone,
    Failing,
}

/// One notice target. Its signing secret never leaves the daemon after its add or rotation, and its URL shows without its path, which can hold a token.
#[derive(Debug, Serialize, ToSchema)]
pub struct NotifyTargetEntry {
    #[schema(example = "nt-3")]
    pub id: String,
    #[schema(example = "Team chat")]
    pub name: String,
    pub kind: NotifyTargetKind,
    /// The URL's scheme, host and port, with `/…` when a path or a query follows.
    pub url: String,
    pub events: Vec<NotifyEventName>,
    pub preset: NotifyPreset,
    /// The columns whose card moves the target gets.
    pub moves_to: Vec<TaskStatus>,
    /// The notices of this many seconds after the first one go in one send; 0 sends each alone.
    #[schema(example = 15)]
    pub window_seconds: u32,
    /// A person's switch.
    pub enabled: bool,
    pub state: NotifyTargetState,
    /// Failed tries in a row.
    pub failures: u32,
    /// UTC milliseconds of the last try that got through.
    #[schema(required = true, value_type = Option<i64>)]
    pub last_ok_at: Option<Timestamp>,
    #[schema(required = true, value_type = Option<i64>)]
    pub last_failed_at: Option<Timestamp>,
    /// UTC milliseconds of this project's next try of a failed send to the target; null on the host's page.
    #[schema(required = true, value_type = Option<i64>)]
    pub next_try_at: Option<Timestamp>,
    /// True for a target of the host.
    pub host: bool,
    #[schema(value_type = i64)]
    pub created_at: Timestamp,
    #[schema(example = "tanim")]
    pub created_by: String,
}

impl NotifyTargetEntry {
    fn new(target: Target, host: bool, next_try_at: Option<Timestamp>) -> Self {
        let state = match target.health.off {
            Some(OffReason::Gone) => NotifyTargetState::Gone,
            Some(OffReason::Failures) => NotifyTargetState::Failing,
            None if !target.enabled => NotifyTargetState::Off,
            None => NotifyTargetState::On,
        };
        Self {
            id: target.id.to_string(),
            name: target.name,
            kind: target.kind.into(),
            url: shown_url(target.url.expose()),
            events: target
                .filter
                .types
                .iter()
                .map(|kind| (*kind).into())
                .collect(),
            preset: Preset::of(&target.filter.types).into(),
            moves_to: target
                .filter
                .moves_to
                .iter()
                .map(|status| (*status).into())
                .collect(),
            window_seconds: target.window_seconds,
            enabled: target.enabled,
            state,
            failures: target.health.failures,
            last_ok_at: target.health.last_ok_at,
            last_failed_at: target.health.last_failed_at,
            next_try_at,
            host,
            created_at: target.created_at,
            created_by: target.created_by,
        }
    }
}

/// The fields of a new or changed target. On a change, a field left out keeps its value; on an add it takes the default.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NotifyTargetFields {
    #[schema(example = "Team chat")]
    pub name: Option<String>,
    /// The receiver's URL, or the incoming webhook URL Slack or Discord gave: https, or http only to localhost or a loopback address. fold never shows it whole again.
    pub url: Option<String>,
    /// A preset of events; `custom` or no preset takes `events`.
    pub preset: Option<NotifyPreset>,
    pub events: Option<Vec<NotifyEventName>>,
    /// The columns whose card moves the target gets; `review` and `done` on an add that leaves it out.
    pub moves_to: Option<Vec<TaskStatus>>,
    /// 0 to 3600; 15 on an add that leaves it out.
    #[schema(example = 15)]
    pub window_seconds: Option<u32>,
    pub enabled: Option<bool>,
}

impl TryFrom<NotifyTargetFields> for TargetFields {
    type Error = FoldError;

    fn try_from(body: NotifyTargetFields) -> Result<Self, FoldError> {
        let types = match (body.preset.map(Preset::from), body.events) {
            (Some(Preset::Custom) | None, Some(events)) => {
                Some(events.into_iter().map(NoticeType::from).collect())
            }
            (Some(Preset::Custom), None) => {
                return Err(FoldError::new(
                    codes::INVALID_NOTIFY_TARGET,
                    "the preset custom needs its events",
                ));
            }
            (Some(preset), None) => Some(preset.types()),
            (Some(_), Some(_)) => {
                return Err(FoldError::new(
                    codes::INVALID_NOTIFY_TARGET,
                    "give a preset or events, not both",
                ));
            }
            (None, None) => None,
        };
        Ok(TargetFields {
            name: body.name,
            url: body.url,
            types,
            moves_to: body.moves_to.map(|columns| {
                columns
                    .into_iter()
                    .map(fold_core::TaskStatus::from)
                    .collect::<BTreeSet<_>>()
            }),
            window_seconds: body.window_seconds,
            enabled: body.enabled,
        })
    }
}

/// A new target: its kind, name and URL, and the fields that take their default when left out.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewNotifyTargetBody {
    pub kind: NotifyTargetKind,
    #[schema(example = "Team chat")]
    pub name: String,
    /// The receiver's URL, or the incoming webhook URL Slack or Discord gave: https, or http only to localhost or a loopback address. fold never shows it whole again.
    pub url: String,
    /// A preset of events, `recommended` when both it and `events` are left out; `custom` or no preset takes `events`.
    pub preset: Option<NotifyPreset>,
    pub events: Option<Vec<NotifyEventName>>,
    /// The columns whose card moves the target gets; `review` and `done` when left out.
    pub moves_to: Option<Vec<TaskStatus>>,
    /// 0 to 3600; 15 when left out.
    #[schema(example = 15)]
    pub window_seconds: Option<u32>,
    pub enabled: Option<bool>,
}

impl From<NewNotifyTargetBody> for NotifyTargetFields {
    fn from(body: NewNotifyTargetBody) -> Self {
        Self {
            name: Some(body.name),
            url: Some(body.url),
            preset: body.preset,
            events: body.events,
            moves_to: body.moves_to,
            window_seconds: body.window_seconds,
            enabled: body.enabled,
        }
    }
}

/// A target as an add or a rotation leaves it, with the signing secret of a generic target, which fold shows only in this answer.
#[derive(Debug, Serialize, ToSchema)]
pub struct NotifyTargetAdded {
    pub target: NotifyTargetEntry,
    /// `whsec_` and base64: the key of each post's `webhook-signature`. Null for Slack and Discord, whose URL is their secret.
    #[schema(required = true)]
    pub secret: Option<String>,
}

/// The host's notice settings: the targets that every project that uses the host targets sends to, and the web address of links.
#[derive(Debug, Serialize, ToSchema)]
pub struct HostNotify {
    /// Oldest first.
    pub targets: Vec<NotifyTargetEntry>,
    /// The web address links start with; null when notices carry no links.
    #[schema(required = true)]
    pub web_url: Option<String>,
}

async fn host_notify(state: &AppState) -> Result<HostNotify, ApiError> {
    let settings = settings(state)?;
    Ok(HostNotify {
        targets: settings
            .targets(&Scope::Host)
            .await?
            .into_iter()
            .map(|target| NotifyTargetEntry::new(target, true, None))
            .collect(),
        web_url: settings.web_url().await?,
    })
}

/// The host's notice targets and the web address of links.
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
    /// The address people open fold at, with its scheme; null or empty for notices without links.
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

async fn add(
    state: &AppState,
    human: &Human,
    scope: Scope,
    body: NewNotifyTargetBody,
    now: Timestamp,
) -> Result<(StatusCode, Json<NotifyTargetAdded>), ApiError> {
    let kind = body.kind.into();
    let fields = TargetFields::try_from(NotifyTargetFields::from(body))?;
    let host = scope == Scope::Host;
    let (target, secret) =
        notify::add_target(settings(state)?, &human.actor(), &scope, kind, fields, now).await?;
    Ok((
        StatusCode::CREATED,
        Json(NotifyTargetAdded {
            target: NotifyTargetEntry::new(target, host, None),
            secret,
        }),
    ))
}

/// A named human adds a host target, which gets the notices of every project that uses the host targets. A generic target's answer holds its signing secret, shown only there.
#[utoipa::path(
    post,
    path = "/api/v1/notify/targets",
    operation_id = "add_host_notify_target",
    request_body = NewNotifyTargetBody,
    responses(
        (status = 201, description = "The new target", body = NotifyTargetAdded),
        (status = 400, description = "`invalid_notify_target`, `plaintext_notify_target` (a plain-text URL off this machine) or `invalid_request`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 500, description = "`notify_settings_io` or internal error", body = ErrorEnvelope),
    )
)]
pub async fn add_host_target(
    State(state): State<AppState>,
    human: Human,
    ApiJson(body): ApiJson<NewNotifyTargetBody>,
) -> Result<(StatusCode, Json<NotifyTargetAdded>), ApiError> {
    let now = state.clock.now();
    add(&state, &human, Scope::Host, body, now).await
}

/// A named human changes a host target. A change starts it with a clean health.
#[utoipa::path(
    put,
    path = "/api/v1/notify/targets/{id}",
    operation_id = "edit_host_notify_target",
    params(("id" = String, Path, description = "The target, such as `nt-3`")),
    request_body = NotifyTargetFields,
    responses(
        (status = 200, description = "The target after the change", body = NotifyTargetEntry),
        (status = 400, description = "`invalid_notify_target`, `plaintext_notify_target`, `invalid_id` or `invalid_request`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`notify_target_not_found`", body = ErrorEnvelope),
        (status = 500, description = "`notify_settings_io` or internal error", body = ErrorEnvelope),
    )
)]
pub async fn edit_host_target(
    State(state): State<AppState>,
    human: Human,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<NotifyTargetFields>,
) -> Result<Json<NotifyTargetEntry>, ApiError> {
    let target = notify::edit_target(
        settings(&state)?,
        &human.actor(),
        &Scope::Host,
        target_id(&id)?,
        TargetFields::try_from(body)?,
    )
    .await?;
    Ok(Json(NotifyTargetEntry::new(target, true, None)))
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

async fn rotate(
    state: &AppState,
    human: &Human,
    scope: Scope,
    id: NotifyTargetId,
    now: Timestamp,
) -> Result<Json<NotifyTargetAdded>, ApiError> {
    let settings = settings(state)?;
    let secret = notify::rotate_secret(settings, &human.actor(), &scope, id, now).await?;
    let host = scope == Scope::Host;
    let target = settings.find(&scope, id).await?;
    Ok(Json(NotifyTargetAdded {
        target: NotifyTargetEntry::new(target, host, None),
        secret: Some(secret),
    }))
}

/// A named human gives a generic host target a new signing secret, shown only in the answer. For a day each post carries a signature with the old secret too.
#[utoipa::path(
    post,
    path = "/api/v1/notify/targets/{id}/rotate",
    operation_id = "rotate_host_notify_target",
    params(("id" = String, Path, description = "The target, such as `nt-3`")),
    responses(
        (status = 200, description = "The target and its new secret", body = NotifyTargetAdded),
        (status = 400, description = "`invalid_notify_target` (a Slack or Discord target) or `invalid_id`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`notify_target_not_found`", body = ErrorEnvelope),
        (status = 500, description = "`notify_settings_io` or internal error", body = ErrorEnvelope),
    )
)]
pub async fn rotate_host_target(
    State(state): State<AppState>,
    human: Human,
    Path(id): Path<String>,
) -> Result<Json<NotifyTargetAdded>, ApiError> {
    let now = state.clock.now();
    rotate(&state, &human, Scope::Host, target_id(&id)?, now).await
}

/// A project's notice settings.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectNotify {
    /// The project's own targets, oldest first.
    pub targets: Vec<NotifyTargetEntry>,
    /// The host's targets, oldest first, which get the project's notices while `use_host_targets` is on.
    pub host_targets: Vec<NotifyTargetEntry>,
    pub use_host_targets: bool,
    /// The web address links start with; null when notices carry no links.
    #[schema(required = true)]
    pub web_url: Option<String>,
}

async fn project_notify(state: &AppState, project: &Project) -> Result<ProjectNotify, ApiError> {
    let settings = settings(state)?;
    let handle = &project.0;
    let open = outbox::open_sends(handle.store.read())
        .await
        .map_err(from_store)?;
    let next = |id: NotifyTargetId| {
        open.iter()
            .filter(|send| send.target == id && send.state == SendState::Pending && send.tries > 0)
            .filter_map(|send| send.next_try_at)
            .min()
    };
    let entries = |targets: Vec<Target>, host: bool| -> Vec<NotifyTargetEntry> {
        targets
            .into_iter()
            .map(|target| {
                let next_try_at = next(target.id);
                NotifyTargetEntry::new(target, host, next_try_at)
            })
            .collect()
    };
    Ok(ProjectNotify {
        targets: entries(
            settings
                .targets(&Scope::Project(handle.slug.clone()))
                .await?,
            false,
        ),
        host_targets: entries(settings.targets(&Scope::Host).await?, true),
        use_host_targets: settings.use_host_targets(&handle.slug).await?,
        web_url: settings.web_url().await?,
    })
}

/// Where the project's notices go: its own targets and, while it uses them, the host's. Each target gets the events it selected, grouped in its window.
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
    Ok(Json(project_notify(&state, &project).await?))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetProjectNotifyBody {
    /// Whether the host's targets get the project's notices too.
    pub use_host_targets: bool,
}

/// A named human sets whether the host's targets get the project's notices too.
#[utoipa::path(
    put,
    path = "/api/v1/projects/{slug}/notify",
    operation_id = "set_project_notify",
    params(("slug" = String, Path, description = "The project slug")),
    request_body = SetProjectNotifyBody,
    responses(
        (status = 200, description = "The project's settings after the change", body = ProjectNotify),
        (status = 400, description = "`invalid_request`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "`notify_settings_io` or internal error", body = ErrorEnvelope),
    )
)]
pub async fn set_project_notify(
    State(state): State<AppState>,
    project: Project,
    human: Human,
    ApiJson(body): ApiJson<SetProjectNotifyBody>,
) -> Result<Json<ProjectNotify>, ApiError> {
    notify::set_use_host_targets(
        settings(&state)?,
        &human.actor(),
        &project.0.slug,
        body.use_host_targets,
    )
    .await?;
    Ok(Json(project_notify(&state, &project).await?))
}

/// A named human adds a target of the project. A generic target's answer holds its signing secret, shown only there.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/notify/targets",
    operation_id = "add_project_notify_target",
    params(("slug" = String, Path, description = "The project slug")),
    request_body = NewNotifyTargetBody,
    responses(
        (status = 201, description = "The new target", body = NotifyTargetAdded),
        (status = 400, description = "`invalid_notify_target`, `plaintext_notify_target` (a plain-text URL off this machine) or `invalid_request`", body = ErrorEnvelope),
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
) -> Result<(StatusCode, Json<NotifyTargetAdded>), ApiError> {
    let now = project.0.clock.now();
    add(
        &state,
        &human,
        Scope::Project(project.0.slug.clone()),
        body,
        now,
    )
    .await
}

/// A named human changes a target of the project. A change starts it with a clean health.
#[utoipa::path(
    put,
    path = "/api/v1/projects/{slug}/notify/targets/{id}",
    operation_id = "edit_project_notify_target",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The target, such as `nt-3`"),
    ),
    request_body = NotifyTargetFields,
    responses(
        (status = 200, description = "The target after the change", body = NotifyTargetEntry),
        (status = 400, description = "`invalid_notify_target`, `plaintext_notify_target`, `invalid_id` or `invalid_request`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`notify_target_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "`notify_settings_io` or internal error", body = ErrorEnvelope),
    )
)]
pub async fn edit_project_target(
    State(state): State<AppState>,
    project: Project,
    human: Human,
    Path((_slug, id)): Path<(String, String)>,
    ApiJson(body): ApiJson<NotifyTargetFields>,
) -> Result<Json<NotifyTargetEntry>, ApiError> {
    let target = notify::edit_target(
        settings(&state)?,
        &human.actor(),
        &Scope::Project(project.0.slug.clone()),
        target_id(&id)?,
        TargetFields::try_from(body)?,
    )
    .await?;
    Ok(Json(NotifyTargetEntry::new(target, false, None)))
}

/// A named human removes a target of the project, with every notice that waited for it.
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
    let id = target_id(&id)?;
    notify::remove_target(
        settings(&state)?,
        &human.actor(),
        &Scope::Project(project.0.slug.clone()),
        id,
    )
    .await?;
    project
        .0
        .store
        .write(async |tx| outbox::forget_target(tx, id).await)
        .await
        .map_err(from_store)?;
    Ok(StatusCode::NO_CONTENT)
}

/// A named human gives a generic target of the project a new signing secret, shown only in the answer. For a day each post carries a signature with the old secret too.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/notify/targets/{id}/rotate",
    operation_id = "rotate_project_notify_target",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The target, such as `nt-3`"),
    ),
    responses(
        (status = 200, description = "The target and its new secret", body = NotifyTargetAdded),
        (status = 400, description = "`invalid_notify_target` (a Slack or Discord target) or `invalid_id`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`notify_target_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "`notify_settings_io` or internal error", body = ErrorEnvelope),
    )
)]
pub async fn rotate_project_target(
    State(state): State<AppState>,
    project: Project,
    human: Human,
    Path((_slug, id)): Path<(String, String)>,
) -> Result<Json<NotifyTargetAdded>, ApiError> {
    let now = project.0.clock.now();
    rotate(
        &state,
        &human,
        Scope::Project(project.0.slug.clone()),
        target_id(&id)?,
        now,
    )
    .await
}

/// One send of the send log: when it was made, what it carried and how its newest try went. It never holds the URL, the body or the receiver's answer text.
#[derive(Debug, Serialize, ToSchema)]
pub struct NotifySendEntry {
    #[schema(example = "ns-12")]
    pub id: String,
    #[schema(example = "nt-3")]
    pub target: String,
    /// The `webhook-id` every try of the send carries.
    pub webhook_id: String,
    /// `pending` waits for its next try, `sending` is in flight, `delivered` got a 2xx, `given_up` failed its last try.
    #[schema(example = "delivered")]
    pub state: String,
    pub tries: u32,
    #[schema(required = true, value_type = Option<i64>)]
    pub next_try_at: Option<Timestamp>,
    /// The type and first id of each notice, such as `task.moved T-18`.
    pub notices: Vec<String>,
    pub count: u32,
    /// True for a test send.
    pub test: bool,
    #[schema(value_type = i64)]
    pub created_at: Timestamp,
    #[schema(required = true, value_type = Option<i64>)]
    pub last_try_at: Option<Timestamp>,
    /// The HTTP status of the newest try.
    #[schema(required = true, example = 200)]
    pub status: Option<u16>,
    /// `timeout`, `host_not_found`, `tls` or `unreachable` when the newest try got no status.
    #[schema(required = true)]
    pub error: Option<String>,
    #[schema(required = true)]
    pub duration_ms: Option<u64>,
}

impl From<SendRow> for NotifySendEntry {
    fn from(send: SendRow) -> Self {
        Self {
            id: send.number.to_string(),
            target: send.target.to_string(),
            webhook_id: send.webhook_id,
            state: send.state.as_str().to_owned(),
            tries: send.tries,
            next_try_at: send.next_try_at,
            notices: serde_json::from_str(&send.summary).unwrap_or_default(),
            count: send.notices,
            test: send.test,
            created_at: send.created_at,
            last_try_at: send.last_try_at,
            status: send.status,
            error: send.error,
            duration_ms: send.duration_ms,
        }
    }
}

/// The send log of a target.
#[derive(Debug, Serialize, ToSchema)]
pub struct NotifySendList {
    /// Newest first.
    pub sends: Vec<NotifySendEntry>,
}

/// A named human sends a test notice that names the project through one of its targets or a host target and waits for the answer. The send goes into the target's log; one that gets through turns a target fold turned off on again.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/notify/targets/{id}/test",
    operation_id = "test_notify_target",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The target, such as `nt-3`"),
    ),
    responses(
        (status = 200, description = "The receiver took the test", body = NotifySendEntry),
        (status = 400, description = "`invalid_id`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`notify_target_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 502, description = "`notify_send_failed`: the message says how the receiver answered", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "`notify_settings_io` or internal error", body = ErrorEnvelope),
    )
)]
pub async fn test_notify_target(
    State(state): State<AppState>,
    project: Project,
    human: Human,
    Path((_slug, id)): Path<(String, String)>,
) -> Result<Json<NotifySendEntry>, ApiError> {
    let handle = &project.0;
    let number =
        notify::test_target(handle, settings(&state)?, &human.actor(), target_id(&id)?).await?;
    let send = outbox::get_send(handle.store.read(), number)
        .await
        .map_err(from_store)?
        .ok_or_else(|| FoldError::new(codes::INTERNAL_ERROR, "the test send is gone"))?;
    Ok(Json(send.into()))
}

/// The newest sends of a target of the project or of the host, at most 50: the send log.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/notify/targets/{id}/sends",
    operation_id = "list_notify_sends",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The target, such as `nt-3`"),
    ),
    responses(
        (status = 200, description = "The send log", body = NotifySendList),
        (status = 400, description = "`invalid_id`", body = ErrorEnvelope),
        (status = 404, description = "`notify_target_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "`notify_settings_io` or internal error", body = ErrorEnvelope),
    )
)]
pub async fn list_notify_sends(
    State(state): State<AppState>,
    project: Project,
    Path((_slug, id)): Path<(String, String)>,
) -> Result<Json<NotifySendList>, ApiError> {
    let id = target_id(&id)?;
    settings(&state)?.find_for(&project.0.slug, id).await?;
    let sends = outbox::send_log(
        project.0.store.read(),
        id,
        fold_core::projects::notify::LOG_KEPT,
    )
    .await
    .map_err(from_store)?;
    Ok(Json(NotifySendList {
        sends: sends.into_iter().map(Into::into).collect(),
    }))
}

/// A named human sends a finished send again, with its webhook id and body, so a receiver that took it before drops the repeat.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/notify/sends/{id}/again",
    operation_id = "send_notify_again",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The send, such as `ns-12`"),
    ),
    responses(
        (status = 200, description = "The send, waiting for its try", body = NotifySendEntry),
        (status = 400, description = "`invalid_id` or `invalid_request` (the send is still open)", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`notify_send_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "`notify_settings_io` or internal error", body = ErrorEnvelope),
    )
)]
pub async fn send_notify_again(
    State(state): State<AppState>,
    project: Project,
    human: Human,
    Path((_slug, id)): Path<(String, String)>,
) -> Result<Json<NotifySendEntry>, ApiError> {
    let number = id.parse::<NotifySendId>().map_err(FoldError::from)?;
    let send = notify::send_again(&project.0, settings(&state)?, &human.actor(), number).await?;
    Ok(Json(send.into()))
}
