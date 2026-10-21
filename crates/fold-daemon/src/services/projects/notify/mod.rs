use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use fold_core::projects::notify::{
    Notice, NoticeEvent, NoticeProject, check_webhook_url, web_link,
};
use fold_core::{Actor, FoldError, NotifyTargetId, Timestamp, codes, policy};

mod deliver;
mod items;
mod job;
mod settings;

pub use deliver::{SEND_TIMEOUT, deliver};
pub use items::{item_key, notice_item};
pub use job::{NotifyPass, notify_pass, notify_project};
pub use settings::{
    Channel, EmailChannel, NewChannel, NewTarget, NotifySettings, Scope, Secret, Target,
};

fn human(actor: &Actor) -> Result<fold_core::HumanName, FoldError> {
    policy::can_change_notifications(actor).map_err(FoldError::from)?;
    match actor {
        Actor::Human { name } => Ok(name.clone()),
        Actor::Agent(_) | Actor::System => Err(FoldError::new(
            codes::HUMAN_REQUIRED,
            "only a human changes where fold sends its notices",
        )),
    }
}

/// Adds a target to `scope` for a named human. The target's checks run before it takes a number, so a refused one changes nothing.
pub async fn add_target(
    settings: &NotifySettings,
    actor: &Actor,
    scope: &Scope,
    new: NewTarget,
    now: Timestamp,
) -> Result<Target, FoldError> {
    let name = human(actor)?;
    let target = settings
        .add(scope, move |id| new.checked(id, &name, now))
        .await?;
    tracing::info!(notify_target = %target.id, kind = target.channel.kind(), host = matches!(scope, Scope::Host), "a notification target was added");
    Ok(target)
}

pub async fn remove_target(
    settings: &NotifySettings,
    actor: &Actor,
    scope: &Scope,
    id: NotifyTargetId,
) -> Result<(), FoldError> {
    human(actor)?;
    settings.remove(scope, id).await?;
    tracing::info!(notify_target = %id, "a notification target was removed");
    Ok(())
}

/// Sends a test notice through the target `id` of `scope` and waits for the receiver or SMTP server to take it. The project's own test can also name a host target, as the project uses the host's while it has none of its own. `project` names the project in the notice.
pub async fn test_target(
    settings: &NotifySettings,
    actor: &Actor,
    scope: &Scope,
    id: NotifyTargetId,
    project: Option<NoticeProject>,
    now: Timestamp,
) -> Result<(), FoldError> {
    human(actor)?;
    let target = match settings.find(scope, id).await {
        Err(error) if error.code() == codes::NOTIFY_TARGET_NOT_FOUND && *scope != Scope::Host => {
            settings.find(&Scope::Host, id).await?
        }
        found => found?,
    };
    let notice = Notice {
        event: NoticeEvent::Test,
        project,
        items: Vec::new(),
        sent_at: now.as_millis(),
    };
    let channel = target.channel.clone();
    let sent = tokio::task::spawn_blocking(move || deliver(&channel, &notice))
        .await
        .map_err(|_| FoldError::new(codes::INTERNAL_ERROR, "the test send stopped"))?;
    match &sent {
        Ok(()) => tracing::info!(notify_target = %id, "a test notice went out"),
        Err(error) => {
            tracing::warn!(notify_target = %id, code = error.code(), "a test notice was not sent")
        }
    }
    sent
}

/// Sets the address of the web app that the links of every notice start with, or clears it with `None` so links follow the daemon's listen address.
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
        check_webhook_url(url).map_err(|_| {
            FoldError::new(
                codes::INVALID_REQUEST,
                "the web address must be an http or https URL with a host",
            )
            .with_hint("give the address people open fold at, such as https://fold.example.com")
        })?;
    }
    settings.set_web_url(url).await
}

/// The address the links of a notice start with: the one a human set, else the daemon's listen address, where an unspecified address becomes the loopback one.
pub async fn link_base(settings: &NotifySettings, listen: SocketAddr) -> Result<String, FoldError> {
    if let Some(url) = settings.web_url().await? {
        return Ok(url);
    }
    Ok(listen_url(listen))
}

pub fn listen_url(listen: SocketAddr) -> String {
    let ip = match listen.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    format!("http://{}", SocketAddr::new(ip, listen.port()))
}

/// The project as a notice names it, with its board as the link.
pub fn notice_project(base: &str, slug: &str, name: &str) -> NoticeProject {
    NoticeProject {
        slug: slug.to_owned(),
        name: name.to_owned(),
        link: web_link(base, &format!("/p/{slug}/board")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_follow_the_listen_address_with_a_loopback_for_any_address() {
        assert_eq!(
            listen_url("0.0.0.0:7420".parse().unwrap()),
            "http://127.0.0.1:7420"
        );
        assert_eq!(
            listen_url("[::]:7420".parse().unwrap()),
            "http://[::1]:7420"
        );
        assert_eq!(
            listen_url("192.168.1.5:80".parse().unwrap()),
            "http://192.168.1.5:80"
        );
        let project = notice_project("https://fold.example.com", "demo", "Demo");
        assert_eq!(project.link, "https://fold.example.com/p/demo/board");
    }

    #[tokio::test]
    async fn only_a_human_changes_the_targets_and_the_web_address() {
        let temp = tempfile::tempdir().unwrap();
        let settings = NotifySettings::new(temp.path().join("notify"));
        let ana = Actor::human(fold_core::HumanName::parse("ana").unwrap());
        let new = || NewTarget {
            interval_minutes: Some(1),
            channel: NewChannel::Webhook {
                url: "http://127.0.0.1:9/x".to_owned(),
                secret: "0123456789abcdef".to_owned(),
            },
        };
        let error = add_target(
            &settings,
            &Actor::System,
            &Scope::Host,
            new(),
            Timestamp::EPOCH,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), codes::HUMAN_REQUIRED);
        let target = add_target(&settings, &ana, &Scope::Host, new(), Timestamp::EPOCH)
            .await
            .unwrap();
        assert_eq!(target.created_by, "ana");
        let listen: SocketAddr = "0.0.0.0:7420".parse().unwrap();
        assert_eq!(
            link_base(&settings, listen).await.unwrap(),
            "http://127.0.0.1:7420"
        );
        set_web_url(
            &settings,
            &ana,
            Some(" https://fold.example.com/ ".to_owned()),
        )
        .await
        .unwrap();
        assert_eq!(
            link_base(&settings, listen).await.unwrap(),
            "https://fold.example.com"
        );
        let refused = set_web_url(&settings, &ana, Some("fold.example.com".to_owned()))
            .await
            .unwrap_err();
        assert_eq!(refused.code(), codes::INVALID_REQUEST);
        set_web_url(&settings, &ana, Some(String::new()))
            .await
            .unwrap();
        assert_eq!(
            link_base(&settings, listen).await.unwrap(),
            "http://127.0.0.1:7420"
        );
        let project = Scope::Project("demo".to_owned());
        let missing = test_target(
            &settings,
            &ana,
            &project,
            NotifyTargetId::from_number(9).unwrap(),
            None,
            Timestamp::EPOCH,
        )
        .await
        .unwrap_err();
        assert_eq!(missing.code(), codes::NOTIFY_TARGET_NOT_FOUND);
        assert_eq!(
            remove_target(&settings, &Actor::System, &Scope::Host, target.id)
                .await
                .unwrap_err()
                .code(),
            codes::HUMAN_REQUIRED
        );
        remove_target(&settings, &ana, &Scope::Host, target.id)
            .await
            .unwrap();
    }
}
