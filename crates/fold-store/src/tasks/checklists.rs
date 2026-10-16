use fold_core::{ChecklistItemId, ChecklistName, EventActor, StepText, TaskId, Timestamp};
use sqlx::SqliteConnection;

use crate::error::StoreError;
use crate::store::WriteTx;

/// What an item holds: a plain step's text or the task it links. The table's `checklist_items_text_or_link` check keeps exactly one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemContent {
    Step(StepText),
    Linked(TaskId),
}

/// One checklist item with its owner task and checklist, by public ids and names only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemRow {
    pub id: ChecklistItemId,
    pub task: TaskId,
    pub checklist: ChecklistName,
    pub content: ItemContent,
    pub done_at: Option<Timestamp>,
    pub done_by: Option<String>,
    pub created_at: Timestamp,
}

/// The values of a new item. It goes after the last item of its checklist.
#[derive(Debug, Clone)]
pub struct NewItem<'a> {
    pub id: ChecklistItemId,
    pub task: TaskId,
    pub checklist: &'a ChecklistName,
    pub content: &'a ItemContent,
    pub now: Timestamp,
}

struct RawItem {
    number: i64,
    task: i64,
    checklist: String,
    text: Option<String>,
    linked: Option<i64>,
    done_at: Option<i64>,
    done_by: Option<String>,
    created_at: i64,
}

impl RawItem {
    fn into_row(self) -> Result<ItemRow, StoreError> {
        let content = match (self.text, self.linked) {
            (Some(text), None) => ItemContent::Step(
                StepText::parse(&text).map_err(|_| StoreError::Corrupt("checklist_items.text"))?,
            ),
            (None, Some(linked)) => ItemContent::Linked(
                TaskId::try_from_db(linked).ok_or(StoreError::Corrupt("tasks.number"))?,
            ),
            _ => return Err(StoreError::Corrupt("checklist_items.text")),
        };
        Ok(ItemRow {
            id: ChecklistItemId::try_from_db(self.number)
                .ok_or(StoreError::Corrupt("checklist_items.number"))?,
            task: TaskId::try_from_db(self.task).ok_or(StoreError::Corrupt("tasks.number"))?,
            checklist: checklist_name(&self.checklist)?,
            content,
            done_at: self
                .done_at
                .map(|millis| {
                    Timestamp::from_millis(millis)
                        .map_err(|_| StoreError::Corrupt("checklist_items.done_at"))
                })
                .transpose()?,
            done_by: self.done_by,
            created_at: Timestamp::from_millis(self.created_at)
                .map_err(|_| StoreError::Corrupt("checklist_items.created_at"))?,
        })
    }
}

fn checklist_name(text: &str) -> Result<ChecklistName, StoreError> {
    ChecklistName::parse(text).map_err(|_| StoreError::Corrupt("checklists.name"))
}

/// The names of the checklists of `task`, in position order.
pub async fn checklists(
    conn: &mut SqliteConnection,
    task: TaskId,
) -> Result<Vec<ChecklistName>, StoreError> {
    let number = task.to_db();
    sqlx::query_scalar!(
        "SELECT c.name FROM checklists c JOIN tasks t ON t.id = c.task_id
         WHERE t.number = ? ORDER BY c.position",
        number
    )
    .fetch_all(conn)
    .await?
    .iter()
    .map(|name| checklist_name(name))
    .collect()
}

/// Creates the checklist `name` on `task` after its last checklist and returns true, or returns false when the task already has a checklist of that name. A task that does not exist also returns false with nothing inserted, so the caller checks the task first.
pub async fn ensure_checklist(
    tx: &mut WriteTx,
    task: TaskId,
    name: &ChecklistName,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let number = task.to_db();
    let name = name.as_str();
    let now = now.as_millis();
    let result = sqlx::query!(
        "INSERT INTO checklists (task_id, name, position, created_at)
         SELECT t.id, ?2, COALESCE((SELECT MAX(c.position) + 1 FROM checklists c WHERE c.task_id = t.id), 0), ?3
         FROM tasks t WHERE t.number = ?1
         ON CONFLICT (task_id, name) DO NOTHING",
        number,
        name,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Inserts an unticked item at the end of its checklist. A task or checklist that does not exist inserts nothing and fails with `StoreError::Invalid`; a linked task that does not exist fails on the foreign key.
pub async fn insert_item(tx: &mut WriteTx, item: &NewItem<'_>) -> Result<(), StoreError> {
    let number = item.id.to_db();
    let task = item.task.to_db();
    let checklist = item.checklist.as_str();
    let (text, linked) = match item.content {
        ItemContent::Step(text) => (Some(text.as_str()), None),
        ItemContent::Linked(linked) => (None, Some(linked.to_db())),
    };
    let now = item.now.as_millis();
    let result = sqlx::query!(
        "INSERT INTO checklist_items (number, checklist_id, text, linked_task_id, position, created_at)
         SELECT ?1, c.id, ?4, (SELECT l.id FROM tasks l WHERE l.number = ?5),
                COALESCE((SELECT MAX(i.position) + 1 FROM checklist_items i WHERE i.checklist_id = c.id), 0), ?6
         FROM checklists c JOIN tasks t ON t.id = c.task_id
         WHERE t.number = ?2 AND c.name = ?3",
        number,
        task,
        checklist,
        text,
        linked,
        now
    )
    .execute(tx.conn())
    .await?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid(
            "the task or the checklist does not exist",
        ))
    }
}

pub async fn fetch_item(
    conn: &mut SqliteConnection,
    item: ChecklistItemId,
) -> Result<Option<ItemRow>, StoreError> {
    let number = item.to_db();
    sqlx::query_as!(
        RawItem,
        r#"SELECT i.number, t.number AS "task!: i64", c.name AS checklist, i.text,
                  l.number AS "linked?: i64", i.done_at, i.done_by, i.created_at
           FROM checklist_items i
           JOIN checklists c ON c.id = i.checklist_id
           JOIN tasks t ON t.id = c.task_id
           LEFT JOIN tasks l ON l.id = i.linked_task_id
           WHERE i.number = ?"#,
        number
    )
    .fetch_optional(conn)
    .await?
    .map(RawItem::into_row)
    .transpose()
}

/// Every item of `task`, by checklist position, then item position.
pub async fn items(conn: &mut SqliteConnection, task: TaskId) -> Result<Vec<ItemRow>, StoreError> {
    let number = task.to_db();
    sqlx::query_as!(
        RawItem,
        r#"SELECT i.number, t.number AS "task!: i64", c.name AS checklist, i.text,
                  l.number AS "linked?: i64", i.done_at, i.done_by, i.created_at
           FROM checklist_items i
           JOIN checklists c ON c.id = i.checklist_id
           JOIN tasks t ON t.id = c.task_id
           LEFT JOIN tasks l ON l.id = i.linked_task_id
           WHERE t.number = ?
           ORDER BY c.position, i.position"#,
        number
    )
    .fetch_all(conn)
    .await?
    .into_iter()
    .map(RawItem::into_row)
    .collect()
}

/// Replaces the text of a plain step. A linked item or an unknown number fails with `StoreError::Invalid`.
pub async fn set_text(
    tx: &mut WriteTx,
    item: ChecklistItemId,
    text: &StepText,
) -> Result<(), StoreError> {
    let number = item.to_db();
    let text = text.as_str();
    let result = sqlx::query!(
        "UPDATE checklist_items SET text = ?2 WHERE number = ?1 AND text IS NOT NULL",
        number,
        text
    )
    .execute(tx.conn())
    .await?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid("the plain step does not exist"))
    }
}

/// Ticks the item with the time and the actor that ticked it, or unticks it with `None`.
pub async fn set_done(
    tx: &mut WriteTx,
    item: ChecklistItemId,
    done: Option<(Timestamp, &str)>,
) -> Result<(), StoreError> {
    let number = item.to_db();
    let (done_at, done_by) = match done {
        Some((at, by)) => (Some(at.as_millis()), Some(by)),
        None => (None, None),
    };
    let result = sqlx::query!(
        "UPDATE checklist_items SET done_at = ?2, done_by = ?3 WHERE number = ?1",
        number,
        done_at,
        done_by
    )
    .execute(tx.conn())
    .await?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid("the checklist item does not exist"))
    }
}

/// Deletes the item and returns whether it existed. The edge a linked item made is not touched.
pub async fn delete_item(tx: &mut WriteTx, item: ChecklistItemId) -> Result<bool, StoreError> {
    let number = item.to_db();
    let result = sqlx::query!("DELETE FROM checklist_items WHERE number = ?", number)
        .execute(tx.conn())
        .await?;
    Ok(result.rows_affected() == 1)
}

/// Ticks every unticked linked item of `task` whose child's stored status is `done`, with `done_by = fold`, and returns each ticked item with its child in item order. Done is final, so a tick made here is never undone.
pub async fn tick_done_children(
    tx: &mut WriteTx,
    task: TaskId,
    now: Timestamp,
) -> Result<Vec<(ChecklistItemId, TaskId)>, StoreError> {
    let number = task.to_db();
    let due = sqlx::query!(
        r#"SELECT i.number AS "item!: i64", l.number AS "linked!: i64"
           FROM checklist_items i
           JOIN checklists c ON c.id = i.checklist_id
           JOIN tasks t ON t.id = c.task_id
           JOIN tasks l ON l.id = i.linked_task_id
           WHERE t.number = ? AND i.done_at IS NULL AND l.status = 'done'
           ORDER BY c.position, i.position"#,
        number
    )
    .fetch_all(tx.conn())
    .await?;
    let mut ticked = Vec::with_capacity(due.len());
    for row in due {
        let item = ChecklistItemId::try_from_db(row.item)
            .ok_or(StoreError::Corrupt("checklist_items.number"))?;
        let linked = TaskId::try_from_db(row.linked).ok_or(StoreError::Corrupt("tasks.number"))?;
        set_done(tx, item, Some((now, EventActor::fold().as_str()))).await?;
        ticked.push((item, linked));
    }
    Ok(ticked)
}

/// Ticked items and all items of `task`, over every checklist and both kinds.
pub async fn progress(conn: &mut SqliteConnection, task: TaskId) -> Result<(u32, u32), StoreError> {
    let number = task.to_db();
    let row = sqlx::query!(
        r#"SELECT COUNT(i.done_at) AS "done!: i64", COUNT(i.id) AS "total!: i64"
           FROM checklist_items i
           JOIN checklists c ON c.id = i.checklist_id
           JOIN tasks t ON t.id = c.task_id
           WHERE t.number = ?"#,
        number
    )
    .fetch_one(conn)
    .await?;
    let count =
        |value: i64| u32::try_from(value).map_err(|_| StoreError::Corrupt("checklist_items"));
    Ok((count(row.done)?, count(row.total)?))
}

/// Gives every item in `lists` its checklist (by name, on `task`) and its index in the list as its position. An item that is not on a checklist of `task` changes nothing and fails with `StoreError::Invalid`. `checklist_items_one_per_position` is checked row by row, so a plain renumbering would collide midway: the items first take distinct negative positions, which no stored item holds, and then their final ones. Each list must name every item its checklist keeps, so the positions end dense from 0.
pub async fn arrange(
    tx: &mut WriteTx,
    task: TaskId,
    lists: &[(&ChecklistName, &[ChecklistItemId])],
) -> Result<(), StoreError> {
    let number = task.to_db();
    let mut parked: i64 = 0;
    for (checklist, items) in lists {
        let name = checklist.as_str();
        for item in *items {
            parked -= 1;
            let item = item.to_db();
            let result = sqlx::query!(
                "UPDATE checklist_items SET position = ?4, checklist_id = (
                     SELECT c.id FROM checklists c JOIN tasks t ON t.id = c.task_id
                     WHERE t.number = ?1 AND c.name = ?2)
                 WHERE number = ?3 AND checklist_id IN (
                     SELECT c.id FROM checklists c JOIN tasks t ON t.id = c.task_id WHERE t.number = ?1)",
                number,
                name,
                item,
                parked
            )
            .execute(tx.conn())
            .await?;
            if result.rows_affected() != 1 {
                return Err(StoreError::Invalid(
                    "the checklist item does not exist on this task",
                ));
            }
        }
    }
    for (_, items) in lists {
        for (position, item) in items.iter().enumerate() {
            let item = item.to_db();
            let position = i64::try_from(position)
                .map_err(|_| StoreError::Invalid("too many checklist items"))?;
            sqlx::query!(
                "UPDATE checklist_items SET position = ?2 WHERE number = ?1",
                item,
                position
            )
            .execute(tx.conn())
            .await?;
        }
    }
    Ok(())
}
