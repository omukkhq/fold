use fold_core::{FoldError, RevisionId, codes};
use fold_vcs::compare::FileChange;
use fold_vcs::diff::{DiffLine, Hunk, LineKind};
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, Tool};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::FoldServer;
use super::error::from_workspace;
use crate::services::workspaces::{RevisionSummary, StateRef, StateSummary};

pub const GET_REPOSITORY_STATE: &str = "get_repository_state";
pub const GET_REVISION: &str = "get_revision";
pub const COMPARE_REVISIONS: &str = "compare_revisions";

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetRepositoryStateInput {
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetRevisionInput {
    /// The revision, such as r17.
    pub r: String,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CompareRevisionsInput {
    /// The old side: a revision r<n>, a changeset C-<n>, a checkpoint ck-<n> or the working files of a workspace ws-<n>.
    pub from: String,
    /// The new side, in the same forms as from.
    pub to: String,
    /// One changed file, such as src/lib.rs. With it the result holds that file's hunks; without it, the list of changed files.
    #[serde(default)]
    pub path: Option<String>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            GET_REPOSITORY_STATE,
            "Show the head of main, the path of the read-only main directory and the newest 10 revisions.",
            schema_for_type::<GetRepositoryStateInput>(),
        ),
        Tool::new(
            GET_REVISION,
            "Show one main revision: its parent, tree, task and proposal, and the files it changed against its parent with totals. No hunks; use compare_revisions with a path for those.",
            schema_for_type::<GetRevisionInput>(),
        ),
        Tool::new(
            COMPARE_REVISIONS,
            "Compare two states: revisions, changesets, checkpoints or the working files of a workspace. Lists the changed files with line counts, or one file's hunks when you pass path.",
            schema_for_type::<CompareRevisionsInput>(),
        ),
    ]
}

fn parse<T: for<'de> Deserialize<'de>>(arguments: &JsonObject) -> Result<T, FoldError> {
    serde_json::from_value(Value::Object(arguments.clone())).map_err(|_| {
        FoldError::new(
            codes::INVALID_REQUEST,
            "the arguments do not match the tool's input schema",
        )
    })
}

fn revision_line(summary: &RevisionSummary) -> Value {
    json!({
        "r": summary.number,
        "created_at": summary.created_at.as_millis(),
        "task": summary.task,
        "proposal": summary.proposal,
        "title": summary.title,
    })
}

fn file_line(file: &FileChange) -> Value {
    let mut line = json!({
        "path": file.path.to_string(),
        "change": file.change.as_str(),
        "added": file.added_lines,
        "removed": file.removed_lines,
    });
    if let Some(old) = &file.old_path {
        line["old_path"] = json!(old.to_string());
    }
    if file.binary {
        line["binary"] = json!(true);
    }
    line
}

fn state_line(state: &StateSummary) -> Value {
    let mut line = json!({ "id": state.label, "kind": state.kind.as_str() });
    if let Some(base) = state.base {
        line["base"] = json!(base);
    }
    if let Some(task) = state.task {
        line["task"] = json!(task);
    }
    if let Some(worker) = &state.worker {
        line["worker"] = json!(worker);
    }
    line
}

fn hunk_value(hunk: &Hunk) -> Value {
    json!({
        "old_start": hunk.old_start,
        "old_lines": hunk.old_lines,
        "new_start": hunk.new_start,
        "new_lines": hunk.new_lines,
        "lines": hunk.lines.iter().map(line_value).collect::<Vec<_>>(),
    })
}

fn line_value(line: &DiffLine) -> Value {
    let kind = match line.kind {
        LineKind::Context => "context",
        LineKind::Added => "added",
        LineKind::Removed => "removed",
    };
    let mut value = json!({
        "kind": kind,
        "old_no": line.old_no,
        "new_no": line.new_no,
        "text": line.text,
    });
    if line.no_newline {
        value["no_newline"] = json!(true);
    }
    value
}

pub async fn get_repository_state(
    server: &FoldServer,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let _: GetRepositoryStateInput = parse(arguments)?;
    let state = server
        .workspaces
        .get_repository_state()
        .await
        .map_err(from_workspace)?;
    Ok(json!({
        "head": state.head,
        "main_path": state.main_path.to_string_lossy(),
        "recent": state.recent.iter().map(revision_line).collect::<Vec<_>>(),
    }))
}

pub async fn get_revision(server: &FoldServer, arguments: &JsonObject) -> Result<Value, FoldError> {
    let input: GetRevisionInput = parse(arguments)?;
    let number: RevisionId = input.r.parse()?;
    let detail = server
        .workspaces
        .get_revision(number)
        .await
        .map_err(from_workspace)?;
    let summary = &detail.summary;
    Ok(json!({
        "r": summary.number,
        "parent": summary.parent,
        "created_at": summary.created_at.as_millis(),
        "tree": summary.tree.to_string(),
        "task": summary.task,
        "proposal": summary.proposal,
        "files": detail.files.iter().map(file_line).collect::<Vec<_>>(),
        "totals": {
            "files": detail.totals.files,
            "added": detail.totals.added_lines,
            "removed": detail.totals.removed_lines,
        },
    }))
}

pub async fn compare_revisions(
    server: &FoldServer,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: CompareRevisionsInput = parse(arguments)?;
    let from = StateRef::parse(&input.from)?;
    let to = StateRef::parse(&input.to)?;
    if let Some(path) = input.path {
        let diff = server
            .workspaces
            .diff_file(from, to, &path)
            .await
            .map_err(from_workspace)?;
        let mut value = json!({
            "path": diff.path.to_string(),
            "change": diff.change.as_str(),
            "binary": diff.binary,
            "hunks": diff.hunks.iter().map(hunk_value).collect::<Vec<_>>(),
        });
        if let Some(old) = &diff.old_path {
            value["old_path"] = json!(old.to_string());
        }
        if diff.too_large {
            value["too_large"] = json!(true);
        }
        return Ok(value);
    }
    let comparison = server
        .workspaces
        .compare_states(from, to)
        .await
        .map_err(from_workspace)?;
    Ok(json!({
        "from": state_line(&comparison.from),
        "to": state_line(&comparison.to),
        "files": comparison.files.iter().map(file_line).collect::<Vec<_>>(),
        "totals": {
            "files": comparison.totals.files,
            "added": comparison.totals.added_lines,
            "removed": comparison.totals.removed_lines,
        },
    }))
}
