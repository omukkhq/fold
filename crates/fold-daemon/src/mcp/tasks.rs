use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;

use fold_core::tasks::{PlanRef, PlannedChecklistItem, PlannedReview, PlannedTask};
use fold_core::{
    Actor, ChecklistItemId, ChecklistName, FoldError, HumanName, IdParseError, LabelColor,
    LabelName, StepText, TaskDescription, TaskId, TaskKind, TaskTitle, WorkerName,
};
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, Tool};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::error::{from_agent, from_task};
use super::parse;
use super::{FoldServer, FooterPart, FooterSource};
use crate::services::agents::{AgentService, SessionContext};
use crate::services::tasks::{
    ChecklistItemPatch, ClaimView, Claimed, CreateTask, DetailEdge, ItemContent, LabelView,
    NewChecklistItem, TaskDetail, TaskPatch, TaskService, item_content,
};

pub mod requests;

pub const GET_TASK_GRAPH: &str = "get_task_graph";
pub const GET_READY_TASKS: &str = "get_ready_tasks";
pub const GET_TASK: &str = "get_task";
pub const CREATE_TASK: &str = "create_task";
pub const CREATE_TASK_PLAN: &str = "create_task_plan";
pub const UPDATE_TASK: &str = "update_task";
pub const DELETE_TASK: &str = "delete_task";
pub const ARCHIVE_TASK: &str = "archive_task";
pub const ADD_DEPENDENCY: &str = "add_dependency";
pub const REMOVE_DEPENDENCY: &str = "remove_dependency";
pub const ADD_CHECKLIST_ITEM: &str = "add_checklist_item";
pub const UPDATE_CHECKLIST_ITEM: &str = "update_checklist_item";
pub const MOVE_CHECKLIST_ITEM: &str = "move_checklist_item";
pub const REMOVE_CHECKLIST_ITEM: &str = "remove_checklist_item";
pub const CREATE_LABEL: &str = "create_label";
pub const CLAIM_TASK: &str = "claim_task";
pub const RELEASE_TASK: &str = "release_task";
pub const TAKEOVER_CLAIM: &str = "takeover_claim";
pub const REPORT_TASK: &str = "report_task";
pub const COMPLETE_TASK: &str = "complete_task";
pub const SEND_BACK_TASK: &str = "send_back_task";
pub const SET_BLOCKER: &str = "set_blocker";
pub const CLEAR_BLOCKER: &str = "clear_blocker";

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetTaskGraphInput {
    /// Include Done tasks. Leave it out to see only the open work.
    #[serde(default)]
    pub include_done: bool,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AsOnlyInput {
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct TaskIdInput {
    /// The task, such as T-12.
    pub id: String,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateTaskInput {
    /// The title: one line, not empty.
    pub title: String,
    /// The description in Markdown. Leave it out for an empty one. In the text `#` names a fold item (#T-12) and `@` names a file (@web/src/board.ts); the daemon parses nothing.
    #[serde(default)]
    pub description: Option<String>,
    /// Names of existing labels to attach. Call create_label first for a new one.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Tasks the new task needs, such as T-3. Each adds one direct dependency.
    #[serde(default)]
    pub needs: Vec<String>,
    /// A task to split: the new task becomes a linked item of its Subtasks checklist, so the parent needs the new task. Only an agent that may change the parent's checklist passes it (checklist_denied).
    #[serde(default)]
    pub parent: Option<String>,
    /// proposal (the default), no_proposal or human. A no_proposal task is for work that changes no project file, such as designs made through an MCP server: its coder claims it, does the work without a workspace and calls report_task, and a lead check completes it. A human task is work only a person can do outside every agent, such as verifying an app with a provider: no agent claims it, and a human, or the main agent when the user says so, marks it Done with complete_task. Only the main agent, a human or the lead of the task's area creates a no_proposal or human task.
    #[serde(default)]
    pub kind: Option<String>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SetBlockerInput {
    /// The task, such as T-12.
    pub id: String,
    /// Why the task waits for a person, such as "Wait until the Meta app review passes": plain text, not empty, at most 500 characters.
    pub reason: String,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct TaskNoteInput {
    /// The task, such as T-12.
    pub id: String,
    /// What was done, or what to change: plain text, not empty.
    pub note: String,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct PlannedReviewInput {
    /// The registered human whose review is required. Omit to let any human review.
    #[serde(default)]
    pub reviewer: Option<String>,
    /// Why a human must review this task: not blank, at most 500 characters.
    pub reason: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct PlannedBlockerInput {
    /// Why the task waits for a person, such as "Wait until the Meta app review passes": plain text, not empty, at most 500 characters.
    pub reason: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct PlannedChecklistItemInput {
    /// The text of a plain step: one line, not empty.
    pub text: String,
    /// The checklist name. Leave it out for Subtasks; a name the task does not have creates that checklist.
    #[serde(default)]
    pub checklist: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct PlannedTaskInput {
    /// A unique plan-local key: 1 to 32 ASCII letters, digits, - or _, distinct from a public task id.
    pub key: String,
    /// The task title: one line, not empty.
    pub title: String,
    /// The task template: summary, Refs, Scope, Out of scope and testable Acceptance.
    pub description: String,
    /// Existing labels, including the label of the area that owns this task.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Prerequisites, each an existing task id such as T-3 or a key in this plan.
    #[serde(default)]
    pub needs: Vec<String>,
    /// The parent task's existing id or plan-local key. Creates its linked Subtasks item atomically.
    #[serde(default)]
    pub parent: Option<String>,
    /// proposal (the default), no_proposal for work that changes no project file, or human for work only a person can do outside every agent: no agent claims a human task, and a human, or the main agent when the user says so, marks it Done with complete_task. The same planners as for create_task may choose no_proposal or human.
    #[serde(default)]
    pub kind: Option<String>,
    /// The human review mark to set before workers can see this task.
    #[serde(default)]
    pub human_review: Option<PlannedReviewInput>,
    /// A blocker to set before workers can see this task, for agent work that waits until a person says it can start. Only a human or the main agent clears it with clear_blocker.
    #[serde(default)]
    pub blocker: Option<PlannedBlockerInput>,
    /// Plain checklist steps to create with the task, in order, so they exist before any worker sees it. Do not add them with add_checklist_item after the plan.
    #[serde(default)]
    pub checklist: Vec<PlannedChecklistItemInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(deny_unknown_fields)]
pub struct CreateTaskPlanInput {
    /// Between 1 and 100 tasks to publish together with their dependencies, parent links, human review marks and blockers. Only the main agent, a human or the lead of each task's area may publish them. Any refusal leaves the entire plan uncreated.
    pub tasks: Vec<PlannedTaskInput>,
    /// The worker that makes this call. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct UpdateTaskInput {
    /// The task, such as T-12.
    pub id: String,
    /// The new title. Leave it out to keep the title.
    #[serde(default)]
    pub title: Option<String>,
    /// The new description in Markdown. Leave it out to keep the description. In the text `#` names a fold item (#T-12) and `@` names a file (@web/src/board.ts); the daemon parses nothing.
    #[serde(default)]
    pub description: Option<String>,
    /// The whole new label set, by name. Leave it out to keep the labels; pass an empty list to remove them all.
    #[serde(default)]
    pub labels: Option<Vec<String>>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DependencyInput {
    /// The task that waits, such as T-12.
    pub task: String,
    /// The task it needs, such as T-3.
    pub needs: String,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AddChecklistItemInput {
    /// The task that owns the checklist, such as T-12.
    pub task: String,
    /// The text of a plain step. Give text or linked_task, not both.
    #[serde(default)]
    pub text: Option<String>,
    /// A child task to link, such as T-13. The task then needs the child, and fold ticks the item when the child is Done.
    #[serde(default)]
    pub linked_task: Option<String>,
    /// The checklist name. Leave it out for the task's first checklist; a name the task does not have creates that checklist.
    #[serde(default)]
    pub checklist: Option<String>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct UpdateChecklistItemInput {
    /// The checklist item, such as ci-4.
    pub id: String,
    /// The new text of a plain step. Leave it out to keep the text.
    #[serde(default)]
    pub text: Option<String>,
    /// Tick (true) or untick (false) a plain step. Linked items tick themselves.
    #[serde(default)]
    pub done: Option<bool>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct MoveChecklistItemInput {
    /// The checklist item, such as ci-4.
    pub id: String,
    /// The new place in the checklist, counted from 0. A number past the end puts the item last.
    pub position: u32,
    /// Another checklist of the same task to move the item into. Leave it out to stay in the item's checklist.
    #[serde(default)]
    pub checklist: Option<String>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ChecklistItemIdInput {
    /// The checklist item, such as ci-4.
    pub id: String,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateLabelInput {
    /// The label name: one line, not empty, unique in the project.
    pub name: String,
    /// One of sage, slate, teal, ink, gold, moss, plum, stone.
    pub color: String,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            GET_TASK_GRAPH,
            "Read the task graph: every task that is not archived with its status, blocked flag, kind, labels, the tasks it needs and its claim holder, and the reason of its blocker when it has one. No agent takes a human task or a task with a blocker.",
            schema_for_type::<GetTaskGraphInput>(),
        ),
        Tool::new(
            GET_READY_TASKS,
            "List the Todo tasks nobody has claimed, the one that unblocks the most tasks first. Human tasks and tasks with a blocker are left out: no agent takes them.",
            schema_for_type::<AsOnlyInput>(),
        ),
        Tool::new(
            GET_TASK,
            "Read one task: description, labels, checklists with progress, dependencies, dependents, claim, workspace, its blocker with the reason, and for a no_proposal task its reports and lead checks.",
            schema_for_type::<TaskIdInput>(),
        ),
        Tool::new(
            CREATE_TASK,
            "Create a task, with its dependencies and its parent link in the same step. A refused dependency, such as a cycle, creates nothing. Pass kind no_proposal for work that changes no project file, and kind human for work only a person can do.",
            schema_for_type::<CreateTaskInput>(),
        ),
        Tool::new(
            CREATE_TASK_PLAN,
            "Publish an approved plan atomically: create all tasks, dependencies, parent links, human review marks, blockers and plain checklist items before workers receive ready notices. Each task has kind proposal, no_proposal or human. A refusal creates nothing. Re-read the graph before retrying an uncertain result.",
            schema_for_type::<CreateTaskPlanInput>(),
        ),
        Tool::new(
            UPDATE_TASK,
            "Change the title, the description or the labels of a task.",
            schema_for_type::<UpdateTaskInput>(),
        ),
        Tool::new(
            DELETE_TASK,
            "Delete a task that never had a workspace, a proposal, a completion or a message. Use archive_task for a task with history.",
            schema_for_type::<TaskIdInput>(),
        ),
        Tool::new(
            ARCHIVE_TASK,
            "Hide a task from the board and remove its dependencies, keeping its history.",
            schema_for_type::<TaskIdInput>(),
        ),
        Tool::new(
            ADD_DEPENDENCY,
            "Make one task need another. An edge that would close a cycle is refused.",
            schema_for_type::<DependencyInput>(),
        ),
        Tool::new(
            REMOVE_DEPENDENCY,
            "Remove a direct dependency. A dependency from a linked checklist item goes only with its item.",
            schema_for_type::<DependencyInput>(),
        ),
        Tool::new(
            ADD_CHECKLIST_ITEM,
            "Add a plain step or a linked child task to a checklist of a task. On a held task only its holder's agent, its lead, the main agent or a human changes the checklist; on a task nobody holds, the lead of its area, the main agent or a human. Anyone else gets checklist_denied.",
            schema_for_type::<AddChecklistItemInput>(),
        ),
        Tool::new(
            UPDATE_CHECKLIST_ITEM,
            "Change the text of a plain step, or tick or untick it. A task coder ticks each step of its own task when the step is done and checked. On a held task only its holder's agent, its lead, the main agent or a human changes the checklist; on a task nobody holds, the lead of its area, the main agent or a human. Anyone else gets checklist_denied.",
            schema_for_type::<UpdateChecklistItemInput>(),
        ),
        Tool::new(
            MOVE_CHECKLIST_ITEM,
            "Move a checklist item to a new place in its checklist or into another checklist of the same task. The rights of update_checklist_item apply.",
            schema_for_type::<MoveChecklistItemInput>(),
        ),
        Tool::new(
            REMOVE_CHECKLIST_ITEM,
            "Remove a checklist item. Removing a linked item also removes its dependency. The rights of update_checklist_item apply.",
            schema_for_type::<ChecklistItemIdInput>(),
        ),
        Tool::new(
            CREATE_LABEL,
            "Create a label that tasks can carry.",
            schema_for_type::<CreateLabelInput>(),
        ),
        Tool::new(
            CLAIM_TASK,
            "Claim a task before you change files for it. A repeat call by the holder renews the claim. A workspace left by a worker that released the task becomes yours; the result names it. A human task (task_needs_human) and a task with a blocker (task_on_hold) are refused.",
            schema_for_type::<TaskIdInput>(),
        ),
        Tool::new(
            RELEASE_TASK,
            "Give up your claim on a task. Its workspace stays in place.",
            schema_for_type::<TaskIdInput>(),
        ),
        Tool::new(
            TAKEOVER_CLAIM,
            "Take over a claim whose lease has run out, when its holder has stopped working. The task's workspace moves to you with its files as they are; the result names it.",
            schema_for_type::<TaskIdInput>(),
        ),
        Tool::new(
            REPORT_TASK,
            "Report the work on a no_proposal task you claimed as finished, with a note on what you did. The task moves to Review and waits for a lead check; it has no workspace and no proposal.",
            schema_for_type::<TaskNoteInput>(),
        ),
        Tool::new(
            COMPLETE_TASK,
            "Complete a reported no_proposal task after you checked the work, or mark a human task Done: the task is Done and its dependents unblock. Only the lead of the task's area, the main agent or a human checks a no_proposal task. Only a human or the main agent marks a human task Done, and the main agent only when the user says the work is done. Refused with prerequisites_not_done while a prerequisite is not Done, and with human_approval_missing on a task with the human review mark, which only a human completes.",
            schema_for_type::<TaskIdInput>(),
        ),
        Tool::new(
            SEND_BACK_TASK,
            "Send a reported no_proposal task back to its coder with a note on what to change. The task returns to In Progress and fold resumes the coder with the note.",
            schema_for_type::<TaskNoteInput>(),
        ),
        Tool::new(
            SET_BLOCKER,
            "Put a blocker on a task with the reason it waits for a person, or change the reason. Only a task in Backlog or Todo that no agent has started gets one: a task with a claim, a workspace or an open proposal is refused with task_hold_denied. While it is set, no agent claims the task, no lead starts a coder for it and get_ready_tasks leaves it out; the status does not change. Only a human, the main agent or the lead of the task's area sets one.",
            schema_for_type::<SetBlockerInput>(),
        ),
        Tool::new(
            CLEAR_BLOCKER,
            "Remove the blocker of a task, so agents can take it again. Only a human or the main agent clears one, and the main agent only when the user says the task can start.",
            schema_for_type::<TaskIdInput>(),
        ),
    ]
}

fn id<T>(text: &str) -> Result<T, FoldError>
where
    T: FromStr<Err = IdParseError>,
{
    text.parse().map_err(FoldError::from)
}

fn label_names(labels: &[String]) -> Result<Vec<LabelName>, FoldError> {
    labels
        .iter()
        .map(|name| LabelName::parse(name).map_err(FoldError::from))
        .collect()
}

fn labels_json(labels: &[LabelView]) -> Vec<&str> {
    labels.iter().map(|label| label.name.as_str()).collect()
}

fn claim_json(task: TaskId, holder: Option<&WorkerName>) -> Value {
    json!({"id": task, "holder": holder.map(WorkerName::as_str)})
}

/// The claim with the workspace the holder works in, adopted or already its own; `null` when it has none yet.
fn claimed_json(claimed: &Claimed) -> Value {
    let mut value = claim_json(claimed.claim.task, Some(&claimed.claim.holder));
    value["workspace"] = json!(claimed.workspace.map(|ws| ws.to_string()));
    value
}

fn edge_json(edge: &DetailEdge) -> Value {
    json!({
        "id": edge.task.id,
        "title": edge.task.title.as_str(),
        "status": edge.task.status.as_str(),
        "origin": edge.origin.as_str(),
    })
}

fn detail_json(detail: &TaskDetail) -> Value {
    let task = &detail.task;
    let checklists: Vec<Value> = detail
        .checklists
        .iter()
        .map(|list| {
            let items: Vec<Value> = list
                .items
                .iter()
                .map(|entry| {
                    let item = &entry.item;
                    let done = item.done_at.is_some();
                    match &item.content {
                        ItemContent::Step(text) => {
                            json!({"id": item.id, "text": text.as_str(), "done": done})
                        }
                        ItemContent::Linked(child) => json!({
                            "id": item.id,
                            "linked_task": child,
                            "status": entry.linked.as_ref().map(|linked| linked.status.as_str()),
                            "done": done,
                        }),
                    }
                })
                .collect();
            json!({"name": list.name.as_str(), "items": items})
        })
        .collect();
    let progress = &detail.progress;
    let mut value = json!({
        "id": task.id,
        "title": task.title.as_str(),
        "description": task.description.as_str(),
        "status": task.status.as_str(),
        "blocked": task.blocked,
        "kind": task.kind.as_str(),
        "archived": task.archived_at.is_some(),
        "labels": labels_json(&task.labels),
        "checklists": checklists,
        "progress": {
            "done": progress.steps_done + progress.linked_done,
            "total": progress.steps_total + progress.linked_total,
        },
        "needs": detail.needs.iter().map(edge_json).collect::<Vec<_>>(),
        "needed_by": detail.needed_by.iter().map(edge_json).collect::<Vec<_>>(),
        "claim": detail.claim.as_ref().map(|claim: &ClaimView| json!({
            "holder": claim.holder.as_str(),
            "stale": claim.stale,
            "renewed_at": claim.renewed_at,
        })),
        "workspace": detail.workspace,
        "human_review": task.human_review.as_ref().map(|mark| json!({
            "reviewer": mark.reviewer.as_ref().map(HumanName::as_str),
            "reason": mark.reason,
            "set_by": mark.set_by.as_str(),
            "set_at": detail.human_review_set_at,
        })),
        "blocker": task.hold.as_ref().map(|hold| json!({
            "reason": hold.reason.as_str(),
            "set_by": hold.set_by.as_str(),
            "set_at": hold.set_at,
        })),
        "status_reason": detail.status_reason,
        "workspace_info": detail.workspace_info.as_ref().map(|info| json!({
            "directory": info.directory,
            "base": info.base,
            "main": info.main,
            "behind": info.behind,
        })),
        "changesets": detail.changesets.iter().map(|changeset| json!({
            "id": changeset.id,
            "message": changeset.message,
        })).collect::<Vec<_>>(),
        "proposal": detail.proposal.as_ref().map(|proposal| json!({
            "id": proposal.id,
            "state": proposal.standing.as_str(),
            "version": proposal.version,
        })),
        "integration": detail.integration.as_ref().map(|integration| json!({
            "revision": integration.revision,
            "proposal": integration.proposal,
            "by": integration.by,
        })),
        "history": detail.history.iter().map(|event| json!({
            "seq": event.seq,
            "type": event.event_type.as_str(),
            "actor": event.actor.to_string(),
        })).collect::<Vec<_>>(),
    });
    if task.kind == TaskKind::NoProposal {
        value["reports"] = detail
            .reports
            .iter()
            .map(|report| {
                json!({
                    "by": report.by.as_str(),
                    "note": report.note,
                    "resolution": report.resolution.map(|resolution| resolution.as_str()),
                    "resolved_by": report.resolved_by,
                    "send_back_note": report.send_back_note,
                })
            })
            .collect();
    }
    value
}

/// Runs a task or request tool acting as `as_worker`. `None` when `tool` is neither.
pub async fn call(
    server: &FoldServer,
    tool: &str,
    as_worker: Option<&WorkerName>,
    arguments: &JsonObject,
) -> Result<Option<Value>, FoldError> {
    let actor = server.actor(as_worker).await?;
    if let Some(value) = requests::run(server, tool, &actor, arguments).await? {
        return Ok(Some(value));
    }
    run(server, tool, &actor, arguments).await
}

async fn run(
    server: &FoldServer,
    tool: &str,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Option<Value>, FoldError> {
    let value = match tool {
        GET_TASK_GRAPH => get_task_graph(server, arguments).await?,
        GET_READY_TASKS => get_ready_tasks(server, arguments).await?,
        GET_TASK => get_task(server, arguments).await?,
        CREATE_TASK => create_task(server, actor, arguments).await?,
        CREATE_TASK_PLAN => create_task_plan(server, actor, arguments).await?,
        UPDATE_TASK => update_task(server, actor, arguments).await?,
        DELETE_TASK => delete_task(server, actor, arguments).await?,
        ARCHIVE_TASK => archive_task(server, actor, arguments).await?,
        ADD_DEPENDENCY => add_dependency(server, actor, arguments).await?,
        REMOVE_DEPENDENCY => remove_dependency(server, actor, arguments).await?,
        ADD_CHECKLIST_ITEM => add_checklist_item(server, actor, arguments).await?,
        UPDATE_CHECKLIST_ITEM => update_checklist_item(server, actor, arguments).await?,
        MOVE_CHECKLIST_ITEM => move_checklist_item(server, actor, arguments).await?,
        REMOVE_CHECKLIST_ITEM => remove_checklist_item(server, actor, arguments).await?,
        CREATE_LABEL => create_label(server, actor, arguments).await?,
        CLAIM_TASK => claim_task(server, actor, arguments).await?,
        RELEASE_TASK => release_task(server, actor, arguments).await?,
        TAKEOVER_CLAIM => takeover_claim(server, actor, arguments).await?,
        REPORT_TASK => report_task(server, actor, arguments).await?,
        COMPLETE_TASK => complete_task(server, actor, arguments).await?,
        SEND_BACK_TASK => send_back_task(server, actor, arguments).await?,
        SET_BLOCKER => set_blocker(server, actor, arguments).await?,
        CLEAR_BLOCKER => clear_blocker(server, actor, arguments).await?,
        _ => return Ok(None),
    };
    Ok(Some(value))
}

async fn get_task_graph(server: &FoldServer, arguments: &JsonObject) -> Result<Value, FoldError> {
    let tasks = &server.tasks;
    let input: GetTaskGraphInput = parse(arguments)?;
    let graph = tasks.graph(input.include_done).await.map_err(from_task)?;
    let tasks: Vec<Value> = graph
        .iter()
        .map(|task| {
            let mut node = json!({
                "id": task.id,
                "title": task.title.as_str(),
                "status": task.status.as_str(),
                "blocked": task.blocked,
                "kind": task.kind.as_str(),
                "labels": labels_json(&task.labels),
                "needs": task.needs,
                "holder": task.holder.as_ref().map(WorkerName::as_str),
            });
            if let Some(hold) = &task.hold {
                node["blocker"] = json!({"reason": hold.reason.as_str()});
            }
            node
        })
        .collect();
    Ok(json!({"tasks": tasks}))
}

async fn get_ready_tasks(server: &FoldServer, arguments: &JsonObject) -> Result<Value, FoldError> {
    let tasks = &server.tasks;
    let _: AsOnlyInput = parse(arguments)?;
    let ready = tasks.ready().await.map_err(from_task)?;
    let tasks: Vec<Value> = ready
        .iter()
        .map(|task| {
            json!({
                "id": task.id,
                "title": task.title.as_str(),
                "labels": labels_json(&task.labels),
                "unblocks": task.unblocks,
            })
        })
        .collect();
    Ok(json!({"tasks": tasks}))
}

async fn get_task(server: &FoldServer, arguments: &JsonObject) -> Result<Value, FoldError> {
    let tasks = &server.tasks;
    let input: TaskIdInput = parse(arguments)?;
    let detail = tasks.get_view(id(&input.id)?).await.map_err(from_task)?;
    Ok(detail_json(&detail))
}

async fn create_task(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let tasks = &server.tasks;
    let input: CreateTaskInput = parse(arguments)?;
    let create = CreateTask {
        title: TaskTitle::parse(&input.title)?,
        description: TaskDescription::new(input.description.unwrap_or_default()),
        labels: label_names(&input.labels)?,
    };
    let needs = input
        .needs
        .iter()
        .map(|need| id(need))
        .collect::<Result<Vec<TaskId>, _>>()?;
    let parent = input.parent.as_deref().map(id).transpose()?;
    let kind = match input.kind.as_deref() {
        Some(kind) => kind.parse::<TaskKind>()?,
        None => TaskKind::Proposal,
    };
    let view = tasks
        .create_task_of_kind(actor, create, kind, &needs, parent)
        .await
        .map_err(from_task)?;
    Ok(json!({"id": view.id, "status": view.status.as_str()}))
}

async fn create_task_plan(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: CreateTaskPlanInput = parse(arguments)?;
    let plan = input
        .tasks
        .into_iter()
        .map(|task| {
            Ok(PlannedTask {
                key: task.key,
                title: TaskTitle::parse(&task.title)?,
                description: TaskDescription::new(task.description),
                labels: label_names(&task.labels)?,
                kind: task
                    .kind
                    .as_deref()
                    .map(str::parse)
                    .transpose()?
                    .unwrap_or(TaskKind::Proposal),
                needs: task
                    .needs
                    .iter()
                    .map(|value| PlanRef::parse(value))
                    .collect(),
                parent: task.parent.as_deref().map(PlanRef::parse),
                human_review: task
                    .human_review
                    .map(|review| {
                        Ok::<_, FoldError>(PlannedReview {
                            reviewer: review
                                .reviewer
                                .as_deref()
                                .map(HumanName::parse)
                                .transpose()?,
                            reason: review.reason,
                        })
                    })
                    .transpose()?,
                blocker: task.blocker.map(|blocker| blocker.reason),
                checklist: task
                    .checklist
                    .into_iter()
                    .map(|item| PlannedChecklistItem {
                        text: item.text,
                        checklist: item.checklist,
                    })
                    .collect(),
            })
        })
        .collect::<Result<Vec<_>, FoldError>>()?;
    let registry = server
        .reviews
        .as_ref()
        .and_then(crate::services::reviews::ReviewService::registry);
    let created = server
        .tasks
        .create_task_plan(actor, plan, registry)
        .await
        .map_err(from_task)?;
    Ok(
        json!({"tasks": created.into_iter().map(|(key, task)| json!({"key": key, "id": task.id, "status": task.status.as_str()})).collect::<Vec<_>>()}),
    )
}

async fn update_task(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let tasks = &server.tasks;
    let input: UpdateTaskInput = parse(arguments)?;
    let patch = TaskPatch {
        title: input.title.as_deref().map(TaskTitle::parse).transpose()?,
        description: input.description.map(TaskDescription::new),
        labels: input.labels.as_deref().map(label_names).transpose()?,
    };
    let view = tasks
        .update_task(actor, id(&input.id)?, patch)
        .await
        .map_err(from_task)?;
    Ok(json!({"id": view.id}))
}

async fn delete_task(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let tasks = &server.tasks;
    let input: TaskIdInput = parse(arguments)?;
    let task: TaskId = id(&input.id)?;
    tasks.delete_task(actor, task).await.map_err(from_task)?;
    Ok(json!({"id": task}))
}

async fn archive_task(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let tasks = &server.tasks;
    let input: TaskIdInput = parse(arguments)?;
    let view = tasks
        .archive_task(actor, id(&input.id)?)
        .await
        .map_err(from_task)?;
    Ok(json!({"id": view.id}))
}

async fn add_dependency(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let tasks = &server.tasks;
    let input: DependencyInput = parse(arguments)?;
    let edge = tasks
        .add_dependency(actor, id(&input.task)?, id(&input.needs)?)
        .await
        .map_err(from_task)?;
    Ok(json!({"task": edge.task, "needs": edge.needs}))
}

async fn remove_dependency(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let tasks = &server.tasks;
    let input: DependencyInput = parse(arguments)?;
    let (task, needs): (TaskId, TaskId) = (id(&input.task)?, id(&input.needs)?);
    tasks
        .remove_dependency(actor, task, needs)
        .await
        .map_err(from_task)?;
    Ok(json!({"task": task, "needs": needs}))
}

async fn add_checklist_item(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let tasks = &server.tasks;
    let input: AddChecklistItemInput = parse(arguments)?;
    let item = NewChecklistItem {
        content: item_content(input.text.as_deref(), input.linked_task.as_deref())?,
        checklist: input
            .checklist
            .as_deref()
            .map(ChecklistName::parse)
            .transpose()?,
    };
    let view = tasks
        .add_checklist_item(actor, id(&input.task)?, item)
        .await
        .map_err(from_task)?;
    Ok(json!({"item": view.id, "checklist": view.checklist.as_str()}))
}

async fn update_checklist_item(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let tasks = &server.tasks;
    let input: UpdateChecklistItemInput = parse(arguments)?;
    let patch = ChecklistItemPatch {
        text: input.text.as_deref().map(StepText::parse).transpose()?,
        done: input.done,
    };
    let view = tasks
        .update_checklist_item(actor, id(&input.id)?, patch)
        .await
        .map_err(from_task)?;
    Ok(json!({"item": view.id}))
}

async fn move_checklist_item(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let tasks = &server.tasks;
    let input: MoveChecklistItemInput = parse(arguments)?;
    let checklist = input
        .checklist
        .as_deref()
        .map(ChecklistName::parse)
        .transpose()?;
    let item: ChecklistItemId = id(&input.id)?;
    let view = tasks
        .move_checklist_item(actor, item, Some(input.position), checklist)
        .await
        .map_err(from_task)?;
    Ok(json!({"item": view.id, "checklist": view.checklist.as_str()}))
}

async fn remove_checklist_item(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let tasks = &server.tasks;
    let input: ChecklistItemIdInput = parse(arguments)?;
    let item: ChecklistItemId = id(&input.id)?;
    tasks
        .remove_checklist_item(actor, item)
        .await
        .map_err(from_task)?;
    Ok(json!({"item": item}))
}

async fn create_label(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let tasks = &server.tasks;
    let input: CreateLabelInput = parse(arguments)?;
    let color: LabelColor = input.color.parse()?;
    let label = tasks
        .create_label(actor, LabelName::parse(&input.name)?, color)
        .await
        .map_err(from_task)?;
    Ok(json!({"label": label.name.as_str()}))
}

async fn claim_task(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let tasks = &server.tasks;
    let input: TaskIdInput = parse(arguments)?;
    let claimed = tasks
        .claim_task(actor, id(&input.id)?)
        .await
        .map_err(from_task)?;
    Ok(claimed_json(&claimed))
}

async fn release_task(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let tasks = &server.tasks;
    let input: TaskIdInput = parse(arguments)?;
    let task: TaskId = id(&input.id)?;
    tasks.release_task(actor, task).await.map_err(from_task)?;
    Ok(claim_json(task, None))
}

async fn takeover_claim(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let tasks = &server.tasks;
    let input: TaskIdInput = parse(arguments)?;
    let claimed = tasks
        .takeover_claim(actor, id(&input.id)?)
        .await
        .map_err(from_task)?;
    Ok(claimed_json(&claimed))
}

/// The `claims near stale` part of the footer: the unreleased claims of every worker of the calling agent that are near the end of their lease. Every recorded call renews those claims, so the count is taken as the call arrives: it tells an agent that it went quiet long enough for its claims to near stale, even though the call itself renews them.
pub struct ClaimsNearStaleFooter {
    agents: AgentService,
    tasks: TaskService,
}

impl ClaimsNearStaleFooter {
    pub fn new(agents: AgentService, tasks: TaskService) -> Self {
        Self { agents, tasks }
    }
}

impl FooterSource for ClaimsNearStaleFooter {
    fn part(&self) -> FooterPart {
        FooterPart::ClaimsNearStale
    }

    fn before_call(&self) -> bool {
        true
    }

    fn count<'a>(
        &'a self,
        session: &'a SessionContext,
    ) -> Pin<Box<dyn Future<Output = u64> + Send + 'a>> {
        Box::pin(async move {
            if session.agent.is_none() {
                return 0;
            }
            let agent = match self.agents.require_agent(session).await {
                Ok(agent) => agent,
                Err(error) => {
                    tracing::warn!(
                        code = from_agent(error).code(),
                        "could not count claims near stale"
                    );
                    return 0;
                }
            };
            match self.tasks.claims_near_stale(agent.id).await {
                Ok(count) => count,
                Err(error) => {
                    tracing::warn!(
                        code = from_task(error).code(),
                        "could not count claims near stale"
                    );
                    0
                }
            }
        })
    }
}

async fn report_task(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: TaskNoteInput = parse(arguments)?;
    let view = server
        .tasks
        .report_task(actor, id(&input.id)?, &input.note)
        .await
        .map_err(from_task)?;
    Ok(json!({"id": view.id, "status": view.status.as_str()}))
}

async fn complete_task(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: TaskIdInput = parse(arguments)?;
    let view = server
        .tasks
        .complete_task(actor, id(&input.id)?)
        .await
        .map_err(from_task)?;
    Ok(json!({"id": view.id, "status": view.status.as_str()}))
}

async fn set_blocker(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: SetBlockerInput = parse(arguments)?;
    let view = server
        .tasks
        .set_hold(actor, id(&input.id)?, &input.reason)
        .await
        .map_err(from_task)?;
    Ok(json!({"id": view.id, "blocker": view.hold.is_some()}))
}

async fn clear_blocker(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: TaskIdInput = parse(arguments)?;
    let view = server
        .tasks
        .clear_hold(actor, id(&input.id)?)
        .await
        .map_err(from_task)?;
    Ok(json!({"id": view.id, "blocker": view.hold.is_some()}))
}

async fn send_back_task(
    server: &FoldServer,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: TaskNoteInput = parse(arguments)?;
    let view = server
        .tasks
        .send_back_task(actor, id(&input.id)?, &input.note)
        .await
        .map_err(from_task)?;
    Ok(json!({"id": view.id, "status": view.status.as_str()}))
}
