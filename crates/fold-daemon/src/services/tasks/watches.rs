use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use fold_core::events::types;
use fold_core::{
    Actor, AddressedAs, FoldError, MessageBody, Recipient, Target, TaskId, TaskStatus, TaskTitle,
    WorkerName, codes,
};
use fold_store::{StoreError, tasks};
use serde_json::json;
use tokio::sync::broadcast::{self, error::RecvError, error::TryRecvError};

use super::{TaskError, TaskService};
use crate::services::agents::{actor_worker, worker_not_found};
use crate::services::events::{BusMessage, EventBuffer, EventBus};
use crate::services::messages::{MessageError, send_from_fold_in_tx};
use crate::support::lock;

/// The most tasks one worker watches at a time, so a watch stays one short scan of the store.
pub const MAX_WATCHED: usize = 100;

/// The task watches of one project's workers, in memory only: a daemon restart ends every watch. Each worker has at most one watch; a second `watch` adds to it.
#[derive(Debug, Clone, Default)]
pub struct TaskWatches {
    watches: Arc<Mutex<HashMap<WorkerName, Watch>>>,
    /// Held for each delivery pass, so a task is reported once and the last message of a watch comes after its other messages.
    delivering: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Debug, Default)]
struct Watch {
    pending: BTreeSet<TaskId>,
    /// The watched tasks reported Done so far, in the order they were reported, which the last message names.
    done: Vec<TaskId>,
}

/// The state of a worker's watch after a call: the tasks it still waits for, and the tasks of the call that were Done already and reported at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchView {
    pub watching: Vec<TaskId>,
    pub done: Vec<TaskId>,
}

impl TaskWatches {
    fn has_any(&self) -> bool {
        !lock(&self.watches).is_empty()
    }

    fn pending(&self) -> BTreeSet<TaskId> {
        lock(&self.watches)
            .values()
            .flat_map(|watch| watch.pending.iter().copied())
            .collect()
    }

    fn watching(&self, worker: &WorkerName) -> Vec<TaskId> {
        lock(&self.watches)
            .get(worker)
            .map(|watch| watch.pending.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Takes the tasks of `finished` out of every watch. Each worker gets the Done tasks it watched, and whether its watch ended with them, with every task it reported; a watch with nothing left is removed. A task in `gone` leaves its watches without a report.
    fn take(&self, finished: &BTreeSet<TaskId>, gone: &BTreeSet<TaskId>) -> Vec<Taken> {
        let mut watches = lock(&self.watches);
        let mut taken = Vec::new();
        watches.retain(|worker, watch| {
            let done: Vec<TaskId> = watch.pending.intersection(finished).copied().collect();
            watch
                .pending
                .retain(|task| !finished.contains(task) && !gone.contains(task));
            watch.done.extend(done.iter().copied());
            let ended = watch.pending.is_empty();
            if !done.is_empty() {
                taken.push(Taken {
                    worker: worker.clone(),
                    done,
                    all: ended.then(|| watch.done.clone()),
                });
            }
            !ended
        });
        taken
    }

    /// Puts the tasks of a pass whose messages were not written back into their watches, so the next pass reports them.
    fn restore(&self, taken: &[Taken]) {
        let mut watches = lock(&self.watches);
        for one in taken {
            if one.all.is_none() && !watches.contains_key(&one.worker) {
                continue;
            }
            let watch = watches.entry(one.worker.clone()).or_default();
            if let Some(all) = &one.all
                && watch.done.is_empty()
            {
                watch.done = all.clone();
            }
            watch.done.retain(|task| !one.done.contains(task));
            watch.pending.extend(one.done.iter().copied());
        }
    }
}

/// What one delivery pass sends to one worker.
struct Taken {
    worker: WorkerName,
    done: Vec<TaskId>,
    /// Every task the ended watch reported, when this pass ended it.
    all: Option<Vec<TaskId>>,
}

fn watcher(actor: &Actor) -> Result<WorkerName, TaskError> {
    match actor {
        Actor::Agent(agent) => Ok(agent.worker.clone()),
        _ => Err(FoldError::new(
            codes::INVALID_REQUEST,
            "only an agent's worker watches tasks",
        )
        .with_hint("call watch_tasks from an agent session")
        .into()),
    }
}

fn from_message(error: MessageError) -> TaskError {
    match error {
        MessageError::Fold(error) => TaskError::Fold(error),
        MessageError::Store(error) => TaskError::Store(error),
        MessageError::Event(error) => TaskError::Event(error),
    }
}

fn listed(tasks: &[TaskId]) -> String {
    tasks
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

impl TaskService {
    /// Watches `tasks` for the actor's worker: fold sends the worker one message when each task is Done and a last one when every watched task is, and the watch then ends. A task that is Done already is reported at once. A watch the worker has already takes the new tasks too. An unknown task is refused with `task_not_found`, and an archived task that is not Done with `task_closed`, and nothing is watched; more than [`MAX_WATCHED`] tasks in the watch is refused with `invalid_request`.
    pub async fn watch_tasks(
        &self,
        actor: &Actor,
        tasks: &[TaskId],
    ) -> Result<WatchView, TaskError> {
        let worker = watcher(actor)?;
        if tasks.is_empty() {
            return Err(too_many_or_none("name at least one task to watch"));
        }
        let wanted: BTreeSet<TaskId> = tasks.iter().copied().collect();
        if let Actor::Agent(agent) = actor {
            let mut conn = self
                .store
                .read()
                .acquire()
                .await
                .map_err(StoreError::from)?;
            if actor_worker(&mut conn, agent).await?.is_none() {
                return Err(worker_not_found().into());
            }
            drop(conn);
        }
        for task in &wanted {
            let row = tasks::get_by_number(self.store.read(), *task)
                .await?
                .ok_or_else(|| super::task_not_found(*task))?;
            if row.archived_at.is_some() && row.status != TaskStatus::Done {
                return Err(
                    FoldError::new(codes::TASK_CLOSED, format!("{task} is archived"))
                        .with_details([("reason", json!("archived")), ("task", json!(task))])
                        .with_hint("an archived task never becomes Done; watch other tasks")
                        .into(),
                );
            }
        }
        {
            let mut watches = lock(&self.watches.watches);
            let watch = watches.entry(worker.clone()).or_default();
            let total = watch.pending.union(&wanted).count();
            if total > MAX_WATCHED {
                if watch.pending.is_empty() {
                    watches.remove(&worker);
                }
                return Err(too_many_or_none(&format!(
                    "a worker watches at most {MAX_WATCHED} tasks at a time"
                )));
            }
            watch.pending.extend(wanted.iter().copied());
        }
        self.deliver_watches().await?;
        let watching = self.watches.watching(&worker);
        let done = wanted
            .iter()
            .filter(|task| !watching.contains(task))
            .copied()
            .collect();
        Ok(WatchView { watching, done })
    }

    /// Stops watching `tasks` for the actor's worker, or every task of its watch when `tasks` is `None`. A task the worker does not watch changes nothing. A watch with no task left ends without a last message.
    pub fn unwatch_tasks(
        &self,
        actor: &Actor,
        tasks: Option<&[TaskId]>,
    ) -> Result<WatchView, TaskError> {
        let worker = watcher(actor)?;
        let mut watches = lock(&self.watches.watches);
        if let Some(watch) = watches.get_mut(&worker) {
            match tasks {
                Some(tasks) => watch.pending.retain(|task| !tasks.contains(task)),
                None => watch.pending.clear(),
            }
            if watch.pending.is_empty() {
                watches.remove(&worker);
            }
        }
        let watching = watches
            .get(&worker)
            .map(|watch| watch.pending.iter().copied().collect())
            .unwrap_or_default();
        Ok(WatchView {
            watching,
            done: Vec::new(),
        })
    }

    /// Reports every watched task that is Done now to the workers that watch it, in one write, and ends each watch whose last task this reports. A watched task that no longer exists, or that is archived without being Done, leaves its watches without a report. When the write fails, the tasks stay watched for the next pass.
    pub async fn deliver_watches(&self) -> Result<(), TaskError> {
        let _delivering = self.watches.delivering.lock().await;
        let mut finished = BTreeSet::new();
        let mut gone = BTreeSet::new();
        let mut titles: HashMap<TaskId, TaskTitle> = HashMap::new();
        for task in self.watches.pending() {
            match tasks::get_by_number(self.store.read(), task).await? {
                Some(row) if row.status == TaskStatus::Done => {
                    finished.insert(task);
                    titles.insert(task, row.title);
                }
                Some(row) if row.archived_at.is_some() => {
                    gone.insert(task);
                }
                Some(_) => {}
                None => {
                    gone.insert(task);
                }
            }
        }
        if finished.is_empty() && gone.is_empty() {
            return Ok(());
        }
        let taken = self.watches.take(&finished, &gone);
        if taken.is_empty() {
            return Ok(());
        }
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let written = self
            .store
            .write(async |tx| {
                for one in &taken {
                    let to = || {
                        vec![Recipient {
                            target: Target::Worker(one.worker.clone()),
                            addressed_as: AddressedAs::Worker,
                        }]
                    };
                    for task in &one.done {
                        let title = titles.get(task).map(TaskTitle::as_str).unwrap_or_default();
                        let body = MessageBody::parse(format!(
                            "{task} is Done: {title}. You asked for this with watch_tasks."
                        ))?;
                        send_from_fold_in_tx(tx, &mut buffer, to(), (*task, None), &body, now)
                            .await
                            .map_err(from_message)?;
                    }
                    if let (Some(all), Some(last)) = (&one.all, one.done.last()) {
                        let body = MessageBody::parse(format!(
                            "Every task you watched is Done: {}. The watch has ended.",
                            listed(all)
                        ))?;
                        send_from_fold_in_tx(tx, &mut buffer, to(), (*last, None), &body, now)
                            .await
                            .map_err(from_message)?;
                    }
                }
                Ok::<_, TaskError>(())
            })
            .await;
        if let Err(error) = written {
            self.watches.restore(&taken);
            return Err(error);
        }
        self.bus.publish(buffer);
        Ok(())
    }
}

fn too_many_or_none(message: &str) -> TaskError {
    FoldError::new(codes::INVALID_REQUEST, message.to_owned())
        .with_details([("field", json!("tasks"))])
        .with_hint("call watch_tasks with 1 to 100 tasks, or unwatch_tasks first")
        .into()
}

/// Reports the watched tasks that became Done after each commit that can complete, archive or delete a task, and scans them all again after the bus dropped events. It ends when the project closes.
pub async fn follow_task_watches(
    service: TaskService,
    bus: EventBus,
    mut events: broadcast::Receiver<BusMessage>,
) {
    let mut closing = bus.closing();
    loop {
        let due = tokio::select! {
            biased;
            _ = closing.wait_for(|closed| *closed) => return,
            received = events.recv() => match received {
                Ok(BusMessage::Durable(event)) => {
                    event.event_type == types::TASK_STATUS_CHANGED
                        || event.event_type == types::TASK_COMPLETED
                        || event.event_type == types::TASK_ARCHIVED
                        || event.event_type == types::TASK_DELETED
                }
                Ok(BusMessage::Live(_)) => false,
                Err(RecvError::Lagged(_)) => true,
                Err(RecvError::Closed) => return,
            },
        };
        if !due || !service.watches.has_any() {
            continue;
        }
        // Everything queued so far was committed before it was sent, so the pass below covers it.
        while !matches!(
            events.try_recv(),
            Err(TryRecvError::Empty | TryRecvError::Closed)
        ) {}
        if let Err(error) = service.deliver_watches().await {
            tracing::warn!(
                code = error.log_code(),
                "the watched tasks were not reported; the next status change reports them"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker(name: &str) -> WorkerName {
        WorkerName::parse(name).unwrap()
    }

    fn task(number: u32) -> TaskId {
        TaskId::from_number(number).unwrap()
    }

    fn watching(watches: &TaskWatches, tasks: &[u32]) {
        let mut map = lock(&watches.watches);
        let watch = map.entry(worker("main")).or_default();
        watch
            .pending
            .extend(tasks.iter().map(|number| task(*number)));
    }

    #[test]
    fn a_pass_that_was_not_written_reports_its_tasks_again() {
        let watches = TaskWatches::default();
        watching(&watches, &[1, 2]);
        let first = watches.take(&BTreeSet::from([task(1)]), &BTreeSet::new());
        watches.restore(&first);
        assert_eq!(watches.watching(&worker("main")), vec![task(1), task(2)]);

        let second = watches.take(&BTreeSet::from([task(1)]), &BTreeSet::new());
        assert_eq!(second[0].done, vec![task(1)]);
        let last = watches.take(&BTreeSet::from([task(2)]), &BTreeSet::new());
        assert!(!watches.has_any());
        watches.restore(&last);
        assert_eq!(watches.watching(&worker("main")), vec![task(2)]);
        let again = watches.take(&BTreeSet::from([task(2)]), &BTreeSet::new());
        assert_eq!(again[0].all, Some(vec![task(1), task(2)]));
        assert!(!watches.has_any());
    }

    #[test]
    fn a_failed_pass_does_not_bring_back_an_ended_watch() {
        let watches = TaskWatches::default();
        watching(&watches, &[1, 2]);
        let taken = watches.take(&BTreeSet::from([task(1)]), &BTreeSet::new());
        lock(&watches.watches).clear();
        watches.restore(&taken);
        assert!(!watches.has_any());
    }
}
