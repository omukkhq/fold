use std::sync::Arc;

use tokio::sync::{Mutex, OwnedMutexGuard, watch};

/// Which place in the integration queue a caller takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnKind {
    /// A direct change of the main agent: the user approved it in the main agent's conversation and the main agent waits for its answer, so it goes before every integration that waits.
    First,
    /// Every other integration: a proposal, automatic or queued by hand, a review batch, a pull, a record, a reset or an untrack.
    Queued,
}

/// Whether a caller of `kind` may start its integration while `waiting_first` direct changes wait for the mutex. A direct change always may; any other integration only when no direct change waits, so a direct change waits at most for the integration that runs when it is submitted.
#[must_use]
pub fn may_take_turn(kind: TurnKind, waiting_first: usize) -> bool {
    match kind {
        TurnKind::First => true,
        TurnKind::Queued => waiting_first == 0,
    }
}

/// The project's integration mutex with first place for direct changes. `turn` is the mutex every integration holds while it runs, so integrations stay serial. A direct change counts itself in `first` and then waits on `turn`, in FIFO order with the other direct changes. Every other integration first takes `queue`, FIFO, so only one of them at a time contends with the direct changes and their order among themselves stays the order of their calls; it then takes `turn` only while no direct change waits, and gives `turn` back at once when one started to wait meanwhile.
#[derive(Debug)]
pub struct IntegrationLock {
    turn: Arc<Mutex<()>>,
    queue: Arc<Mutex<()>>,
    first: watch::Sender<usize>,
}

impl Default for IntegrationLock {
    fn default() -> Self {
        Self {
            turn: Arc::new(Mutex::new(())),
            queue: Arc::new(Mutex::new(())),
            first: watch::Sender::new(0),
        }
    }
}

/// One integration's hold of the project's integration mutex. Dropping it lets the next integration go.
#[derive(Debug)]
#[must_use = "the integration turn ends when the guard drops"]
pub struct IntegrationTurn {
    _turn: OwnedMutexGuard<()>,
    _queue: Option<OwnedMutexGuard<()>>,
}

/// Counts one waiting direct change until it holds `turn` or its caller gives up, so a cancelled call never leaves a count behind.
struct WaitingFirst<'a>(&'a watch::Sender<usize>);

impl<'a> WaitingFirst<'a> {
    fn new(first: &'a watch::Sender<usize>) -> Self {
        first.send_modify(|count| *count = count.saturating_add(1));
        Self(first)
    }
}

impl Drop for WaitingFirst<'_> {
    fn drop(&mut self) {
        self.0.send_modify(|count| *count = count.saturating_sub(1));
    }
}

impl IntegrationLock {
    /// Waits for a turn of `kind`. The guard is owned, so it can outlive the caller's borrow.
    pub async fn lock_as(&self, kind: TurnKind) -> IntegrationTurn {
        match kind {
            TurnKind::First => self.lock_first().await,
            TurnKind::Queued => self.lock().await,
        }
    }

    /// Waits for a turn behind the running integration and every direct change that waits, in the order of the calls among the other integrations.
    pub async fn lock(&self) -> IntegrationTurn {
        let queue = Arc::clone(&self.queue).lock_owned().await;
        let mut waiting = self.first.subscribe();
        loop {
            // The sender lives in `self`, so the wait ends only on a count that lets the turn go.
            let _ = waiting
                .wait_for(|count| may_take_turn(TurnKind::Queued, *count))
                .await;
            let turn = Arc::clone(&self.turn).lock_owned().await;
            if may_take_turn(TurnKind::Queued, *self.first.borrow()) {
                return IntegrationTurn {
                    _turn: turn,
                    _queue: Some(queue),
                };
            }
            drop(turn);
        }
    }

    /// Waits for a direct change's turn: only behind the integration that runs and the direct changes that called before it.
    pub async fn lock_first(&self) -> IntegrationTurn {
        let waiting = WaitingFirst::new(&self.first);
        let turn = Arc::clone(&self.turn).lock_owned().await;
        drop(waiting);
        IntegrationTurn {
            _turn: turn,
            _queue: None,
        }
    }

    /// How many direct changes wait for their turn now.
    #[must_use]
    pub fn waiting_first(&self) -> usize {
        *self.first.borrow()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    use super::*;

    const WAIT: Duration = Duration::from_secs(10);

    #[test]
    fn a_direct_change_always_may_take_its_turn() {
        assert!(may_take_turn(TurnKind::First, 0));
        assert!(may_take_turn(TurnKind::First, 3));
    }

    #[test]
    fn another_integration_waits_while_a_direct_change_waits() {
        assert!(may_take_turn(TurnKind::Queued, 0));
        assert!(!may_take_turn(TurnKind::Queued, 1));
    }

    /// Waits until `lock` counts `count` waiting direct changes.
    async fn first_waiting(lock: &IntegrationLock, count: usize) {
        let mut seen = lock.first.subscribe();
        tokio::time::timeout(WAIT, seen.wait_for(|now| *now == count))
            .await
            .expect("the count arrives")
            .expect("the sender lives");
    }

    /// Spawns a caller of `kind` that writes `name` into `order` once it holds its turn, and holds it until `release` fires.
    fn caller(
        lock: &Arc<IntegrationLock>,
        kind: TurnKind,
        name: &'static str,
        order: &Arc<StdMutex<Vec<&'static str>>>,
    ) -> (
        tokio::task::JoinHandle<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (release, released) = tokio::sync::oneshot::channel();
        let (lock, order) = (Arc::clone(lock), Arc::clone(order));
        let task = tokio::spawn(async move {
            let _turn = lock.lock_as(kind).await;
            order.lock().unwrap().push(name);
            let _ = released.await;
        });
        (task, release)
    }

    async fn turns_taken(
        order: &Arc<StdMutex<Vec<&'static str>>>,
        count: usize,
    ) -> Vec<&'static str> {
        tokio::time::timeout(WAIT, async {
            loop {
                let now = order.lock().unwrap().clone();
                if now.len() >= count {
                    return now;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the turns are taken")
    }

    #[tokio::test]
    async fn a_direct_change_goes_before_the_waiting_integrations_and_two_keep_their_order() {
        let lock = Arc::new(IntegrationLock::default());
        let order = Arc::new(StdMutex::new(Vec::new()));
        let (running, end_running) = caller(&lock, TurnKind::Queued, "running", &order);
        assert_eq!(turns_taken(&order, 1).await, ["running"]);
        let (b, end_b) = caller(&lock, TurnKind::Queued, "b", &order);
        let (c, end_c) = caller(&lock, TurnKind::Queued, "c", &order);
        let (d1, end_d1) = caller(&lock, TurnKind::First, "d1", &order);
        first_waiting(&lock, 1).await;
        let (d2, end_d2) = caller(&lock, TurnKind::First, "d2", &order);
        first_waiting(&lock, 2).await;
        end_running.send(()).unwrap();
        assert_eq!(turns_taken(&order, 2).await, ["running", "d1"]);
        end_d1.send(()).unwrap();
        assert_eq!(turns_taken(&order, 3).await, ["running", "d1", "d2"]);
        end_d2.send(()).unwrap();
        assert_eq!(turns_taken(&order, 4).await, ["running", "d1", "d2", "b"]);
        end_b.send(()).unwrap();
        end_c.send(()).unwrap();
        for task in [running, b, c, d1, d2] {
            task.await.unwrap();
        }
        assert_eq!(*order.lock().unwrap(), ["running", "d1", "d2", "b", "c"]);
        assert_eq!(lock.waiting_first(), 0);
    }

    #[tokio::test]
    async fn no_integration_starts_while_a_direct_change_holds_its_turn() {
        let lock = Arc::new(IntegrationLock::default());
        let order = Arc::new(StdMutex::new(Vec::new()));
        let (direct, end_direct) = caller(&lock, TurnKind::First, "direct", &order);
        assert_eq!(turns_taken(&order, 1).await, ["direct"]);
        let (queued, end_queued) = caller(&lock, TurnKind::Queued, "queued", &order);
        let (second, end_second) = caller(&lock, TurnKind::First, "second", &order);
        first_waiting(&lock, 1).await;
        assert!(lock.turn.try_lock().is_err());
        assert_eq!(*order.lock().unwrap(), ["direct"]);
        end_direct.send(()).unwrap();
        assert_eq!(turns_taken(&order, 2).await, ["direct", "second"]);
        end_second.send(()).unwrap();
        assert_eq!(turns_taken(&order, 3).await, ["direct", "second", "queued"]);
        end_queued.send(()).unwrap();
        for task in [direct, queued, second] {
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_cancelled_direct_change_gives_up_its_place() {
        let lock = Arc::new(IntegrationLock::default());
        let held = lock.lock().await;
        let waiting = {
            let lock = Arc::clone(&lock);
            tokio::spawn(async move {
                let _turn = lock.lock_first().await;
            })
        };
        first_waiting(&lock, 1).await;
        waiting.abort();
        let _ = waiting.await;
        assert_eq!(lock.waiting_first(), 0);
        drop(held);
        let _next = tokio::time::timeout(WAIT, lock.lock())
            .await
            .expect("the next integration takes its turn");
    }
}
