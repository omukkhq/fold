use fold_core::requests::{RequestState, Requester};
use fold_core::{
    AgentName, Choice, HumanName, MessageId, PermissionQuestionId, ProposalId, RequestId, Sender,
    TaskId, Timestamp,
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
    /// A question with choices from an agent to the humans, or a worker's `AskUserQuestion` call, that no human has answered in its thread. A plain message to the humans is not an item.
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
}

impl NeedsYouKind {
    pub fn screen(self) -> Screen {
        match self {
            NeedsYouKind::Question => Screen::Messages,
            NeedsYouKind::Permission => Screen::Worker,
            NeedsYouKind::HumanReview => Screen::Proposal,
            NeedsYouKind::RequestDraft => Screen::Request,
            NeedsYouKind::HumanTask | NeedsYouKind::Blocker => Screen::Task,
            NeedsYouKind::HumanChange | NeedsYouKind::HeadMoved | NeedsYouKind::FolderHeld => {
                Screen::Repository
            }
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
    /// The agent that asks: the question's sender or the worker of a permission question.
    pub agent: Option<AgentName>,
    pub tool: Option<String>,
    pub task: Option<TaskId>,
    pub proposal: Option<ProposalId>,
    pub request: Option<RequestId>,
    /// The human a human review or a request draft waits for; `None` when any human may answer.
    pub human: Option<HumanName>,
    pub choices: Vec<Choice>,
    /// The short header of a `Question`.
    pub header: Option<String>,
    /// The proposal title of a `HumanReview`; the request title of a `RequestDraft`, which the claim gave it before any draft; the task title of a `HumanTask` or a `Blocker`.
    pub title: Option<String>,
    /// The reason of a `Blocker`; the command that brings the branch back for `HeadMoved`, or records the folder's changes for `FolderHeld`.
    pub reason: Option<String>,
    /// The human's Git commit of a `HumanChange`.
    pub commit: Option<String>,
    /// The paths of a `HumanChange` that conflict with main, or those of a `FolderHeld` that keep the folder where it is, in path order.
    pub paths: Vec<String>,
}

impl NeedsYouItem {
    /// The `HeadMoved` item of a branch that moved at `since`, with the command that brings it back.
    pub fn head_moved(since: Timestamp, hint: String) -> Self {
        let mut item = Self::new(NeedsYouKind::HeadMoved, since);
        item.reason = Some(hint);
        item
    }

    /// The `FolderHeld` item of a project folder that waits since `since` on the human's changes to `paths`.
    pub fn folder_held(since: Timestamp, paths: Vec<String>) -> Self {
        let mut item = Self::new(NeedsYouKind::FolderHeld, since);
        item.reason = Some("run fld record, or take the changes back".to_owned());
        item.paths = paths;
        item
    }

    fn new(kind: NeedsYouKind, since: Timestamp) -> Self {
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
            choices: Vec::new(),
            header: None,
            title: None,
            reason: None,
            commit: None,
            paths: Vec::new(),
        }
    }
}

/// `items` with the items of the project folder's Git attention, oldest first: the branch that moved, from `moved`, and the folder that waits on the human's changes, from `held`. While a conflicted human change waits, its item alone stands for the folder: it names the conflicting files, and its resolution lets the folder go on, so the waiting folder shows again only when it still waits after that.
pub fn with_folder_attention(
    mut items: Vec<NeedsYouItem>,
    moved: Option<(Timestamp, String)>,
    held: Option<(Timestamp, Vec<String>)>,
) -> Vec<NeedsYouItem> {
    if let Some((since, hint)) = moved {
        items.push(NeedsYouItem::head_moved(since, hint));
    }
    let conflicted = items
        .iter()
        .any(|item| item.kind == NeedsYouKind::HumanChange);
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
    let awaiting = messages::awaiting_human(&mut conn).await?;
    let ids: Vec<MessageId> = awaiting.iter().map(|row| row.id).collect();
    let asked = agents::asked_among(&mut *conn, &ids).await?;
    for row in awaiting {
        if row.choices.is_empty() && !asked.contains(&row.id) {
            continue;
        }
        let mut item = NeedsYouItem::new(NeedsYouKind::Question, row.created_at);
        item.message = Some((row.id, row.thread));
        item.agent = match &row.sender {
            Sender::Worker(worker) => Some(worker.agent()),
            Sender::Human(_) | Sender::Fold => None,
        };
        item.task = row.task;
        item.proposal = row.proposal;
        item.request = row.request;
        item.choices = row.choices;
        item.header = row.header;
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
        item.human = HumanName::parse(&change.human).ok();
        item.commit = change.commit;
        item.paths = change
            .conflicts
            .iter()
            .map(|row| String::from_utf8_lossy(&row.path).into_owned())
            .collect();
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
        let moved = Some((at(3), "run git switch main".to_owned()));
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
    }
}
