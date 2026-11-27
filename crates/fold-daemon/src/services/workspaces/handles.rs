use std::error::Error;
use std::future::Future;
use std::io;
use std::time::Duration;

use fold_core::{FoldError, codes};
use serde_json::json;

/// The wait before the one retry of a workspace placement that ran out of file handles: long enough for parallel creates and check runs to close theirs, short enough that the caller does not notice.
pub(super) const OUT_OF_HANDLES_BACKOFF: Duration = Duration::from_millis(200);

/// The number of attempts a placement makes when every one runs out of file handles.
pub(super) const OUT_OF_HANDLES_ATTEMPTS: u32 = 2;

/// A failed placement attempt, with whether the process or the system ran out of file handles (`EMFILE` or `ENFILE`), which is worth one retry: the handles of parallel creates are freed a moment later. The flag is read from the `io::Error` before the conversion to a `FoldError`, which drops it.
#[derive(Debug)]
pub(super) struct Failed {
    pub error: FoldError,
    pub out_of_handles: bool,
}

impl Failed {
    /// A failure that no retry helps.
    pub fn plain(error: FoldError) -> Self {
        Self {
            error,
            out_of_handles: false,
        }
    }

    /// A failure classified by the `io::Error` anywhere in its source chain.
    pub fn classify<E: Error + Into<FoldError> + 'static>(error: E) -> Self {
        let out_of_handles = out_of_handles(&error);
        Self {
            error: error.into(),
            out_of_handles,
        }
    }
}

/// True when `error` or one of its sources is an `io::Error` for `EMFILE` or `ENFILE`.
pub(super) fn out_of_handles(error: &(dyn Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(io) = error.downcast_ref::<io::Error>()
            && matches!(
                io.raw_os_error(),
                Some(code) if code == rustix::io::Errno::MFILE.raw_os_error()
                    || code == rustix::io::Errno::NFILE.raw_os_error()
            )
        {
            return true;
        }
        current = error.source();
    }
    false
}

/// Runs `attempt`, and once more after `backoff` when it ran out of file handles. A second such failure becomes [`out_of_handles_error`]; any other failure is returned as it is.
pub(super) async fn retry_out_of_handles<T, F, Fut>(
    mut attempt: F,
    backoff: Duration,
) -> Result<T, FoldError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Failed>>,
{
    for tried in 1..=OUT_OF_HANDLES_ATTEMPTS {
        match attempt().await {
            Ok(value) => return Ok(value),
            Err(failed) if !failed.out_of_handles => return Err(failed.error),
            Err(_) if tried < OUT_OF_HANDLES_ATTEMPTS => {
                tracing::warn!(
                    attempt = tried,
                    "a workspace placement ran out of file handles; retrying"
                );
                tokio::time::sleep(backoff).await;
            }
            Err(_) => {}
        }
    }
    Err(out_of_handles_error())
}

/// `workspace_io` with the reason `out_of_handles`, after the retry also ran out of file handles.
pub(crate) fn out_of_handles_error() -> FoldError {
    FoldError::new(
        codes::WORKSPACE_IO,
        "the machine ran out of file handles while it wrote the workspace files",
    )
    .with_details([
        ("reason", json!("out_of_handles")),
        ("attempts", json!(OUT_OF_HANDLES_ATTEMPTS)),
    ])
    .with_hint("the machine ran out of file handles; wait a moment, then call the same tool again")
}

/// True for the error of [`out_of_handles_error`], which a caller that wraps placement errors passes on as it is, so its hint reaches the agent.
pub(crate) fn is_out_of_handles_error(error: &FoldError) -> bool {
    error.code() == codes::WORKSPACE_IO
        && error
            .details()
            .and_then(|details| details.get("reason"))
            .is_some_and(|reason| reason == "out_of_handles")
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn errno(errno: rustix::io::Errno) -> io::Error {
        io::Error::from_raw_os_error(errno.raw_os_error())
    }

    fn exhausted() -> Failed {
        Failed::classify(fold_vcs::paths::WorkspaceError::from(errno(
            rustix::io::Errno::MFILE,
        )))
    }

    #[test]
    fn emfile_and_enfile_are_out_of_handles_through_the_source_chain() {
        assert!(out_of_handles(&errno(rustix::io::Errno::MFILE)));
        assert!(out_of_handles(&errno(rustix::io::Errno::NFILE)));
        assert!(!out_of_handles(&errno(rustix::io::Errno::NOENT)));
        assert!(!out_of_handles(&io::Error::other("no os error")));
        let wrapped = fold_vcs::paths::WorkspaceError::from(errno(rustix::io::Errno::NFILE));
        assert!(out_of_handles(&wrapped));
        let object = fold_vcs::objects::ObjectError::from(errno(rustix::io::Errno::MFILE));
        assert!(out_of_handles(&object));
        assert!(exhausted().out_of_handles);
        assert!(!Failed::plain(FoldError::new(codes::WORKSPACE_IO, "x")).out_of_handles);
    }

    #[tokio::test]
    async fn one_retry_then_workspace_io_with_a_hint() {
        let attempts = Cell::new(0);
        let result: Result<(), FoldError> = retry_out_of_handles(
            || {
                attempts.set(attempts.get() + 1);
                async { Err(exhausted()) }
            },
            Duration::ZERO,
        )
        .await;
        assert_eq!(attempts.get(), 2);
        let Err(error) = result else {
            panic!("an exhausted retry succeeded");
        };
        assert_eq!(error.code(), codes::WORKSPACE_IO);
        assert!(is_out_of_handles_error(&error));
        assert!(!is_out_of_handles_error(&FoldError::new(
            codes::WORKSPACE_IO,
            "x"
        )));
        let hint = error.hint().unwrap_or_default();
        assert!(hint.contains("file handles"), "hint: {hint}");
    }

    #[tokio::test]
    async fn the_retry_returns_what_the_second_attempt_gives() {
        let attempts = Cell::new(0);
        let result = retry_out_of_handles(
            || {
                attempts.set(attempts.get() + 1);
                let first = attempts.get() == 1;
                async move { if first { Err(exhausted()) } else { Ok(7) } }
            },
            Duration::ZERO,
        )
        .await;
        assert_eq!(attempts.get(), 2);
        assert_eq!(result.ok(), Some(7));
    }

    #[tokio::test]
    async fn another_failure_is_not_retried() {
        let attempts = Cell::new(0);
        let result: Result<(), FoldError> = retry_out_of_handles(
            || {
                attempts.set(attempts.get() + 1);
                async { Err(Failed::plain(FoldError::new(codes::INVALID_PATH, "bad"))) }
            },
            Duration::ZERO,
        )
        .await;
        assert_eq!(attempts.get(), 1);
        assert_eq!(result.err().map(|e| e.code()), Some(codes::INVALID_PATH));
    }

    #[tokio::test]
    async fn a_second_failure_of_another_kind_is_returned_as_it_is() {
        let attempts = Cell::new(0);
        let result: Result<(), FoldError> = retry_out_of_handles(
            || {
                attempts.set(attempts.get() + 1);
                let first = attempts.get() == 1;
                async move {
                    if first {
                        Err(exhausted())
                    } else {
                        Err(Failed::plain(FoldError::new(codes::INVALID_PATH, "bad")))
                    }
                }
            },
            Duration::ZERO,
        )
        .await;
        assert_eq!(attempts.get(), 2);
        assert_eq!(result.err().map(|e| e.code()), Some(codes::INVALID_PATH));
    }
}
