use std::sync::{LockResult, Mutex, MutexGuard, PoisonError};

use fold_core::{FoldError, codes};

/// Session-specific variables of a Claude Code session that started the daemon. A worker is a session of its own and a check command is no session, so neither gets them.
pub(crate) const SESSION_ENV: [&str; 5] = [
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SSE_PORT",
];

/// The variables a service manager (systemd or launchd) sets for the daemon's unit. A process the daemon starts is not part of the unit, and a `fld serve` it starts would record the service as its starter.
pub(crate) const SERVICE_ENV: [&str; 5] = [
    "FOLD_SERVICE",
    "INVOCATION_ID",
    "JOURNAL_STREAM",
    "MANAGERPID",
    "SYSTEMD_EXEC_PID",
];

/// Takes the variables of the Claude Code session and of the service manager that started the daemon out of what `command` inherits.
pub(crate) fn remove_daemon_context(command: &mut tokio::process::Command) {
    for name in SESSION_ENV.iter().chain(&SERVICE_ENV) {
        command.env_remove(name);
    }
}

/// What a lock call gives, also when an earlier holder panicked. The state behind every lock of the daemon stays valid after a panic, so the next holder continues instead of the daemon failing for good.
pub(crate) fn unpoisoned<T>(result: LockResult<T>) -> T {
    result.unwrap_or_else(PoisonError::into_inner)
}

/// The guard of `mutex`, as [`unpoisoned`] gives it.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    unpoisoned(mutex.lock())
}

/// Runs `work` on the blocking pool. A panic or a cancelled task is an `internal_error`, never a panic of the caller.
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, FoldError> + Send + 'static,
) -> Result<T, FoldError> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|error| {
            Err(FoldError::new(
                codes::INTERNAL_ERROR,
                format!("blocking work stopped: {error}"),
            ))
        })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn a_poisoned_mutex_still_gives_its_value() {
        let mutex = Arc::new(Mutex::new(5));
        let poisoner = Arc::clone(&mutex);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.lock();
            panic!("the holder panics");
        })
        .join();
        assert!(mutex.is_poisoned());
        assert_eq!(*lock(&mutex), 5);
        let rw = Arc::new(std::sync::RwLock::new(6));
        let poisoner = Arc::clone(&rw);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.write();
            panic!("the holder panics");
        })
        .join();
        assert!(rw.is_poisoned());
        assert_eq!(*unpoisoned(rw.read()), 6);
    }

    #[tokio::test]
    async fn a_child_inherits_neither_session_nor_service_variables_but_keeps_the_rest() {
        let mut command = tokio::process::Command::new("/usr/bin/env");
        for name in SESSION_ENV.iter().chain(&SERVICE_ENV) {
            command.env(name, "set");
        }
        command.env("HOME", "/home/kept");
        remove_daemon_context(&mut command);
        let output = command.output().await.unwrap();
        let seen = String::from_utf8(output.stdout).unwrap();
        let names: Vec<&str> = seen
            .lines()
            .filter_map(|line| line.split_once('=').map(|(name, _)| name))
            .collect();
        for name in SESSION_ENV.iter().chain(&SERVICE_ENV) {
            assert!(!names.contains(name), "{name}");
        }
        assert!(names.contains(&"PATH"));
        assert!(seen.contains("HOME=/home/kept"));
    }

    #[tokio::test]
    async fn blocking_work_gives_its_result_or_an_internal_error() {
        assert_eq!(blocking(|| Ok(7)).await.unwrap(), 7);
        let refused = blocking(|| Err::<(), _>(FoldError::new(codes::INVALID_NAME, "bad"))).await;
        assert_eq!(refused.unwrap_err().code(), codes::INVALID_NAME);
        let stopped = blocking(|| -> Result<(), FoldError> { panic!("the work panics") }).await;
        assert_eq!(stopped.unwrap_err().code(), codes::INTERNAL_ERROR);
    }
}
