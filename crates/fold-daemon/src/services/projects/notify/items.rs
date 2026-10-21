use fold_core::projects::notify::{NoticeItem, short_title, web_link};

use crate::services::agents::{NeedsYouItem, NeedsYouKind};

/// The key that names `item` across reads of Needs you and across a daemon restart: its kind and the id, agent or [`NeedsYouItem::about`] it is about. The project folder's waiting item has its kind alone, as at most one waits, so it keeps its key while its paths change.
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

/// The public id a notice names for `item`: what its screen opens, or the agent of an item about a worker.
fn public_id(item: &NeedsYouItem) -> Option<String> {
    match item.kind {
        NeedsYouKind::WorkerUnreachable
        | NeedsYouKind::WorkerModelUnreachable
        | NeedsYouKind::ClaudeNotFound => item.agent.as_ref().map(ToString::to_string),
        NeedsYouKind::HumanChange
        | NeedsYouKind::HeadMoved
        | NeedsYouKind::FolderHeld
        | NeedsYouKind::UnstashConflicted
        | NeedsYouKind::ChecksFileInvalidOnMain => None,
        _ => item_key(item).split_once(':').map(|(_, id)| id.to_owned()),
    }
}

/// The title of a notice: the proposal, request or task title, a question's short header, or words of fold's own. Never a message body, a blocker's reason, a path, a commit or Claude Code's error text.
fn title(item: &NeedsYouItem) -> String {
    let agent = item
        .agent
        .as_ref()
        .map_or_else(|| "An agent".to_owned(), ToString::to_string);
    let own = |text: &Option<String>, fallback: &str| {
        text.as_deref()
            .map(short_title)
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| fallback.to_owned())
    };
    match item.kind {
        NeedsYouKind::Question => own(&item.header, &format!("{agent} asks a question")),
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
        NeedsYouKind::ClaudeNotFound => format!("{agent} cannot start: the daemon finds no claude"),
    }
}

/// The path in the web app of the screen that answers `item`, as the web app's Needs you list links it.
fn path(slug: &str, item: &NeedsYouItem) -> String {
    let project = format!("/p/{slug}");
    let id = |value: Option<String>| value.unwrap_or_default();
    match item.kind {
        NeedsYouKind::Question => match item.message {
            Some((_, thread)) => format!("{project}/messages?thread={thread}"),
            None => format!("{project}/messages"),
        },
        NeedsYouKind::Permission => match item.question {
            Some(question) => format!("{project}/agents?question={question}"),
            None => format!("{project}/agents"),
        },
        NeedsYouKind::HumanReview => {
            format!(
                "{project}/proposals/{}",
                id(item.proposal.map(|p| p.to_string()))
            )
        }
        NeedsYouKind::RequestDraft => {
            format!(
                "{project}/requests/{}",
                id(item.request.map(|r| r.to_string()))
            )
        }
        NeedsYouKind::HumanTask | NeedsYouKind::Blocker => {
            format!("{project}/t/{}", id(item.task.map(|t| t.to_string())))
        }
        NeedsYouKind::HumanChange
        | NeedsYouKind::HeadMoved
        | NeedsYouKind::FolderHeld
        | NeedsYouKind::UnstashConflicted
        | NeedsYouKind::ChecksFileInvalidOnMain => format!("{project}/repository"),
        NeedsYouKind::WorkerUnreachable
        | NeedsYouKind::WorkerModelUnreachable
        | NeedsYouKind::ClaudeNotFound => format!("{project}/agents"),
    }
}

/// `item` as a notice item of the project `slug`, with its link below the web app at `base`.
pub fn notice_item(base: &str, slug: &str, item: &NeedsYouItem) -> NoticeItem {
    NoticeItem {
        kind: item.kind.as_str().to_owned(),
        id: public_id(item),
        title: title(item),
        link: web_link(base, &path(slug, item)),
        since: item.since.as_millis(),
    }
}

#[cfg(test)]
mod tests {
    use fold_core::{AgentName, MessageId, PermissionQuestionId, ProposalId, TaskId, Timestamp};

    use super::*;

    fn of(kind: NeedsYouKind) -> NeedsYouItem {
        NeedsYouItem::new(kind, Timestamp::from_millis(7).unwrap())
    }

    const BASE: &str = "https://fold.example.com/";

    #[test]
    fn a_question_names_its_message_and_never_its_body() {
        let mut item = of(NeedsYouKind::Question);
        item.message = Some((
            MessageId::from_number(31).unwrap(),
            MessageId::from_number(30).unwrap(),
        ));
        item.agent = Some(AgentName::parse("web-worker").unwrap());
        assert_eq!(item_key(&item), "question:m-31");
        let notice = notice_item(BASE, "demo", &item);
        assert_eq!(notice.id.as_deref(), Some("m-31"));
        assert_eq!(notice.title, "web-worker asks a question");
        assert_eq!(
            notice.link,
            "https://fold.example.com/p/demo/messages?thread=m-30"
        );
        assert_eq!(notice.since, 7);
        item.header = Some("Chart\nlibrary".to_owned());
        assert_eq!(notice_item(BASE, "demo", &item).title, "Chart library");
    }

    #[test]
    fn every_kind_has_a_key_a_title_and_a_link() {
        let mut review = of(NeedsYouKind::HumanReview);
        review.proposal = Some(ProposalId::from_number(3).unwrap());
        review.title = Some("Database schema".to_owned());
        let review = notice_item(BASE, "demo", &review);
        assert_eq!(
            (review.id.as_deref(), review.title.as_str()),
            (Some("P-3"), "Database schema")
        );
        assert_eq!(review.link, "https://fold.example.com/p/demo/proposals/P-3");

        let mut blocker = of(NeedsYouKind::Blocker);
        blocker.task = Some(TaskId::from_number(12).unwrap());
        blocker.title = Some("Ship it".to_owned());
        blocker.reason = Some("a reason a human wrote".to_owned());
        assert_eq!(item_key(&blocker), "blocker:T-12");
        let shown = notice_item(BASE, "demo", &blocker);
        assert_eq!(shown.title, "Ship it");
        assert_eq!(shown.link, "https://fold.example.com/p/demo/t/T-12");

        let mut permission = of(NeedsYouKind::Permission);
        permission.question = Some(PermissionQuestionId::from_number(2).unwrap());
        permission.tool = Some("Bash".to_owned());
        let shown = notice_item(BASE, "demo", &permission);
        assert_eq!(
            (shown.id.as_deref(), shown.title.as_str()),
            (Some("pq-2"), "An agent asks to use Bash")
        );
        assert_eq!(
            shown.link,
            "https://fold.example.com/p/demo/agents?question=pq-2"
        );

        let mut worker = of(NeedsYouKind::WorkerModelUnreachable);
        worker.agent = Some(AgentName::parse("api-lead").unwrap());
        worker.title = Some("Claude Code's own error text".to_owned());
        assert_eq!(item_key(&worker), "worker_model_unreachable:api-lead");
        let shown = notice_item(BASE, "demo", &worker);
        assert_eq!(shown.title, "api-lead cannot reach the model");
        assert_eq!(shown.id.as_deref(), Some("api-lead"));

        let mut change = of(NeedsYouKind::HumanChange);
        change.paths = vec!["secret/path.rs".to_owned()];
        change.commit = Some("9f2c4e1".to_owned());
        change.about = Some("tree-1".to_owned());
        assert_eq!(item_key(&change), "human_change:tree-1");
        let shown = notice_item(BASE, "demo", &change);
        assert_eq!(shown.id, None);
        assert_eq!(shown.link, "https://fold.example.com/p/demo/repository");

        let moved = |since: i64, hint: &str| {
            item_key(&NeedsYouItem::head_moved(
                Timestamp::from_millis(since).unwrap(),
                hint.to_owned(),
            ))
        };
        assert_eq!(
            moved(5, "run `git switch main`"),
            moved(90, "run `git switch main`"),
            "the same move keeps its key whenever it is seen again"
        );
        assert_ne!(
            moved(5, "run `git switch main`"),
            moved(5, "run `git reset --soft 1a2b`")
        );
        assert!(moved(5, "x").starts_with("head_moved:"));
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
        ] {
            let item = notice_item(BASE, "demo", &of(kind));
            assert!(!item.title.is_empty(), "{kind:?}");
            assert!(
                item.link.starts_with("https://fold.example.com/p/demo/"),
                "{kind:?}"
            );
            assert_eq!(item.kind, kind.as_str());
        }
    }
}
