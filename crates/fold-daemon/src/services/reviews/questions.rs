use fold_core::messages::line_excerpt;
use fold_core::reviews::Side;
use fold_core::{
    Actor, AddressedAs, FoldError, MessageBody, ProposalId, Recipient, Target, codes, policy,
};
use fold_store::messages::MessageLines;
use fold_store::reviews;
use fold_store::tasks::claims;
use fold_vcs::browse;
use serde_json::json;

use super::comments::CommentLocation;
use super::proposals::{proposal_not_found, version_not_found};
use super::{ReviewError, ReviewService};
use crate::services::events::EventBuffer;
use crate::services::messages::{LineQuestion, MessageView, ask_about_lines_in_tx};

impl ReviewService {
    /// A named human's question about a run of lines of `version` of the proposal, sent to its author: the worker holding the implementation claim on the proposal's task, else the proposal's author. It is a message linked to the task and the proposal that carries the lines and their excerpt; it writes no review, verdict or comment, so the proposal's turn stays as it was. Any version of a proposal in any state takes a question.
    pub async fn ask_author(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        version: u32,
        location: CommentLocation,
        body: MessageBody,
    ) -> Result<MessageView, ReviewError> {
        policy::can_send_feedback(actor).map_err(FoldError::from)?;
        if location.start_line == 0 || location.start_line > location.end_line {
            return Err(range_refusal(&location, None));
        }
        let path = browse::parse_path(&location.path)?.ok_or_else(|| {
            FoldError::new(
                codes::INVALID_PATH,
                "a question about lines needs a file path",
            )
            .with_details([("field", json!("path"))])
            .with_hint("name the file the lines belong to")
        })?;
        let read = self.store.read();
        let row = reviews::get_proposal(read, proposal)
            .await?
            .ok_or_else(|| proposal_not_found(proposal))?;
        let target = reviews::version_target(read, proposal, version)
            .await?
            .ok_or_else(|| version_not_found(proposal, version, row.current_version))?;
        let tree = match location.side {
            Side::Old => target.base_tree,
            Side::New => target.tree,
        };
        let text = self
            .workspaces
            .text_file(tree, path)
            .await?
            .ok_or_else(|| unknown_path(&location, version))?;
        let (excerpt, excerpt_cut) = line_excerpt(&text, location.start_line, location.end_line)
            .ok_or_else(|| range_refusal(&location, Some(fold_vcs::diff::line_count(&text))))?;
        let lines = MessageLines {
            version,
            path: location.path,
            side: location.side,
            start_line: location.start_line,
            end_line: location.end_line,
            excerpt,
            excerpt_cut,
        };
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let sent = self
            .store
            .write(async |tx| {
                let author = match claims::unreleased_for_task(tx.conn(), row.task).await? {
                    Some(claim) => claim.worker,
                    None => row.author.clone(),
                };
                let question = LineQuestion {
                    actor,
                    to: Recipient {
                        target: Target::Worker(author),
                        addressed_as: AddressedAs::Worker,
                    },
                    links: (row.task, proposal),
                    body: &body,
                    lines,
                    now,
                };
                Ok::<_, ReviewError>(ask_about_lines_in_tx(tx, &mut buffer, question).await?)
            })
            .await?;
        self.bus.publish(buffer);
        Ok(sent)
    }
}

fn unknown_path(location: &CommentLocation, version: u32) -> ReviewError {
    FoldError::new(
        codes::UNKNOWN_COMMENT_PATH,
        format!(
            "the {} side of v{version} has no text file at this path",
            location.side.as_str()
        ),
    )
    .with_details([
        ("field", json!("path")),
        ("path", json!(location.path)),
        ("side", json!(location.side.as_str())),
    ])
    .with_hint("pick lines of a changed text file of the version")
    .into()
}

fn range_refusal(location: &CommentLocation, lines: Option<u32>) -> ReviewError {
    let message = match lines {
        Some(lines) => format!(
            "lines {}-{} run past the end of the file, which has {lines}",
            location.start_line, location.end_line
        ),
        None => format!(
            "lines {}-{} are not a forward run of lines from line 1",
            location.start_line, location.end_line
        ),
    };
    let mut details = vec![
        ("field", json!("end_line")),
        ("path", json!(location.path)),
        ("side", json!(location.side.as_str())),
        ("start_line", json!(location.start_line)),
        ("end_line", json!(location.end_line)),
    ];
    if let Some(lines) = lines {
        details.push(("line_count", json!(lines)));
    }
    FoldError::new(codes::INVALID_COMMENT_RANGE, message)
        .with_details(details)
        .with_hint("pick lines that exist on that side of the version")
        .into()
}
