use fold_core::projects::notify::{NeedsYouNotice, short_title};

use crate::services::agents::{NeedsYouItem, NeedsYouKind};

/// The key that names `item` across reads of Needs you and across a daemon restart: its kind and the id, agent, target or [`NeedsYouItem::about`] it is about. The project folder's waiting item has its kind alone, as at most one waits, so it keeps its key while its paths change.
pub fn item_key(item: &NeedsYouItem) -> String {
    let kind = item.kind.as_str();
    let about = match item.kind {
        NeedsYouKind::Question => item.message.map(|(message, _)| message.to_string()),
        NeedsYouKind::Permission => item.question.map(|id| id.to_string()),
        NeedsYouKind::HumanReview => item.proposal.map(|id| id.to_string()),
        NeedsYouKind::RequestDraft => item.request.map(|id| id.to_string()),
        NeedsYouKind::HumanTask | NeedsYouKind::Blocker => item.task.map(|id| id.to_string()),
        NeedsYouKind::WorkerUnreachable
        | NeedsYouKind::WorkerModelUnreachable
        | NeedsYouKind::ClaudeNotFound => item.agent.as_ref().map(ToString::to_string),
        NeedsYouKind::NotifyTargetOff => item.target.map(|id| id.to_string()),
        NeedsYouKind::HumanChange
        | NeedsYouKind::HeadMoved
        | NeedsYouKind::UnstashConflicted
        | NeedsYouKind::ChecksFileInvalidOnMain => item.about.clone(),
        NeedsYouKind::FolderHeld => None,
    };
    match about {
        Some(about) => format!("{kind}:{about}"),
        None => kind.to_owned(),
    }
}

/// The public id a notice names for `item`: what its screen opens, the agent of an item about a worker, or the target of a target fold turned off.
fn public_id(item: &NeedsYouItem) -> Option<String> {
    match item.kind {
        NeedsYouKind::HumanChange
        | NeedsYouKind::HeadMoved
        | NeedsYouKind::FolderHeld
        | NeedsYouKind::UnstashConflicted
        | NeedsYouKind::ChecksFileInvalidOnMain => None,
        _ => item_key(item).split_once(':').map(|(_, id)| id.to_owned()),
    }
}

/// The title of a notice: the proposal, request, task or target title, a question's short header, or words of fold's own. Never a message body, a blocker's reason, a path, a commit or Claude Code's error text.
fn title(item: &NeedsYouItem) -> String {
    let agent = item
        .agent
        .as_ref()
        .map_or_else(|| "An agent".to_owned(), |agent| format!("@{agent}"));
    let own = |text: &Option<String>, fallback: &str| {
        text.as_deref()
            .map(short_title)
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| fallback.to_owned())
    };
    let on_task = item
        .task
        .map_or_else(String::new, |task| format!(" on {task}"));
    match item.kind {
        NeedsYouKind::Question => match &item.header {
            Some(header) if !short_title(header).is_empty() => short_title(header),
            _ => format!("{agent} asks a question{on_task}"),
        },
        NeedsYouKind::Permission => short_title(&format!(
            "{agent} asks to use {}",
            item.tool.as_deref().unwrap_or("a tool")
        )),
        NeedsYouKind::HumanReview => own(&item.title, "A proposal waits for your approval"),
        NeedsYouKind::RequestDraft => own(&item.title, "A card draft waits for you"),
        NeedsYouKind::HumanTask => own(&item.title, "A task is ready for a person"),
        NeedsYouKind::Blocker => own(&item.title, "A task has a blocker"),
        NeedsYouKind::HumanChange => "Your change conflicts with main".to_owned(),
        NeedsYouKind::HeadMoved => "The project folder's Git branch moved".to_owned(),
        NeedsYouKind::FolderHeld => "The project folder waits on your changes".to_owned(),
        NeedsYouKind::UnstashConflicted => "An unstash left conflict markers".to_owned(),
        NeedsYouKind::ChecksFileInvalidOnMain => "The checks file on main is not valid".to_owned(),
        NeedsYouKind::WorkerUnreachable => format!("{agent} cannot reach fold"),
        NeedsYouKind::WorkerModelUnreachable => format!("{agent} cannot reach the model"),
        NeedsYouKind::ClaudeNotFound => {
            format!("{agent} cannot start: the daemon finds no claude")
        }
        NeedsYouKind::NotifyTargetOff => format!(
            "fold turned the notice target {} off",
            own(&item.title, "a target")
        ),
    }
}

/// The path below the project in the web app of the screen that answers `item`, as the web app's Needs you list links it.
fn path(item: &NeedsYouItem) -> String {
    let id = |value: Option<String>| value.unwrap_or_default();
    match item.kind {
        NeedsYouKind::Question => match item.message {
            Some((_, thread)) => format!("messages?thread={thread}"),
            None => "messages".to_owned(),
        },
        NeedsYouKind::Permission => match item.question {
            Some(question) => format!("agents?question={question}"),
            None => "agents".to_owned(),
        },
        NeedsYouKind::HumanReview => {
            format!("proposals/{}", id(item.proposal.map(|p| p.to_string())))
        }
        NeedsYouKind::RequestDraft => {
            format!("requests/{}", id(item.request.map(|r| r.to_string())))
        }
        NeedsYouKind::HumanTask | NeedsYouKind::Blocker => {
            format!("t/{}", id(item.task.map(|t| t.to_string())))
        }
        NeedsYouKind::HumanChange
        | NeedsYouKind::HeadMoved
        | NeedsYouKind::FolderHeld
        | NeedsYouKind::UnstashConflicted
        | NeedsYouKind::ChecksFileInvalidOnMain => "repository".to_owned(),
        NeedsYouKind::WorkerUnreachable
        | NeedsYouKind::WorkerModelUnreachable
        | NeedsYouKind::ClaudeNotFound => "agents".to_owned(),
        NeedsYouKind::NotifyTargetOff => "settings".to_owned(),
    }
}

/// `item` as a notice names it.
pub fn notice_item(item: &NeedsYouItem) -> NeedsYouNotice {
    NeedsYouNotice {
        kind: item.kind.as_str().to_owned(),
        item: public_id(item),
        title: title(item),
        humans: item
            .humans
            .iter()
            .chain(item.human.iter())
            .map(ToString::to_string)
            .collect(),
        path: path(item),
    }
}

#[cfg(test)]
mod tests {
    use fold_core::{
        AgentName, MessageId, NotifyTargetId, PermissionQuestionId, ProposalId, TaskId, Timestamp,
    };

    use super::*;

    fn of(kind: NeedsYouKind) -> NeedsYouItem {
        NeedsYouItem::new(kind, Timestamp::from_millis(7).unwrap())
    }

    #[test]
    fn a_question_names_its_message_and_never_its_body() {
        let mut item = of(NeedsYouKind::Question);
        item.message = Some((
            MessageId::from_number(31).unwrap(),
            MessageId::from_number(30).unwrap(),
        ));
        item.agent = Some(AgentName::parse("backend").unwrap());
        item.task = TaskId::from_number(18);
        assert_eq!(item_key(&item), "question:m-31");
        let notice = notice_item(&item);
        assert_eq!(notice.item.as_deref(), Some("m-31"));
        assert_eq!(notice.title, "@backend asks a question on T-18");
        assert_eq!(notice.path, "messages?thread=m-30");
        item.header = Some("Chart\nlibrary".to_owned());
        assert_eq!(notice_item(&item).title, "Chart library");
    }

    #[test]
    fn every_kind_has_a_key_a_title_and_a_path() {
        let mut review = of(NeedsYouKind::HumanReview);
        review.proposal = ProposalId::from_number(3);
        review.title = Some("Database schema".to_owned());
        review.human = fold_core::HumanName::parse("ana").ok();
        let shown = notice_item(&review);
        assert_eq!(
            (shown.item.as_deref(), shown.title.as_str()),
            (Some("P-3"), "Database schema")
        );
        assert_eq!(
            (shown.path.as_str(), shown.humans.as_slice()),
            ("proposals/P-3", ["ana".to_owned()].as_slice())
        );

        let mut blocker = of(NeedsYouKind::Blocker);
        blocker.task = TaskId::from_number(12);
        blocker.title = Some("Ship it".to_owned());
        blocker.reason = Some("a reason a human wrote".to_owned());
        assert_eq!(item_key(&blocker), "blocker:T-12");
        assert_eq!(notice_item(&blocker).title, "Ship it");

        let mut permission = of(NeedsYouKind::Permission);
        permission.question = PermissionQuestionId::from_number(2);
        permission.tool = Some("Bash".to_owned());
        let shown = notice_item(&permission);
        assert_eq!(shown.title, "An agent asks to use Bash");
        assert_eq!(shown.path, "agents?question=pq-2");

        let mut worker = of(NeedsYouKind::WorkerModelUnreachable);
        worker.agent = Some(AgentName::parse("api-lead").unwrap());
        worker.title = Some("Claude Code's own error text".to_owned());
        assert_eq!(item_key(&worker), "worker_model_unreachable:api-lead");
        assert_eq!(
            notice_item(&worker).title,
            "@api-lead cannot reach the model"
        );

        let off = NeedsYouItem::notify_target_off(
            Timestamp::EPOCH,
            NotifyTargetId::from_number(4).unwrap(),
            "Release bot",
            "check the URL",
        );
        assert_eq!(item_key(&off), "notify_target_off:nt-4");
        let shown = notice_item(&off);
        assert_eq!(shown.item.as_deref(), Some("nt-4"));
        assert_eq!(shown.path, "settings");
        assert!(shown.title.contains("Release bot"));

        let mut change = of(NeedsYouKind::HumanChange);
        change.paths = vec!["secret/path.rs".to_owned()];
        change.about = Some("tree-1".to_owned());
        assert_eq!(item_key(&change), "human_change:tree-1");
        assert_eq!(notice_item(&change).item, None);

        let moved = |since: i64, hint: &str| {
            item_key(&NeedsYouItem::head_moved(
                Timestamp::from_millis(since).unwrap(),
                hint.to_owned(),
            ))
        };
        assert_eq!(
            moved(5, "run `git switch main`"),
            moved(90, "run `git switch main`")
        );
        assert_ne!(
            moved(5, "run `git switch main`"),
            moved(5, "run `git reset --soft 1a2b`")
        );
        let held = |paths: &[&str]| {
            item_key(&NeedsYouItem::folder_held(
                Timestamp::from_millis(3).unwrap(),
                paths.iter().map(|path| (*path).to_owned()).collect(),
            ))
        };
        assert_eq!(held(&["a.txt"]), held(&["a.txt", "b.txt"]));
        assert_eq!(held(&["a.txt"]), "folder_held");

        for kind in [
            NeedsYouKind::Question,
            NeedsYouKind::Permission,
            NeedsYouKind::HumanReview,
            NeedsYouKind::RequestDraft,
            NeedsYouKind::HumanTask,
            NeedsYouKind::Blocker,
            NeedsYouKind::HumanChange,
            NeedsYouKind::HeadMoved,
            NeedsYouKind::FolderHeld,
            NeedsYouKind::UnstashConflicted,
            NeedsYouKind::ChecksFileInvalidOnMain,
            NeedsYouKind::WorkerUnreachable,
            NeedsYouKind::WorkerModelUnreachable,
            NeedsYouKind::ClaudeNotFound,
            NeedsYouKind::NotifyTargetOff,
        ] {
            let item = notice_item(&of(kind));
            assert!(!item.title.is_empty(), "{kind:?}");
            assert!(!item.path.is_empty(), "{kind:?}");
            assert_eq!(item.kind, kind.as_str());
        }
    }
}
