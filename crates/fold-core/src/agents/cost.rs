use std::collections::BTreeMap;

use crate::{AgentName, AreaName, ProposalId, Role, TaskId, Timestamp, WorkerState};

/// What one turn of a session cost, and the session's running total after it. Claude Code reports the running total of the session in each result, and `--resume` goes on from the total the session had, so a turn costs what the total grew by. A total below `session_total` comes from a process that counted again from nothing, so all of it is the turn's; a total of 0 is a result without a count, such as an answer to an interrupt, and changes nothing.
pub fn session_turn(reported: u64, session_total: u64) -> (u64, u64) {
    if reported >= session_total {
        (reported - session_total, reported)
    } else if reported == 0 {
        (0, session_total)
    } else {
        (reported, reported)
    }
}

/// The slot of the cost by time for a range of at most [`SHORT_RANGE_MILLIS`]: every time zone's offset is a whole number of quarter hours, so the slots add up to the local hours and days of any viewer.
pub const SHORT_SLOT_MILLIS: i64 = 15 * 60 * 1000;

/// The slot of a longer range, which a viewer adds up into days only. A time zone with a half-hour offset sees each hour slot in the day it starts in.
pub const LONG_SLOT_MILLIS: i64 = 60 * 60 * 1000;

/// The longest range that gets [`SHORT_SLOT_MILLIS`]: seven days and the hour a clock change adds.
pub const SHORT_RANGE_MILLIS: i64 = (7 * 24 + 1) * 60 * 60 * 1000;

/// The slot length for the turns since `since`, every turn without it, at `now`.
pub fn slot_millis(since: Option<Timestamp>, now: Timestamp) -> i64 {
    match since {
        Some(since) if now.as_millis().saturating_sub(since.as_millis()) <= SHORT_RANGE_MILLIS => {
            SHORT_SLOT_MILLIS
        }
        _ => LONG_SLOT_MILLIS,
    }
}

/// The turns of one worker with one model for one subject: a task its task worker built, or a proposal its reviewer reviewed with that proposal's task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnGroup {
    pub agent: AgentName,
    pub role: Role,
    pub area: Option<AreaName>,
    /// The worker's state now, so a stopped worker reads as one.
    pub state: WorkerState,
    /// The model of the turns; `None` for turns that ended before Claude Code named one.
    pub model: Option<String>,
    /// The task a task worker's turns built.
    pub task: Option<TaskId>,
    /// The proposal a reviewer's turns reviewed.
    pub proposal: Option<ProposalId>,
    /// The task of `task` or of `proposal`, with its title.
    pub subject: Option<(TaskId, String)>,
    pub turns: u64,
    pub cost_micro_usd: u64,
    pub tokens: Tokens,
    pub first_at: Timestamp,
    pub last_at: Timestamp,
}

/// The tokens of turns, each turn's own from the usage Claude Code reported for it, so a resumed session counts none twice. A stop's interrupt adds none, as it adds no turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Tokens {
    /// Every token the model read or wrote: the input, the cache reads and writes, and the output.
    pub fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }

    #[must_use]
    pub fn plus(self, other: Self) -> Self {
        Self {
            input: self.input.saturating_add(other.input),
            output: self.output.saturating_add(other.output),
            cache_read: self.cache_read.saturating_add(other.cache_read),
            cache_write: self.cache_write.saturating_add(other.cache_write),
        }
    }
}

/// A sum of turns: their cost and tokens, how many there were and how many workers ran them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Amount {
    pub cost_micro_usd: u64,
    pub tokens: Tokens,
    pub turns: u64,
    pub workers: u64,
}

/// One worker's turns in the range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerCost {
    pub agent: AgentName,
    pub role: Role,
    pub area: Option<AreaName>,
    pub state: WorkerState,
    /// The model of the worker's newest turn.
    pub model: Option<String>,
    pub turns: u64,
    pub cost_micro_usd: u64,
    pub tokens: Tokens,
}

/// What one task cost: `build` is the turns of its task workers, `review` the turns of the reviewers of its proposals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCost {
    pub task: TaskId,
    pub title: String,
    /// The area of the task workers that built it; `None` while only reviewers worked on it.
    pub area: Option<AreaName>,
    pub build_micro_usd: u64,
    pub review_micro_usd: u64,
    pub tokens: Tokens,
    pub turns: u64,
}

impl TaskCost {
    pub fn total_micro_usd(&self) -> u64 {
        self.build_micro_usd.saturating_add(self.review_micro_usd)
    }
}

/// What a project's workers cost in a range, split as the cost view shows it. Every split adds up to `total`, except `tasks`, which leaves out the turns of no task.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CostReport {
    pub total: Amount,
    /// The end of the oldest turn in the range; `None` without a turn.
    pub first_turn_at: Option<Timestamp>,
    /// Every role in [`Role::ALL`] order, a role without a turn with nothing.
    pub by_role: Vec<(Role, Amount)>,
    /// The areas of the leads and the task workers, and `None` for the review lead, the reviewers and the supervised main, who work for every area; the most expensive first.
    pub by_area: Vec<(Option<AreaName>, Amount)>,
    /// The most expensive first.
    pub by_model: Vec<(Option<String>, Amount)>,
    /// The most expensive first, then by name.
    pub workers: Vec<WorkerCost>,
    /// The most expensive first, then by task number.
    pub tasks: Vec<TaskCost>,
}

/// The area a turn counts for in the split by area: reviewers and the review lead review every area's work, and the supervised main plans for all of them, so they have none.
fn area_of(group: &TurnGroup) -> Option<AreaName> {
    match group.role {
        Role::ReviewLead | Role::Reviewer | Role::Main => None,
        Role::AreaLead | Role::TaskWorker => group.area.clone(),
    }
}

/// Adds the group to the amount of `key`, counting each worker once per key.
fn add<K: Ord>(sums: &mut BTreeMap<K, (Amount, Vec<AgentName>)>, key: K, group: &TurnGroup) {
    let (amount, agents) = sums.entry(key).or_default();
    amount.cost_micro_usd = amount.cost_micro_usd.saturating_add(group.cost_micro_usd);
    amount.tokens = amount.tokens.plus(group.tokens);
    amount.turns = amount.turns.saturating_add(group.turns);
    if !agents.contains(&group.agent) {
        agents.push(group.agent.clone());
        amount.workers += 1;
    }
}

fn amounts<K: Ord>(sums: BTreeMap<K, (Amount, Vec<AgentName>)>) -> Vec<(K, Amount)> {
    let mut list: Vec<(K, Amount)> = sums
        .into_iter()
        .map(|(key, (amount, _))| (key, amount))
        .collect();
    list.sort_by_key(|(_, amount)| std::cmp::Reverse(amount.cost_micro_usd));
    list
}

/// Sums `groups`, the turns of one range, into the totals and splits of the cost view.
pub fn report(groups: &[TurnGroup]) -> CostReport {
    let mut total = BTreeMap::new();
    let mut roles = BTreeMap::new();
    let mut areas = BTreeMap::new();
    let mut models = BTreeMap::new();
    let mut workers: BTreeMap<AgentName, (WorkerCost, Timestamp)> = BTreeMap::new();
    let mut tasks: BTreeMap<TaskId, TaskCost> = BTreeMap::new();
    for group in groups {
        add(&mut total, (), group);
        add(&mut roles, group.role, group);
        add(&mut areas, area_of(group), group);
        add(&mut models, group.model.clone(), group);
        let (worker, newest) = workers.entry(group.agent.clone()).or_insert_with(|| {
            (
                WorkerCost {
                    agent: group.agent.clone(),
                    role: group.role,
                    area: group.area.clone(),
                    state: group.state,
                    model: group.model.clone(),
                    turns: 0,
                    cost_micro_usd: 0,
                    tokens: Tokens::default(),
                },
                group.last_at,
            )
        });
        worker.turns = worker.turns.saturating_add(group.turns);
        worker.cost_micro_usd = worker.cost_micro_usd.saturating_add(group.cost_micro_usd);
        worker.tokens = worker.tokens.plus(group.tokens);
        if group.last_at > *newest {
            *newest = group.last_at;
            worker.model = group.model.clone();
        }
        if let Some((task, title)) = &group.subject {
            let entry = tasks.entry(*task).or_insert_with(|| TaskCost {
                task: *task,
                title: title.clone(),
                area: None,
                build_micro_usd: 0,
                review_micro_usd: 0,
                tokens: Tokens::default(),
                turns: 0,
            });
            entry.turns = entry.turns.saturating_add(group.turns);
            entry.tokens = entry.tokens.plus(group.tokens);
            if group.proposal.is_some() {
                entry.review_micro_usd =
                    entry.review_micro_usd.saturating_add(group.cost_micro_usd);
            } else {
                entry.build_micro_usd = entry.build_micro_usd.saturating_add(group.cost_micro_usd);
                if entry.area.is_none() {
                    entry.area = group.area.clone();
                }
            }
        }
    }
    let mut workers: Vec<WorkerCost> = workers.into_values().map(|(worker, _)| worker).collect();
    workers.sort_by(|a, b| {
        b.cost_micro_usd
            .cmp(&a.cost_micro_usd)
            .then_with(|| a.agent.cmp(&b.agent))
    });
    let mut tasks: Vec<TaskCost> = tasks.into_values().collect();
    tasks.sort_by(|a, b| {
        b.total_micro_usd()
            .cmp(&a.total_micro_usd())
            .then_with(|| a.task.cmp(&b.task))
    });
    let roles = amounts(roles);
    CostReport {
        total: total
            .remove(&())
            .map(|(amount, _)| amount)
            .unwrap_or_default(),
        first_turn_at: groups.iter().map(|group| group.first_at).min(),
        by_role: Role::ALL
            .into_iter()
            .map(|role| {
                let amount = roles
                    .iter()
                    .find(|(known, _)| *known == role)
                    .map(|(_, amount)| *amount)
                    .unwrap_or_default();
                (role, amount)
            })
            .collect(),
        by_area: amounts(areas),
        by_model: amounts(models),
        workers,
        tasks,
    }
}

/// Which part of a task's cost a worker's turns are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostPart {
    /// A task worker built the task.
    Build,
    /// A reviewer reviewed a proposal of the task.
    Review,
}

/// One worker's turns for one task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskWorkerCost {
    pub agent: AgentName,
    pub role: Role,
    pub part: CostPart,
    /// The model of the worker's newest turn for the task.
    pub model: Option<String>,
    /// The proposals of the task a reviewer reviewed, in number order; empty for a build.
    pub proposals: Vec<ProposalId>,
    pub turns: u64,
    pub cost_micro_usd: u64,
    pub tokens: Tokens,
}

/// What one task cost, with one row per worker: the builds first, then the reviews, each in the order the workers first worked on it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskCostReport {
    pub build_micro_usd: u64,
    pub review_micro_usd: u64,
    pub tokens: Tokens,
    pub turns: u64,
    pub workers: Vec<TaskWorkerCost>,
}

/// Sums `groups`, every turn that built one task or reviewed one of its proposals.
pub fn task_report(groups: &[TurnGroup]) -> TaskCostReport {
    let mut report = TaskCostReport::default();
    let mut rows: Vec<(TaskWorkerCost, Timestamp, Timestamp)> = Vec::new();
    for group in groups {
        let part = if group.proposal.is_some() {
            CostPart::Review
        } else {
            CostPart::Build
        };
        match part {
            CostPart::Build => {
                report.build_micro_usd =
                    report.build_micro_usd.saturating_add(group.cost_micro_usd);
            }
            CostPart::Review => {
                report.review_micro_usd =
                    report.review_micro_usd.saturating_add(group.cost_micro_usd);
            }
        }
        report.turns = report.turns.saturating_add(group.turns);
        report.tokens = report.tokens.plus(group.tokens);
        let index = match rows
            .iter()
            .position(|(row, _, _)| row.agent == group.agent && row.part == part)
        {
            Some(index) => index,
            None => {
                rows.push((
                    TaskWorkerCost {
                        agent: group.agent.clone(),
                        role: group.role,
                        part,
                        model: group.model.clone(),
                        proposals: Vec::new(),
                        turns: 0,
                        cost_micro_usd: 0,
                        tokens: Tokens::default(),
                    },
                    group.first_at,
                    group.last_at,
                ));
                rows.len() - 1
            }
        };
        let (row, first, last) = &mut rows[index];
        row.turns = row.turns.saturating_add(group.turns);
        row.cost_micro_usd = row.cost_micro_usd.saturating_add(group.cost_micro_usd);
        row.tokens = row.tokens.plus(group.tokens);
        if let Some(proposal) = group.proposal
            && !row.proposals.contains(&proposal)
        {
            row.proposals.push(proposal);
            row.proposals.sort();
        }
        *first = (*first).min(group.first_at);
        if group.last_at > *last {
            *last = group.last_at;
            row.model = group.model.clone();
        }
    }
    rows.sort_by(|a, b| {
        (a.0.part == CostPart::Review)
            .cmp(&(b.0.part == CostPart::Review))
            .then_with(|| a.1.cmp(&b.1))
    });
    report.workers = rows.into_iter().map(|(row, _, _)| row).collect();
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis).unwrap()
    }

    fn group(agent: &str, role: Role, area: Option<&str>, cost: u64) -> TurnGroup {
        TurnGroup {
            agent: AgentName::parse(agent).unwrap(),
            role,
            area: area.map(|area| AreaName::parse(area).unwrap()),
            state: WorkerState::Idle,
            model: Some("sonnet".to_owned()),
            task: None,
            proposal: None,
            subject: None,
            turns: 1,
            cost_micro_usd: cost,
            tokens: tokens(1),
            first_at: at(10),
            last_at: at(20),
        }
    }

    fn built(mut group: TurnGroup, task: u32, title: &str) -> TurnGroup {
        let task = TaskId::from_number(task).unwrap();
        group.task = Some(task);
        group.subject = Some((task, title.to_owned()));
        group
    }

    fn reviewed(mut group: TurnGroup, proposal: u32, task: u32, title: &str) -> TurnGroup {
        group.proposal = ProposalId::from_number(proposal);
        group.subject = Some((TaskId::from_number(task).unwrap(), title.to_owned()));
        group
    }

    /// The tokens of `turns` groups of [`group`].
    fn tokens(turns: u64) -> Tokens {
        Tokens {
            input: turns,
            output: 2 * turns,
            cache_read: 30 * turns,
            cache_write: 4 * turns,
        }
    }

    fn area(name: &str) -> Option<AreaName> {
        Some(AreaName::parse(name).unwrap())
    }

    #[test]
    fn a_turn_costs_what_the_sessions_total_grew_by() {
        assert_eq!(session_turn(2970, 0), (2970, 2970));
        assert_eq!(session_turn(6514, 6136), (378, 6514));
        assert_eq!(session_turn(6136, 6136), (0, 6136));
        assert_eq!(session_turn(0, 6136), (0, 6136));
        assert_eq!(session_turn(300, 6136), (300, 300));
    }

    #[test]
    fn a_range_of_a_week_has_quarter_hour_slots_and_a_longer_one_hours() {
        let now = at(30 * 24 * 60 * 60 * 1000);
        let before = |millis: i64| Some(at(now.as_millis() - millis));
        assert_eq!(slot_millis(before(60_000), now), SHORT_SLOT_MILLIS);
        assert_eq!(
            slot_millis(before(SHORT_RANGE_MILLIS), now),
            SHORT_SLOT_MILLIS
        );
        assert_eq!(
            slot_millis(before(SHORT_RANGE_MILLIS + 1), now),
            LONG_SLOT_MILLIS
        );
        assert_eq!(slot_millis(None, now), LONG_SLOT_MILLIS);
    }

    #[test]
    fn no_turns_cost_nothing_and_every_role_is_listed() {
        let report = report(&[]);
        assert_eq!(report.total, Amount::default());
        assert_eq!(report.first_turn_at, None);
        assert_eq!(
            report.by_role,
            Role::ALL
                .into_iter()
                .map(|role| (role, Amount::default()))
                .collect::<Vec<_>>()
        );
        assert!(report.by_area.is_empty() && report.workers.is_empty() && report.tasks.is_empty());
    }

    #[test]
    fn the_splits_add_up_and_reviews_have_no_area() {
        let mut later = built(
            group("web-worker", Role::TaskWorker, Some("web"), 300),
            2,
            "two",
        );
        later.model = Some("opus".to_owned());
        later.first_at = at(5);
        later.last_at = at(40);
        let groups = [
            group("web-lead", Role::AreaLead, Some("web"), 100),
            built(
                group("web-worker", Role::TaskWorker, Some("web"), 200),
                1,
                "one",
            ),
            later,
            built(
                group("api-worker", Role::TaskWorker, Some("api"), 50),
                3,
                "three",
            ),
            reviewed(group("reviewer-1", Role::Reviewer, None, 80), 7, 1, "one"),
            group("review-lead", Role::ReviewLead, None, 20),
        ];
        let report = report(&groups);
        assert_eq!(
            report.total,
            Amount {
                cost_micro_usd: 750,
                tokens: tokens(6),
                turns: 6,
                workers: 5
            }
        );
        assert_eq!(report.first_turn_at, Some(at(5)));
        let role = |role: Role| {
            report
                .by_role
                .iter()
                .find(|(known, _)| *known == role)
                .unwrap()
                .1
        };
        assert_eq!(
            role(Role::TaskWorker),
            Amount {
                cost_micro_usd: 550,
                tokens: tokens(3),
                turns: 3,
                workers: 2
            }
        );
        assert_eq!(role(Role::ReviewLead).cost_micro_usd, 20);
        assert_eq!(
            report.by_area,
            [
                (
                    area("web"),
                    Amount {
                        cost_micro_usd: 600,
                        tokens: tokens(3),
                        turns: 3,
                        workers: 2
                    }
                ),
                (
                    None,
                    Amount {
                        cost_micro_usd: 100,
                        tokens: tokens(2),
                        turns: 2,
                        workers: 2
                    }
                ),
                (
                    area("api"),
                    Amount {
                        cost_micro_usd: 50,
                        tokens: tokens(1),
                        turns: 1,
                        workers: 1
                    }
                ),
            ]
        );
        assert_eq!(
            report.by_model,
            [
                (
                    Some("sonnet".to_owned()),
                    Amount {
                        cost_micro_usd: 450,
                        tokens: tokens(5),
                        turns: 5,
                        workers: 5
                    }
                ),
                (
                    Some("opus".to_owned()),
                    Amount {
                        cost_micro_usd: 300,
                        tokens: tokens(1),
                        turns: 1,
                        workers: 1
                    }
                ),
            ]
        );
        let worker = &report.workers[0];
        assert_eq!(
            (worker.agent.as_str(), worker.turns, worker.cost_micro_usd),
            ("web-worker", 2, 500)
        );
        assert_eq!(
            worker.model.as_deref(),
            Some("opus"),
            "the newest turn's model"
        );
        let tasks: Vec<(String, u64, u64, Option<AreaName>)> = report
            .tasks
            .iter()
            .map(|task| {
                (
                    task.task.to_string(),
                    task.build_micro_usd,
                    task.review_micro_usd,
                    task.area.clone(),
                )
            })
            .collect();
        assert_eq!(
            tasks,
            [
                ("T-2".to_owned(), 300, 0, area("web")),
                ("T-1".to_owned(), 200, 80, area("web")),
                ("T-3".to_owned(), 50, 0, area("api")),
            ]
        );
        assert_eq!(report.tasks[1].total_micro_usd(), 280);
        assert_eq!(report.tasks[1].tokens, tokens(2));
        assert_eq!(report.workers[0].tokens, tokens(2));
        assert_eq!(tokens(2).total(), 74);
        assert_eq!(report.tasks[1].turns, 2);
    }

    #[test]
    fn equal_costs_keep_a_stable_order() {
        let groups = [
            built(group("b", Role::TaskWorker, Some("web"), 10), 2, "two"),
            built(group("a", Role::TaskWorker, Some("web"), 10), 1, "one"),
        ];
        let report = report(&groups);
        let names: Vec<&str> = report.workers.iter().map(|w| w.agent.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        let tasks: Vec<String> = report.tasks.iter().map(|t| t.task.to_string()).collect();
        assert_eq!(tasks, ["T-1", "T-2"]);
    }

    #[test]
    fn a_task_reviewed_before_it_was_built_has_no_area_until_a_build() {
        let only_review = report(&[reviewed(group("r", Role::Reviewer, None, 5), 4, 1, "one")]);
        assert_eq!(only_review.tasks[0].area, None);
        assert_eq!(only_review.tasks[0].build_micro_usd, 0);
    }

    #[test]
    fn a_tasks_cost_lists_its_builders_then_its_reviewers() {
        let mut second = built(
            group("web-worker", Role::TaskWorker, Some("web"), 30),
            1,
            "one",
        );
        second.model = Some("opus".to_owned());
        second.last_at = at(90);
        let mut early_review = reviewed(group("reviewer-2", Role::Reviewer, None, 7), 5, 1, "one");
        early_review.first_at = at(1);
        let groups = [
            reviewed(group("reviewer-1", Role::Reviewer, None, 9), 6, 1, "one"),
            built(
                group("web-worker", Role::TaskWorker, Some("web"), 70),
                1,
                "one",
            ),
            second,
            reviewed(group("reviewer-1", Role::Reviewer, None, 1), 4, 1, "one"),
            early_review,
        ];
        let report = task_report(&groups);
        assert_eq!(
            (
                report.build_micro_usd,
                report.review_micro_usd,
                report.turns
            ),
            (100, 17, 5)
        );
        assert_eq!(report.tokens, tokens(5));
        assert_eq!(report.workers[0].tokens, tokens(2));
        type Row<'a> = (&'a str, CostPart, u64, u64, Vec<String>, Option<&'a str>);
        let rows: Vec<Row> = report
            .workers
            .iter()
            .map(|row| {
                (
                    row.agent.as_str(),
                    row.part,
                    row.turns,
                    row.cost_micro_usd,
                    row.proposals.iter().map(ToString::to_string).collect(),
                    row.model.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("web-worker", CostPart::Build, 2, 100, vec![], Some("opus")),
                (
                    "reviewer-2",
                    CostPart::Review,
                    1,
                    7,
                    vec!["P-5".to_owned()],
                    Some("sonnet")
                ),
                (
                    "reviewer-1",
                    CostPart::Review,
                    2,
                    10,
                    vec!["P-4".to_owned(), "P-6".to_owned()],
                    Some("sonnet")
                ),
            ]
        );
        assert_eq!(task_report(&[]), TaskCostReport::default());
    }
}
