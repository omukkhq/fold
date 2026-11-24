use std::future::Future;
use std::time::Duration;

use crate::daemon::DRAIN_TIMEOUT;

// The daemon's stop runs these phases one after the other; inside a phase every project, worker and job goes at the same time, so the sum below is the worst case whatever the number of projects, workspaces or workers: the jobs (`DRAIN_TIMEOUT`, 5 s), the worker stops (22 s), the socket and HTTP drains (6 s), each project's stop (checks 2 s, live detach 2 s, project tasks 3 s, sessions 1 s, database 2 s) and the registry (1 s) make 44 s, inside the 45 s that leaves `fld restart`'s 60 s `STOP_TIMEOUT` a margin. The worker stops get most of it, because a clean stop keeps each worker resumable: the default stop steps send SIGKILL after 20 s.

/// How long the stop of one project's workers may take, the read of their rows included, before fold sends SIGKILL to the process groups still running.
pub const WORKER_STOPS_LIMIT: Duration = Duration::from_secs(22);
/// How long the socket's connections may take to drain, a little more than the `DRAIN_TIMEOUT` after which the socket cuts them.
pub const SOCKET_LIMIT: Duration = Duration::from_secs(6);
/// How long a project's check runs may take to end.
pub const CHECKS_LIMIT: Duration = Duration::from_secs(2);
/// How long the live state of all of a project's workspaces may take to let go.
pub const LIVE_DETACH_LIMIT: Duration = Duration::from_secs(2);
/// How long a project's commit writer, push delivery, cleanup, stall and watch tasks may take to end, all at the same time.
pub const PROJECT_TASKS_LIMIT: Duration = Duration::from_secs(3);
/// How long the end of a project's open sessions may take.
pub const CLOSE_SESSIONS_LIMIT: Duration = Duration::from_secs(1);
/// How long the checkpoint and close of a project database may take.
pub const STORE_CLOSE_LIMIT: Duration = Duration::from_secs(2);
/// How long the checkpoint and close of the registry may take.
pub const REGISTRY_CLOSE_LIMIT: Duration = Duration::from_secs(1);

/// The worst case of the whole stop.
pub const WORST_CASE: Duration = Duration::from_secs(
    DRAIN_TIMEOUT.as_secs()
        + WORKER_STOPS_LIMIT.as_secs()
        + SOCKET_LIMIT.as_secs()
        + CHECKS_LIMIT.as_secs()
        + LIVE_DETACH_LIMIT.as_secs()
        + PROJECT_TASKS_LIMIT.as_secs()
        + CLOSE_SESSIONS_LIMIT.as_secs()
        + STORE_CLOSE_LIMIT.as_secs()
        + REGISTRY_CLOSE_LIMIT.as_secs(),
);

const _: () = assert!(WORST_CASE.as_secs() < 45);
const _: () = assert!(DRAIN_TIMEOUT.as_secs() < SOCKET_LIMIT.as_secs());

/// Runs one step of the stop for at most `limit`. A step that goes over it is dropped, logged with its name and the project's slug, and the stop goes on with the next step: `None`. A dropped write transaction rolls back, so a step let go leaves no write half done.
pub(crate) async fn within<T>(
    step: &'static str,
    project: Option<&str>,
    limit: Duration,
    work: impl Future<Output = T>,
) -> Option<T> {
    let limit = test_limit(limit);
    #[cfg(feature = "failpoints")]
    let work = async {
        hold("shutdown_hold", step).await;
        work.await
    };
    match tokio::time::timeout(limit, work).await {
        Ok(done) => Some(done),
        Err(_) => {
            let limit_ms = u64::try_from(limit.as_millis()).unwrap_or(u64::MAX);
            match project {
                Some(project) => tracing::warn!(
                    step,
                    project,
                    limit_ms,
                    "a step of the daemon stop went over its time limit and was let go"
                ),
                None => tracing::warn!(
                    step,
                    limit_ms,
                    "a step of the daemon stop went over its time limit and was let go"
                ),
            }
            None
        }
    }
}

/// The failpoint `shutdown_limit_ms` with `return(<ms>)` gives every step that limit instead, so a test sees a held step let go quickly.
fn test_limit(limit: Duration) -> Duration {
    #[cfg(feature = "failpoints")]
    fail::fail_point!("shutdown_limit_ms", |ms: Option<String>| {
        ms.and_then(|ms| ms.parse().ok())
            .map_or(limit, Duration::from_millis)
    });
    limit
}

/// Never resolves while the failpoint `point` is set to `return(<name>)`. Unlike the `pause` and `sleep` actions, which block the thread, it holds only its own task, so a time limit around it still fires.
#[cfg(feature = "failpoints")]
pub(crate) async fn hold(point: &str, name: &str) {
    let held = (|| {
        fail::fail_point!(point, |arg: Option<String>| arg.as_deref() == Some(name));
        false
    })();
    if held {
        std::future::pending::<()>().await;
    }
}

/// Waits while the failpoint `point` is set to `return(<name>)` and goes on once a test removes it, so a test can hold a step of a project's close that runs without a limit and then let it finish.
#[cfg(feature = "failpoints")]
pub(crate) async fn hold_while(point: &str, name: &str) {
    loop {
        let held = (|| {
            fail::fail_point!(point, |arg: Option<String>| arg.as_deref() == Some(name));
            false
        })();
        if !held {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
