use fold_core::requests::{RequestState, Requester};
use fold_core::{
    AgentName, Choice, HumanName, MessageId, PermissionQuestionId, ProposalId, RequestId, Sender,
    TaskId, Timestamp,
};
use fold_store::agents::{self, QuestionKind};
use fold_store::reviews;
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
}

/// The screen on which a human answers an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Messages,
    Worker,
    Proposal,
    Request,
}

impl NeedsYouKind {
    pub fn screen(self) -> Screen {
        match self {
            NeedsYouKind::Question => Screen::Messages,
            NeedsYouKind::Permission => Screen::Worker,
            NeedsYouKind::HumanReview => Screen::Proposal,
            NeedsYouKind::RequestDraft => Screen::Request,
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
    /// The proposal title of a `HumanReview`; the request title of a `RequestDraft`, which the claim gave it before any draft.
    pub title: Option<String>,
}

impl NeedsYouItem {
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
        }
    }
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
    drop(conn);
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
    for waiting in awaiting_human_review(store).await? {
        let mut item = NeedsYouItem::new(NeedsYouKind::HumanReview, waiting.since);
        item.task = Some(waiting.task);
        item.proposal = Some(waiting.proposal);
        item.human = waiting.reviewer;
        let mut conn = store.read().acquire().await?;
        item.title = reviews::get_proposal(&mut *conn, waiting.proposal)
            .await?
            .map(|proposal| proposal.title);
        items.push(item);
    }
    items.sort_by_key(|item| item.since);
    Ok(items)
}
