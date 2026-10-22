use fold_core::roles::{Role, WorkerState};
use fold_core::{
    AgentName, AreaName, IdKind, LabelColor, LabelName, ProposalId, ReviewBatchId, TaskId,
    Timestamp, WorkerName,
};
use fold_store::agents::{self, NewWorkerProcess};
use fold_store::reviews::batches;
use fold_store::{Store, StoreError, tasks};

/// The area an area lead or task worker gets when the caller names none, made on first use over the label of the same name.
const DEFAULT_AREA: &str = "work";

/// Records a stopped worker process of `role` for the registered agent `agent`, as if fold had started it in `area`; nothing runs. An agent that has that role in that area already keeps its row, and an agent that is not registered yet is made with its main worker, as `start_worker` makes it. An area lead or task worker always has an area, so without `area` it gets [`DEFAULT_AREA`], and an area lead becomes its area's lead. The rules that read an agent's role, such as the review rights and the archive rule, then see it.
pub async fn give_role(store: &Store, agent: &str, role: Role, area: Option<&str>) {
    let name = AgentName::parse(agent).unwrap();
    let area = match role {
        Role::AreaLead | Role::TaskWorker => Some(area.unwrap_or(DEFAULT_AREA)),
        Role::ReviewLead | Role::Reviewer | Role::Main => area,
    }
    .map(|area| AreaName::parse(area).unwrap());
    let now = Timestamp::from_millis(1).unwrap();
    store
        .write(async |tx| {
            if let Some(row) = agents::find_worker_process(tx.conn(), &name).await? {
                assert_eq!(
                    (row.role, row.area),
                    (role, area.clone()),
                    "{name} has a fold role already"
                );
                return Ok(());
            }
            let agent_id = match agents::find_agent(tx.conn(), &name).await? {
                Some(row) => row.id,
                None => {
                    let id = agents::insert_agent(tx, &name, now).await?;
                    agents::insert_worker(tx, id, &WorkerName::main(&name), None, now).await?;
                    id
                }
            };
            let area_id = match &area {
                Some(area) => match agents::find_area(tx.conn(), area).await? {
                    Some(row) => Some(row.id),
                    None => {
                        let label = LabelName::parse(area.as_str()).unwrap();
                        tasks::insert_label(tx, &label, LabelColor::Sage, now).await?;
                        let label_id = agents::label_id(tx.conn(), &label).await?.unwrap();
                        Some(agents::insert_area(tx, area, label_id, "main", now).await?)
                    }
                },
                None => None,
            };
            let session_id = format!("00000000-0000-4000-9000-{agent_id:012}");
            let id = agents::insert_worker_process(
                tx,
                &NewWorkerProcess {
                    agent_id,
                    role,
                    area_id,
                    model: None,
                    session_id: &session_id,
                    started_by: "main",
                    now,
                },
            )
            .await?;
            agents::set_worker_state(tx, id, WorkerState::Stopped, now).await?;
            if let (Role::AreaLead, Some(area_id)) = (role, area_id) {
                agents::set_area_lead(tx, area_id, agent_id).await?;
            }
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
}

/// The main worker row id of the registered agent `agent`.
async fn main_worker_id(tx: &mut fold_store::WriteTx, agent: &str) -> i64 {
    let name = AgentName::parse(agent).unwrap();
    let agent_id = agents::find_agent(tx.conn(), &name)
        .await
        .unwrap()
        .unwrap()
        .id;
    agents::find_worker(tx.conn(), agent_id, &WorkerName::main(&name))
        .await
        .unwrap()
        .unwrap()
        .id
}

/// Gives the main worker of `agent` the implementation claim on `task` and calculates the task's status, at `now`, as the lead's `start_worker` gives it to a worker that fold starts; for an agent whose worker process [`give_role`] recorded, which never claims itself.
pub async fn give_claim(store: &Store, agent: &str, task: TaskId, now: Timestamp) {
    store
        .write(async |tx| {
            let worker = main_worker_id(tx, agent).await;
            tasks::claims::insert_implementation(tx, task, worker, now).await?;
            fold_daemon::services::tasks::recompute(tx, &[task], now)
                .await
                .unwrap();
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
}

/// Gives the main worker of `agent` the review claim on `proposal` at `now`, as the review lead's `start_worker` gives it to a reviewer that fold starts; for an agent whose worker process [`give_role`] recorded.
pub async fn give_review_claim(store: &Store, agent: &str, proposal: ProposalId, now: Timestamp) {
    store
        .write(async |tx| {
            let worker = main_worker_id(tx, agent).await;
            fold_store::reviews::insert_review_claim(tx, proposal, worker, now).await
        })
        .await
        .unwrap();
}

/// Makes a review batch of `proposals`, in the order given, for the main worker of `agent` with the review claim of each member at `now`, as the review lead's `start_worker` makes it for a reviewer that fold starts with two or more proposals; for an agent whose worker process [`give_role`] recorded. A member whose review claim the worker holds already keeps it.
pub async fn give_batch(
    store: &Store,
    agent: &str,
    proposals: &[ProposalId],
    now: Timestamp,
) -> ReviewBatchId {
    store
        .write(async |tx| {
            let worker = main_worker_id(tx, agent).await;
            let number = fold_store::counters::allocate(tx, IdKind::ReviewBatch).await?;
            let batch = ReviewBatchId::from_number(number).unwrap();
            batches::insert_batch(tx, batch, worker, now).await?;
            for (position, proposal) in (1u32..).zip(proposals) {
                let row = fold_store::reviews::get_proposal(tx.conn(), *proposal)
                    .await?
                    .unwrap();
                let held = fold_store::reviews::unreleased_review_claim(tx.conn(), *proposal)
                    .await?
                    .is_some_and(|claim| claim.worker_id == worker);
                if !held {
                    fold_store::reviews::insert_review_claim(tx, *proposal, worker, now).await?;
                }
                batches::insert_member(tx, batch, *proposal, row.current_version, position).await?;
            }
            Ok::<_, StoreError>(batch)
        })
        .await
        .unwrap()
}

/// The areas, each over the label of the same name, that `frontend` and `backend` lead in the fixtures where they do the task work, as if fold had started them: only a lead, or the main agent, claims a task.
pub const WORK_AREAS: [(&str, &str); 2] =
    [("frontend", "frontend-work"), ("backend", "backend-work")];

/// The label of `frontend`'s area of [`WORK_AREAS`]: a task has at most one area, so a task with it is in that area and its lead claims it. [`work_labels_of`] names another lead's area.
pub fn work_labels() -> Vec<LabelName> {
    work_labels_of("frontend")
}

/// The label of the area of [`WORK_AREAS`] that `agent` leads, for a task that `agent` claims.
pub fn work_labels_of(agent: &str) -> Vec<LabelName> {
    let (_, label) = WORK_AREAS
        .into_iter()
        .find(|(name, _)| *name == agent)
        .expect("an agent of WORK_AREAS");
    vec![LabelName::parse(label).unwrap()]
}

/// Makes `frontend` the lead of its area of [`WORK_AREAS`] alone, for the fixtures whose tasks are all in that area: a task has one area, so another agent that claims one of them is the main agent ([`give_main_tag`]).
pub async fn lead_frontend_area(store: &Store) {
    let (agent, area) = WORK_AREAS[0];
    give_role(store, agent, Role::AreaLead, Some(area)).await;
}

/// Gives the registered agent `agent` the `main` tag, as the handshake of `fld claude` gives it, so it claims any task.
pub async fn give_main_tag(store: &Store, agent: &str) {
    let name = AgentName::parse(agent).unwrap();
    let now = Timestamp::from_millis(1).unwrap();
    store
        .write(async |tx| {
            let row = agents::find_agent(tx.conn(), &name)
                .await?
                .expect("a registered agent");
            agents::add_tag(
                tx,
                row.id,
                &fold_core::Tag::parse("main").unwrap(),
                None,
                now,
            )
            .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
}

/// Makes each registered agent of [`WORK_AREAS`] the lead of its area.
pub async fn lead_work_areas(store: &Store) {
    for (agent, area) in WORK_AREAS {
        give_role(store, agent, Role::AreaLead, Some(area)).await;
    }
}
