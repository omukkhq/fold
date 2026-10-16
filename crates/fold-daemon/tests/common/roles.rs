use fold_core::roles::{Role, WorkerState};
use fold_core::{AgentName, AreaName, LabelColor, LabelName, Timestamp};
use fold_store::agents::{self, NewWorkerProcess};
use fold_store::{Store, StoreError, tasks};

/// The area an area lead or task coder gets when the caller names none, made on first use over the label of the same name.
const DEFAULT_AREA: &str = "work";

/// Records a stopped worker process of `role` for the registered agent `agent`, as if fold had started it in `area`; nothing runs. An area lead or task coder always has an area, so without `area` it gets [`DEFAULT_AREA`], and an area lead becomes its area's lead. The rules that read an agent's role, such as the review rights and the archive rule, then see it.
pub async fn give_role(store: &Store, agent: &str, role: Role, area: Option<&str>) {
    let name = AgentName::parse(agent).unwrap();
    let area = match role {
        Role::AreaLead | Role::TaskCoder => Some(area.unwrap_or(DEFAULT_AREA)),
        Role::ReviewLead | Role::BatchReviewer => area,
    }
    .map(|area| AreaName::parse(area).unwrap());
    let now = Timestamp::from_millis(1).unwrap();
    store
        .write(async |tx| {
            let agent_id = agents::find_agent(tx.conn(), &name).await?.unwrap().id;
            let area_id = match &area {
                Some(area) => match agents::find_area(tx.conn(), area).await? {
                    Some(row) => Some(row.id),
                    None => {
                        let label = LabelName::parse(area.as_str()).unwrap();
                        tasks::insert_label(tx, &label, LabelColor::Sage, now).await?;
                        let label_id = agents::label_id(tx.conn(), &label).await?.unwrap();
                        Some(agents::insert_area(tx.conn(), area, label_id, "main", now).await?)
                    }
                },
                None => None,
            };
            let session_id = format!("00000000-0000-4000-9000-{agent_id:012}");
            let id = agents::insert_worker_process(
                tx.conn(),
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
            agents::set_worker_state(tx.conn(), id, WorkerState::Stopped, now).await?;
            if let (Role::AreaLead, Some(area_id)) = (role, area_id) {
                agents::set_area_lead(tx.conn(), area_id, agent_id).await?;
            }
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
}
