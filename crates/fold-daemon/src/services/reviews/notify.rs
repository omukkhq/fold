use std::collections::BTreeSet;

use fold_core::events::types;
use fold_core::reviews::standing_agent_approval;
use fold_core::{
    AddressedAs, MessageBody, Recipient, RevisionId, Target, TaskId, TaskStatus, Timestamp,
    WorkerName,
};
use fold_store::WriteTx;
use fold_store::reviews::IntegrationFacts;
use fold_store::tasks::claims;

use super::ReviewError;
use crate::services::events::EventBuffer;
use crate::services::messages::send_from_fold_in_tx;

/// Tells the agents an integrated proposal concerns, in the integration's own transaction and after its events: the author's agent, the reviewer's agent (the holder of the review claim, else the agent whose approval stands) and the agent holding the claim on each task that `buffer` shows turning Todo. It is one message from `fold` linked to the task, so an agent that has several of these roles gets it once. `facts` are the ones the transaction rechecked, read before the review claim was released.
pub(super) async fn integration_messages(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    facts: &IntegrationFacts,
    revision: RevisionId,
    now: Timestamp,
) -> Result<(), ReviewError> {
    let (proposal, task) = (facts.proposal.id, facts.proposal.task);
    let mut to = BTreeSet::from([facts.proposal.author.agent()]);
    let reviewer = match &facts.proposal.review_claim {
        Some(holder) => Some(holder.agent()),
        None => standing_agent_approval(&facts.reviews),
    };
    to.extend(reviewer);
    let ready = newly_ready(buffer);
    for ready_task in &ready {
        if let Some(claim) = claims::unreleased_for_task(tx.conn(), *ready_task).await? {
            to.insert(claim.worker.agent());
        }
    }
    let body = MessageBody::parse(body_text(proposal, task, revision, &ready))?;
    let recipients = to
        .iter()
        .map(|agent| Recipient {
            target: Target::Worker(WorkerName::main(agent)),
            addressed_as: AddressedAs::Agent,
        })
        .collect();
    send_from_fold_in_tx(tx, buffer, recipients, (task, proposal), &body, now).await?;
    Ok(())
}

/// The tasks whose `task.status_changed` in this transaction moved them to Todo, in the order the recalculation reached them.
fn newly_ready(buffer: &EventBuffer) -> Vec<TaskId> {
    buffer
        .events()
        .iter()
        .filter(|event| event.event_type == types::TASK_STATUS_CHANGED)
        .filter(|event| {
            event.data.get("to").and_then(serde_json::Value::as_str)
                == Some(TaskStatus::Todo.as_str())
        })
        .filter_map(|event| event.task)
        .collect()
}

fn body_text(
    proposal: fold_core::ProposalId,
    task: TaskId,
    revision: RevisionId,
    ready: &[TaskId],
) -> String {
    let integrated = format!("{proposal} for {task} is integrated into main as {revision}.");
    if ready.is_empty() {
        return integrated;
    }
    let names: Vec<String> = ready.iter().map(ToString::to_string).collect();
    format!("{integrated} Now ready: {}.", names.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_names_the_integration_and_the_ready_tasks() {
        let proposal = fold_core::ProposalId::from_number(3).unwrap();
        let task = TaskId::from_number(12).unwrap();
        let revision = RevisionId::from_number(5).unwrap();
        assert_eq!(
            body_text(proposal, task, revision, &[]),
            "P-3 for T-12 is integrated into main as r5."
        );
        let ready = [
            TaskId::from_number(14).unwrap(),
            TaskId::from_number(15).unwrap(),
        ];
        assert_eq!(
            body_text(proposal, task, revision, &ready),
            "P-3 for T-12 is integrated into main as r5. Now ready: T-14, T-15."
        );
    }
}
