use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use serde_json::json;
use thiserror::Error;

use crate::actor::Actor;
use crate::agents::{AgentName, HumanName, Tag, WorkerName};
use crate::error::{FoldError, codes};
use crate::events::EventActor;
use crate::text::{LineFault, one_line};

const TAG_PREFIX: &str = "tag:";
const HUMAN_PREFIX: &str = EventActor::HUMAN_PREFIX;
pub(crate) const ALL: &str = "all";
pub(crate) const HUMANS: &str = "humans";

/// One `to[]` entry. `all` and `humans` are reserved agent names, so the bare words always mean the broadcast forms.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Address {
    /// The agent's worker.
    Agent(AgentName),
    /// The main worker of every agent carrying the tag when the message is sent.
    Tag(Tag),
    /// The main worker of every agent.
    All,
    Human(HumanName),
    /// Every human in the host registry when the message is sent.
    Humans,
}

impl Address {
    pub fn parse(entry: &str) -> Result<Address, AddressError> {
        let invalid = || AddressError {
            entry: entry.to_owned(),
        };
        let address = if entry == ALL {
            Address::All
        } else if entry == HUMANS {
            Address::Humans
        } else if let Some(tag) = entry.strip_prefix(TAG_PREFIX) {
            Address::Tag(Tag::parse(tag).map_err(|_| invalid())?)
        } else if let Some(name) = entry.strip_prefix(HUMAN_PREFIX) {
            Address::Human(HumanName::parse(name).map_err(|_| invalid())?)
        } else {
            Address::Agent(AgentName::parse(entry).map_err(|_| invalid())?)
        };
        Ok(address)
    }

    /// Parses every entry in order and stops at the first that is not an address.
    pub fn parse_all<S: AsRef<str>>(entries: &[S]) -> Result<Vec<Address>, AddressError> {
        entries
            .iter()
            .map(|entry| Address::parse(entry.as_ref()))
            .collect()
    }

    pub fn addressed_as(&self) -> AddressedAs {
        match self {
            Address::Agent(_) => AddressedAs::Agent,
            Address::Tag(_) => AddressedAs::Tag,
            Address::All => AddressedAs::All,
            Address::Human(_) => AddressedAs::Human,
            Address::Humans => AddressedAs::Humans,
        }
    }

    /// The address that reaches `worker`'s agent.
    pub fn of_worker(worker: &WorkerName) -> Address {
        Address::Agent(worker.agent())
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Address::Agent(agent) => write!(f, "{agent}"),
            Address::Tag(tag) => write!(f, "{TAG_PREFIX}{tag}"),
            Address::All => f.write_str(ALL),
            Address::Human(name) => write!(f, "{HUMAN_PREFIX}{name}"),
            Address::Humans => f.write_str(HUMANS),
        }
    }
}

/// A `to[]` entry that is not an address. The entry is a name, never message content, so the error details may carry it.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("a recipient is an agent, tag:<tag>, all, human:<name> or humans")]
pub struct AddressError {
    entry: String,
}

impl AddressError {
    pub fn entry(&self) -> &str {
        &self.entry
    }
}

impl From<AddressError> for FoldError {
    fn from(error: AddressError) -> Self {
        FoldError::new(codes::INVALID_NAME, error.to_string())
            .with_details([("to", json!([error.entry]))])
            .with_hint("list the agents with list_agents")
    }
}

/// How a recipient row was reached, as stored in `message_recipients.addressed_as`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddressedAs {
    Agent,
    Worker,
    Tag,
    All,
    Human,
    Humans,
}

impl AddressedAs {
    pub fn as_str(self) -> &'static str {
        match self {
            AddressedAs::Agent => "agent",
            AddressedAs::Worker => "worker",
            AddressedAs::Tag => "tag",
            AddressedAs::All => "all",
            AddressedAs::Human => "human",
            AddressedAs::Humans => "humans",
        }
    }
}

impl FromStr for AddressedAs {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        [
            AddressedAs::Agent,
            AddressedAs::Worker,
            AddressedAs::Tag,
            AddressedAs::All,
            AddressedAs::Human,
            AddressedAs::Humans,
        ]
        .into_iter()
        .find(|value| value.as_str() == text)
        .ok_or(())
    }
}

/// Who receives a message: a worker or a named human.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Target {
    Worker(WorkerName),
    Human(HumanName),
}

impl fmt::Display for Target {
    /// The worker name, or `human:<name>`, the form the web and the event data use.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Worker(worker) => write!(f, "{worker}"),
            Target::Human(name) => write!(f, "{HUMAN_PREFIX}{name}"),
        }
    }
}

/// One recipient row of a message before it is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient {
    pub target: Target,
    pub addressed_as: AddressedAs,
}

/// Who sent a message, as stored in `messages.sender_kind` and its sender column. The system sends as `fold`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Sender {
    Worker(WorkerName),
    Human(HumanName),
    Fold,
}

impl Sender {
    pub fn of(actor: &Actor) -> Sender {
        match actor {
            Actor::Agent(agent) => Sender::Worker(agent.worker.clone()),
            Actor::Human { name } => Sender::Human(name.clone()),
            Actor::System => Sender::Fold,
        }
    }

    /// The value of `messages.sender_kind`.
    pub fn kind(&self) -> &'static str {
        match self {
            Sender::Worker(_) => "worker",
            Sender::Human(_) => "human",
            Sender::Fold => "fold",
        }
    }

    fn is(&self, target: &Target) -> bool {
        match (self, target) {
            (Sender::Worker(sender), Target::Worker(worker)) => sender == worker,
            (Sender::Human(sender), Target::Human(name)) => sender == name,
            _ => false,
        }
    }
}

impl fmt::Display for Sender {
    /// The worker name, `human:<name>` or `fold`, the same label an event actor has.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Sender::Worker(worker) => write!(f, "{worker}"),
            Sender::Human(name) => write!(f, "{HUMAN_PREFIX}{name}"),
            Sender::Fold => f.write_str("fold"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryAgent {
    pub name: AgentName,
    pub tags: Vec<Tag>,
}

/// The agents and registry humans at send time. `tag`, `all` and `humans` expand in the order of these lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Directory {
    pub agents: Vec<DirectoryAgent>,
    pub humans: Vec<HumanName>,
}

/// Why `to[]` reaches nobody it can name. The `to` values are address strings.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ResolveError {
    #[error("recipient {to} does not exist")]
    UnknownRecipient { to: String },
    #[error("{to} is not a known human")]
    UnknownHuman { to: String },
    #[error("the message has no recipient")]
    NoRecipient { to: Vec<String> },
}

impl From<ResolveError> for FoldError {
    /// `details.to` is always a list of address strings: the unknown entry, or every entry when none of them reaches anyone.
    fn from(error: ResolveError) -> Self {
        let message = error.to_string();
        match error {
            ResolveError::UnknownRecipient { to } => {
                FoldError::new(codes::RECIPIENT_NOT_FOUND, message)
                    .with_details([("to", json!([to]))])
                    .with_hint("list the agents with list_agents")
            }
            ResolveError::UnknownHuman { to } => FoldError::new(codes::UNKNOWN_HUMAN, message)
                .with_details([("to", json!([to]))])
                .with_hint("a human is known once they have used the CLI or the web interface"),
            ResolveError::NoRecipient { to } => FoldError::new(codes::RECIPIENT_NOT_FOUND, message)
                .with_details([("to", json!(to))])
                .with_hint("address an agent, a worker or a human by name"),
        }
    }
}

/// The recipient rows of `addresses`, one per worker or human, in input order. The first entry that reaches a target sets its `addressed_as`. `tag`, `all` and `humans` leave out the sender itself; a name the sender gives explicitly is kept.
pub fn resolve(
    addresses: &[Address],
    directory: &Directory,
    sender: &Sender,
) -> Result<Vec<Recipient>, ResolveError> {
    let mut rows = Rows::default();
    for address in addresses {
        let addressed_as = address.addressed_as();
        match address {
            Address::Agent(agent) => {
                if !directory.agents.iter().any(|known| &known.name == agent) {
                    return Err(ResolveError::UnknownRecipient {
                        to: address.to_string(),
                    });
                }
                rows.add(Target::Worker(WorkerName::main(agent)), addressed_as);
            }
            Address::Tag(tag) => {
                for agent in directory.agents.iter().filter(|a| a.tags.contains(tag)) {
                    rows.add_expanded(
                        Target::Worker(WorkerName::main(&agent.name)),
                        addressed_as,
                        sender,
                    );
                }
            }
            Address::All => {
                for agent in &directory.agents {
                    rows.add_expanded(
                        Target::Worker(WorkerName::main(&agent.name)),
                        addressed_as,
                        sender,
                    );
                }
            }
            Address::Human(name) => {
                if !directory.humans.contains(name) {
                    return Err(ResolveError::UnknownHuman {
                        to: address.to_string(),
                    });
                }
                rows.add(Target::Human(name.clone()), addressed_as);
            }
            Address::Humans => {
                for name in &directory.humans {
                    rows.add_expanded(Target::Human(name.clone()), addressed_as, sender);
                }
            }
        }
    }
    if rows.list.is_empty() {
        return Err(ResolveError::NoRecipient {
            to: addresses.iter().map(Address::to_string).collect(),
        });
    }
    Ok(rows.list)
}

/// The recipients of a reply that names none: the parent's sender, so an answer reaches whoever asked. When the replier sent the parent itself, or `fold` did, the reply goes to the parent's other recipients instead. An empty list resolves to `recipient_not_found`.
pub fn reply_addresses(
    parent_sender: &Sender,
    parent_recipients: &[Target],
    replier: &Sender,
) -> Vec<Address> {
    let sender = match parent_sender {
        Sender::Worker(worker) if parent_sender != replier => Some(Address::of_worker(worker)),
        Sender::Human(name) if parent_sender != replier => Some(Address::Human(name.clone())),
        Sender::Worker(_) | Sender::Human(_) | Sender::Fold => None,
    };
    if let Some(address) = sender {
        return vec![address];
    }
    parent_recipients
        .iter()
        .filter(|target| !replier.is(target))
        .map(|target| match target {
            Target::Worker(worker) => Address::of_worker(worker),
            Target::Human(name) => Address::Human(name.clone()),
        })
        .collect()
}

#[derive(Default)]
struct Rows {
    list: Vec<Recipient>,
    seen: HashSet<Target>,
}

impl Rows {
    fn add(&mut self, target: Target, addressed_as: AddressedAs) {
        if self.seen.insert(target.clone()) {
            self.list.push(Recipient {
                target,
                addressed_as,
            });
        }
    }

    fn add_expanded(&mut self, target: Target, addressed_as: AddressedAs, sender: &Sender) {
        if !sender.is(&target) {
            self.add(target, addressed_as);
        }
    }
}

/// The text of a message. It is never logged and never put in an event or an error, so its `Debug` form gives the length alone.
#[derive(Clone, PartialEq, Eq)]
pub struct MessageBody(String);

impl std::fmt::Debug for MessageBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MessageBody")
            .field("len", &self.0.len())
            .finish_non_exhaustive()
    }
}

impl MessageBody {
    /// A body with no visible text is `invalid_request` with `details.reason = "empty_message"`.
    pub fn parse(text: impl Into<String>) -> Result<MessageBody, FoldError> {
        let text = text.into();
        if text.trim().is_empty() {
            return Err(
                FoldError::new(codes::INVALID_REQUEST, "a message body has no text")
                    .with_details([("reason", json!("empty_message"))]),
            );
        }
        Ok(MessageBody(text))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The fewest options a question with choices offers.
pub const MIN_CHOICES: usize = 2;

/// The most options a question with choices offers.
pub const MAX_CHOICES: usize = 6;

/// The longest option label, in characters.
pub const MAX_CHOICE_LEN: usize = 100;

/// The longest option description, in characters.
pub const MAX_CHOICE_DESCRIPTION_LEN: usize = 300;

/// The longest question header, in characters. Claude Code's `AskUserQuestion` keeps its headers to 12.
pub const MAX_QUESTION_HEADER_LEN: usize = 40;

/// One option of a multiple-choice question, as Claude Code's `AskUserQuestion` draws it: a short label and an optional one-line description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    pub description: Option<String>,
}

impl Choice {
    pub fn label(label: impl Into<String>) -> Choice {
        Choice {
            label: label.into(),
            description: None,
        }
    }
}

/// `text` trimmed when it is one line of at most `max` characters; `None` for empty, multi-line or too long text.
pub fn short_line(text: &str, max: usize) -> Option<String> {
    one_line(text, max).ok().map(str::to_owned)
}

/// The options of a multiple-choice question from an agent to a human: 2 to 6 options with distinct single-line labels, and an optional short header. The human answers with a normal reply that names one label, or with free text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageChoices {
    options: Vec<Choice>,
    header: Option<String>,
}

impl MessageChoices {
    /// Trims each label, description and the header; an empty description or header counts as none. Too few or too many options, an empty, multi-line or too long label, a multi-line or too long description or header, or two labels that differ only in case are `invalid_request` with `details.reason = "choices"`.
    pub fn parse(options: &[Choice], header: Option<&str>) -> Result<MessageChoices, FoldError> {
        let invalid = |message: &str| {
            FoldError::new(codes::INVALID_REQUEST, message)
                .with_details([("reason", json!("choices"))])
                .with_hint(format!(
                    "pass {MIN_CHOICES} to {MAX_CHOICES} short single-line choices"
                ))
        };
        if !(MIN_CHOICES..=MAX_CHOICES).contains(&options.len()) {
            return Err(invalid("a question has 2 to 6 choices"));
        }
        let optional = |text: Option<&str>, max: usize, message: &str| match text.map(str::trim) {
            None | Some("") => Ok(None),
            Some(text) => short_line(text, max)
                .map(Some)
                .ok_or_else(|| invalid(message)),
        };
        let mut seen = HashSet::new();
        let mut kept = Vec::with_capacity(options.len());
        for option in options {
            let label = match one_line(&option.label, MAX_CHOICE_LEN) {
                Ok(label) => label,
                Err(LineFault::Empty | LineFault::NotOneLine) => {
                    return Err(invalid("a choice is one line of text"));
                }
                Err(LineFault::TooLong) => {
                    return Err(invalid("a choice is at most 100 characters"));
                }
            };
            if !seen.insert(label.to_lowercase()) {
                return Err(invalid("the choices repeat a label"));
            }
            let description = optional(
                option.description.as_deref(),
                MAX_CHOICE_DESCRIPTION_LEN,
                "a choice's description is one line of at most 300 characters",
            )?;
            kept.push(Choice {
                label: label.to_owned(),
                description,
            });
        }
        let header = optional(
            header,
            MAX_QUESTION_HEADER_LEN,
            "a question's header is one line of at most 40 characters",
        )?;
        Ok(MessageChoices {
            options: kept,
            header,
        })
    }

    pub fn options(&self) -> &[Choice] {
        &self.options
    }

    pub fn labels(&self) -> Vec<String> {
        self.options
            .iter()
            .map(|option| option.label.clone())
            .collect()
    }

    pub fn header(&self) -> Option<&str> {
        self.header.as_deref()
    }
}

/// The answer a human's reply gives to a question with `choices`: the label the reply names, compared without case and surrounding space, or else the reply's own text, trimmed.
pub fn chosen_answer(choices: &[String], reply: &str) -> String {
    let reply = reply.trim();
    choices
        .iter()
        .find(|label| label.to_lowercase() == reply.to_lowercase())
        .cloned()
        .unwrap_or_else(|| reply.to_owned())
}

/// The most lines of a run that a question about lines of a proposal keeps as its excerpt.
pub const MAX_EXCERPT_LINES: usize = 100;

/// The most characters a question's excerpt keeps.
pub const MAX_EXCERPT_CHARS: usize = 20_000;

/// The lines of `text` without their endings, split as the diff numbers them: a line ends at `\r\n`, at a lone `\r` or at `\n`, and text after the last ending is one more line.
pub fn text_lines(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                lines.push(&text[start..index]);
                index += if bytes.get(index + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                start = index;
            }
            b'\n' => {
                lines.push(&text[start..index]);
                index += 1;
                start = index;
            }
            _ => index += 1,
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// A question's excerpt: lines `start..=end` of `text` as [`text_lines`] splits it, 1-based and joined by newlines, and whether it was cut. A run longer than [`MAX_EXCERPT_LINES`] lines or [`MAX_EXCERPT_CHARS`] characters keeps its first lines, and a first line longer than that its first characters. `None` when the run is not a forward run from line 1 inside the text.
pub fn line_excerpt(text: &str, start: u32, end: u32) -> Option<(String, bool)> {
    if start == 0 || start > end {
        return None;
    }
    let skip = usize::try_from(start - 1).ok()?;
    let wanted = usize::try_from(end - start).ok()?.checked_add(1)?;
    let lines: Vec<&str> = text_lines(text)
        .into_iter()
        .skip(skip)
        .take(wanted)
        .collect();
    if lines.len() < wanted {
        return None;
    }
    let mut excerpt = String::new();
    let mut chars = 0;
    let mut kept = 0;
    for line in lines.iter().take(MAX_EXCERPT_LINES) {
        let separator = usize::from(kept > 0);
        let length = line.chars().count();
        if chars + separator + length > MAX_EXCERPT_CHARS {
            if kept == 0 {
                excerpt.extend(line.chars().take(MAX_EXCERPT_CHARS));
            }
            return Some((excerpt, true));
        }
        if kept > 0 {
            excerpt.push('\n');
        }
        excerpt.push_str(line);
        chars += separator + length;
        kept += 1;
    }
    Some((excerpt, kept < wanted))
}
