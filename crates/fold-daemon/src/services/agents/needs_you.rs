use std::collections::BTreeSet;

use fold_core::requests::{RequestState, Requester};
use fold_core::reviews::checks::CHECKS_FILE;
use fold_core::roles::{CLAUDE_NOT_FOUND, restart_hint};
use fold_core::turn_errors::{MODEL_UNREACHABLE_HINT, model_unreachable};
use fold_core::{
    AddressedAs, AgentName, Choice, HumanName, MessageId, PermissionQuestionId, ProposalId,
    RequestId, Sender, Target, TaskId, Timestamp, WorkerState,
};
use fold_store::agents::{self, QuestionKind};
use fold_store::reviews;
use fold_store::tasks;
use fold_store::tasks::requests::{self, RequestFilter};
use fold_store::{Store, StoreError, messages};

use crate::services::reviews::awaiting_human_review;

/// What waits for a human.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeedsYouKind {
    /// An open ask from an agent to a human: a message the agent marked as asking, a question with choices, or a worker's `AskUserQuestion` call, with no human reply in its thread and no closure. A report to the humans is not an item. The board's `question` mark and the Messages count read the same asks.
    Question,
    /// A worker's tool call that auto mode would not decide.
    Permission,
    /// A proposal of a marked task that waits for a human approval.
    HumanReview,
    /// A card draft that waits for its human requester.
    RequestDraft,
    /// A human task in Todo: every prerequisite is Done and a person does the work.
    HumanTask,
    /// A task with a blocker, which waits until a human clears it.
    Blocker,
    /// A human change from the project folder that conflicts with main. Main did not move; the main agent explains the conflict in its conversation and prepares the merge for the human's OK.
    HumanChange,
    /// The project folder's Git branch or `HEAD` shows a change fold did not make, so fold does not update the folder until the human brings the branch back; `reason` holds the command that does it.
    HeadMoved,
    /// Paths main changed hold changes of the human's that no revision records, so fold does not update the project folder until the human records them with `fld record` or takes them back; `paths` names them.
    FolderHeld,
    /// `fld unstash` wrote conflict markers into the files `paths` names and kept the stash entry, so fold does not update the project folder until the human fixes the files and records them with `fld record`.
    UnstashConflicted,
    /// The checks file on main is not valid, so fold holds the integrations of proposals until a revision of main fixes it; `title` states the file, the line and the problem, and `paths` names the file.
    ChecksFileInvalidOnMain,
    /// A worker failed because it cannot reach fold: its fold MCP server did not connect, or the fld binary it would start is gone. A new start fails the same way until the daemon runs the installed fld, so `reason` names `fld restart`, after which the daemon start resumes the worker once fld is there; the item ends when the worker runs again, through that resume or through `resume_worker`.
    WorkerUnreachable,
    /// A worker's turns failed [`fold_core::turn_errors::MODEL_UNREACHABLE_TURNS`] times in a row before any token reached the model, so it does no work until a human fixes what Claude Code needs, such as its sign-in: `title` holds Claude Code's error and `reason` what to check. The item ends when a turn of the worker reaches the model, with or without an error, or when the worker is stopped. A worker whose process failed cannot be stopped, so it keeps the item until `resume_worker` runs it again and a turn reaches the model; a start that runs a stopped worker again shows the item again until then.
    WorkerModelUnreachable,
    /// A worker failed because the daemon found no `claude` on the `PATH` it gives workers. The daemon reads that `PATH` from the user's login shell at its start, so `reason` names `fld restart`, after which the daemon start resumes the worker; the item ends when the worker runs again.
    ClaudeNotFound,
    /// fold turned a notice target off after [`fold_core::projects::notify::OFF_AFTER_FAILURES`] failed sends in a row; `title` holds the target's name and `target` its id. A successful test send turns it on again and ends the item.
    NotifyTargetOff,
}

/// The screen on which a human answers an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Messages,
    Worker,
    Proposal,
    Request,
    Task,
    Repository,
    Settings,
}

impl NeedsYouKind {
    /// The kind's wire name, as the Needs you list and the notices name it.
    pub fn as_str(self) -> &'static str {
        match self {
            NeedsYouKind::Question => "question",
            NeedsYouKind::Permission => "permission",
            NeedsYouKind::HumanReview => "human_review",
            NeedsYouKind::RequestDraft => "request_draft",
            NeedsYouKind::HumanTask => "human_task",
            NeedsYouKind::Blocker => "blocker",
            NeedsYouKind::HumanChange => "human_change",
            NeedsYouKind::HeadMoved => "head_moved",
            NeedsYouKind::FolderHeld => "folder_held",
            NeedsYouKind::UnstashConflicted => "unstash_conflicted",
            NeedsYouKind::ChecksFileInvalidOnMain => "checks_file_invalid_on_main",
            NeedsYouKind::WorkerUnreachable => "worker_unreachable",
            NeedsYouKind::WorkerModelUnreachable => "worker_model_unreachable",
            NeedsYouKind::ClaudeNotFound => "claude_not_found",
            NeedsYouKind::NotifyTargetOff => "notify_target_off",
        }
    }

    pub fn screen(self) -> Screen {
        match self {
            NeedsYouKind::Question => Screen::Messages,
            NeedsYouKind::Permission => Screen::Worker,
            NeedsYouKind::HumanReview => Screen::Proposal,
            NeedsYouKind::RequestDraft => Screen::Request,
            NeedsYouKind::HumanTask | NeedsYouKind::Blocker => Screen::Task,
            NeedsYouKind::HumanChange
            | NeedsYouKind::HeadMoved
            | NeedsYouKind::FolderHeld
            | NeedsYouKind::UnstashConflicted
            | NeedsYouKind::ChecksFileInvalidOnMain => Screen::Repository,
            NeedsYouKind::WorkerUnreachable
            | NeedsYouKind::WorkerModelUnreachable
            | NeedsYouKind::ClaudeNotFound => Screen::Worker,
            NeedsYouKind::NotifyTargetOff => Screen::Settings,
        }
    }
}

/// One item that waits for a human, with the public ids its screen opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeedsYouItem {
    pub kind: NeedsYouKind,
    /// When the item started to wait.
    pub since: Timestamp,
    /// The question message and its thread.
    pub message: Option<(MessageId, MessageId)>,
    pub question: Option<PermissionQuestionId>,
    /// The agent that asks: the question's sender, the worker of a permission question, or the worker that cannot reach fold.
    pub agent: Option<AgentName>,
    pub tool: Option<String>,
    pub task: Option<TaskId>,
    pub proposal: Option<ProposalId>,
    pub request: Option<RequestId>,
    /// The human a human review or a request draft waits for, or whose change a `HumanChange` is; `None` when any human may answer.
    pub human: Option<HumanName>,
    /// The humans a `Question` names, in name order, when none of its recipients came from `humans`; empty when any human may answer. The Messages count of each of them, and of no one else, counts the ask.
    pub humans: Vec<HumanName>,
    pub choices: Vec<Choice>,
    /// The short header of a `Question`.
    pub header: Option<String>,
    /// The proposal title of a `HumanReview`; the request title of a `RequestDraft`, which the claim gave it before any draft; the task title of a `HumanTask` or a `Blocker`; Claude Code's error of a `WorkerModelUnreachable`.
    pub title: Option<String>,
    /// The reason of a `Blocker`; the command that brings the branch back for `HeadMoved`, records the folder's changes for `FolderHeld`, ends the conflict for `UnstashConflicted`, or lets a `WorkerUnreachable` worker reach fold again; what to check for a `WorkerModelUnreachable`.
    pub reason: Option<String>,
    /// The human's Git commit of a `HumanChange`.
    pub commit: Option<String>,
    /// The paths of a `HumanChange` that conflict with main, those of a `FolderHeld` that keep the folder where it is, or those of an `UnstashConflicted` that hold the conflict, in path order.
    pub paths: Vec<String>,
    /// What a project folder item is about, the same for as long as it is the same item and across a daemon restart: the tree of a `HumanChange` or of an `UnstashConflicted` stash entry, the time main's checks file started to hold integrations for a `ChecksFileInvalidOnMain`, or a digest of the hint of a `HeadMoved`, which names the commit fold left or the branch to go back to. `None` for every other kind: the project folder waits as one `FolderHeld` item whatever paths hold it.
    pub about: Option<String>,
    /// The notice target of a `NotifyTargetOff`.
    pub target: Option<fold_core::NotifyTargetId>,
}

impl NeedsYouItem {
    /// The `HeadMoved` item of a branch that moved at `since`, with the command that brings it back.
    pub fn head_moved(since: Timestamp, hint: String) -> Self {
        let mut item = Self::new(NeedsYouKind::HeadMoved, since);
        item.about = Some(blake3::hash(hint.as_bytes()).to_hex()[..16].to_owned());
        item.reason = Some(hint);
        item
    }

    /// The `FolderHeld` item of a project folder that waits since `since` on the human's changes to `paths`.
    pub fn folder_held(since: Timestamp, paths: Vec<String>) -> Self {
        let mut item = Self::new(NeedsYouKind::FolderHeld, since);
        item.reason = Some(fold_core::folder::FOLDER_HELD_NEXT.to_owned());
        item.paths = paths;
        item
    }

    pub(crate) fn new(kind: NeedsYouKind, since: Timestamp) -> Self {
        Self {
            kind,
            since,
            message: None,
            question: None,
            agent: None,
            tool: None,
            task: None,
            proposal: None,
            request: None,
            human: None,
            humans: Vec::new(),
            choices: Vec::new(),
            header: None,
            title: None,
            reason: None,
            commit: None,
            paths: Vec::new(),
            about: None,
            target: None,
        }
    }

    /// The item of a notice target fold turned off at `since` after its sends failed.
    pub fn notify_target_off(
        since: Timestamp,
        target: fold_core::NotifyTargetId,
        name: &str,
    ) -> Self {
        let mut item = Self::new(NeedsYouKind::NotifyTargetOff, since);
        item.target = Some(target);
        item.title = Some(name.to_owned());
        item.reason = Some("send a test from the Settings tab or with `fld notify test`; a test that gets through turns the target on again".to_owned());
        item
    }
}

/// The humans an ask waits for: its human recipients by name, in name order, or none when a recipient came from `humans`, which any human answers.
fn asked_humans(recipients: &[messages::RecipientRow]) -> Vec<HumanName> {
    if recipients
        .iter()
        .any(|recipient| recipient.addressed_as == AddressedAs::Humans)
    {
        return Vec::new();
    }
    recipients
        .iter()
        .filter_map(|recipient| match &recipient.target {
            Target::Human(name) => Some(name.clone()),
            Target::Worker(_) => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The second line of a `ChecksFileInvalidOnMain` item: the file, then the problem as the parse states it, such as `fold-checks.toml line 12: unknown field`.
fn held_title(problem: &str) -> String {
    match problem
        .strip_prefix(CHECKS_FILE)
        .and_then(|rest| rest.strip_prefix(": "))
    {
        Some(rest) => format!("{CHECKS_FILE} {rest}"),
        None => problem.to_owned(),
    }
}

/// `items` with the items of the project folder's Git attention, oldest first: the branch that moved, from `moved`, and the folder that waits on the human's changes, from `held`. While a conflicted human change or a conflicted unstash waits, its item alone stands for the folder: it names the conflicting files, and its end lets the folder go on, so the waiting folder shows again only when it still waits after that.
pub fn with_folder_attention(
    mut items: Vec<NeedsYouItem>,
    moved: Option<(Timestamp, String)>,
    held: Option<(Timestamp, Vec<String>)>,
) -> Vec<NeedsYouItem> {
    if let Some((since, hint)) = moved {
        items.push(NeedsYouItem::head_moved(since, hint));
    }
    let conflicted = items.iter().any(|item| {
        matches!(
            item.kind,
            NeedsYouKind::HumanChange | NeedsYouKind::UnstashConflicted
        )
    });
    if let Some((since, paths)) = held.filter(|_| !conflicted) {
        items.push(NeedsYouItem::folder_held(since, paths));
    }
    items.sort_by_key(|item| item.since);
    items
}

/// Every item that waits for a human, oldest first. The daemon decides each item, so no client derives one.
pub async fn needs_you(store: &Store) -> Result<Vec<NeedsYouItem>, StoreError> {
    let mut items = Vec::new();
    let mut conn = store.read().acquire().await?;
    for row in messages::awaiting_human(&mut conn).await? {
        let mut item = NeedsYouItem::new(NeedsYouKind::Question, row.created_at);
        item.message = Some((row.id, row.thread));
        item.agent = match &row.sender {
            Sender::Worker(worker) => Some(worker.agent()),
            Sender::Human(_) | Sender::Fold => None,
        };
        item.task = row.task;
        item.proposal = row.proposal;
        item.request = row.request;
        item.humans = asked_humans(&row.recipients);
        item.choices = row.choices;
        item.header = row.header;
        items.push(item);
    }
    for row in agents::list_worker_processes(&mut conn).await? {
        if model_unreachable(row.failed_turns, row.state) {
            let mut item = NeedsYouItem::new(
                NeedsYouKind::WorkerModelUnreachable,
                row.last_error_at.unwrap_or(row.started_at),
            );
            item.agent = Some(row.agent.clone());
            item.title = row.last_error.clone();
            item.reason = Some(MODEL_UNREACHABLE_HINT.to_owned());
            items.push(item);
        }
        let hint = row
            .failed_reason
            .as_deref()
            .filter(|_| row.state == WorkerState::Failed)
            .and_then(restart_hint);
        let (Some(since), Some(hint)) = (row.stopped_at, hint) else {
            continue;
        };
        let kind = if row.failed_reason.as_deref() == Some(CLAUDE_NOT_FOUND) {
            NeedsYouKind::ClaudeNotFound
        } else {
            NeedsYouKind::WorkerUnreachable
        };
        let mut item = NeedsYouItem::new(kind, since);
        item.agent = Some(row.agent);
        item.reason = Some(hint.to_owned());
        items.push(item);
    }
    for question in agents::open_questions(&mut *conn).await? {
        if question.kind != QuestionKind::Tool {
            continue;
        }
        let mut item = NeedsYouItem::new(NeedsYouKind::Permission, question.created_at);
        item.question = question.number;
        item.agent = Some(question.agent);
        item.tool = Some(question.tool);
        items.push(item);
    }
    let drafts = requests::list(
        &mut conn,
        &RequestFilter {
            state: Some(RequestState::DraftReady),
            ..RequestFilter::default()
        },
    )
    .await?;
    if let Some(change) = fold_store::workspaces::conflicted_human_change(&mut conn).await? {
        let mut item = NeedsYouItem::new(NeedsYouKind::HumanChange, change.created_at);
        item.about = Some(change.tree.to_string());
        item.human = HumanName::parse(&change.human).ok();
        item.commit = change.commit;
        item.paths = change
            .conflicts
            .iter()
            .map(|row| String::from_utf8_lossy(&row.path).into_owned())
            .collect();
        items.push(item);
    }
    let stack = fold_store::workspaces::stash_stack(&mut conn).await?;
    if let Some(entry) = stack.first()
        && let Some(since) = entry.conflicted_at
    {
        let mut item = NeedsYouItem::new(NeedsYouKind::UnstashConflicted, since);
        item.about = Some(entry.tree.to_string());
        item.reason = Some(fold_core::folder::UNSTASH_CONFLICTED_NEXT.to_owned());
        item.paths = entry
            .conflicts
            .iter()
            .map(|row| String::from_utf8_lossy(&row.path).into_owned())
            .collect();
        items.push(item);
    }
    if let Some((since, problem)) = reviews::checks::held_by_main(&mut *conn).await? {
        let mut item = NeedsYouItem::new(NeedsYouKind::ChecksFileInvalidOnMain, since);
        item.about = Some(since.as_millis().to_string());
        item.title = Some(held_title(&problem));
        item.paths = vec![CHECKS_FILE.to_owned()];
        items.push(item);
    }
    let human_tasks = tasks::ready_human_tasks(&mut *conn).await?;
    let held = tasks::held_tasks(&mut *conn).await?;
    drop(conn);
    for (kind, waiting) in human_tasks
        .into_iter()
        .map(|task| (NeedsYouKind::HumanTask, task))
        .chain(held.into_iter().map(|task| (NeedsYouKind::Blocker, task)))
    {
        let mut item = NeedsYouItem::new(kind, waiting.since);
        item.task = Some(waiting.id);
        item.title = Some(waiting.title.as_str().to_owned());
        item.reason = waiting.reason.map(|reason| reason.as_str().to_owned());
        items.push(item);
    }
    for request in drafts {
        let Requester::Human(human) = request.requester else {
            continue;
        };
        let mut item = NeedsYouItem::new(NeedsYouKind::RequestDraft, request.updated_at);
        item.request = Some(request.id);
        item.human = Some(human);
        item.title = request.title;
        items.push(item);
    }
    let waiting_reviews = awaiting_human_review(store).await?;
    let mut conn = store.read().acquire().await?;
    for waiting in waiting_reviews {
        let mut item = NeedsYouItem::new(NeedsYouKind::HumanReview, waiting.since);
        item.task = Some(waiting.task);
        item.proposal = Some(waiting.proposal);
        item.human = waiting.reviewer;
        item.title = reviews::get_proposal(&mut *conn, waiting.proposal)
            .await?
            .map(|proposal| proposal.title);
        items.push(item);
    }
    items.sort_by_key(|item| item.since);
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis).unwrap()
    }

    fn kinds(items: &[NeedsYouItem]) -> Vec<NeedsYouKind> {
        items.iter().map(|item| item.kind).collect()
    }

    #[test]
    fn a_waiting_conflict_stands_for_the_held_folder_until_it_is_resolved() {
        let held = || Some((at(5), vec!["README.md".to_owned()]));
        let moved = Some((at(3), "run `git switch main`".to_owned()));
        let mut conflict = NeedsYouItem::new(NeedsYouKind::HumanChange, at(9));
        conflict.paths = vec!["README.md".to_owned()];
        assert_eq!(
            kinds(&with_folder_attention(vec![conflict], moved, held())),
            [NeedsYouKind::HeadMoved, NeedsYouKind::HumanChange]
        );
        let items = with_folder_attention(Vec::new(), None, held());
        assert_eq!(kinds(&items), [NeedsYouKind::FolderHeld]);
        assert_eq!(items[0].paths, ["README.md"]);
        assert!(with_folder_attention(Vec::new(), None, None).is_empty());
        let unstash = NeedsYouItem::new(NeedsYouKind::UnstashConflicted, at(7));
        assert_eq!(
            kinds(&with_folder_attention(vec![unstash], None, held())),
            [NeedsYouKind::UnstashConflicted]
        );
        assert_eq!(NeedsYouKind::UnstashConflicted.screen(), Screen::Repository);
    }

    #[test]
    fn the_held_title_puts_the_line_after_the_file() {
        assert_eq!(
            held_title("fold-checks.toml: line 12: unknown field `tiemout`"),
            "fold-checks.toml line 12: unknown field `tiemout`"
        );
        assert_eq!(held_title("something else"), "something else");
    }
}
