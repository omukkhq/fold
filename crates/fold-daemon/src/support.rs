use std::sync::{LockResult, Mutex, MutexGuard, PoisonError};

use fold_core::{FoldError, codes};

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
    async fn blocking_work_gives_its_result_or_an_internal_error() {
        assert_eq!(blocking(|| Ok(7)).await.unwrap(), 7);
        let refused = blocking(|| Err::<(), _>(FoldError::new(codes::INVALID_NAME, "bad"))).await;
        assert_eq!(refused.unwrap_err().code(), codes::INVALID_NAME);
        let stopped = blocking(|| -> Result<(), FoldError> { panic!("the work panics") }).await;
        assert_eq!(stopped.unwrap_err().code(), codes::INTERNAL_ERROR);
    }
}
