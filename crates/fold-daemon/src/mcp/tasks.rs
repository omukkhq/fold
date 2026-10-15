use std::str::FromStr;

use fold_core::{
    Actor, ChecklistItemId, ChecklistName, FoldError, HumanName, IdParseError, LabelColor,
    LabelName, StepText, TaskDescription, TaskId, TaskTitle, WorkerName,
};
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, Tool};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::FoldServer;
use super::error::{from_agent, from_task};
use super::identity::parse;
use crate::services::tasks::{
    ChecklistItemPatch, ClaimView, Claimed, CreateTask, DetailEdge, ItemContent, LabelView,
    NewChecklistItem, TaskDetail, TaskPatch,
};

pub const GET_TASK_GRAPH: &str = "get_task_graph";
pub const GET_READY_TASKS: &str = "get_ready_tasks";
pub const GET_TASK: &str = "get_task";
pub const CREATE_TASK: &str = "create_task";
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
    /// The description in Markdown. Leave it out for an empty one.
    #[serde(default)]
    pub description: Option<String>,
    /// Names of existing labels to attach. Call create_label first for a new one.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Tasks the new task needs, such as T-3. Each adds one direct dependency.
    #[serde(default)]
    pub needs: Vec<String>,
    /// A task to split: the new task becomes a linked item of its Subtasks checklist, so the parent needs the new task.
    #[serde(default)]
    pub parent: Option<String>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
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
    /// The new description in Markdown. Leave it out to keep the description.
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
            "Read the task graph: every task that is not archived with its status, blocked flag, labels, the tasks it needs and its claim holder.",
            schema_for_type::<GetTaskGraphInput>(),
        ),
        Tool::new(
            GET_READY_TASKS,
            "List the Todo tasks nobody has claimed, the one that unblocks the most tasks first.",
            schema_for_type::<AsOnlyInput>(),
        ),
        Tool::new(
            GET_TASK,
            "Read one task: description, labels, checklists with progress, dependencies, dependents, claim and workspace.",
            schema_for_type::<TaskIdInput>(),
        ),
        Tool::new(
            CREATE_TASK,
            "Create a task, with its dependencies and its parent link in the same step. A refused dependency, such as a cycle, creates nothing.",
            schema_for_type::<CreateTaskInput>(),
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
            "Add a plain step or a linked child task to a checklist of a task.",
            schema_for_type::<AddChecklistItemInput>(),
        ),
        Tool::new(
            UPDATE_CHECKLIST_ITEM,
            "Change the text of a plain step, or tick or untick it.",
            schema_for_type::<UpdateChecklistItemInput>(),
        ),
        Tool::new(
            MOVE_CHECKLIST_ITEM,
            "Move a checklist item to a new place in its checklist or into another checklist of the same task.",
            schema_for_type::<MoveChecklistItemInput>(),
        ),
        Tool::new(
            REMOVE_CHECKLIST_ITEM,
            "Remove a checklist item. Removing a linked item also removes its dependency.",
            schema_for_type::<ChecklistItemIdInput>(),
        ),
        Tool::new(
            CREATE_LABEL,
            "Create a label that tasks can carry.",
            schema_for_type::<CreateLabelInput>(),
        ),
        Tool::new(
            CLAIM_TASK,
            "Claim a task before you change files for it. A repeat call by the holder renews the claim. A workspace left by a worker that released the task becomes yours; the result names it.",
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
    json!({
        "id": task.id,
        "title": task.title.as_str(),
        "description": task.description.as_str(),
        "status": task.status.as_str(),
        "blocked": task.blocked,
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
        })),
    })
}

/// Runs a task tool acting as `as_worker`. `None` when `tool` is not a task tool.
pub async fn call(
    server: &FoldServer,
    tool: &str,
    as_worker: Option<&WorkerName>,
    arguments: &JsonObject,
) -> Result<Option<Value>, FoldError> {
    if !tools().iter().any(|known| known.name == tool) {
        return Ok(None);
    }
    let session = server.session().await;
    let actor = server
        .agents
        .actor(&session, as_worker)
        .await
        .map_err(from_agent)?;
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
            json!({
                "id": task.id,
                "title": task.title.as_str(),
                "status": task.status.as_str(),
                "blocked": task.blocked,
                "labels": labels_json(&task.labels),
                "needs": task.needs,
                "holder": task.holder.as_ref().map(WorkerName::as_str),
            })
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
    let view = tasks
        .create_linked_task(actor, create, &needs, parent)
        .await
        .map_err(from_task)?;
    Ok(json!({"id": view.id, "status": view.status.as_str()}))
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
    let content = match (input.text, input.linked_task) {
        (Some(text), None) => ItemContent::Step(StepText::parse(&text)?),
        (None, Some(child)) => ItemContent::Linked(id(&child)?),
        _ => {
            return Err(FoldError::new(
                fold_core::codes::INVALID_REQUEST,
                "give text or linked_task, not both",
            )
            .with_hint(
                "call add_checklist_item with text for a step or linked_task for a child task",
            ));
        }
    };
    let item = NewChecklistItem {
        content,
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
        .move_checklist_item(actor, item, input.position, checklist)
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
