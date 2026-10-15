use std::collections::BTreeMap;
use std::fmt;

use serde_json::json;

use crate::actor::Actor;
use crate::agents::{AgentName, HumanName};
use crate::claims::is_stale;
use crate::dag::{Dag, EdgeRefusal};
use crate::error::{FoldError, codes};
use crate::ids::{RequestId, TaskId};
use crate::time::Timestamp;

/// The most cards one draft holds.
pub const MAX_CARDS: usize = 20;
/// The longest request or card title, in characters.
pub const MAX_TITLE_CHARS: usize = 200;
/// The longest request body, card description, note, comment or close reason, in characters.
pub const MAX_TEXT_CHARS: usize = 20_000;
/// The longest draft-local card key.
pub const MAX_KEY_LEN: usize = 32;

/// What a request asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequestKind {
    Bug,
    Feature,
    Question,
}

impl RequestKind {
    pub const ALL: &'static [RequestKind] = &[
        RequestKind::Bug,
        RequestKind::Feature,
        RequestKind::Question,
    ];

    /// The text stored in `requests.kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            RequestKind::Bug => "bug",
            RequestKind::Feature => "feature",
            RequestKind::Question => "question",
        }
    }

    pub fn parse(text: &str) -> Option<RequestKind> {
        Self::ALL.iter().copied().find(|kind| kind.as_str() == text)
    }
}

impl fmt::Display for RequestKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a request stands. `Drafting` is a claimed request without a draft; `Open` has neither a drafter nor a draft. `Approved` and `Closed` are final.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequestState {
    Open,
    Drafting,
    DraftReady,
    ChangesRequested,
    Approved,
    Closed,
}

impl RequestState {
    pub const ALL: &'static [RequestState] = &[
        RequestState::Open,
        RequestState::Drafting,
        RequestState::DraftReady,
        RequestState::ChangesRequested,
        RequestState::Approved,
        RequestState::Closed,
    ];

    /// The text stored in `requests.state`.
    pub fn as_str(self) -> &'static str {
        match self {
            RequestState::Open => "open",
            RequestState::Drafting => "drafting",
            RequestState::DraftReady => "draft_ready",
            RequestState::ChangesRequested => "changes_requested",
            RequestState::Approved => "approved",
            RequestState::Closed => "closed",
        }
    }

    pub fn parse(text: &str) -> Option<RequestState> {
        Self::ALL
            .iter()
            .copied()
            .find(|state| state.as_str() == text)
    }

    pub fn is_final(self) -> bool {
        matches!(self, RequestState::Approved | RequestState::Closed)
    }

    /// `request_not_open` for an approved or closed request, which only reads.
    pub fn require_open(self, request: RequestId) -> Result<(), FoldError> {
        if !self.is_final() {
            return Ok(());
        }
        Err(FoldError::new(
            codes::REQUEST_NOT_OPEN,
            format!("{request} is {}", self.as_str()),
        )
        .with_details([("state", json!(self.as_str()))])
        .with_hint("an approved or closed request only reads; call create_request for new work"))
    }

    /// The state after `step`. A final state refuses every step with `request_not_open`; a step that does not fit an open state is `invalid_request` with `details.reason = "state"`.
    ///
    /// A claim moves only `open` to `drafting`: a request that already has a draft keeps its state, so a new drafter of a request with changes requested still owes the requester a revision. A release returns `drafting` to `open` and leaves a request with a draft where it is. Asking for changes and editing need a draft; approving needs a draft that waits for the requester.
    pub fn after(self, step: RequestStep, request: RequestId) -> Result<RequestState, FoldError> {
        use RequestState::{Approved, ChangesRequested, Closed, DraftReady, Drafting, Open};
        self.require_open(request)?;
        let next = match (step, self) {
            (RequestStep::Claim, Open) => Some(Drafting),
            (RequestStep::Claim, state) => Some(state),
            (RequestStep::Release, Drafting) => Some(Open),
            (RequestStep::Release, state @ (DraftReady | ChangesRequested)) => Some(state),
            (RequestStep::Submit, Drafting | DraftReady | ChangesRequested) => Some(DraftReady),
            (RequestStep::RequestChanges, DraftReady | ChangesRequested) => Some(ChangesRequested),
            (RequestStep::Edit, DraftReady | ChangesRequested) => Some(DraftReady),
            (RequestStep::Approve, DraftReady) => Some(Approved),
            (RequestStep::Close, _) => Some(Closed),
            _ => None,
        };
        next.ok_or_else(|| {
            FoldError::new(
                codes::INVALID_REQUEST,
                format!(
                    "{request} is {} and cannot be {}",
                    self.as_str(),
                    step.done()
                ),
            )
            .with_details([
                ("reason", json!("state")),
                ("state", json!(self.as_str())),
                ("step", json!(step.as_str())),
            ])
            .with_hint("call get_request to see its state")
        })
    }
}

impl fmt::Display for RequestState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The commands that move a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequestStep {
    Claim,
    Release,
    Submit,
    RequestChanges,
    Edit,
    Approve,
    Close,
}

impl RequestStep {
    pub const ALL: &'static [RequestStep] = &[
        RequestStep::Claim,
        RequestStep::Release,
        RequestStep::Submit,
        RequestStep::RequestChanges,
        RequestStep::Edit,
        RequestStep::Approve,
        RequestStep::Close,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            RequestStep::Claim => "claim",
            RequestStep::Release => "release",
            RequestStep::Submit => "submit",
            RequestStep::RequestChanges => "request_changes",
            RequestStep::Edit => "edit",
            RequestStep::Approve => "approve",
            RequestStep::Close => "close",
        }
    }

    fn done(self) -> &'static str {
        match self {
            RequestStep::Claim => "claimed",
            RequestStep::Release => "released",
            RequestStep::Submit => "given a draft",
            RequestStep::RequestChanges => "sent back for changes",
            RequestStep::Edit => "edited",
            RequestStep::Approve => "approved",
            RequestStep::Close => "closed",
        }
    }
}

/// Who created a request. Every worker of a requesting agent acts as the requester, so the agent, not the worker, is recorded.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Requester {
    Agent(AgentName),
    Human(HumanName),
}

impl Requester {
    /// The requester `actor` would be; the system creates no requests.
    pub fn of(actor: &Actor) -> Option<Requester> {
        match actor {
            Actor::Agent(agent) => Some(Requester::Agent(agent.agent.clone())),
            Actor::Human { name } => Some(Requester::Human(name.clone())),
            Actor::System => None,
        }
    }

    /// The value of `requests.requester`: the agent name, or `human:<name>`. An agent name never holds a colon, so the two never collide.
    pub fn label(&self) -> String {
        match self {
            Requester::Agent(agent) => agent.as_str().to_owned(),
            Requester::Human(name) => format!("human:{name}"),
        }
    }

    pub fn parse(label: &str) -> Option<Requester> {
        match label.strip_prefix("human:") {
            Some(name) => HumanName::parse(name).ok().map(Requester::Human),
            None => AgentName::parse(label).ok().map(Requester::Agent),
        }
    }

    /// True when `actor` acts as this requester: the same named human, or any worker of the same agent.
    pub fn matches(&self, actor: &Actor) -> bool {
        match (self, actor) {
            (Requester::Agent(name), Actor::Agent(agent)) => agent.agent == *name,
            (Requester::Human(name), Actor::Human { name: actor }) => actor == name,
            _ => false,
        }
    }
}

impl fmt::Display for Requester {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

/// The most attachments one request holds.
pub const MAX_ATTACHMENTS: usize = 10;
/// The largest attachment, in bytes.
pub const MAX_ATTACHMENT_BYTES: u64 = 10 * 1024 * 1024;
/// The longest attachment name, in bytes.
pub const MAX_ATTACHMENT_NAME_BYTES: usize = 255;
/// The longest attachment media type, in bytes.
pub const MAX_MEDIA_TYPE_BYTES: usize = 255;

fn invalid_item(reason: &'static str, position: usize, hint: &'static str) -> FoldError {
    FoldError::new(
        codes::INVALID_REQUEST,
        format!("the request is invalid: {}", reason.replace('_', " ")),
    )
    .with_details([("reason", json!(reason)), ("position", json!(position))])
    .with_hint(hint)
}

/// What is checked of an attachment before its bytes are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttachmentFacts<'a> {
    pub name: &'a str,
    pub media_type: &'a str,
    pub size: u64,
}

/// A plain file name: 1 to [`MAX_ATTACHMENT_NAME_BYTES`] bytes with no `/`, no `..` and no control character.
pub fn valid_attachment_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_ATTACHMENT_NAME_BYTES
        && !name.contains('/')
        && !name.contains("..")
        && !name.chars().any(char::is_control)
}

/// A media type such as `image/png`: printable ASCII without spaces, one `/` between two non-empty parts, at most [`MAX_MEDIA_TYPE_BYTES`] bytes.
pub fn valid_media_type(text: &str) -> bool {
    text.len() <= MAX_MEDIA_TYPE_BYTES
        && text.bytes().all(|b| b.is_ascii_graphic())
        && text
            .split_once('/')
            .is_some_and(|(kind, sub)| !kind.is_empty() && !sub.is_empty() && !sub.contains('/'))
}

/// Checks up to [`MAX_ATTACHMENTS`] attachments of at most [`MAX_ATTACHMENT_BYTES`] each. A refusal is `invalid_request` with `details.reason` (`too_many_attachments`, `attachment_too_large`, `invalid_attachment_name`, `invalid_media_type`) and the 1-based `position`, never the name.
pub fn check_attachments(attachments: &[AttachmentFacts<'_>]) -> Result<(), FoldError> {
    check_attachment_count(attachments.len())?;
    for (index, attachment) in attachments.iter().enumerate() {
        let position = index + 1;
        if attachment.size > MAX_ATTACHMENT_BYTES {
            return Err(attachment_too_large(position));
        }
        if !valid_attachment_name(attachment.name) {
            return Err(invalid_item(
                "invalid_attachment_name",
                position,
                ATTACHMENT_HINT,
            ));
        }
        if !valid_media_type(attachment.media_type) {
            return Err(invalid_item(
                "invalid_media_type",
                position,
                ATTACHMENT_HINT,
            ));
        }
    }
    Ok(())
}

const ATTACHMENT_HINT: &str =
    "attach at most 10 files of at most 10 MiB, each with a plain file name and a media type";

/// Refuses more than [`MAX_ATTACHMENTS`] attachments, so a transport can stop before it reads their bytes.
pub fn check_attachment_count(count: usize) -> Result<(), FoldError> {
    if count > MAX_ATTACHMENTS {
        return Err(invalid_item(
            "too_many_attachments",
            MAX_ATTACHMENTS + 1,
            ATTACHMENT_HINT,
        ));
    }
    Ok(())
}

/// The refusal of the attachment at the 1-based `position` for holding more than [`MAX_ATTACHMENT_BYTES`], also given by a transport that stops reading it at the limit.
pub fn attachment_too_large(position: usize) -> FoldError {
    invalid_item("attachment_too_large", position, ATTACHMENT_HINT)
}

/// A drafter whose agent has no open session may lose the request once `lease_minutes` have passed since the agent's last session ended or since the drafter took the request, whichever is later. An agent with an open session keeps it however long it is quiet.
pub fn drafter_lapsed(
    open_session: bool,
    last_session_end: Option<Timestamp>,
    drafter_since: Timestamp,
    now: Timestamp,
    lease_minutes: u64,
) -> bool {
    let seen = last_session_end.map_or(drafter_since, |end| end.max(drafter_since));
    !open_session && is_stale(seen, now, lease_minutes)
}

fn invalid_text(reason: &'static str, field: &'static str) -> FoldError {
    FoldError::new(
        codes::INVALID_REQUEST,
        format!("the {field} is {}", reason.replace('_', " ")),
    )
    .with_details([("reason", json!(reason)), ("field", json!(field))])
    .with_hint("send a non-empty value within the length limit")
}

/// A request title: one trimmed line of at most [`MAX_TITLE_CHARS`] characters. Refused with `invalid_request` and `details.reason`.
pub fn request_title(text: &str) -> Result<String, FoldError> {
    let title = text.trim();
    if title.is_empty() {
        return Err(invalid_text("empty", "title"));
    }
    if title.contains(['\n', '\r']) {
        return Err(invalid_text("not_one_line", "title"));
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(invalid_text("too_long", "title"));
    }
    Ok(title.to_owned())
}

/// A body, comment, note or close reason: kept as given, at most [`MAX_TEXT_CHARS`] characters, and not blank unless `optional`. A blank optional text is `None`.
pub fn request_text(
    text: &str,
    field: &'static str,
    optional: bool,
) -> Result<Option<String>, FoldError> {
    if text.trim().is_empty() {
        return if optional {
            Ok(None)
        } else {
            Err(invalid_text("empty", field))
        };
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(invalid_text("too_long", field));
    }
    Ok(Some(text.to_owned()))
}

/// The longest excerpt, in characters, before the ellipsis.
pub const EXCERPT_CHARS: usize = 80;

/// What a list shows of a request nobody has named yet: the first line of the body that holds a visible character, trimmed and cut to [`EXCERPT_CHARS`] characters with an ellipsis.
pub fn excerpt(body: &str) -> String {
    let line = body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    if line.chars().count() <= EXCERPT_CHARS {
        return line.to_owned();
    }
    let cut: String = line.chars().take(EXCERPT_CHARS).collect();
    format!("{}…", cut.trim_end())
}

/// One card of a draft. `key` is local to the draft and names the card in its edges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftCard {
    pub key: String,
    pub title: String,
    pub description: String,
}

/// The dependent card needs the prerequisite card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftEdge {
    pub prerequisite: String,
    pub dependent: String,
}

/// A draft that `validate_draft` accepted: titles are trimmed, and the edges form no cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    cards: Vec<DraftCard>,
    edges: Vec<DraftEdge>,
}

impl Draft {
    /// The cards in the order given, which is the order an approval creates their tasks in.
    pub fn cards(&self) -> &[DraftCard] {
        &self.cards
    }

    pub fn edges(&self) -> &[DraftEdge] {
        &self.edges
    }
}

/// Why a draft is refused. Each names cards by key or, for a key that is itself invalid, by position, and never repeats a title or a description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftRefusal {
    Empty,
    TooManyCards(usize),
    /// The 1-based position of the card whose key is not 1 to 32 characters of letters, digits, `-` and `_`.
    InvalidKey(usize),
    DuplicateKey(String),
    EmptyTitle(String),
    TitleNotOneLine(String),
    TitleTooLong(String),
    EmptyDescription(String),
    DescriptionTooLong(String),
    /// The 1-based position of the edge that names a key no card has.
    UnknownKey(usize),
    SelfEdge(String),
    DuplicateEdge {
        prerequisite: String,
        dependent: String,
    },
    /// The keys of the cycle the edge `dependent` needs `prerequisite` closes, from the dependent back to itself.
    Cycle(Vec<String>),
}

impl DraftRefusal {
    /// The stable `details.reason` of an `invalid_draft` refusal; a cycle is `dependency_cycle` with the reason `cycle`.
    pub fn reason(&self) -> &'static str {
        match self {
            DraftRefusal::Empty => "empty",
            DraftRefusal::TooManyCards(_) => "too_many_cards",
            DraftRefusal::InvalidKey(_) => "invalid_key",
            DraftRefusal::DuplicateKey(_) => "duplicate_key",
            DraftRefusal::EmptyTitle(_) => "empty_title",
            DraftRefusal::TitleNotOneLine(_) => "title_not_one_line",
            DraftRefusal::TitleTooLong(_) => "title_too_long",
            DraftRefusal::EmptyDescription(_) => "empty_description",
            DraftRefusal::DescriptionTooLong(_) => "description_too_long",
            DraftRefusal::UnknownKey(_) => "unknown_key",
            DraftRefusal::SelfEdge(_) => "self_edge",
            DraftRefusal::DuplicateEdge { .. } => "duplicate_edge",
            DraftRefusal::Cycle(_) => "cycle",
        }
    }
}

impl From<DraftRefusal> for FoldError {
    fn from(refusal: DraftRefusal) -> Self {
        let reason = refusal.reason();
        let hint = "fix the draft and send it again";
        let detail = match &refusal {
            DraftRefusal::Cycle(path) => {
                return FoldError::new(codes::DEPENDENCY_CYCLE, "the draft's edges form a cycle")
                    .with_details([("reason", json!(reason)), ("path", json!(path))])
                    .with_hint("remove an edge on the path, then send the draft again");
            }
            DraftRefusal::Empty => None,
            DraftRefusal::TooManyCards(count) => Some(("cards", json!(count))),
            DraftRefusal::InvalidKey(position) | DraftRefusal::UnknownKey(position) => {
                Some(("position", json!(position)))
            }
            DraftRefusal::DuplicateKey(key)
            | DraftRefusal::EmptyTitle(key)
            | DraftRefusal::TitleNotOneLine(key)
            | DraftRefusal::TitleTooLong(key)
            | DraftRefusal::EmptyDescription(key)
            | DraftRefusal::DescriptionTooLong(key)
            | DraftRefusal::SelfEdge(key) => Some(("key", json!(key))),
            DraftRefusal::DuplicateEdge {
                prerequisite,
                dependent,
            } => Some((
                "edge",
                json!({"prerequisite": prerequisite, "dependent": dependent}),
            )),
        };
        let details = std::iter::once(("reason", json!(reason))).chain(detail);
        FoldError::new(
            codes::INVALID_DRAFT,
            format!("the draft is invalid: {}", reason.replace('_', " ")),
        )
        .with_details(details)
        .with_hint(hint)
    }
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_KEY_LEN
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// Checks a draft: 1 to [`MAX_CARDS`] cards with unique keys, each with a one-line title of at most [`MAX_TITLE_CHARS`] characters and a non-blank description of at most [`MAX_TEXT_CHARS`]; every edge names two different existing keys, once, and the edges form no cycle. The title rule is the task title rule, so an approval never meets a card its task would refuse.
pub fn validate_draft(cards: Vec<DraftCard>, edges: Vec<DraftEdge>) -> Result<Draft, DraftRefusal> {
    if cards.is_empty() {
        return Err(DraftRefusal::Empty);
    }
    if cards.len() > MAX_CARDS {
        return Err(DraftRefusal::TooManyCards(cards.len()));
    }
    // Each key stands for a synthetic task id, its 1-based position, so the task graph's cycle check applies unchanged.
    let mut nodes: BTreeMap<&str, TaskId> = BTreeMap::new();
    let mut checked = Vec::with_capacity(cards.len());
    for (index, card) in cards.iter().enumerate() {
        if !valid_key(&card.key) {
            return Err(DraftRefusal::InvalidKey(index + 1));
        }
        let node = u32::try_from(index + 1)
            .ok()
            .and_then(TaskId::from_number)
            .ok_or(DraftRefusal::TooManyCards(cards.len()))?;
        if nodes.insert(card.key.as_str(), node).is_some() {
            return Err(DraftRefusal::DuplicateKey(card.key.clone()));
        }
        let key = || card.key.clone();
        let title = card.title.trim();
        if title.is_empty() {
            return Err(DraftRefusal::EmptyTitle(key()));
        }
        if title.contains(['\n', '\r']) {
            return Err(DraftRefusal::TitleNotOneLine(key()));
        }
        if title.chars().count() > MAX_TITLE_CHARS {
            return Err(DraftRefusal::TitleTooLong(key()));
        }
        if card.description.trim().is_empty() {
            return Err(DraftRefusal::EmptyDescription(key()));
        }
        if card.description.chars().count() > MAX_TEXT_CHARS {
            return Err(DraftRefusal::DescriptionTooLong(key()));
        }
        checked.push(DraftCard {
            key: card.key.clone(),
            title: title.to_owned(),
            description: card.description.clone(),
        });
    }
    let mut graph: Vec<(TaskId, TaskId)> = Vec::with_capacity(edges.len());
    for (index, edge) in edges.iter().enumerate() {
        let (Some(&dependent), Some(&prerequisite)) = (
            nodes.get(edge.dependent.as_str()),
            nodes.get(edge.prerequisite.as_str()),
        ) else {
            return Err(DraftRefusal::UnknownKey(index + 1));
        };
        if graph.contains(&(dependent, prerequisite)) {
            return Err(DraftRefusal::DuplicateEdge {
                prerequisite: edge.prerequisite.clone(),
                dependent: edge.dependent.clone(),
            });
        }
        match Dag::from_edges(graph.iter().copied()).check_edge(dependent, prerequisite) {
            Ok(()) => graph.push((dependent, prerequisite)),
            Err(EdgeRefusal::SelfDependency) => {
                return Err(DraftRefusal::SelfEdge(edge.dependent.clone()));
            }
            Err(EdgeRefusal::Cycle(path)) => {
                let key_of = |node: TaskId| {
                    nodes
                        .iter()
                        .find(|(_, id)| **id == node)
                        .map(|(key, _)| (*key).to_owned())
                        .unwrap_or_default()
                };
                return Err(DraftRefusal::Cycle(path.into_iter().map(key_of).collect()));
            }
        }
    }
    Ok(Draft {
        cards: checked,
        edges,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::AgentActor;
    use crate::agents::WorkerName;

    fn rq() -> RequestId {
        RequestId::from_number(4).unwrap()
    }

    fn card(key: &str) -> DraftCard {
        DraftCard {
            key: key.to_owned(),
            title: format!("Card {key}"),
            description: "**Scope** one thing".to_owned(),
        }
    }

    fn edge(prerequisite: &str, dependent: &str) -> DraftEdge {
        DraftEdge {
            prerequisite: prerequisite.to_owned(),
            dependent: dependent.to_owned(),
        }
    }

    fn refusal(cards: Vec<DraftCard>, edges: Vec<DraftEdge>) -> DraftRefusal {
        validate_draft(cards, edges).unwrap_err()
    }

    fn agent(worker: &str) -> Actor {
        let worker = WorkerName::parse(worker).unwrap();
        Actor::Agent(AgentActor {
            agent: worker.agent(),
            worker,
            tags: Vec::new(),
        })
    }

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis).unwrap()
    }

    #[test]
    fn kinds_and_states_round_trip_through_their_text() {
        for kind in RequestKind::ALL {
            assert_eq!(RequestKind::parse(kind.as_str()), Some(*kind));
            assert_eq!(kind.to_string(), kind.as_str());
        }
        for state in RequestState::ALL {
            assert_eq!(RequestState::parse(state.as_str()), Some(*state));
            assert_eq!(state.to_string(), state.as_str());
        }
        assert_eq!(RequestKind::parse("Bug"), None);
        assert_eq!(RequestState::parse("ready"), None);
    }

    #[test]
    fn every_step_follows_the_transition_table() {
        use RequestState::{Approved, ChangesRequested, Closed, DraftReady, Drafting, Open};
        use RequestStep::{Approve, Claim, Close, Edit, Release, RequestChanges, Submit};
        let table: [(RequestStep, [Option<RequestState>; 4]); 7] = [
            // From: Open, Drafting, DraftReady, ChangesRequested.
            (
                Claim,
                [
                    Some(Drafting),
                    Some(Drafting),
                    Some(DraftReady),
                    Some(ChangesRequested),
                ],
            ),
            (
                Release,
                [None, Some(Open), Some(DraftReady), Some(ChangesRequested)],
            ),
            (
                Submit,
                [None, Some(DraftReady), Some(DraftReady), Some(DraftReady)],
            ),
            (
                RequestChanges,
                [None, None, Some(ChangesRequested), Some(ChangesRequested)],
            ),
            (Edit, [None, None, Some(DraftReady), Some(DraftReady)]),
            (Approve, [None, None, Some(Approved), None]),
            (Close, [Some(Closed); 4]),
        ];
        for (step, expected) in table {
            for (from, expected) in [Open, Drafting, DraftReady, ChangesRequested]
                .into_iter()
                .zip(expected)
            {
                let result = from.after(step, rq());
                match expected {
                    Some(to) => assert_eq!(result, Ok(to), "{from} {}", step.as_str()),
                    None => {
                        let error = result.unwrap_err();
                        assert_eq!(
                            error.code(),
                            codes::INVALID_REQUEST,
                            "{from} {}",
                            step.as_str()
                        );
                        assert_eq!(
                            error.details().and_then(|d| d.get("reason")),
                            Some(&json!("state"))
                        );
                        assert_eq!(
                            error.details().and_then(|d| d.get("step")),
                            Some(&json!(step.as_str()))
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_final_request_refuses_every_step() {
        for state in [RequestState::Approved, RequestState::Closed] {
            assert!(state.is_final());
            for step in RequestStep::ALL {
                let error = state.after(*step, rq()).unwrap_err();
                assert_eq!(error.code(), codes::REQUEST_NOT_OPEN);
                assert_eq!(error.message(), format!("RQ-4 is {state}"));
            }
        }
        assert!(!RequestState::DraftReady.is_final());
    }

    #[test]
    fn a_requester_is_the_agent_or_the_named_human() {
        let tanim = Actor::human(HumanName::parse("tanim").unwrap());
        let backend = Requester::of(&agent("backend/api")).unwrap();
        assert_eq!(backend.label(), "backend");
        assert_eq!(Requester::of(&tanim).unwrap().to_string(), "human:tanim");
        assert_eq!(Requester::of(&Actor::System), None);
        for label in ["backend", "human:tanim"] {
            assert_eq!(Requester::parse(label).unwrap().label(), label);
        }
        assert_eq!(Requester::parse("human:"), None);
        assert_eq!(Requester::parse("Back end"), None);

        assert!(backend.matches(&agent("backend")));
        assert!(backend.matches(&agent("backend/api/tests")));
        assert!(!backend.matches(&agent("frontend")));
        assert!(!backend.matches(&tanim));
        let human = Requester::parse("human:tanim").unwrap();
        assert!(human.matches(&tanim));
        assert!(!human.matches(&Actor::human(HumanName::parse("ana").unwrap())));
        assert!(!human.matches(&agent("tanim")));
        assert!(!human.matches(&Actor::System));
    }

    #[test]
    fn a_drafter_lapses_only_without_an_open_session_after_the_lease() {
        const MINUTE: i64 = 60_000;
        let since = at(10 * MINUTE);
        assert!(!drafter_lapsed(false, None, since, at(39 * MINUTE), 30));
        assert!(drafter_lapsed(false, None, since, at(40 * MINUTE), 30));
        assert!(!drafter_lapsed(true, None, since, at(400 * MINUTE), 30));
        let ended = Some(at(20 * MINUTE));
        assert!(!drafter_lapsed(false, ended, since, at(49 * MINUTE), 30));
        assert!(drafter_lapsed(false, ended, since, at(50 * MINUTE), 30));
        let older_end = Some(at(MINUTE));
        assert!(drafter_lapsed(false, older_end, since, at(40 * MINUTE), 30));
    }

    #[test]
    fn titles_and_texts_are_checked() {
        assert_eq!(request_title("  Crash on save  ").unwrap(), "Crash on save");
        for (title, reason) in [
            ("   ", "empty"),
            ("two\nlines", "not_one_line"),
            (&"x".repeat(201), "too_long"),
        ] {
            let error = request_title(title).unwrap_err();
            assert_eq!(error.code(), codes::INVALID_REQUEST);
            assert_eq!(
                error.details().and_then(|d| d.get("reason")),
                Some(&json!(reason))
            );
        }
        assert!(request_title(&"é".repeat(200)).is_ok());
        assert_eq!(request_text("", "note", true).unwrap(), None);
        assert_eq!(
            request_text(" body ", "body", false).unwrap().as_deref(),
            Some(" body ")
        );
        let empty = request_text(" ", "comment", false).unwrap_err();
        assert_eq!(empty.message(), "the comment is empty");
        let long = request_text(&"x".repeat(20_001), "body", true).unwrap_err();
        assert_eq!(
            long.details().and_then(|d| d.get("field")),
            Some(&json!("body"))
        );
    }

    #[test]
    fn attachments_are_checked_before_their_bytes_are_stored() {
        let ok = AttachmentFacts {
            name: "screenshot 1.png",
            media_type: "image/png",
            size: MAX_ATTACHMENT_BYTES,
        };
        assert!(check_attachments(&[ok; 10]).is_ok());
        assert!(check_attachments(&[]).is_ok());
        let reason = |attachments: &[AttachmentFacts<'_>]| {
            let error = check_attachments(attachments).unwrap_err();
            assert_eq!(error.code(), codes::INVALID_REQUEST);
            let details = error.details().unwrap();
            (details["reason"].clone(), details["position"].clone())
        };
        assert_eq!(
            reason(&[ok; 11]),
            (json!("too_many_attachments"), json!(11))
        );
        let big = AttachmentFacts {
            size: MAX_ATTACHMENT_BYTES + 1,
            ..ok
        };
        assert_eq!(
            reason(&[ok, big]),
            (json!("attachment_too_large"), json!(2))
        );
        for name in ["", "a/b", "..", "a..b", "tab\there", &"n".repeat(256)] {
            let bad = AttachmentFacts { name, ..ok };
            assert_eq!(
                reason(&[bad]),
                (json!("invalid_attachment_name"), json!(1)),
                "{name:?}"
            );
        }
        assert!(valid_attachment_name(&"n".repeat(255)));
        for media_type in [
            "",
            "image",
            "image/",
            "/png",
            "text/plain; charset=utf-8",
            "a/b/c",
        ] {
            let bad = AttachmentFacts { media_type, ..ok };
            assert_eq!(
                reason(&[bad]),
                (json!("invalid_media_type"), json!(1)),
                "{media_type:?}"
            );
        }
        assert!(valid_media_type("application/vnd.fold+json"));
    }

    #[test]
    fn a_valid_draft_keeps_its_order_and_trims_titles() {
        let mut first = card("c1");
        first.title = "  Add the route  ".to_owned();
        let draft = validate_draft(
            vec![first, card("c2"), card("c3")],
            vec![edge("c1", "c2"), edge("c2", "c3"), edge("c1", "c3")],
        )
        .unwrap();
        let keys: Vec<&str> = draft.cards().iter().map(|card| card.key.as_str()).collect();
        assert_eq!(keys, ["c1", "c2", "c3"]);
        assert_eq!(draft.cards()[0].title, "Add the route");
        assert_eq!(draft.edges().len(), 3);
        let most: Vec<DraftCard> = (1..=20).map(|n| card(&format!("c{n}"))).collect();
        assert!(validate_draft(most, Vec::new()).is_ok());
    }

    #[test]
    fn every_draft_refusal_names_its_reason() {
        let long_title = DraftCard {
            title: "x".repeat(201),
            ..card("c1")
        };
        let empty_description = DraftCard {
            description: " \n".to_owned(),
            ..card("c1")
        };
        let cases = [
            (refusal(Vec::new(), Vec::new()), "empty"),
            (
                refusal(
                    (0..21).map(|n| card(&format!("c{n}"))).collect(),
                    Vec::new(),
                ),
                "too_many_cards",
            ),
            (
                refusal(vec![card("c1"), card("c1")], Vec::new()),
                "duplicate_key",
            ),
            (refusal(vec![card("c 1")], Vec::new()), "invalid_key"),
            (
                refusal(vec![card(&"k".repeat(33))], Vec::new()),
                "invalid_key",
            ),
            (
                refusal(vec![card("c1")], vec![edge("c1", "c9")]),
                "unknown_key",
            ),
            (
                refusal(vec![card("c1")], vec![edge("c1", "c1")]),
                "self_edge",
            ),
            (
                refusal(
                    vec![card("c1"), card("c2")],
                    vec![edge("c1", "c2"), edge("c2", "c1")],
                ),
                "cycle",
            ),
            (
                refusal(
                    vec![card("c1"), card("c2")],
                    vec![edge("c1", "c2"), edge("c1", "c2")],
                ),
                "duplicate_edge",
            ),
            (refusal(vec![long_title], Vec::new()), "title_too_long"),
            (
                refusal(
                    vec![DraftCard {
                        title: " ".to_owned(),
                        ..card("c1")
                    }],
                    Vec::new(),
                ),
                "empty_title",
            ),
            (
                refusal(
                    vec![DraftCard {
                        title: "a\rb".to_owned(),
                        ..card("c1")
                    }],
                    Vec::new(),
                ),
                "title_not_one_line",
            ),
            (
                refusal(vec![empty_description], Vec::new()),
                "empty_description",
            ),
            (
                refusal(
                    vec![DraftCard {
                        description: "d".repeat(20_001),
                        ..card("c1")
                    }],
                    Vec::new(),
                ),
                "description_too_long",
            ),
        ];
        for (refusal, reason) in cases {
            assert_eq!(refusal.reason(), reason);
            let error = FoldError::from(refusal);
            let expected = if reason == "cycle" {
                codes::DEPENDENCY_CYCLE
            } else {
                codes::INVALID_DRAFT
            };
            assert_eq!(error.code(), expected, "{reason}");
            assert_eq!(
                error.details().and_then(|d| d.get("reason")),
                Some(&json!(reason))
            );
            assert!(!error.message().contains("Card"), "{reason}");
        }
    }

    #[test]
    fn refusals_name_keys_and_positions_only() {
        let details = |refusal: DraftRefusal| {
            serde_json::Value::Object(FoldError::from(refusal).details().cloned().unwrap())
        };
        let cycle = refusal(
            vec![card("a"), card("b"), card("c")],
            vec![edge("a", "b"), edge("b", "c"), edge("c", "a")],
        );
        assert_eq!(
            cycle,
            DraftRefusal::Cycle(vec!["a".into(), "c".into(), "b".into(), "a".into()])
        );
        assert_eq!(
            details(cycle),
            json!({"reason": "cycle", "path": ["a", "c", "b", "a"]})
        );
        assert_eq!(
            details(refusal(vec![card("c1"), card("bad key")], Vec::new())),
            json!({"reason": "invalid_key", "position": 2})
        );
        assert_eq!(
            details(refusal(vec![card("c1")], vec![edge("c1", "nope")])),
            json!({"reason": "unknown_key", "position": 1})
        );
        assert_eq!(
            details(refusal(
                (0..21).map(|n| card(&format!("c{n}"))).collect(),
                Vec::new()
            )),
            json!({"reason": "too_many_cards", "cards": 21})
        );
        assert_eq!(
            details(refusal(Vec::new(), Vec::new())),
            json!({"reason": "empty"})
        );
        assert_eq!(
            details(refusal(vec![card("c1"), card("c1")], Vec::new())),
            json!({"reason": "duplicate_key", "key": "c1"})
        );
        assert_eq!(
            details(refusal(
                vec![card("c1"), card("c2")],
                vec![edge("c1", "c2"), edge("c1", "c2")]
            )),
            json!({"reason": "duplicate_edge", "edge": {"prerequisite": "c1", "dependent": "c2"}})
        );
    }

    #[test]
    fn an_excerpt_is_the_first_visible_line_cut_by_characters() {
        assert_eq!(excerpt("\n  \n  Board is slow  \nmore"), "Board is slow");
        assert_eq!(excerpt(""), "");
        let long = "é".repeat(EXCERPT_CHARS + 5);
        let cut = excerpt(&long);
        assert_eq!(cut.chars().count(), EXCERPT_CHARS + 1);
        assert!(cut.ends_with('…'));
        assert_eq!(
            excerpt(&"a".repeat(EXCERPT_CHARS)),
            "a".repeat(EXCERPT_CHARS)
        );
    }
}
