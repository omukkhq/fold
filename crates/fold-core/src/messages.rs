use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use serde_json::json;
use thiserror::Error;

use crate::actor::Actor;
use crate::agents::{AgentName, HumanName, Tag, WorkerName};
use crate::error::{FoldError, codes};

const TAG_PREFIX: &str = "tag:";
const HUMAN_PREFIX: &str = "human:";
pub(crate) const ALL: &str = "all";
pub(crate) const HUMANS: &str = "humans";

/// One `to[]` entry. `all` and `humans` are reserved agent names, so the bare words always mean the broadcast forms.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Address {
    /// The agent's main worker.
    Agent(AgentName),
    Worker(WorkerName),
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
        } else if entry.contains('/') {
            Address::Worker(WorkerName::parse(entry).map_err(|_| invalid())?)
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
            Address::Worker(_) => AddressedAs::Worker,
            Address::Tag(_) => AddressedAs::Tag,
            Address::All => AddressedAs::All,
            Address::Human(_) => AddressedAs::Human,
            Address::Humans => AddressedAs::Humans,
        }
    }

    /// The address that reaches exactly `worker`: the agent form for a main worker, the worker form for a child.
    pub fn of_worker(worker: &WorkerName) -> Address {
        if worker.is_main() {
            Address::Agent(worker.agent())
        } else {
            Address::Worker(worker.clone())
        }
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Address::Agent(agent) => write!(f, "{agent}"),
            Address::Worker(worker) => write!(f, "{worker}"),
            Address::Tag(tag) => write!(f, "{TAG_PREFIX}{tag}"),
            Address::All => f.write_str(ALL),
            Address::Human(name) => write!(f, "{HUMAN_PREFIX}{name}"),
            Address::Humans => f.write_str(HUMANS),
        }
    }
}

/// A `to[]` entry that is not an address. The entry is a name, never message content, so the error details may carry it.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("a recipient is an agent, an agent/worker path, tag:<tag>, all, human:<name> or humans")]
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
            .with_hint("list the agents and their workers with list_agents")
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

/// The agents, workers and registry humans at send time. `tag`, `all` and `humans` expand in the order of these lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Directory {
    pub agents: Vec<DirectoryAgent>,
    pub workers: Vec<WorkerName>,
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
                    .with_hint("list the agents and their workers with list_agents")
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
            Address::Worker(worker) => {
                if !directory.workers.contains(worker) {
                    return Err(ResolveError::UnknownRecipient {
                        to: address.to_string(),
                    });
                }
                rows.add(Target::Worker(worker.clone()), addressed_as);
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

/// The text of a message. It is never logged and never put in an event or an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageBody(String);

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
