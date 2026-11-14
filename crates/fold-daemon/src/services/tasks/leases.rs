use std::collections::{BTreeSet, HashMap, HashSet};

use fold_core::claims::{
    HolderRuns, HolderWait, holding, is_lapsed, near_stale_cutoff, stale_needs_person,
};
use fold_core::events::types;
use fold_core::stall::stall_receivers;
use fold_core::{AgentName, EventActor, NewEvent, Timestamp};
use fold_store::reviews::checks;
use fold_store::tasks::claims::{self, ClaimTarget, StaleClaim};
use fold_store::{StoreError, WriteTx, agents};
use serde_json::json;

use super::{TaskError, TaskService};
use crate::projects::ProjectHandle;
use crate::push::starter_agent;
use crate::services::events::EventBuffer;
use crate::services::projects::load_config;

impl TaskService {
    /// How many unreleased claims the workers of agent `agent_id` hold that are near stale now (see [`near_stale_cutoff`]), with the project's lease. Every tool call's footer counts them, so the lease comes from [`crate::services::projects::Lease::recent_minutes`].
    pub async fn claims_near_stale(&self, agent_id: i64) -> Result<u64, TaskError> {
        let lease_minutes = self.lease.recent_staleness_minutes(&self.store).await?;
        let Some(cutoff) = near_stale_cutoff(self.clock.now(), lease_minutes) else {
            return Ok(0);
        };
        let count = claims::count_renewed_by(self.store.read(), agent_id, cutoff).await?;
        Ok(count)
    }
}

/// Marks each unreleased claim of `project` whose lease ran out at `now` and that is not marked yet, with one `claim.stale` per claim, in one write transaction; the events reach the bus after the commit. The lease is the project's `lease_minutes`, counted by [`is_lapsed`]: in the same transaction the claims of each holder whose own check run goes are renewed at `now`, because the holder waits for the run and makes no calls, and so are the claims of each holder that an agent parked while that agent runs, because the holder waits for its go, and of each holder whose every claim waits on others ([`fold_core::claims::ClaimWait`]): a sent proposal, an ask to a person, an open report or a verdict it gave. So no such claim runs out, a person never hears of a claim they can do nothing about, and every reader of `renewed_at` agrees. `claim.stale` names in `told` the agents that hear of it first, the holder's lead or else the main agents, as a stall does: the claim needs a person at once when that list is empty, and else once one more `stall_minutes` passed without a renewal, which appends `claim.needs_person`. A marked claim keeps its holder, its workspace and its task's status: only `takeover_claim` moves it, and a renewal clears the mark. Returns the claims it marked.
pub async fn sweep_leases(
    project: &ProjectHandle,
    now: Timestamp,
) -> Result<Vec<StaleClaim>, TaskError> {
    let tasks = &project.tasks;
    let lease_minutes = tasks.lease_minutes().await?;
    let stall_minutes = load_config(&project.paths).await?.stall_minutes;
    let mut buffer = EventBuffer::new();
    let marked = tasks
        .store
        .write(async |tx| {
            let runs = checks::holder_runs(tx.conn()).await?;
            let parked: HashSet<i64> = agents::parked_holders(tx.conn())
                .await?
                .into_iter()
                .filter(|(_, parker_runs)| *parker_runs)
                .map(|(worker, _)| worker)
                .collect();
            // A holder waits on others only while every claim it holds does.
            let mut waits: HashMap<i64, bool> = HashMap::new();
            for (worker, wait) in claims::claim_waits(tx.conn()).await? {
                let all = waits.entry(worker).or_insert(true);
                *all = *all && wait.waits_on_others();
            }
            let runs_of = |worker: i64| {
                holding(
                    runs.get(&worker).copied().unwrap_or_default(),
                    HolderWait {
                        parked_by_running_agent: parked.contains(&worker),
                        claims_wait_on_others: waits.get(&worker).copied().unwrap_or(false),
                    },
                )
            };
            let running: Vec<i64> = runs
                .keys()
                .chain(parked.iter())
                .chain(waits.keys())
                .copied()
                .collect::<BTreeSet<i64>>()
                .into_iter()
                .filter(|worker| runs_of(*worker) == HolderRuns::Running)
                .collect();
            claims::renew(tx, &running, now).await?;
            let marked = claims::mark_stale(tx, now, |renewed_at, worker| {
                is_lapsed(renewed_at, runs_of(worker), now, lease_minutes)
            })
            .await?;
            let receivers = Receivers::read(tx).await?;
            let mut nobody = Vec::new();
            for claim in &marked {
                let told = receivers.of(&claim.worker.agent());
                if stale_needs_person(now, now, !told.is_empty(), stall_minutes) {
                    nobody.push(claim.id);
                }
                buffer.append(tx, &stale_event(claim, &told)?, now).await?;
            }
            claims::set_needs_person(tx, &nobody, now).await?;
            // While the agents are paused no agent can act, so the window of the agent told waits for the resume, which renews every claim.
            let paused = agents::pause_state(tx.conn()).await?.is_paused();
            let unanswered = claims::mark_needs_person(tx, now, |stale_at| {
                !paused && stale_needs_person(now, stale_at, true, stall_minutes)
            })
            .await?;
            for claim in &unanswered {
                buffer.append(tx, &needs_person_event(claim)?, now).await?;
            }
            Ok::<_, TaskError>(marked)
        })
        .await?;
    tasks.bus.publish(buffer);
    for claim in &marked {
        match claim.target {
            ClaimTarget::Task(task) => {
                tracing::info!(task = %task, worker = %claim.worker, "an implementation claim went stale");
            }
            ClaimTarget::Review { proposal, .. } => {
                tracing::info!(proposal = %proposal, worker = %claim.worker, "a review claim went stale");
            }
        }
    }
    Ok(marked)
}

/// Who hears of a stale claim first, as [`stall_receivers`] decides for a stall: the agent that started the holder's process, when that agent is known, or else the main agents.
struct Receivers {
    starters: HashMap<AgentName, AgentName>,
    main_agents: BTreeSet<AgentName>,
}

impl Receivers {
    async fn read(tx: &mut WriteTx) -> Result<Self, StoreError> {
        let every = agents::list_agents(tx.conn()).await?;
        let known: HashSet<&AgentName> = every.iter().map(|agent| &agent.name).collect();
        let tagged: HashSet<i64> = agents::all_tags(tx.conn())
            .await?
            .into_iter()
            .filter(|(_, tag)| tag.is_main())
            .map(|(agent, _)| agent)
            .collect();
        let main_agents = every
            .iter()
            .filter(|agent| tagged.contains(&agent.id))
            .map(|agent| agent.name.clone())
            .collect();
        let starters = agents::list_worker_processes(tx.conn())
            .await?
            .into_iter()
            .filter_map(|row| {
                let starter =
                    starter_agent(&row.started_by).filter(|agent| known.contains(agent))?;
                Some((row.agent, starter))
            })
            .collect();
        Ok(Self {
            starters,
            main_agents,
        })
    }

    fn of(&self, holder: &AgentName) -> Vec<AgentName> {
        stall_receivers(holder, self.starters.get(holder), &self.main_agents)
    }
}

/// The data of a claim event: its kind, the proposal of a review claim, the holder and the last renewal.
fn claim_event(kind: fold_core::EventType, claim: &StaleClaim) -> Result<NewEvent, TaskError> {
    let event = NewEvent::new(kind, EventActor::fold())?;
    let renewed_at = claim.renewed_at.as_millis();
    let worker = claim.worker.as_str();
    let event = match claim.target {
        ClaimTarget::Task(task) => event.task(task).data(json!({
            "kind": "implementation",
            "worker": worker,
            "renewed_at": renewed_at,
        }))?,
        ClaimTarget::Review { proposal, task } => {
            event.task(task).proposal(proposal).data(json!({
                "kind": "review",
                "proposal": proposal.to_string(),
                "worker": worker,
                "renewed_at": renewed_at,
            }))?
        }
    };
    Ok(event)
}

fn stale_event(claim: &StaleClaim, told: &[AgentName]) -> Result<NewEvent, TaskError> {
    let mut event = claim_event(types::CLAIM_STALE, claim)?;
    if let Some(data) = event.data.as_object_mut() {
        data.insert(
            "told".to_owned(),
            json!(told.iter().map(AgentName::as_str).collect::<Vec<_>>()),
        );
    }
    Ok(event)
}

fn needs_person_event(claim: &StaleClaim) -> Result<NewEvent, TaskError> {
    claim_event(types::CLAIM_NEEDS_PERSON, claim)
}
