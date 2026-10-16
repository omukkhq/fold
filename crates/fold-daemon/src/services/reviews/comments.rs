use fold_core::reviews::{Side, Verdict};
use fold_core::{Actor, CommentId, EventActor, FoldError, ProposalId, Timestamp, codes};
use fold_store::StoreError;
use fold_store::reviews::{self, BatchCommentRef, CommentRow, LineRange, ReviewTarget};
use fold_vcs::browse;
use serde_json::json;

use super::proposals::{proposal_not_found, version_not_found};
use super::{ReviewError, ReviewService};
use crate::services::workspaces::WorkspaceService;

/// The longest review or comment body, in characters.
pub(super) const MAX_BODY_CHARS: usize = 20_000;

/// One comment to add: about the whole version when `location` is `None`, otherwise about a run of lines of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentInput {
    pub location: Option<CommentLocation>,
    pub reply_to: Option<CommentId>,
    pub body: String,
}

/// A line or a run of consecutive lines of `path`, 1-based and inclusive, in the version's base tree (`Old`) or the version's own tree (`New`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentLocation {
    pub path: String,
    pub side: Side,
    pub start_line: u32,
    pub end_line: u32,
}

impl CommentLocation {
    /// The location of a comment from the four fields a caller sent: all of them for a line comment, none for a comment on the whole version. Any other mix is `invalid_request` naming `field`. Every transport takes the rule and its refusal from here.
    pub fn from_parts(
        field: &str,
        path: Option<String>,
        side: Option<Side>,
        start_line: Option<u32>,
        end_line: Option<u32>,
    ) -> Result<Option<CommentLocation>, FoldError> {
        match (path, side, start_line, end_line) {
            (None, None, None, None) => Ok(None),
            (Some(path), Some(side), Some(start_line), Some(end_line)) => {
                Ok(Some(CommentLocation {
                    path,
                    side,
                    start_line,
                    end_line,
                }))
            }
            _ => Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("{field}: a line comment needs path, side, start_line and end_line"),
            )
            .with_details([("field", json!(field))])
            .with_hint("give path, side, start_line and end_line together, or none of them")),
        }
    }

    pub(super) fn as_range(&self) -> LineRange<'_> {
        LineRange {
            path: &self.path,
            side: self.side,
            start_line: self.start_line,
            end_line: self.end_line,
        }
    }
}

/// A stored comment with public ids only. `outdated` is true when the comment was made on a version before the current one; nothing is rewritten when a version is added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentView {
    pub id: CommentId,
    pub version: u32,
    /// The verdict of the review the comment came with.
    pub verdict: Verdict,
    pub author: EventActor,
    pub location: Option<CommentLocation>,
    pub reply_to: Option<CommentId>,
    pub body: String,
    pub created_at: Timestamp,
    pub outdated: bool,
    /// The comment of a review batch this comment puts on the proposal, with every member it is on.
    pub batch_comment: Option<BatchCommentRef>,
}

impl CommentView {
    fn from_row(row: CommentRow, current_version: u32) -> Result<Self, ReviewError> {
        let location = match (row.path, row.side, row.start_line, row.end_line) {
            (None, None, None, None) => None,
            (Some(path), Some(side), Some(start_line), Some(end_line)) => Some(CommentLocation {
                path,
                side,
                start_line,
                end_line,
            }),
            _ => return Err(StoreError::Corrupt("review_comments.path").into()),
        };
        Ok(Self {
            id: row.id,
            outdated: row.version != current_version,
            version: row.version,
            verdict: row.verdict,
            author: row.author,
            location,
            reply_to: row.reply_to,
            body: row.body,
            created_at: row.created_at,
            batch_comment: row.batch_comment,
        })
    }
}

impl ReviewService {
    /// Adds one comment to `version` of an open proposal, as a `comment` review with an empty body; another current version is `stale_version`, so the lines are never checked against a version the commenter did not read. It is the path for a line selected in the browser; it never changes whose turn it is.
    pub async fn add_comment(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        version: u32,
        input: CommentInput,
    ) -> Result<CommentView, ReviewError> {
        let submitted = self
            .submit_review(actor, proposal, version, Verdict::Comment, "", vec![input])
            .await?;
        let current = submitted.summary.current_version;
        let id = submitted
            .comments
            .first()
            .copied()
            .ok_or(StoreError::Corrupt("review_comments.number"))?;
        let row = reviews::list_comments(self.store.read(), proposal, Some(submitted.version))
            .await?
            .into_iter()
            .find(|row| row.id == id)
            .ok_or(StoreError::Corrupt("review_comments.number"))?;
        CommentView::from_row(row, current)
    }

    /// The comments of a proposal in number order, only those on `version` when it is given, each marked `outdated` when it was made on an earlier version than the current one.
    pub async fn list_comments(
        &self,
        proposal: ProposalId,
        version: Option<u32>,
    ) -> Result<Vec<CommentView>, ReviewError> {
        let read = self.store.read();
        let row = reviews::get_proposal(read, proposal)
            .await?
            .ok_or_else(|| proposal_not_found(proposal))?;
        if let Some(version) = version
            && (version == 0 || version > row.current_version)
        {
            return Err(version_not_found(proposal, version, row.current_version));
        }
        reviews::list_comments(read, proposal, version)
            .await?
            .into_iter()
            .map(|comment| CommentView::from_row(comment, row.current_version))
            .collect()
    }
}

/// The refusals that need no files: a comment has a body, and a location names a path and a forward run of lines from line 1.
pub(super) fn check_shape(index: usize, comment: &CommentInput) -> Result<(), ReviewError> {
    let body = comment.body.trim();
    if body.is_empty() || body.chars().count() > MAX_BODY_CHARS {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!("a comment body must be 1 to {MAX_BODY_CHARS} characters"),
        )
        .with_details([("field", json!(format!("comments[{index}].body")))])
        .with_hint("give every comment a short body")
        .into());
    }
    if let Some(location) = &comment.location
        && (location.start_line == 0 || location.start_line > location.end_line)
    {
        return Err(range_refusal(index, location, None));
    }
    Ok(())
}

/// Each located comment's path must name a text file in the tree its side reads, and its last line must be within that file. The paths are parsed as relative and normalised first, so a path that could leave the tree is `invalid_path`.
pub(super) async fn check_ranges(
    workspaces: &WorkspaceService,
    target: &ReviewTarget,
    comments: &[CommentInput],
) -> Result<(), ReviewError> {
    let mut located = Vec::new();
    let mut files = Vec::new();
    for (index, comment) in comments.iter().enumerate() {
        let Some(location) = &comment.location else {
            continue;
        };
        let path = browse::parse_path(&location.path)?.ok_or_else(|| {
            FoldError::new(codes::INVALID_PATH, "a line comment needs a file path")
                .with_details([("field", json!(format!("comments[{index}].path")))])
                .with_hint("name the file the lines belong to, or drop the location")
        })?;
        let tree = match location.side {
            Side::Old => target.base_tree,
            Side::New => target.tree,
        };
        located.push((index, location));
        files.push((tree, path));
    }
    if files.is_empty() {
        return Ok(());
    }
    let counts = workspaces.text_line_counts(files).await?;
    for ((index, location), lines) in located.into_iter().zip(counts) {
        match lines {
            None => {
                return Err(FoldError::new(
                    codes::UNKNOWN_COMMENT_PATH,
                    format!(
                        "the {} side of v{} has no text file at this path",
                        location.side.as_str(),
                        target.version
                    ),
                )
                .with_details([
                    ("field", json!(format!("comments[{index}].path"))),
                    ("path", json!(location.path)),
                    ("side", json!(location.side.as_str())),
                ])
                .with_hint("call get_proposal and compare the version to find the changed files")
                .into());
            }
            Some(lines) if location.end_line > lines => {
                return Err(range_refusal(index, location, Some(lines)));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

fn range_refusal(index: usize, location: &CommentLocation, lines: Option<u32>) -> ReviewError {
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
        ("field", json!(format!("comments[{index}]"))),
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
