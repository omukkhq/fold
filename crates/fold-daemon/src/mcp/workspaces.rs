use std::str::FromStr;

use fold_core::{
    Actor, CheckpointId, FoldError, RevisionId, TaskId, WorkerName, WorkspaceId, codes,
};
use fold_vcs::compare::FileChange;
use fold_vcs::diff::{DiffLine, Hunk, LineKind};
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, Tool};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::FoldServer;
use super::error::{from_agent, from_workspace};
use crate::services::workspaces::{RevisionSummary, StateRef, StateSummary, WorkspaceRef};

pub const GET_REPOSITORY_STATE: &str = "get_repository_state";
pub const GET_REVISION: &str = "get_revision";
pub const COMPARE_REVISIONS: &str = "compare_revisions";

pub const CREATE_WORKSPACE: &str = "create_workspace";
pub const GET_WORKSPACE: &str = "get_workspace";
pub const RECORD_CHECKPOINT: &str = "record_checkpoint";
pub const LIST_CHECKPOINTS: &str = "list_checkpoints";
pub const RESTORE_CHECKPOINT: &str = "restore_checkpoint";
pub const RECORD_CHANGES: &str = "record_changes";
pub const SYNC_WORKSPACE: &str = "sync_workspace";
pub const ABANDON_WORKSPACE: &str = "abandon_workspace";

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

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateWorkspaceInput {
    /// The task to work on, such as T-12. The create claims it for your worker when nobody holds it.
    pub task: String,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetWorkspaceInput {
    /// The workspace, such as ws-9. Give ws or task, not both.
    #[serde(default)]
    pub ws: Option<String>,
    /// A task such as T-12, to read its live workspace, or its newest one when none is live.
    #[serde(default)]
    pub task: Option<String>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RecordCheckpointInput {
    /// Your workspace, such as ws-9.
    pub ws: String,
    /// A short note to find the checkpoint again, at most 200 characters.
    #[serde(default)]
    pub label: Option<String>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListCheckpointsInput {
    /// The workspace, such as ws-9.
    pub ws: String,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RestoreCheckpointInput {
    /// The checkpoint to put back, such as ck-88. Its workspace gets its files and its base revision.
    pub ck: String,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RecordChangesInput {
    /// Your workspace, such as ws-9.
    pub ws: String,
    /// What the changes do and why, 1 to 2000 characters.
    pub message: String,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SyncWorkspaceInput {
    /// Your workspace, such as ws-9.
    pub ws: String,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AbandonWorkspaceInput {
    /// Your workspace, such as ws-9.
    pub ws: String,
    /// Why you stop, 1 to 500 characters. Other agents read it in the workspace.abandoned event.
    pub reason: String,
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
        Tool::new(
            CREATE_WORKSPACE,
            "Create the task workspace for a task: a copy of main under ws/ where you make your changes. It claims the task for your worker and moves it to In Progress.",
            schema_for_type::<CreateWorkspaceInput>(),
        ),
        Tool::new(
            GET_WORKSPACE,
            "Read a workspace by id or by task: state, path, base revision, owner, conflicted paths, changed file count, last activity and the newest checkpoint and changeset.",
            schema_for_type::<GetWorkspaceInput>(),
        ),
        Tool::new(
            RECORD_CHECKPOINT,
            "Record the current files of your workspace as a checkpoint you can restore later.",
            schema_for_type::<RecordCheckpointInput>(),
        ),
        Tool::new(
            LIST_CHECKPOINTS,
            "List the checkpoints of a workspace, newest first.",
            schema_for_type::<ListCheckpointsInput>(),
        ),
        Tool::new(
            RESTORE_CHECKPOINT,
            "Put the files and the base revision of a checkpoint back into your workspace. A safety checkpoint of the current files is taken first, so the restore can be undone.",
            schema_for_type::<RestoreCheckpointInput>(),
        ),
        Tool::new(
            RECORD_CHANGES,
            "Record the files of your workspace as a changeset with a message. Refused while a conflicted file still holds conflict markers or a .fold-main file.",
            schema_for_type::<RecordChangesInput>(),
        ),
        Tool::new(
            SYNC_WORKSPACE,
            "Merge the current main into your workspace. A checkpoint is taken first; a conflicting text file gets conflict markers, and any other conflict keeps your version with main's side beside it as <path>.fold-main.",
            schema_for_type::<SyncWorkspaceInput>(),
        ),
        Tool::new(
            ABANDON_WORKSPACE,
            "Give up your workspace: it closes, and the task goes back to Todo or Backlog. Its files and your claim stay.",
            schema_for_type::<AbandonWorkspaceInput>(),
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

/// Runs one workspace tool for the session's agent, acting as `as_worker` or the main worker. `None` when `tool` is not a workspace tool.
pub async fn call(
    server: &FoldServer,
    tool: &str,
    as_worker: Option<&WorkerName>,
    arguments: &JsonObject,
) -> Result<Option<Value>, FoldError> {
    let value = match tool {
        CREATE_WORKSPACE => {
            let input: CreateWorkspaceInput = parse(arguments)?;
            let task = id::<TaskId>(&input.task, "task", "T-12")?;
            let actor = actor(server, as_worker).await?;
            let view = server
                .workspaces
                .create_workspace(&actor, task)
                .await
                .map_err(from_workspace)?;
            json!({
                "workspace": view.id.to_string(),
                "path": view.path,
                "base": view.base.to_string(),
                "task": view.task.to_string(),
                "adopted": false,
            })
        }
        GET_WORKSPACE => {
            let input: GetWorkspaceInput = parse(arguments)?;
            let target = match (input.ws.as_deref(), input.task.as_deref()) {
                (Some(ws), None) => WorkspaceRef::Workspace(id(ws, "ws", "ws-9")?),
                (None, Some(task)) => WorkspaceRef::Task(id(task, "task", "T-12")?),
                _ => {
                    return Err(FoldError::new(
                        codes::INVALID_REQUEST,
                        "give exactly one of ws and task",
                    )
                    .with_hint("call get_workspace with ws or with task"));
                }
            };
            let details = server
                .workspaces
                .details(target)
                .await
                .map_err(from_workspace)?;
            let view = &details.workspace;
            json!({
                "workspace": view.id.to_string(),
                "state": view.state.as_str(),
                "path": view.path,
                "base": view.base.to_string(),
                "task": view.task.to_string(),
                "worker": view.worker.as_ref().map(WorkerName::as_str),
                "conflicted": details.conflicted,
                "changed_files": view.changed_files,
                "last_activity_at": view.last_activity_at,
                "latest_checkpoint": details.latest_checkpoint.map(|id| id.to_string()),
                "latest_changeset": details.latest_changeset.map(|id| id.to_string()),
            })
        }
        RECORD_CHECKPOINT => {
            let input: RecordCheckpointInput = parse(arguments)?;
            let workspace = id::<WorkspaceId>(&input.ws, "ws", "ws-9")?;
            let actor = actor(server, as_worker).await?;
            let checkpoint = server
                .workspaces
                .record_checkpoint(&actor, workspace, input.label.as_deref())
                .await
                .map_err(from_workspace)?;
            json!({ "checkpoint": checkpoint.id.to_string() })
        }
        LIST_CHECKPOINTS => {
            let input: ListCheckpointsInput = parse(arguments)?;
            let workspace = id::<WorkspaceId>(&input.ws, "ws", "ws-9")?;
            let checkpoints = server
                .workspaces
                .list_checkpoints(workspace)
                .await
                .map_err(from_workspace)?;
            let checkpoints: Vec<Value> = checkpoints
                .iter()
                .map(|checkpoint| {
                    json!({
                        "checkpoint": checkpoint.id.to_string(),
                        "kind": checkpoint.kind.as_str(),
                        "reason": checkpoint.reason.as_str(),
                        "label": checkpoint.label,
                        "created_at": checkpoint.created_at,
                    })
                })
                .collect();
            json!({ "checkpoints": checkpoints })
        }
        RESTORE_CHECKPOINT => {
            let input: RestoreCheckpointInput = parse(arguments)?;
            let checkpoint = id::<CheckpointId>(&input.ck, "ck", "ck-88")?;
            let actor = actor(server, as_worker).await?;
            let restored = server
                .workspaces
                .restore_checkpoint(&actor, checkpoint)
                .await
                .map_err(from_workspace)?;
            json!({
                "restored": restored.restored.to_string(),
                "safety": restored.safety.to_string(),
                "base": restored.base.to_string(),
            })
        }
        RECORD_CHANGES => {
            let input: RecordChangesInput = parse(arguments)?;
            let workspace = id::<WorkspaceId>(&input.ws, "ws", "ws-9")?;
            let actor = actor(server, as_worker).await?;
            let recorded = server
                .workspaces
                .record_changes(&actor, workspace, &input.message)
                .await
                .map_err(from_workspace)?;
            json!({
                "changeset": recorded.changeset.to_string(),
                "files": recorded.files,
            })
        }
        SYNC_WORKSPACE => {
            let input: SyncWorkspaceInput = parse(arguments)?;
            let workspace = id::<WorkspaceId>(&input.ws, "ws", "ws-9")?;
            let actor = actor(server, as_worker).await?;
            let synced = server
                .workspaces
                .sync_workspace(&actor, workspace)
                .await
                .map_err(from_workspace)?;
            json!({
                "base": synced.base.to_string(),
                "conflicted": synced.conflicted,
                "checkpoint": synced.checkpoint.map(|id| id.to_string()),
            })
        }
        ABANDON_WORKSPACE => {
            let input: AbandonWorkspaceInput = parse(arguments)?;
            let workspace = id::<WorkspaceId>(&input.ws, "ws", "ws-9")?;
            let actor = actor(server, as_worker).await?;
            let view = server
                .workspaces
                .abandon_workspace(&actor, workspace, &input.reason)
                .await
                .map_err(from_workspace)?;
            let status = server
                .workspaces
                .task_status(view.task)
                .await
                .map_err(from_workspace)?;
            json!({
                "workspace": view.id.to_string(),
                "task_status": status.as_str(),
            })
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}

async fn actor(server: &FoldServer, as_worker: Option<&WorkerName>) -> Result<Actor, FoldError> {
    let session = server.session().await;
    server
        .agents
        .actor(&session, as_worker)
        .await
        .map_err(from_agent)
}

fn id<T: FromStr>(text: &str, field: &'static str, example: &str) -> Result<T, FoldError> {
    text.parse().map_err(|_| {
        FoldError::new(
            codes::INVALID_ID,
            format!("{field} is not an id such as {example}"),
        )
        .with_details([("field", json!(field))])
    })
}
