use std::time::Duration;

use fold_core::{Role, Timestamp};

/// How many workers a daemon start runs again in one group, across every project.
pub const DEFAULT_RESUME_GROUP: usize = 4;

/// Time on the clock between two groups of workers that a daemon start runs again.
pub const DEFAULT_RESUME_GAP: Duration = Duration::from_secs(10);

/// Where a worker that a daemon start runs again stands in the order of the groups: the resumes fold owes stopped workers first, such as for requested changes or a message, then a worker that owes a turn before one that does not, and within each a lead before the others, since a lead's turn starts the work of others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResumeRank {
    Resume,
    OwedLead,
    Owed,
    Lead,
    Other,
}

impl ResumeRank {
    /// The rank of a worker of `role`; `owes_turn` is true when the start sends it a turn of its own, such as a worker that was working when the daemon stopped.
    pub fn of(role: Role, owes_turn: bool) -> Self {
        let lead = matches!(role, Role::AreaLead | Role::ReviewLead | Role::Main);
        match (owes_turn, lead) {
            (true, true) => Self::OwedLead,
            (true, false) => Self::Owed,
            (false, true) => Self::Lead,
            (false, false) => Self::Other,
        }
    }
}

/// The workers that wait for their resume group, in the order they came within each rank. The first group is due at once; each later group is due `gap` after the one before it.
#[derive(Debug)]
pub struct GroupQueue<T> {
    waiting: Vec<(ResumeRank, u64, T)>,
    next_seq: u64,
    next_at: Option<Timestamp>,
}

impl<T> Default for GroupQueue<T> {
    fn default() -> Self {
        Self {
            waiting: Vec::new(),
            next_seq: 0,
            next_at: None,
        }
    }
}

impl<T> GroupQueue<T> {
    pub fn push(&mut self, rank: ResumeRank, item: T) {
        self.waiting.push((rank, self.next_seq, item));
        self.next_seq += 1;
    }

    /// The next group when it is due at `now`, at most `group` items and at least one, in rank order; the group after it is due `gap` later.
    pub fn due(&mut self, now: Timestamp, group: usize, gap: Duration) -> Vec<T> {
        if self.waiting.is_empty() || self.next_at.is_some_and(|at| now < at) {
            return Vec::new();
        }
        self.next_at = now.checked_add(gap);
        self.waiting.sort_by_key(|(rank, seq, _)| (*rank, *seq));
        let size = group.clamp(1, self.waiting.len());
        self.waiting
            .drain(..size)
            .map(|(_, _, item)| item)
            .collect()
    }

    /// Keeps only the waiting items for which `keep` is true.
    pub fn retain(&mut self, mut keep: impl FnMut(&T) -> bool) {
        self.waiting.retain(|(_, _, item)| keep(item));
    }

    pub fn len(&self) -> usize {
        self.waiting.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_millis(seconds * 1000).unwrap()
    }

    const GAP: Duration = Duration::from_secs(10);

    #[test]
    fn owed_work_and_leads_rank_first() {
        assert_eq!(ResumeRank::of(Role::AreaLead, true), ResumeRank::OwedLead);
        assert_eq!(ResumeRank::of(Role::ReviewLead, true), ResumeRank::OwedLead);
        assert_eq!(ResumeRank::of(Role::Main, false), ResumeRank::Lead);
        assert_eq!(ResumeRank::of(Role::TaskWorker, true), ResumeRank::Owed);
        assert_eq!(ResumeRank::of(Role::Reviewer, false), ResumeRank::Other);
        assert!(ResumeRank::Resume < ResumeRank::OwedLead);
        assert!(ResumeRank::OwedLead < ResumeRank::Owed);
        assert!(ResumeRank::Owed < ResumeRank::Lead);
        assert!(ResumeRank::Lead < ResumeRank::Other);
    }

    #[test]
    fn ten_workers_go_four_then_four_after_the_gap_then_two() {
        let mut queue = GroupQueue::default();
        for n in 0..10 {
            queue.push(ResumeRank::Other, n);
        }
        assert_eq!(queue.due(at(0), 4, GAP), [0, 1, 2, 3]);
        assert!(queue.due(at(0), 4, GAP).is_empty(), "one group per gap");
        assert!(queue.due(at(9), 4, GAP).is_empty());
        assert_eq!(queue.due(at(10), 4, GAP), [4, 5, 6, 7]);
        assert_eq!(queue.len(), 2);
        assert!(queue.due(at(19), 4, GAP).is_empty());
        assert_eq!(queue.due(at(20), 4, GAP), [8, 9]);
        assert_eq!(queue.len(), 0);
        assert!(queue.due(at(40), 4, GAP).is_empty());
    }

    #[test]
    fn a_group_takes_owed_work_and_leads_first_in_the_order_they_came() {
        let mut queue = GroupQueue::default();
        queue.push(ResumeRank::Other, "idle-worker");
        queue.push(ResumeRank::Lead, "idle-lead");
        queue.push(ResumeRank::Owed, "working-worker");
        queue.push(ResumeRank::OwedLead, "working-lead");
        queue.push(ResumeRank::Owed, "second-working-worker");
        assert_eq!(
            queue.due(at(0), 3, GAP),
            ["working-lead", "working-worker", "second-working-worker"]
        );
        queue.push(ResumeRank::OwedLead, "late-lead");
        queue.push(ResumeRank::Resume, "owed-resume");
        assert_eq!(
            queue.due(at(10), 3, GAP),
            ["owed-resume", "late-lead", "idle-lead"]
        );
        assert_eq!(queue.due(at(20), 3, GAP), ["idle-worker"]);
    }

    #[test]
    fn a_group_has_at_least_one_and_a_late_push_waits_for_the_gap() {
        let mut queue = GroupQueue::default();
        queue.push(ResumeRank::Other, 1);
        queue.push(ResumeRank::Other, 2);
        assert_eq!(queue.due(at(0), 0, GAP), [1]);
        assert_eq!(queue.due(at(10), 0, GAP), [2]);
        queue.push(ResumeRank::Other, 3);
        assert!(
            queue.due(at(15), 4, GAP).is_empty(),
            "the gap counts from the last group"
        );
        assert_eq!(queue.due(at(20), 4, GAP), [3]);
    }

    #[test]
    fn retain_drops_the_items_that_left() {
        let mut queue = GroupQueue::default();
        for n in 0..5 {
            queue.push(ResumeRank::Other, n);
        }
        queue.retain(|n| n % 2 == 0);
        assert_eq!(queue.due(at(0), 4, GAP), [0, 2, 4]);
    }
}
