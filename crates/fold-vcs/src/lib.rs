pub mod atomic;
pub mod batch;
pub mod browse;
pub mod checkout;
pub mod compare;
pub mod diff;
pub mod edit;
pub mod gc;
pub mod git;
pub mod git_export;
pub mod git_import;
pub mod git_remote;
pub mod git_sync_map;
pub mod ignore;
pub mod import;
pub mod index;
pub mod integrate;
pub mod materialise;
pub mod objects;
pub mod paths;
pub mod platform;
pub mod restore;
pub mod revision_store;
pub mod scan;
pub mod snapshot;
pub mod sync;
pub mod watch;

/// The guard of `mutex`, also when an earlier holder panicked: the caches and sets behind fold-vcs's mutexes stay valid after a panic.
pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
