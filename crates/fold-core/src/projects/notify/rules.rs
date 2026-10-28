use serde_json::{Map, Value, json};

use super::{Links, Notice, NoticeType, Part, column_label, short_title};
use crate::events::{Event, EventActor, types};
use crate::ids::{RequestId, TaskId};
use crate::tasks::status::TaskStatus;
use crate::time::Timestamp;

/// What the daemon reads for a notice before it builds it: the title, and for a new card its column, of the event's task, and the title of the event's request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Lookup {
    pub task: Option<TaskId>,
    pub request: Option<RequestId>,
}

/// What [`Lookup`] found. A title that is gone stays `None`, and the notice names the id alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventFacts {
    pub task_title: Option<String>,
    pub task_status: Option<TaskStatus>,
    pub request_title: Option<String>,
}

/// The proposal and revision that moved a card, when an earlier event of the same write names them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cause {
    pub proposal: Option<String>,
    pub revision: Option<String>,
}

fn text<'a>(event: &'a Event, key: &str) -> Option<&'a str> {
    event.data.get(key).and_then(Value::as_str)
}

/// The notice type of a stored event, with what its notice needs read, or `None` for an event that sends no notice. A `proposal.integrated` of a batch member sends none, as the batch's `review.batch_integrated` names it with the others.
pub fn classify(event: &Event) -> Option<(NoticeType, Lookup)> {
    let kind = event.event_type;
    let notice = if kind == types::TASK_CREATED {
        NoticeType::TaskCreated
    } else if kind == types::TASK_STATUS_CHANGED {
        NoticeType::TaskMoved
    } else if kind == types::TASK_BLOCKED_CHANGED {
        NoticeType::TaskBlocked
    } else if kind == types::TASK_SENT_BACK {
        NoticeType::TaskSentBack
    } else if kind == types::PROPOSAL_OPENED || kind == types::PROPOSAL_VERSION_ADDED {
        NoticeType::ProposalOpened
    } else if kind == types::REVIEW_SUBMITTED {
        match text(event, "verdict") {
            Some("approve") => NoticeType::ReviewApproved,
            Some("request_changes") => NoticeType::ReviewChangesRequested,
            _ => return None,
        }
    } else if kind == types::PROPOSAL_INTEGRATED {
        if event
            .data
            .get("batch")
            .is_some_and(|batch| !batch.is_null())
        {
            return None;
        }
        NoticeType::ProposalIntegrated
    } else if kind == types::REVIEW_BATCH_INTEGRATED {
        let published = event
            .data
            .get("members")
            .and_then(Value::as_array)
            .is_some_and(|members| !members.is_empty());
        if !published {
            return None;
        }
        NoticeType::ProposalIntegrated
    } else if kind == types::PROPOSAL_CLOSED {
        NoticeType::ProposalClosed
    } else if kind == types::INTEGRATION_CONFLICTED
        || kind == types::INTEGRATION_REFUSED
        || kind == types::INTEGRATION_HELD
    {
        NoticeType::IntegrationFailed
    } else if kind == types::CHECKS_FINISHED {
        if text(event, "state") != Some("failed") {
            return None;
        }
        NoticeType::ChecksFailed
    } else if kind == types::WORKER_FAILED || kind == types::WORKER_MODEL_UNREACHABLE {
        NoticeType::WorkerFailed
    } else if kind == types::GIT_COMMIT_FAILED
        || kind == types::REMOTE_PUSH_FAILED
        || kind == types::REMOTE_PULL_CONFLICTED
    {
        NoticeType::GitFailed
    } else if kind == types::MESSAGE_SENT {
        if event.actor.is_human() || human_recipients(event).is_empty() {
            return None;
        }
        NoticeType::MessageToHuman
    } else if kind == types::REQUEST_APPROVED {
        NoticeType::RequestApproved
    } else if kind == types::PROJECT_PAUSED || kind == types::PROJECT_RESUMED {
        NoticeType::ProjectPaused
    } else {
        return None;
    };
    let request = (kind == types::REQUEST_APPROVED)
        .then(|| text(event, "request").and_then(|id| id.parse().ok()))
        .flatten();
    Some((
        notice,
        Lookup {
            task: event.task,
            request,
        },
    ))
}

/// The human names a `message.sent` went to, from `human:<name>` recipients.
fn human_recipients(event: &Event) -> Vec<String> {
    event
        .data
        .get("recipients")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|name| name.strip_prefix(EventActor::HUMAN_PREFIX))
        .map(str::to_owned)
        .collect()
}

/// The proposal and revision behind a `task.status_changed`: the newest event before it of the same write about the same task that names a proposal. `earlier` holds the events before it, oldest first.
pub fn cause_of(event: &Event, earlier: &[Event]) -> Option<Cause> {
    let task = event.task?;
    earlier
        .iter()
        .rev()
        .take_while(|other| other.created_at == event.created_at)
        .filter(|other| other.task == Some(task))
        .find_map(|other| {
            let proposal = other.proposal?;
            Some(Cause {
                proposal: Some(proposal.to_string()),
                revision: (other.event_type == types::PROPOSAL_INTEGRATED)
                    .then(|| other.revision.map(|revision| revision.to_string()))
                    .flatten(),
            })
        })
}

fn actor(actor: &EventActor) -> Value {
    if actor.as_str() == EventActor::FOLD {
        json!({"kind": "fold"})
    } else if let Some(name) = actor.human_name() {
        json!({"kind": "human", "name": name})
    } else {
        json!({"kind": "agent", "name": actor.as_str()})
    }
}

/// `@ana` for a human, the worker name for an agent, `fold` for fold.
fn who(actor: &EventActor) -> String {
    match actor.human_name() {
        Some(name) => format!("@{name}"),
        None => actor.as_str().to_owned(),
    }
}

struct Builder<'a> {
    links: &'a Links,
    data: Map<String, Value>,
    line: Vec<Part>,
}

impl Builder<'_> {
    fn set(&mut self, key: &str, value: Value) {
        self.data.insert(key.to_owned(), value);
    }

    fn say(&mut self, text: impl Into<String>) {
        self.line.push(Part::text(text));
    }

    fn link(&mut self, label: impl Into<String>, path: &str) {
        self.line.push(Part::link(label, self.links.to(path)));
    }

    /// `{id, title, url}` of the task, and its id and title in the line.
    fn task(&mut self, task: TaskId, title: Option<&str>) {
        let id = task.to_string();
        let path = format!("t/{id}");
        let mut value = json!({"id": id, "url": self.links.to(&path)});
        self.link(id, &path);
        if let Some(title) = title {
            let title = short_title(title);
            self.say(format!(" {title}"));
            value["title"] = json!(title);
        }
        self.set("task", value);
    }

    fn proposal(&mut self, proposal: &str) {
        let path = format!("proposals/{proposal}");
        self.set(
            "proposal",
            json!({"id": proposal, "url": self.links.to(&path)}),
        );
        self.link(proposal, &path);
    }
}

/// The notice of `event`, whose type [`classify`] gave, with `facts` read for it and `cause` for a card move. `None` when the event lacks what its notice names.
pub fn notice(
    kind: NoticeType,
    event: &Event,
    facts: &EventFacts,
    cause: Option<&Cause>,
    links: &Links,
) -> Option<Notice> {
    let mut b = Builder {
        links,
        data: Map::new(),
        line: Vec::new(),
    };
    let title = facts.task_title.as_deref();
    let version = event.data.get("version").and_then(Value::as_u64);
    let proposal = event.proposal.map(|id| id.to_string());
    match kind {
        NoticeType::TaskCreated => {
            b.task(event.task?, title);
            b.say(" was created");
            if let Some(status) = facts.task_status {
                b.set("column", json!(status.as_str()));
                b.say(format!(" in {}", column_label(status)));
            }
            b.set("author", actor(&event.actor));
        }
        NoticeType::TaskMoved => {
            let from = text(event, "from").and_then(TaskStatus::parse)?;
            let to = text(event, "to").and_then(TaskStatus::parse)?;
            b.task(event.task?, title);
            b.set("from", json!(from.as_str()));
            b.set("to", json!(to.as_str()));
            b.say(format!(
                " moved {} → {}",
                column_label(from),
                column_label(to)
            ));
            if let Some(cause) = cause {
                let mut value = Map::new();
                if let Some(proposal) = &cause.proposal {
                    value.insert("proposal".to_owned(), json!(proposal));
                    b.say(" (");
                    b.link(proposal.clone(), &format!("proposals/{proposal}"));
                    b.say(")");
                }
                if let Some(revision) = &cause.revision {
                    value.insert("revision".to_owned(), json!(revision));
                }
                b.set("cause", Value::Object(value));
            }
            b.set("actor", actor(&event.actor));
        }
        NoticeType::TaskBlocked => {
            let blocked = event.data.get("blocked").and_then(Value::as_bool)?;
            b.task(event.task?, title);
            b.set("blocked", json!(blocked));
            b.say(if blocked {
                " is blocked"
            } else {
                " is no longer blocked"
            });
        }
        NoticeType::TaskSentBack => {
            b.task(event.task?, title);
            b.set("by", actor(&event.actor));
            b.say(format!(" went back to its worker ({})", who(&event.actor)));
        }
        NoticeType::ProposalOpened => {
            let proposal = proposal?;
            b.proposal(&proposal);
            let version = version.unwrap_or(1);
            b.set("version", json!(version));
            b.set("author", actor(&event.actor));
            b.say(format!(" v{version} opened by {} for ", who(&event.actor)));
            b.task(event.task?, title);
        }
        NoticeType::ReviewApproved | NoticeType::ReviewChangesRequested => {
            let proposal = proposal?;
            b.proposal(&proposal);
            if let Some(version) = version {
                b.set("version", json!(version));
                b.say(format!(" v{version}"));
            }
            b.set("reviewer", actor(&event.actor));
            b.say(if kind == NoticeType::ReviewApproved {
                format!(" was approved by {}", who(&event.actor))
            } else {
                format!(": {} asks for changes", who(&event.actor))
            });
            if let Some(task) = event.task {
                b.set("task", json!({"id": task.to_string(), "title": title.map(short_title), "url": links.to(&format!("t/{task}"))}));
            }
        }
        NoticeType::ProposalIntegrated => {
            if event.event_type == types::REVIEW_BATCH_INTEGRATED {
                integrated_batch(&mut b, event)?;
            } else {
                let proposal = proposal?;
                b.proposal(&proposal);
                b.say(" integrated into main");
                if let Some(revision) = event.revision {
                    b.set("revision", json!(revision.to_string()));
                    b.say(format!(" as {revision}"));
                }
                let tasks: Vec<String> = event.task.iter().map(ToString::to_string).collect();
                b.set("tasks", json!(tasks));
            }
        }
        NoticeType::ProposalClosed => {
            let proposal = proposal?;
            b.proposal(&proposal);
            let rejected = text(event, "verdict") == Some("reject");
            let reason = if rejected { "rejected" } else { "closed" };
            b.set("reason", json!(reason));
            b.say(format!(" was {reason} without integration"));
        }
        NoticeType::IntegrationFailed => {
            let reason = if event.event_type == types::INTEGRATION_CONFLICTED {
                "conflicted"
            } else if event.event_type == types::INTEGRATION_REFUSED {
                "refused"
            } else {
                "held"
            };
            b.say("Integration failed: ");
            match proposal {
                Some(proposal) => b.proposal(&proposal),
                None => b.link("main", "repository"),
            }
            b.set("reason", json!(reason));
            match event.data.get("files").and_then(Value::as_u64) {
                Some(paths) => {
                    b.set("paths", json!(paths));
                    b.say(format!(" {reason} in {paths} paths"));
                }
                None => b.say(format!(" {reason}")),
            }
        }
        NoticeType::ChecksFailed => {
            let stage = text(event, "stage")?;
            b.say("Check failed: ");
            match proposal {
                Some(proposal) => b.proposal(&proposal),
                None => b.task(event.task?, title),
            }
            if let Some(version) = version {
                b.set("version", json!(version));
                b.say(format!(" v{version}"));
            }
            if let (Some(task), false) = (event.task, b.data.contains_key("task")) {
                b.set(
                    "task",
                    json!({"id": task.to_string(), "url": links.to(&format!("t/{task}"))}),
                );
            }
            b.set("stage", json!(stage));
            b.say(format!(", stage {stage}"));
        }
        NoticeType::WorkerFailed => {
            let agent = text(event, "agent")?;
            let reason = if event.event_type == types::WORKER_MODEL_UNREACHABLE {
                "model_unreachable"
            } else {
                text(event, "reason").unwrap_or("failed")
            };
            b.set("agent", json!(agent));
            b.set("reason", json!(reason));
            if let Some(task) = event.task {
                b.set(
                    "task",
                    json!({"id": task.to_string(), "url": links.to(&format!("t/{task}"))}),
                );
            }
            b.say("Worker failed: ");
            b.link(agent, "agents");
            b.say(if reason == "model_unreachable" {
                " cannot reach its model".to_owned()
            } else {
                format!(" ({reason})")
            });
        }
        NoticeType::GitFailed => {
            let operation = if event.event_type == types::GIT_COMMIT_FAILED {
                "commit"
            } else if event.event_type == types::REMOTE_PUSH_FAILED {
                "push"
            } else {
                "pull"
            };
            b.set("operation", json!(operation));
            b.say(format!("Git {operation} failed"));
            if let Some(remote) = text(event, "remote") {
                let remote = short_title(remote);
                b.say(format!(" for the remote {remote}"));
                b.set("remote", json!(remote));
            }
            b.line.push(Part::text(" "));
            b.link("Repository", "repository");
        }
        NoticeType::MessageToHuman => {
            let message = text(event, "message")?;
            let thread = text(event, "thread").unwrap_or(message);
            let to = human_recipients(event);
            b.set("message", json!(message));
            b.set("thread", json!(thread));
            b.set("from", actor(&event.actor));
            b.set("to", json!(to));
            b.set("url", json!(links.to(&format!("messages?thread={thread}"))));
            b.say(format!("{} sent ", who(&event.actor)));
            b.link(message, &format!("messages?thread={thread}"));
            let names: Vec<String> = to.iter().map(|name| format!("@{name}")).collect();
            b.say(format!(" to {}", names.join(", ")));
        }
        NoticeType::RequestApproved => {
            let request = text(event, "request")?;
            let path = format!("requests/{request}");
            let mut value = json!({"id": request, "url": links.to(&path)});
            b.link(request, &path);
            if let Some(title) = &facts.request_title {
                let title = short_title(title);
                b.say(format!(" {title}"));
                value["title"] = json!(title);
            }
            b.set("request", value);
            b.say(" was approved");
        }
        NoticeType::ProjectPaused => {
            let paused = event.event_type == types::PROJECT_PAUSED;
            b.set("paused", json!(paused));
            b.set("by", actor(&event.actor));
            b.say(format!(
                "{} {} every agent",
                who(&event.actor),
                if paused { "paused" } else { "resumed" }
            ));
        }
        NoticeType::NeedsYouOpened | NoticeType::NeedsYouClosed | NoticeType::WebhookTest => {
            return None;
        }
    }
    Some(Notice {
        kind,
        seq: event.seq,
        at: event.created_at,
        data: b.data,
        line: b.line,
    })
}

fn integrated_batch(b: &mut Builder<'_>, event: &Event) -> Option<()> {
    let batch = text(event, "batch")?;
    let members = event.data.get("members").and_then(Value::as_array)?;
    let field = |key: &str| -> Vec<String> {
        members
            .iter()
            .filter_map(|member| member.get(key)?.as_str().map(str::to_owned))
            .collect()
    };
    let (proposals, tasks, revisions) = (field("proposal"), field("task"), field("revision"));
    let path = format!("batches/{batch}");
    b.set(
        "batch",
        json!({"id": batch, "url": b.links.to(&path), "proposals": proposals}),
    );
    b.set("tasks", json!(tasks));
    b.link(batch, &path);
    b.say(format!(
        " integrated {} proposals into main",
        proposals.len()
    ));
    if let Some(revision) = revisions.last() {
        b.set("revision", json!(revision));
        b.say(format!(" up to {revision}"));
    }
    Some(())
}

/// A Needs you item as a notice names it: its kind, the public id its screen opens, a short title of fold's own or the item's title, the humans it is for and its screen below the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeedsYouNotice {
    pub kind: String,
    pub item: Option<String>,
    pub title: String,
    pub humans: Vec<String>,
    pub path: String,
}

/// The `needs_you.opened` or `needs_you.closed` notice of `item`, with `seq` of the newest event read when the item came or left.
pub fn needs_you_notice(
    item: &NeedsYouNotice,
    opened: bool,
    seq: u64,
    at: Timestamp,
    links: &Links,
) -> Notice {
    let mut data = Map::new();
    data.insert("kind".to_owned(), json!(item.kind));
    data.insert("item".to_owned(), json!(item.item));
    let mut line = Vec::new();
    let kind = if opened {
        let title = short_title(&item.title);
        data.insert("title".to_owned(), json!(title));
        data.insert("humans".to_owned(), json!(item.humans));
        data.insert("url".to_owned(), json!(links.to(&item.path)));
        line.push(Part::text("Needs you: "));
        line.push(Part::link(title, links.to(&item.path)));
        NoticeType::NeedsYouOpened
    } else {
        line.push(Part::text("Needs you no longer waits on "));
        let label = match &item.item {
            Some(id) => format!("{} {id}", item.kind.replace('_', " ")),
            None => item.kind.replace('_', " "),
        };
        line.push(Part::link(label, links.to(&item.path)));
        NoticeType::NeedsYouClosed
    };
    Notice {
        kind,
        seq,
        at,
        data,
        line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventType;
    use crate::ids::{ProposalId, RevisionId};

    fn event(kind: EventType, actor: &str, data: Value) -> Event {
        Event {
            seq: 12,
            event_type: kind,
            actor: EventActor::parse(actor).unwrap(),
            task: TaskId::from_number(18),
            proposal: ProposalId::from_number(95),
            workspace: None,
            revision: RevisionId::from_number(99),
            data,
            created_at: Timestamp::from_millis(1000).unwrap(),
        }
    }

    fn links() -> Links {
        Links::new(Some("https://fold.example.org"), "nobs")
    }

    fn facts() -> EventFacts {
        EventFacts {
            task_title: Some("Add PIN\nsign-in".to_owned()),
            task_status: Some(TaskStatus::Backlog),
            request_title: Some("Billing".to_owned()),
        }
    }

    fn built(event: &Event) -> Option<Notice> {
        let (kind, _) = classify(event)?;
        notice(kind, event, &facts(), None, &links())
    }

    fn line(notice: &Notice) -> String {
        notice
            .line
            .iter()
            .map(|part| match part {
                Part::Text { text } => text.clone(),
                Part::Link { label, .. } => label.clone(),
            })
            .collect()
    }

    #[test]
    fn a_card_move_names_its_columns_its_title_and_its_cause() {
        let moved = event(
            types::TASK_STATUS_CHANGED,
            "fold",
            json!({"from": "in_progress", "to": "review"}),
        );
        let opened = event(
            types::PROPOSAL_OPENED,
            "backend/w-23",
            json!({"version": 1}),
        );
        let cause = cause_of(&moved, std::slice::from_ref(&opened));
        assert_eq!(
            cause,
            Some(Cause {
                proposal: Some("P-95".to_owned()),
                revision: None
            })
        );
        let notice = notice(
            NoticeType::TaskMoved,
            &moved,
            &facts(),
            cause.as_ref(),
            &links(),
        )
        .unwrap();
        assert_eq!(notice.seq, 12);
        assert_eq!(notice.data["to"], "review");
        assert_eq!(notice.data["from"], "in_progress");
        assert_eq!(notice.data["task"]["id"], "T-18");
        assert_eq!(notice.data["task"]["title"], "Add PIN sign-in");
        assert_eq!(
            notice.data["task"]["url"],
            "https://fold.example.org/p/nobs/t/T-18"
        );
        assert_eq!(notice.data["cause"]["proposal"], "P-95");
        assert_eq!(notice.data["actor"]["kind"], "fold");
        assert_eq!(
            line(&notice),
            "T-18 Add PIN sign-in moved In progress → Review (P-95)"
        );
        let mut later = opened.clone();
        later.created_at = Timestamp::from_millis(5).unwrap();
        assert_eq!(cause_of(&moved, &[later]), None);
        let integrated = event(types::PROPOSAL_INTEGRATED, "human:ana", json!({}));
        assert_eq!(
            cause_of(&moved, &[integrated]).unwrap().revision.as_deref(),
            Some("r99")
        );
        let mut bare = moved.clone();
        bare.task = None;
        assert_eq!(cause_of(&bare, &[opened]), None);
        assert!(notice_none(
            &moved,
            json!({"from": "nowhere", "to": "review"})
        ));
    }

    fn notice_none(base: &Event, data: Value) -> bool {
        let mut broken = base.clone();
        broken.data = data;
        built(&broken).is_none()
    }

    #[test]
    fn every_mapped_event_becomes_its_notice() {
        let cases = [
            (
                types::TASK_CREATED,
                "frontend",
                json!({}),
                NoticeType::TaskCreated,
                "T-18 Add PIN sign-in was created in Backlog",
            ),
            (
                types::TASK_BLOCKED_CHANGED,
                "fold",
                json!({"blocked": true}),
                NoticeType::TaskBlocked,
                "T-18 Add PIN sign-in is blocked",
            ),
            (
                types::TASK_BLOCKED_CHANGED,
                "fold",
                json!({"blocked": false}),
                NoticeType::TaskBlocked,
                "T-18 Add PIN sign-in is no longer blocked",
            ),
            (
                types::TASK_SENT_BACK,
                "web-lead",
                json!({"worker": "web-lead/w-1"}),
                NoticeType::TaskSentBack,
                "T-18 Add PIN sign-in went back to its worker (web-lead)",
            ),
            (
                types::PROPOSAL_VERSION_ADDED,
                "backend/w-2",
                json!({"version": 2}),
                NoticeType::ProposalOpened,
                "P-95 v2 opened by backend/w-2 for T-18 Add PIN sign-in",
            ),
            (
                types::REVIEW_SUBMITTED,
                "review",
                json!({"verdict": "approve", "version": 2}),
                NoticeType::ReviewApproved,
                "P-95 v2 was approved by review",
            ),
            (
                types::REVIEW_SUBMITTED,
                "human:ana",
                json!({"verdict": "request_changes", "version": 1}),
                NoticeType::ReviewChangesRequested,
                "P-95 v1: @ana asks for changes",
            ),
            (
                types::PROPOSAL_INTEGRATED,
                "fold",
                json!({"version": 1}),
                NoticeType::ProposalIntegrated,
                "P-95 integrated into main as r99",
            ),
            (
                types::PROPOSAL_CLOSED,
                "review",
                json!({"reason": "REVIEW BODY TEXT", "verdict": "reject"}),
                NoticeType::ProposalClosed,
                "P-95 was rejected without integration",
            ),
            (
                types::PROPOSAL_CLOSED,
                "backend",
                json!({"reason": "an agent's words"}),
                NoticeType::ProposalClosed,
                "P-95 was closed without integration",
            ),
            (
                types::INTEGRATION_CONFLICTED,
                "fold",
                json!({"files": 3}),
                NoticeType::IntegrationFailed,
                "Integration failed: P-95 conflicted in 3 paths",
            ),
            (
                types::INTEGRATION_REFUSED,
                "fold",
                json!({"code": "stale"}),
                NoticeType::IntegrationFailed,
                "Integration failed: P-95 refused",
            ),
            (
                types::CHECKS_FINISHED,
                "fold",
                json!({"state": "failed", "stage": "test", "version": 2}),
                NoticeType::ChecksFailed,
                "Check failed: P-95 v2, stage test",
            ),
            (
                types::WORKER_FAILED,
                "fold",
                json!({"agent": "api-lead", "reason": "exited"}),
                NoticeType::WorkerFailed,
                "Worker failed: api-lead (exited)",
            ),
            (
                types::WORKER_MODEL_UNREACHABLE,
                "fold",
                json!({"agent": "api-lead", "turns": 3}),
                NoticeType::WorkerFailed,
                "Worker failed: api-lead cannot reach its model",
            ),
            (
                types::GIT_COMMIT_FAILED,
                "fold",
                json!({"code": "git_failed"}),
                NoticeType::GitFailed,
                "Git commit failed Repository",
            ),
            (
                types::REMOTE_PUSH_FAILED,
                "fold",
                json!({"remote": "origin", "code": "x"}),
                NoticeType::GitFailed,
                "Git push failed for the remote origin Repository",
            ),
            (
                types::REMOTE_PULL_CONFLICTED,
                "human:ana",
                json!({"remote": "origin"}),
                NoticeType::GitFailed,
                "Git pull failed for the remote origin Repository",
            ),
            (
                types::MESSAGE_SENT,
                "backend/w-2",
                json!({"message": "m-31", "thread": "m-30", "recipients": ["human:ana", "frontend"]}),
                NoticeType::MessageToHuman,
                "backend/w-2 sent m-31 to @ana",
            ),
            (
                types::REQUEST_APPROVED,
                "human:ana",
                json!({"request": "RQ-4"}),
                NoticeType::RequestApproved,
                "RQ-4 Billing was approved",
            ),
            (
                types::PROJECT_PAUSED,
                "human:ana",
                json!({}),
                NoticeType::ProjectPaused,
                "@ana paused every agent",
            ),
            (
                types::PROJECT_RESUMED,
                "human:ana",
                json!({}),
                NoticeType::ProjectPaused,
                "@ana resumed every agent",
            ),
        ];
        for (kind, actor, data, expected, text) in cases {
            let event = event(kind, actor, data);
            let notice = built(&event).unwrap_or_else(|| panic!("{kind}"));
            assert_eq!(notice.kind, expected, "{kind}");
            assert_eq!(line(&notice), text, "{kind}");
            let body = serde_json::to_string(&notice).unwrap();
            assert!(
                !body.contains("REVIEW BODY TEXT") && !body.contains("agent's words"),
                "{body}"
            );
        }
    }

    #[test]
    fn events_without_a_notice_send_nothing() {
        for (kind, actor, data) in [
            (
                types::REVIEW_SUBMITTED,
                "review",
                json!({"verdict": "comment"}),
            ),
            (types::PROPOSAL_INTEGRATED, "fold", json!({"batch": "rb-2"})),
            (
                types::REVIEW_BATCH_INTEGRATED,
                "fold",
                json!({"batch": "rb-2", "members": []}),
            ),
            (
                types::CHECKS_FINISHED,
                "fold",
                json!({"state": "passed", "stage": "test"}),
            ),
            (
                types::MESSAGE_SENT,
                "human:ana",
                json!({"recipients": ["human:bo"]}),
            ),
            (
                types::MESSAGE_SENT,
                "frontend",
                json!({"recipients": ["backend"]}),
            ),
            (types::CLAIM_ACQUIRED, "frontend", json!({})),
        ] {
            assert!(classify(&event(kind, actor, data)).is_none(), "{kind}");
        }
        let created = event(types::TASK_CREATED, "frontend", json!({}));
        assert_eq!(
            classify(&created).unwrap().1,
            Lookup {
                task: TaskId::from_number(18),
                request: None
            }
        );
        let approved = event(
            types::REQUEST_APPROVED,
            "human:ana",
            json!({"request": "RQ-4"}),
        );
        assert_eq!(
            classify(&approved).unwrap().1.request,
            RequestId::from_number(4)
        );
        assert!(classify(&event(types::TASK_UPDATED, "fold", json!({}))).is_none());
    }

    #[test]
    fn a_batch_names_its_proposals_tasks_and_last_revision() {
        let batch = event(
            types::REVIEW_BATCH_INTEGRATED,
            "fold",
            json!({"batch": "rb-3", "outcome": "together", "members": [
                {"proposal": "P-1", "task": "T-1", "revision": "r7"},
                {"proposal": "P-2", "task": "T-2", "revision": "r8"},
            ]}),
        );
        let notice = built(&batch).unwrap();
        assert_eq!(notice.kind, NoticeType::ProposalIntegrated);
        assert_eq!(notice.data["batch"]["id"], "rb-3");
        assert_eq!(notice.data["tasks"], json!(["T-1", "T-2"]));
        assert_eq!(notice.data["revision"], "r8");
        assert_eq!(
            line(&notice),
            "rb-3 integrated 2 proposals into main up to r8"
        );
    }

    #[test]
    fn needs_you_notices_name_the_item_and_never_more() {
        let item = NeedsYouNotice {
            kind: "question".to_owned(),
            item: Some("m-31".to_owned()),
            title: "@backend asks a question".to_owned(),
            humans: vec!["ana".to_owned()],
            path: "messages?thread=m-30".to_owned(),
        };
        let opened = needs_you_notice(&item, true, 40, Timestamp::EPOCH, &links());
        assert_eq!(opened.kind, NoticeType::NeedsYouOpened);
        assert_eq!(opened.data["item"], "m-31");
        assert_eq!(opened.data["humans"], json!(["ana"]));
        assert_eq!(
            opened.data["url"],
            "https://fold.example.org/p/nobs/messages?thread=m-30"
        );
        assert_eq!(line(&opened), "Needs you: @backend asks a question");
        let closed = needs_you_notice(
            &item,
            false,
            41,
            Timestamp::EPOCH,
            &Links::new(None, "nobs"),
        );
        assert_eq!(closed.kind, NoticeType::NeedsYouClosed);
        assert!(!closed.data.contains_key("title"));
        assert_eq!(line(&closed), "Needs you no longer waits on question m-31");
        let folder = NeedsYouNotice {
            item: None,
            kind: "folder_held".to_owned(),
            ..item
        };
        assert_eq!(
            line(&needs_you_notice(
                &folder,
                false,
                1,
                Timestamp::EPOCH,
                &links()
            )),
            "Needs you no longer waits on folder held"
        );
    }
}
