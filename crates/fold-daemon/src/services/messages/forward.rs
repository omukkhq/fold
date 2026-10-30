use fold_core::{
    Actor, AddressedAs, AgentName, MessageBody, Recipient, Role, Sender, Target, TaskId, Timestamp,
    WorkerName,
};
use fold_store::agents::WorkerProcessRow;
use fold_store::messages::{self, OwedMessage};
use fold_store::{StoreError, WriteTx, agents, tasks};

use super::{Draft, MessageError, write_message};
use crate::services::events::EventBuffer;

/// Sends `owed.message` on, because its recipient, the stopped worker of `worker`, cannot start to answer it: a task worker's message goes to the lead of the area of the message's task, else of the worker's area, a reviewer's to the review lead, and a lead's, or one that finds no lead, to the main agent. The lead gets a row of the message, the worker's row records the forward, and fold says so in the thread to the sender and the lead. When the sender is that lead, the message only goes back to it. With nobody to take it, the worker's row records that and fold tells the sender. Returns the lead's worker. The caller publishes the buffer after the commit.
pub(crate) async fn forward_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    owed: &OwedMessage,
    worker: &WorkerProcessRow,
    now: Timestamp,
) -> Result<Option<WorkerName>, MessageError> {
    let (sender, sender_agent) = match &owed.sender {
        Sender::Human(name) => (
            Recipient {
                target: Target::Human(name.clone()),
                addressed_as: AddressedAs::Human,
            },
            None,
        ),
        Sender::Worker(sender) => (
            Recipient {
                target: Target::Worker(sender.clone()),
                addressed_as: AddressedAs::Worker,
            },
            Some(sender.agent()),
        ),
        Sender::Fold => return Ok(None),
    };
    let head = messages::find(tx.conn(), owed.message)
        .await?
        .ok_or(StoreError::Corrupt("messages.number"))?;
    let recipient = messages::get(tx.conn(), owed.message)
        .await?
        .and_then(|row| {
            row.recipients
                .into_iter()
                .find_map(|recipient| match recipient.target {
                    Target::Worker(worker) if worker.agent() == owed.agent => Some(worker),
                    _ => None,
                })
        })
        .unwrap_or_else(|| WorkerName::main(&owed.agent));
    let lead = lead_of(tx, owed.task, worker)
        .await?
        .filter(|lead| *lead != owed.agent)
        .map(|lead| WorkerName::main(&lead));
    let mut recipients = vec![sender];
    let text = match &lead {
        None => format!(
            "@{recipient} cannot start now to answer {}, and no lead can take it, so it stays unread.",
            owed.message
        ),
        Some(lead) if sender_agent.as_ref() == Some(&lead.agent()) => format!(
            "@{recipient} cannot start now to answer {}, so it stays unread. Answer it yourself or start the worker again.",
            owed.message
        ),
        Some(lead) => {
            let to_lead = Recipient {
                target: Target::Worker(lead.clone()),
                addressed_as: AddressedAs::Agent,
            };
            if !messages::is_recipient(tx.conn(), owed.message, lead).await? {
                messages::insert_recipient(tx, owed.message, &to_lead).await?;
            }
            recipients.push(to_lead);
            format!(
                "@{recipient} cannot start now, so fold sent {} on to @{lead}.",
                owed.message
            )
        }
    };
    messages::mark_forwarded(tx, owed.message, &owed.agent, lead.as_ref(), now).await?;
    let body = MessageBody::parse(text)?;
    write_message(
        tx,
        buffer,
        Draft {
            actor: &Actor::System,
            sender: &Sender::Fold,
            task: head.task,
            proposal: head.proposal,
            request: head.request,
            join: None,
            parent: Some(head),
            body: &body,
            choices: None,
            questions: None,
            asks_human: false,
            lines: None,
            recipients,
            now,
        },
    )
    .await?;
    Ok(lead)
}

/// Tells the lead of the worker of `worker`, the one [`forward_in_tx`] sends its messages to, with `text` from fold about the work that waits because the worker cannot start, such as a lost session whose new one the worker limits refuse. `task` and `proposal` name the work. Returns the lead's agent; `None`, and nothing is sent, when no lead or main agent can take it. The caller publishes the buffer after the commit.
pub(crate) async fn tell_lead_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    worker: &WorkerProcessRow,
    task: Option<TaskId>,
    proposal: Option<fold_core::ProposalId>,
    text: String,
    now: Timestamp,
) -> Result<Option<AgentName>, MessageError> {
    let Some(lead) = lead_of(tx, task, worker)
        .await?
        .filter(|lead| *lead != worker.agent)
    else {
        return Ok(None);
    };
    let body = MessageBody::parse(text)?;
    write_message(
        tx,
        buffer,
        Draft {
            actor: &Actor::System,
            sender: &Sender::Fold,
            task,
            proposal,
            request: None,
            join: None,
            parent: None,
            body: &body,
            choices: None,
            questions: None,
            lines: None,
            asks_human: false,
            recipients: vec![Recipient {
                target: Target::Worker(WorkerName::main(&lead)),
                addressed_as: AddressedAs::Agent,
            }],
            now,
        },
    )
    .await?;
    Ok(Some(lead))
}

/// The lead that takes a message to the stopped worker of `worker`; see [`forward_in_tx`].
async fn lead_of(
    tx: &mut WriteTx,
    task: Option<TaskId>,
    worker: &WorkerProcessRow,
) -> Result<Option<AgentName>, StoreError> {
    let lead = match worker.role {
        Role::TaskWorker => area_lead(tx, task, worker).await?,
        Role::Reviewer => agents::review_lead(tx.conn()).await?,
        Role::AreaLead | Role::ReviewLead | Role::Main => None,
    };
    match lead {
        Some(lead) => Ok(Some(lead)),
        None => main_agent(tx).await,
    }
}

/// The lead of the area that `task` belongs to by its labels, else of the worker's own area; `None` for a task of the area Main and when no area names a lead.
async fn area_lead(
    tx: &mut WriteTx,
    task: Option<TaskId>,
    worker: &WorkerProcessRow,
) -> Result<Option<AgentName>, StoreError> {
    let labels = match task {
        Some(task) => tasks::labels_for_conn(tx.conn(), task).await?,
        None => Vec::new(),
    };
    if labels.iter().any(|label| label.name.is_main()) {
        return Ok(None);
    }
    let of_task = agents::list_areas(tx.conn())
        .await?
        .into_iter()
        .find(|row| labels.iter().any(|label| label.name == row.label))
        .and_then(|row| row.lead);
    if of_task.is_some() {
        return Ok(of_task);
    }
    match worker.area.as_ref().filter(|area| !area.is_main()) {
        Some(area) => Ok(agents::find_area(tx.conn(), area)
            .await?
            .and_then(|row| row.lead)),
        None => Ok(None),
    }
}

/// The agent that holds the main tag.
async fn main_agent(tx: &mut WriteTx) -> Result<Option<AgentName>, StoreError> {
    Ok(messages::directory(tx.conn(), Vec::new())
        .await?
        .agents
        .into_iter()
        .find(|agent| agent.tags.iter().any(fold_core::Tag::is_main))
        .map(|agent| agent.name))
}
