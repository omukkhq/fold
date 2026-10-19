use std::path::Path;

use fold_core::events::types;
use fold_core::roles::{AreaLimitChange, WorkerLimits, WorkerLimitsChange, check_limits};
use fold_core::{
    Actor, AreaName, FoldError, NewEvent, ProjectConfig, ProjectPaths, Role, codes, policy,
};
use fold_store::{WriteTx, agents};
use fold_vcs::atomic::{EditLock, write_atomic};
use serde_json::{Value, json};
use toml_edit::{DocumentMut, Item, Table, TableLike, Value as TomlValue, value};

use super::super::{AgentError, AgentService};
use crate::services::events::EventBuffer;
use crate::services::projects::Lease;

/// The limits before and after a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerLimitsChanged {
    pub old: WorkerLimits,
    pub new: WorkerLimits,
}

/// Refuses one more worker of `role` in `area`, with its row id and name, when the running workers already fill a limit. Read on the writer inside the start, so two starts cannot both take the last place.
pub(super) async fn check_running(
    tx: &mut WriteTx,
    limits: WorkerLimits,
    role: Role,
    area: Option<(i64, &AreaName)>,
) -> Result<(), AgentError> {
    let (running, in_area) =
        agents::running_counts(tx.conn(), role, area.map(|(id, _)| id)).await?;
    check_limits(limits, role, running, area.map(|(_, name)| (name, in_area)))?;
    Ok(())
}

fn limits_json(limits: WorkerLimits) -> Value {
    json!({
        "task_worker": limits.task_worker,
        "reviewer": limits.reviewer,
        "max_per_area": limits.max_per_area,
    })
}

fn config_io(error: &std::io::Error) -> FoldError {
    FoldError::new(
        codes::WORKSPACE_IO,
        format!("cannot read or write config.toml: {}", error.kind()),
    )
}

fn joined<T>(result: Result<Result<T, FoldError>, tokio::task::JoinError>) -> Result<T, FoldError> {
    result.unwrap_or_else(|error| {
        Err(FoldError::new(
            codes::INTERNAL_ERROR,
            format!("the config rewrite stopped: {error}"),
        ))
    })
}

/// One rewrite of `config.toml`: the file as it was (`None` when it was missing), the text with the change written into `[workers]` (`None` when the change leaves the limits as they are), and the limits before and after.
struct Planned {
    original: Option<String>,
    text: Option<String>,
    changed: WorkerLimitsChanged,
}

/// The rewrite `change` asks for, every byte outside the changed keys kept. A missing file reads as empty.
fn edit(path: &Path, change: &WorkerLimitsChange) -> Result<Planned, FoldError> {
    let original = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(config_io(&error)),
    };
    let text = original.as_deref().unwrap_or_default();
    let old = ProjectConfig::from_toml(text)?.workers.limits();
    let new = change.apply(old);
    let changed = WorkerLimitsChanged { old, new };
    if new == old {
        return Ok(Planned {
            original,
            text: None,
            changed,
        });
    }
    let mut document: DocumentMut = text.parse().map_err(|_| {
        FoldError::new(codes::INVALID_CONFIG, "config.toml is not valid TOML")
            .with_hint("fix .fold/config.toml, then call set_worker_limits again")
    })?;
    let section = document
        .entry("workers")
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or_else(|| FoldError::new(codes::INVALID_CONFIG, "workers must be a table"))?;
    if let Some(limit) = change.task_worker {
        set_number(section, "task_worker", limit)?;
    }
    if let Some(limit) = change.reviewer {
        set_number(section, "reviewer", limit)?;
    }
    match change.max_per_area {
        AreaLimitChange::Keep => {}
        AreaLimitChange::Set(limit) => set_number(section, "max_per_area", limit)?,
        AreaLimitChange::Clear => {
            section.remove("max_per_area");
        }
    }
    Ok(Planned {
        original,
        text: Some(document.to_string()),
        changed,
    })
}

/// Replaces the number and keeps the comment on its line.
fn set_number(section: &mut dyn TableLike, key: &str, number: u64) -> Result<(), FoldError> {
    let number = i64::try_from(number)
        .map_err(|_| FoldError::new(codes::INVALID_REQUEST, "the limit is too large"))?;
    match section.get_mut(key).and_then(Item::as_value_mut) {
        Some(existing) => {
            let mut replacement = TomlValue::from(number);
            *replacement.decor_mut() = existing.decor().clone();
            *existing = replacement;
        }
        None => {
            section.insert(key, value(number));
        }
    }
    Ok(())
}

/// Writes `text` over `path` through `write_atomic`, so a reader sees the old file or the new one.
fn replace(path: &Path, text: &str) -> Result<(), FoldError> {
    write_atomic(path, text.as_bytes()).map_err(|error| config_io(&error))
}

/// Puts `original` back after a write that did not commit; `None` removes the file, as it was missing before.
fn restore(path: &Path, original: Option<&str>) -> Result<(), FoldError> {
    match original {
        Some(text) => replace(path, text),
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(config_io(&error)),
        },
    }
}

/// Serialises the config rewrites of the daemon's projects; the file lock held beside it serialises them with `fld remote`, so a restore never overwrites a rewrite that started after it.
static REWRITE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

impl AgentService {
    pub(super) fn config_paths(&self) -> Option<&ProjectPaths> {
        match (&self.workers, &self.lease) {
            (Some(supervisor), _) => Some(supervisor.paths()),
            (None, Lease::Config(config)) => Some(config.paths()),
            (None, Lease::Minutes(_)) => None,
        }
    }

    /// Changes the `[workers]` limits in `.fold/config.toml`, which stays the single source: the section is rewritten through a flushed temporary file and a rename, every other byte of the file stays, and `workers.limits_changed` is stored with the old and the new numbers in the same write. A change that leaves the limits as they are writes nothing and stores no event. The rename comes before the commit, so a file that cannot be replaced rolls the event back, and a commit that fails puts the old file back. When that restore fails too, or the daemon stops between the rename and the commit, the file holds the new limits and no event records the change. The next start reads the file, so the new limits apply at once and a running worker is never stopped by a lower limit.
    pub async fn set_worker_limits(
        &self,
        actor: &Actor,
        change: WorkerLimitsChange,
    ) -> Result<WorkerLimitsChanged, AgentError> {
        let starter = self.starter(actor).await?;
        policy::can_set_worker_limits(&starter).map_err(FoldError::from)?;
        change.check()?;
        let path = self
            .config_paths()
            .ok_or_else(|| {
                FoldError::new(
                    codes::INTERNAL_ERROR,
                    "this service knows no project folder",
                )
            })?
            .config();
        let _rewrite = REWRITE.lock().await;
        let _file_lock = {
            let path = path.clone();
            joined(
                tokio::task::spawn_blocking(move || {
                    // The lock file's folder is `.fold/`, which a deleted project must not get back.
                    if !path.parent().is_some_and(Path::is_dir) {
                        return Err(config_io(&std::io::ErrorKind::NotFound.into()));
                    }
                    EditLock::acquire(&path).map_err(|error| config_io(&error))
                })
                .await,
            )?
        };
        let planned = {
            let path = path.clone();
            joined(tokio::task::spawn_blocking(move || edit(&path, &change)).await)?
        };
        let Planned {
            original,
            text,
            changed,
        } = planned;
        let Some(text) = text else {
            return Ok(changed);
        };
        let by = actor.event_actor();
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let mut renamed = false;
        let written = self
            .store
            .write(async |tx| {
                let event = NewEvent::new(types::WORKERS_LIMITS_CHANGED, by.clone())?.data(
                    json!({"old": limits_json(changed.old), "new": limits_json(changed.new)}),
                )?;
                buffer.append(tx, &event, now).await?;
                let path = path.clone();
                joined(tokio::task::spawn_blocking(move || replace(&path, &text)).await)?;
                renamed = true;
                Ok::<_, AgentError>(())
            })
            .await;
        if let Err(error) = written {
            if renamed {
                let restored = joined(
                    tokio::task::spawn_blocking(move || restore(&path, original.as_deref())).await,
                );
                if let Err(failed) = restored {
                    tracing::warn!(
                        code = failed.code(),
                        "the worker limits stayed in config.toml without their event"
                    );
                }
            }
            return Err(error);
        }
        self.bus.publish(buffer);
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(task_worker: u64) -> WorkerLimitsChange {
        WorkerLimitsChange {
            task_worker: Some(task_worker),
            ..WorkerLimitsChange::default()
        }
    }

    #[test]
    fn a_missing_file_gets_a_workers_section() {
        let dir = tempfile::tempdir().unwrap();
        let Planned {
            original,
            text,
            changed,
        } = edit(&dir.path().join("config.toml"), &change(7)).unwrap();
        assert_eq!(original, None);
        assert_eq!(text.as_deref(), Some("[workers]\ntask_worker = 7\n"));
        assert_eq!(
            changed.old,
            WorkerLimits {
                task_worker: 4,
                reviewer: 3,
                max_per_area: None
            }
        );
        assert_eq!(changed.new.task_worker, 7);
    }

    #[test]
    fn a_clear_removes_the_area_limit_and_keeps_the_other_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "# top\n[workers]\nmax_per_area = 2 # per area\ntask_worker = 3\n",
        )
        .unwrap();
        let clear = WorkerLimitsChange {
            max_per_area: AreaLimitChange::Clear,
            ..WorkerLimitsChange::default()
        };
        let Planned { text, changed, .. } = edit(&path, &clear).unwrap();
        assert_eq!(text.as_deref(), Some("# top\n[workers]\ntask_worker = 3\n"));
        assert_eq!(changed.old.max_per_area, Some(2));
        assert_eq!(changed.new.max_per_area, None);
    }

    #[test]
    fn a_file_that_is_not_valid_is_refused_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[workers]\ntask_worker = -1\n").unwrap();
        let Err(error) = edit(&path, &change(2)) else {
            panic!("a negative limit is not a valid file");
        };
        assert_eq!(error.code(), codes::INVALID_CONFIG);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[workers]\ntask_worker = -1\n"
        );
    }

    #[test]
    fn a_failed_replace_leaves_no_staging_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::create_dir(&path).unwrap();
        let error = replace(&path, "x").unwrap_err();
        assert_eq!(error.code(), codes::WORKSPACE_IO);
        assert!(!dir.path().join("config.toml.new").exists());
    }

    #[test]
    fn a_change_to_the_same_numbers_plans_no_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[workers]\ntask_worker = 5\n").unwrap();
        let planned = edit(&path, &change(5)).unwrap();
        assert_eq!(planned.text, None);
        assert_eq!(planned.changed.old, planned.changed.new);
        assert_eq!(planned.changed.new.task_worker, 5);
    }

    #[test]
    fn a_restore_puts_the_old_file_back_or_removes_a_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[workers]\ntask_worker = 9\n").unwrap();
        restore(&path, Some("# old\n")).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# old\n");
        restore(&path, None).unwrap();
        assert!(!path.exists());
        restore(&path, None).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert_eq!(
            restore(&path, None).unwrap_err().code(),
            codes::WORKSPACE_IO
        );
    }
}
