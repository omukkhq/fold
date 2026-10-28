use std::sync::Arc;
use std::time::Duration;

use fold_core::events::types;
use fold_core::projects::notify::{
    Answer, Judgement, LOG_KEPT, Links, Notice, NoticeProject, TargetKind, check_url, discord_body,
    generic_body, judge, new_secret, slack_body, test_notice,
};
use fold_core::{
    Actor, EventActor, FoldError, HumanName, NewEvent, NotifySendId, NotifyTargetId, Timestamp,
    codes,
};
use fold_store::projects::notify::{self as outbox, NewSend, SendRow, SendState, TryResult};

use crate::projects::ProjectHandle;
use crate::services::agents::NeedsYouItem;
use crate::services::error::from_store;
use crate::services::events::EventBuffer;

mod deliver;
mod items;
mod job;
mod settings;

pub use deliver::answer_text;
pub use items::{item_key, notice_item};
pub use job::{NotifyPass, PassState, STALE_TRY, notify_pass, notify_project};
pub use settings::{
    Applying, Health, NotifySettings, OffReason, Recorded, Scope, Secret, Target, TargetFields,
    TargetState, TryOutcome,
};

/// How long a replaced secret still signs after a rotation, so a receiver has a day to take the new one.
pub const ROTATION_OVERLAP: Duration = Duration::from_secs(24 * 60 * 60);

fn human(actor: &Actor) -> Result<HumanName, FoldError> {
    fold_core::policy::can_change_notifications(actor).map_err(FoldError::from)?;
    match actor {
        Actor::Human { name } => Ok(name.clone()),
        Actor::Agent(_) | Actor::System => Err(FoldError::new(
            codes::HUMAN_REQUIRED,
            "only a human changes where fold sends its notices",
        )),
    }
}

fn random<const N: usize>() -> Result<[u8; N], FoldError> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes)
        .map_err(|_| FoldError::new(codes::INTERNAL_ERROR, "the OS random source failed"))?;
    Ok(bytes)
}

/// A new `whsec_` secret of 24 random bytes.
pub fn new_signing_secret() -> Result<String, FoldError> {
    Ok(new_secret(&random::<24>()?))
}

/// The start of the webhook ids of one send: the project and random hex, so the ids of two projects that share a slug over time never meet at a receiver.
fn webhook_id_prefix(slug: &str) -> Result<String, FoldError> {
    let hex: String = random::<4>()?
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(format!("{slug}-{hex}"))
}

/// What `notify.target_changed` tells a project: a target was added, changed or removed by a person, or fold turned it off or on again. It names the target by its id and name, never its URL or a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetChange {
    pub id: NotifyTargetId,
    pub name: String,
    pub host: bool,
    /// The target's state now; `None` once it is removed.
    pub state: Option<TargetState>,
}

impl TargetChange {
    pub fn of(target: &Target, host: bool) -> Self {
        Self {
            id: target.id,
            name: target.name.clone(),
            host,
            state: Some(target.state()),
        }
    }

    fn event(&self, actor: EventActor) -> Result<NewEvent, FoldError> {
        let state = self.state.map_or("removed", TargetState::as_str);
        NewEvent::new(types::NOTIFY_TARGET_CHANGED, actor)
            .and_then(|event| {
                event.data(serde_json::json!({
                    "target": self.id.to_string(),
                    "name": self.name,
                    "host": self.host,
                    "state": state,
                }))
            })
            .map_err(|error| {
                tracing::error!(%error, "a notice target change did not make an event");
                FoldError::new(codes::INTERNAL_ERROR, "internal error")
            })
    }
}

/// Stores `notify.target_changed` in each project of `handles` that the target serves: every one for a host target, the project `slug` for a project's own. A removed target's waiting notices and sends leave the project's outbox in the same write. Each project's event is published once its write commits; a project that is closing is skipped, and the first other failure is returned once every project had its turn.
pub async fn announce(
    handles: &[Arc<ProjectHandle>],
    slug: Option<&str>,
    change: &TargetChange,
    actor: &EventActor,
) -> Result<(), FoldError> {
    let event = change.event(actor.clone())?;
    let mut failed = None;
    for handle in handles {
        if !change.host && slug != Some(handle.slug.as_str()) {
            continue;
        }
        let now = handle.clock.now();
        let mut buffer = EventBuffer::new();
        let written = handle
            .store
            .write(async |tx| {
                if change.state.is_none() {
                    outbox::forget_target(tx, change.id).await?;
                }
                buffer.append(tx, &event, now).await?;
                Ok::<_, fold_store::StoreError>(())
            })
            .await;
        match written {
            Ok(()) => handle.bus.publish(buffer),
            Err(error) if error.closed() => {}
            Err(error) => {
                tracing::warn!(project = %handle.slug, notify_target = %change.id, "a notice target change was not stored");
                failed.get_or_insert(from_store(error));
            }
        }
    }
    failed.map_or(Ok(()), Err)
}

/// Adds a target to `scope` for a named human and returns it with the signing secret of a generic target, which fold shows only this once. The checks run before the target takes a number, so a refused one changes nothing.
pub async fn add_target(
    settings: &NotifySettings,
    actor: &Actor,
    scope: &Scope,
    kind: TargetKind,
    fields: TargetFields,
    now: Timestamp,
) -> Result<(Target, Option<String>), FoldError> {
    let name = human(actor)?;
    let secret = match kind {
        TargetKind::Webhook => Some(new_signing_secret()?),
        TargetKind::Slack | TargetKind::Discord => None,
    };
    let kept = secret.clone();
    let target = settings
        .add(scope, move |id| {
            fields.new_target(kind, id, kept, &name, now)
        })
        .await?;
    tracing::info!(notify_target = %target.id, kind = %target.kind, host = matches!(scope, Scope::Host), "a notice target was added");
    tell(
        settings,
        scope,
        &TargetChange::of(&target, *scope == Scope::Host),
        actor,
    )
    .await?;
    Ok((target, secret))
}

/// Changes the fields of a target a named human set; a changed target starts with a clean health.
pub async fn edit_target(
    settings: &NotifySettings,
    actor: &Actor,
    scope: &Scope,
    id: NotifyTargetId,
    fields: TargetFields,
) -> Result<Target, FoldError> {
    human(actor)?;
    let target = settings
        .update(scope, id, move |target| fields.apply(target))
        .await?;
    tracing::info!(notify_target = %id, "a notice target was changed");
    tell(
        settings,
        scope,
        &TargetChange::of(&target, *scope == Scope::Host),
        actor,
    )
    .await?;
    Ok(target)
}

pub async fn remove_target(
    settings: &NotifySettings,
    actor: &Actor,
    scope: &Scope,
    id: NotifyTargetId,
) -> Result<(), FoldError> {
    human(actor)?;
    let target = settings.find(scope, id).await?;
    settings.remove(scope, id).await?;
    tracing::info!(notify_target = %id, "a notice target was removed");
    let change = TargetChange {
        state: None,
        ..TargetChange::of(&target, *scope == Scope::Host)
    };
    tell(settings, scope, &change, actor).await
}

/// [`announce`] to the projects a change of a target in `scope` reaches, after a person's change of the settings file.
async fn tell(
    settings: &NotifySettings,
    scope: &Scope,
    change: &TargetChange,
    actor: &Actor,
) -> Result<(), FoldError> {
    let slug = match scope {
        Scope::Host => None,
        Scope::Project(slug) => Some(slug.as_str()),
    };
    announce(&settings.peers(), slug, change, &actor.event_actor()).await
}

/// Gives a generic target a new signing secret and returns it, shown this once. The replaced secret signs too for [`ROTATION_OVERLAP`].
pub async fn rotate_secret(
    settings: &NotifySettings,
    actor: &Actor,
    scope: &Scope,
    id: NotifyTargetId,
    now: Timestamp,
) -> Result<String, FoldError> {
    human(actor)?;
    let secret = new_signing_secret()?;
    let kept = secret.clone();
    let until = now.checked_add(ROTATION_OVERLAP);
    settings
        .update(scope, id, move |target| {
            if target.kind != TargetKind::Webhook {
                return Err(FoldError::new(
                    codes::INVALID_NOTIFY_TARGET,
                    "only a generic webhook target has a signing secret",
                )
                .with_hint(
                    "a Slack or Discord target's URL is its secret: change the URL instead",
                ));
            }
            target.previous_secret = target.secret.take();
            target.previous_until = until;
            target.secret = Some(Secret::new(kept));
            Ok(())
        })
        .await?;
    tracing::info!(notify_target = %id, "a notice target got a new signing secret");
    Ok(secret)
}

/// Sets whether the host's targets get the notices of the project `slug` too.
pub async fn set_use_host_targets(
    settings: &NotifySettings,
    actor: &Actor,
    slug: &str,
    on: bool,
) -> Result<(), FoldError> {
    human(actor)?;
    settings.set_use_host_targets(slug, on).await?;
    tracing::info!(project = %slug, use_host_targets = on, "a project's use of the host targets changed");
    Ok(())
}

/// Sets the address of the web app that the links of every notice start with, or clears it with `None`, after which notices carry no links.
pub async fn set_web_url(
    settings: &NotifySettings,
    actor: &Actor,
    url: Option<String>,
) -> Result<(), FoldError> {
    human(actor)?;
    let url = url
        .map(|url| url.trim().trim_end_matches('/').to_owned())
        .filter(|url| !url.is_empty());
    if let Some(url) = &url {
        check_url(url).map_err(|_| {
            FoldError::new(
                codes::INVALID_REQUEST,
                "the web address must be an http or https URL with a host",
            )
            .with_hint("give the address people open fold at")
        })?;
    }
    settings.set_web_url(url).await?;
    settings.wake(None);
    Ok(())
}

/// The links of the project's notices.
pub async fn links(settings: &NotifySettings, slug: &str) -> Result<Links, FoldError> {
    Ok(Links::new(settings.web_url().await?.as_deref(), slug))
}

/// The body of a send of `notices` to a target of `kind`.
pub(crate) fn body(
    kind: TargetKind,
    project: &NoticeProject,
    notices: &[Notice],
    now: Timestamp,
) -> Result<Vec<u8>, FoldError> {
    match kind {
        TargetKind::Webhook => generic_body(project, notices, now),
        TargetKind::Slack => slack_body(project, notices),
        TargetKind::Discord => discord_body(project, notices),
    }
}

/// What a try of a send did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tried {
    pub answer: Answer,
    pub judgement: Judgement,
    /// False when the try turned the target off.
    pub active: bool,
}

/// One try of `send` to `target`: the send is in flight while the post runs off the async runtime, then its result goes into the send log and the target's health. A test send has one try, also when a restart or a person sends it again. The log line names the target and its host, never its URL path, a secret or a body.
pub(crate) async fn attempt(
    handle: &ProjectHandle,
    settings: &NotifySettings,
    applying: &Applying,
    send: &SendRow,
) -> Result<Option<Tried>, FoldError> {
    let started = handle.clock.now();
    if !handle
        .store
        .write(async |tx| outbox::start_try(tx, send.number, started).await)
        .await
        .map_err(from_store)?
    {
        return Ok(None);
    }
    post_and_record(handle, settings, applying, send, started).await
}

/// Posts a send whose try is in flight and records the answer: first the target's health, then in one write the send log and, when the try turned the target off or on again, `notify.target_changed`. A failed try of a test send gives the send up instead of waiting for the next, and a failed try that leaves its target off ends its send, so the log keeps the send that turned the target off. A host target's change reaches the other open projects after the write.
async fn post_and_record(
    handle: &ProjectHandle,
    settings: &NotifySettings,
    applying: &Applying,
    send: &SendRow,
    started: Timestamp,
) -> Result<Option<Tried>, FoldError> {
    let once = send.test;
    let target = applying.target.clone();
    let (webhook_id, body) = (send.webhook_id.clone(), send.body.clone());
    let (answer, took) =
        tokio::task::spawn_blocking(move || deliver::post(&target, &webhook_id, &body, started))
            .await
            .map_err(|_| FoldError::new(codes::INTERNAL_ERROR, "a notice send stopped"))??;
    #[cfg(feature = "failpoints")]
    fail::fail_point!("notify_after_post", |_| Err(FoldError::new(
        codes::INTERNAL_ERROR,
        "failpoint notify_after_post"
    )));
    let now = handle.clock.now();
    let tries = send.tries.saturating_add(1);
    let judged = judge(answer, tries, now);
    let outcome = match judged {
        Judgement::Delivered => TryOutcome::Delivered,
        Judgement::Gone => TryOutcome::Gone,
        Judgement::GiveUp if !once => TryOutcome::GaveUp,
        Judgement::Retry(_) | Judgement::GiveUp => TryOutcome::Failed,
    };
    let id = applying.target.id;
    let recorded = settings
        .record_try(&handle.slug, id, applying.host, outcome, now)
        .await?;
    let active = recorded
        .as_ref()
        .is_some_and(|recorded| recorded.target.active());
    let judgement = match judged {
        Judgement::Retry(_) if once || !active => Judgement::GiveUp,
        judged => judged,
    };
    let (state, next_try_at) = match judgement {
        Judgement::Delivered => (SendState::Delivered, None),
        Judgement::Retry(at) => (SendState::Pending, Some(at)),
        Judgement::Gone | Judgement::GiveUp => (SendState::GivenUp, None),
    };
    let result = TryResult {
        state,
        tries,
        next_try_at,
        status: answer.status(),
        error: answer.error_kind(),
        duration_ms: u64::try_from(took.as_millis()).unwrap_or(u64::MAX),
    };
    let change = recorded.as_ref().and_then(|recorded| {
        recorded
            .changed()
            .map(|_| TargetChange::of(&recorded.target, applying.host))
    });
    let event = change
        .as_ref()
        .map(|change| change.event(EventActor::fold()))
        .transpose()?;
    let mut buffer = EventBuffer::new();
    handle
        .store
        .write(async |tx| {
            outbox::finish_try(tx, send.number, &result).await?;
            outbox::prune_log(tx, id, LOG_KEPT).await?;
            if let Some(event) = &event {
                buffer.append(tx, event, now).await?;
            }
            Ok::<_, fold_store::StoreError>(())
        })
        .await
        .map_err(from_store)?;
    handle.bus.publish(buffer);
    if let Some(change) = change.as_ref().filter(|change| change.host) {
        let others: Vec<Arc<ProjectHandle>> = settings
            .peers()
            .into_iter()
            .filter(|peer| peer.slug != handle.slug)
            .collect();
        if announce(&others, None, change, &EventActor::fold())
            .await
            .is_err()
        {
            tracing::warn!(project = %handle.slug, notify_target = %id, "a host target's change did not reach every project");
        }
    }
    let host = fold_core::projects::notify::url_host(applying.target.url.expose()).to_owned();
    match (answer.status(), answer.error_kind()) {
        (Some(status), _) if outcome == TryOutcome::Delivered => {
            tracing::info!(project = %handle.slug, notify_target = %id, host, status, send = %send.number, tries, "a notice send went out");
        }
        (status, error) => {
            tracing::warn!(project = %handle.slug, notify_target = %id, host, status, error, send = %send.number, tries, "a notice send failed");
        }
    }
    if let Some(state) = change.as_ref().and_then(|change| change.state) {
        tracing::warn!(project = %handle.slug, notify_target = %id, state = state.as_str(), "fold changed the state of a notice target");
    }
    Ok(Some(Tried {
        answer,
        judgement,
        active,
    }))
}

/// Sends a test notice that names the project through its target `id` or a host target and waits for the answer. The send goes into the target's send log; one that gets through turns a target fold had turned off on again. A failed one is `notify_send_failed` with the answer in words of fold's own. A test before the notifier's first look at the events starts the notifier's cursor where that look would, so no event of the targets is skipped.
pub async fn test_target(
    handle: &ProjectHandle,
    settings: &NotifySettings,
    actor: &Actor,
    id: NotifyTargetId,
) -> Result<NotifySendId, FoldError> {
    human(actor)?;
    let applying = settings.find_for(&handle.slug, id).await?;
    let now = handle.clock.now();
    let links = links(settings, &handle.slug).await?;
    let notice = test_notice(&id.to_string(), &applying.target.name, now);
    let body = body(
        applying.target.kind,
        &links.project(&handle.name),
        std::slice::from_ref(&notice),
        now,
    )?;
    let prefix = webhook_id_prefix(&handle.slug)?;
    let summary = serde_json::to_string(&[notice.summary()])
        .map_err(|_| FoldError::new(codes::INTERNAL_ERROR, "the summary did not serialise"))?;
    let mut counted: Vec<Applying> = settings
        .applying(&handle.slug)
        .await?
        .into_iter()
        .filter(|other| other.target.active())
        .collect();
    counted.push(applying.clone());
    let start = job::start_cursor(handle, &counted).await?;
    let send = handle
        .store
        .write(async |tx| {
            outbox::start_cursor(tx, start).await?;
            let send = outbox::create_send(
                tx,
                &NewSend {
                    target: id,
                    webhook_id_prefix: &prefix,
                    body: &body,
                    summary: &summary,
                    notices: 1,
                    test: true,
                    outbox: &[],
                },
                now,
            )
            .await?;
            outbox::start_try(tx, send.number, now).await?;
            Ok::<_, fold_store::StoreError>(send)
        })
        .await
        .map_err(from_store)?;
    let tried = post_and_record(handle, settings, &applying, &send, now).await?;
    settings.wake(Some(&handle.slug));
    match tried {
        Some(tried) if tried.judgement == Judgement::Delivered => Ok(send.number),
        Some(tried) => Err(FoldError::new(
            codes::NOTIFY_SEND_FAILED,
            format!("the test send failed: {}", answer_text(tried.answer)),
        )
        .with_hint("check the URL and the receiver, then send a test again")),
        None => Err(FoldError::new(
            codes::INTERNAL_ERROR,
            "the test send was not tried",
        )),
    }
}

fn send_not_found(number: NotifySendId) -> FoldError {
    FoldError::new(
        codes::NOTIFY_SEND_NOT_FOUND,
        format!("there is no send {number} in this project"),
    )
    .with_hint("the send log of a target lists its sends: `fld notify log <target>`")
}

/// Refuses a send to a target that gets no notices of the project now, which would wait for a try that never comes.
async fn check_sends_now(
    handle: &ProjectHandle,
    settings: &NotifySettings,
    number: NotifySendId,
    target: NotifyTargetId,
) -> Result<(), FoldError> {
    let inactive = |message: String, hint: &str| {
        FoldError::new(codes::NOTIFY_TARGET_INACTIVE, message).with_hint(hint)
    };
    let applying = settings.applying(&handle.slug).await?;
    match applying
        .iter()
        .find(|applying| applying.target.id == target)
    {
        Some(applying) if applying.target.active() => Ok(()),
        Some(applying) => Err(inactive(
            format!(
                "the target {target} of the send {number} gets no notices now: it is {}",
                applying.target.state().as_str()
            ),
            "turn the target on, or send a test that gets through, then send it again",
        )),
        None => {
            settings.find_for(&handle.slug, target).await?;
            Err(inactive(
                format!(
                    "the target {target} of the send {number} is a host target, and this project does not use the host targets"
                ),
                "turn the host targets on in the Settings tab or with `fld notify host-targets on`, then send it again",
            ))
        }
    }
}

/// Sends the finished send `number` again with its webhook id and body, while its target gets the project's notices.
pub async fn send_again(
    handle: &ProjectHandle,
    settings: &NotifySettings,
    actor: &Actor,
    number: NotifySendId,
) -> Result<SendRow, FoldError> {
    human(actor)?;
    let before = outbox::get_send(handle.store.read(), number)
        .await
        .map_err(from_store)?
        .ok_or_else(|| send_not_found(number))?;
    check_sends_now(handle, settings, number, before.target).await?;
    let now = handle.clock.now();
    let again = handle
        .store
        .write(async |tx| outbox::send_again(tx, number, now).await)
        .await
        .map_err(from_store)?;
    if !again {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!("the send {number} is still waiting or in flight"),
        ));
    }
    let send = outbox::get_send(handle.store.read(), number)
        .await
        .map_err(from_store)?
        .ok_or_else(|| send_not_found(number))?;
    tracing::info!(project = %handle.slug, send = %number, notify_target = %send.target, "a person sent a notice send again");
    settings.wake(Some(&handle.slug));
    Ok(send)
}

/// The Needs you items of the targets of `slug` that fold turned off, each with what a person does about its reason.
pub async fn off_items(
    settings: &NotifySettings,
    slug: &str,
) -> Result<Vec<NeedsYouItem>, FoldError> {
    Ok(settings
        .applying(slug)
        .await?
        .into_iter()
        .filter_map(|applying| {
            let target = applying.target;
            let reason = target.health.off?;
            Some(NeedsYouItem::notify_target_off(
                target.health.off_at.unwrap_or(target.created_at),
                target.id,
                &target.name,
                reason.advice(),
            ))
        })
        .collect())
}

/// `items` with the items of the project's turned-off targets, oldest first.
pub fn with_notify_attention(
    mut items: Vec<NeedsYouItem>,
    off: Vec<NeedsYouItem>,
) -> Vec<NeedsYouItem> {
    if off.is_empty() {
        return items;
    }
    items.extend(off);
    items.sort_by_key(|item| item.since);
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_a_human_changes_the_targets_and_the_web_address() {
        let temp = tempfile::tempdir().unwrap();
        let settings = NotifySettings::new(temp.path().join("notices"));
        let ana = Actor::human(HumanName::parse("ana").unwrap());
        let fields = || TargetFields {
            name: Some("Release bot".to_owned()),
            url: Some("http://127.0.0.1:9/x".to_owned()),
            ..TargetFields::default()
        };
        let error = add_target(
            &settings,
            &Actor::System,
            &Scope::Host,
            TargetKind::Webhook,
            fields(),
            Timestamp::EPOCH,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), codes::HUMAN_REQUIRED);
        let (target, secret) = add_target(
            &settings,
            &ana,
            &Scope::Host,
            TargetKind::Webhook,
            fields(),
            Timestamp::EPOCH,
        )
        .await
        .unwrap();
        let secret = secret.unwrap();
        assert!(secret.starts_with("whsec_"));
        assert_eq!(target.created_by, "ana");
        let (slack, none) = add_target(
            &settings,
            &ana,
            &Scope::Host,
            TargetKind::Slack,
            fields(),
            Timestamp::EPOCH,
        )
        .await
        .unwrap();
        assert!(none.is_none());
        assert_eq!(
            rotate_secret(&settings, &ana, &Scope::Host, slack.id, Timestamp::EPOCH)
                .await
                .unwrap_err()
                .code(),
            codes::INVALID_NOTIFY_TARGET
        );
        let rotated = rotate_secret(&settings, &ana, &Scope::Host, target.id, Timestamp::EPOCH)
            .await
            .unwrap();
        assert_ne!(rotated, secret);
        let kept = settings.find(&Scope::Host, target.id).await.unwrap();
        assert_eq!(
            kept.signing_secrets(Timestamp::EPOCH),
            [rotated.as_str(), secret.as_str()]
        );
        set_web_url(
            &settings,
            &ana,
            Some(" https://fold.example.com/ ".to_owned()),
        )
        .await
        .unwrap();
        assert_eq!(
            links(&settings, "demo")
                .await
                .unwrap()
                .to("board")
                .as_deref(),
            Some("https://fold.example.com/p/demo/board")
        );
        let refused = set_web_url(&settings, &ana, Some("fold.example.com".to_owned()))
            .await
            .unwrap_err();
        assert_eq!(refused.code(), codes::INVALID_REQUEST);
        set_web_url(&settings, &ana, Some(String::new()))
            .await
            .unwrap();
        assert_eq!(links(&settings, "demo").await.unwrap().to("board"), None);
        let edited = edit_target(
            &settings,
            &ana,
            &Scope::Host,
            target.id,
            TargetFields {
                window_seconds: Some(0),
                ..TargetFields::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(edited.window_seconds, 0);
        for denied in [
            remove_target(&settings, &Actor::System, &Scope::Host, target.id).await,
            set_use_host_targets(&settings, &Actor::System, "demo", false).await,
            set_web_url(&settings, &Actor::System, None).await,
        ] {
            assert_eq!(denied.unwrap_err().code(), codes::HUMAN_REQUIRED);
        }
        remove_target(&settings, &ana, &Scope::Host, target.id)
            .await
            .unwrap();
        assert!(off_items(&settings, "demo").await.unwrap().is_empty());
        let mut with = with_notify_attention(
            Vec::new(),
            vec![NeedsYouItem::notify_target_off(
                Timestamp::EPOCH,
                slack.id,
                "x",
                OffReason::Gone.advice(),
            )],
        );
        assert_eq!(with.len(), 1);
        with = with_notify_attention(with, Vec::new());
        assert_eq!(with.len(), 1);
    }
}
