use fold_core::{
    Actor, AddressedAs, AgentName, AreaName, MessageBody, Recipient, Sender, Target, Timestamp,
    WorkerName,
};
use fold_store::messages::{self, OwedMessage};
use fold_store::{StoreError, WriteTx, agents};

use super::{Draft, MessageError, write_message};
use crate::services::events::EventBuffer;

/// Sends `owed.message` on, because its recipient, the stopped task worker of `owed.agent` in `area`, cannot start to answer it. It goes to the area's lead, or to the main agent for the area Main and for an area without a lead: the lead gets a row of the message, the worker's row records the forward, and fold says so in the thread to the sender and the lead. When the sender is that lead, the message only goes back to it. Returns the lead's worker; `None`, with nothing written, when the project has no lead and no main agent. The caller publishes the buffer after the commit.
pub(crate) async fn forward_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    owed: &OwedMessage,
    area: Option<&AreaName>,
    now: Timestamp,
) -> Result<Option<WorkerName>, MessageError> {
    let Some(lead) = lead_of(tx, area).await? else {
        return Ok(None);
    };
    let lead = WorkerName::main(&lead);
    let head = messages::find(tx.conn(), owed.message)
        .await?
        .ok_or(StoreError::Corrupt("messages.number"))?;
    let worker = messages::get(tx.conn(), owed.message)
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
    let (sender, back) = match &owed.sender {
        Sender::Human(name) => (
            Recipient {
                target: Target::Human(name.clone()),
                addressed_as: AddressedAs::Human,
            },
            false,
        ),
        Sender::Worker(sender) => (
            Recipient {
                target: Target::Worker(sender.clone()),
                addressed_as: AddressedAs::Worker,
            },
            sender.agent() == lead.agent(),
        ),
        Sender::Fold => return Ok(None),
    };
    let mut recipients = vec![sender];
    if !back {
        let to_lead = Recipient {
            target: Target::Worker(lead.clone()),
            addressed_as: AddressedAs::Agent,
        };
        if !messages::is_recipient(tx.conn(), owed.message, &lead).await? {
            messages::insert_recipient(tx, owed.message, &to_lead).await?;
        }
        recipients.push(to_lead);
    }
    messages::mark_forwarded(tx, owed.message, &owed.agent, &lead, now).await?;
    let text = if back {
        format!(
            "@{worker} cannot start now to answer {}, so it stays unread. Answer it yourself or start the worker again.",
            owed.message
        )
    } else {
        format!(
            "@{worker} cannot start now, so fold sent {} on to @{lead}.",
            owed.message
        )
    };
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
            asks_human: false,
            recipients,
            now,
        },
    )
    .await?;
    Ok(Some(lead))
}

/// The lead of `area`, else the agent that holds the main tag.
async fn lead_of(
    tx: &mut WriteTx,
    area: Option<&AreaName>,
) -> Result<Option<AgentName>, MessageError> {
    if let Some(area) = area.filter(|area| !area.is_main())
        && let Some(lead) = agents::find_area(tx.conn(), area)
            .await?
            .and_then(|row| row.lead)
    {
        return Ok(Some(lead));
    }
    let directory = messages::directory(tx.conn(), Vec::new()).await?;
    Ok(directory
        .agents
        .into_iter()
        .find(|agent| agent.tags.iter().any(fold_core::Tag::is_main))
        .map(|agent| agent.name))
}
