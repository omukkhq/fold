use std::sync::Arc;

use fold_core::config::check_branch;
use fold_core::{Actor, FoldError, ProjectPaths, codes, policy};
use fold_vcs::atomic::write_atomic;
use fold_vcs::git::Git;
use fold_vcs::git_remote::{
    Moved, branch_ref, checked_out_branch, operation_in_progress, rename_branch as git_rename,
};
use fold_vcs::git_sync_map::SyncMap;
use serde_json::json;
use toml_edit::{DocumentMut, Item, Table, value};

use super::folder_status::folder_status_locked;
use super::remote::blocking;
use super::{ProjectError, load_config, require_git_sync};
use crate::projects::ProjectHandle;
use crate::support::config_io;

/// The command a person runs to make another branch the project's Git branch.
pub const RENAME_COMMAND: &str = "fld git branch";

/// What a rename of the project's Git branch did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchRenamed {
    /// The `[git]` branch before the call.
    pub from: String,
    pub to: String,
    /// False when the branch was named `to` already and nothing changed.
    pub changed: bool,
}

/// The command that brings the folder back to what fold left. For a `HEAD` on another branch it names `git switch <branch>` only while `refs/heads/<branch>` exists (`present`); a branch that is gone, as after a rename by hand, gets the rename command, which makes the checked-out branch the project's.
pub(crate) fn moved_hint(branch: &str, moved: &Moved, present: bool) -> String {
    match moved {
        Moved::OtherBranch(_) if present => moved.hint(branch),
        Moved::OtherBranch(Some(other)) => format!(
            "the branch {branch} does not exist: run `{RENAME_COMMAND} {other}` in the project folder to make {other} the project's Git branch"
        ),
        Moved::OtherBranch(None) => format!(
            "the branch {branch} does not exist: run `git switch <branch>` in the project folder, then `{RENAME_COMMAND} <branch>` to make it the project's Git branch"
        ),
        other => other.hint(branch),
    }
}

/// What the repository shows before a rename, read under the Git sync lock.
struct Found {
    checked_out: Option<String>,
    from_present: bool,
    to_present: bool,
    operation: Option<&'static str>,
}

/// Renames the project's Git branch: `git branch -m <from> <to>` in the project folder's repository, then `[git] branch = "<to>"` in `.fold/config.toml`, every other byte of the file kept. The remote lock, the folder lock and the Git sync lock are held throughout and the configuration is read under them, so no push, pull, record, follow or commit of the Git job reads the old name and then finds `HEAD` on the new one. The working tree, the index, the commits, the remotes and their branches stay as they are; nothing outside the project's own repository is touched. A run that a stop cut off between the two steps leaves `HEAD` on `to` and no `from`: the same call again only writes the configuration. Refused with `invalid_request` and `details.reason`: `legacy_remote` for a `[git] remote` key, whose remote follows the `[git]` branch; `branch_exists` when `refs/heads/<to>` exists; `head_moved` when `HEAD` is not on `from`; `folder_held` while a Git operation waits half-done or the folder has a blocker that `fld status` lists.
pub async fn rename_branch(
    handle: &Arc<ProjectHandle>,
    actor: &Actor,
    git: &Git,
    to: &str,
) -> Result<BranchRenamed, ProjectError> {
    policy::can_rename_branch(actor).map_err(FoldError::from)?;
    check_branch(to)?;
    require_git_sync(&handle.store, RENAME_COMMAND).await?;
    let _remote = handle.remote_lock.clone().lock_owned().await;
    let _folder = handle.folder_lock.clone().lock_owned().await;
    refuse_legacy_remote(&handle.paths).await?;
    let from = load_config(&handle.paths).await?.git.branch;
    let status = folder_status_locked(handle, git).await?;
    let (git, paths, from_owned, to_owned) = (
        git.clone(),
        handle.paths.clone(),
        from.clone(),
        to.to_owned(),
    );
    let renamed = blocking(move || {
        let (held, _map) = SyncMap::lock(&paths.fold_dir())?;
        let repo = paths.git_dir();
        let found = Found {
            checked_out: checked_out_branch(&git, &repo)?,
            from_present: git.ref_commit(&repo, &branch_ref(&from_owned))?.is_some(),
            to_present: git.ref_commit(&repo, &branch_ref(&to_owned))?.is_some(),
            operation: operation_in_progress(&repo),
        };
        let step = decide(&from_owned, &to_owned, &found, status.blockers.is_empty())?;
        if step == Step::Nothing {
            return Ok(false);
        }
        if step == Step::Rename {
            git_rename(&git, &repo, &from_owned, &to_owned)?;
        }
        #[cfg(feature = "failpoints")]
        fail::fail_point!("rename_branch_before_config", |_| Err(
            crate::services::fired("rename_branch_before_config")
        ));
        write_branch(&paths, &to_owned)?;
        drop(held);
        Ok(true)
    })
    .await?;
    if renamed {
        tracing::info!(project = %handle.slug, "the project's Git branch was renamed");
    }
    Ok(BranchRenamed {
        from,
        to: to.to_owned(),
        changed: renamed,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Nothing,
    /// `HEAD` is on `to` already and `from` is gone: only the configuration waits.
    ConfigOnly,
    Rename,
}

fn decide(from: &str, to: &str, found: &Found, clear: bool) -> Result<Step, FoldError> {
    let on_to = found.checked_out.as_deref() == Some(to);
    if from == to && on_to {
        return Ok(Step::Nothing);
    }
    if from != to && on_to && !found.from_present {
        return Ok(Step::ConfigOnly);
    }
    if let Some(operation) = found.operation {
        return Err(refused(
            format!("a Git {operation} waits half-done in the project folder"),
            format!(
                "finish or abort the {operation} in the project folder, then run `{RENAME_COMMAND} {to}` again"
            ),
            "folder_held",
        ));
    }
    if found.checked_out.as_deref() != Some(from) {
        let hint = if found.from_present {
            format!(
                "run `git switch {from}` in the project folder, then `{RENAME_COMMAND} {to}` again"
            )
        } else {
            format!(
                "check out the branch you want with `git switch <branch>` in the project folder, then run `{RENAME_COMMAND} <branch>`"
            )
        };
        return Err(refused(
            format!("the project folder does not have the project's Git branch {from} checked out"),
            hint,
            "head_moved",
        ));
    }
    if from == to {
        return Ok(Step::Nothing);
    }
    if found.to_present {
        return Err(refused(
            format!("the branch {to} exists already in the project's repository"),
            format!(
                "choose another name, or delete the branch {to} with `git branch -d {to}` if you no longer need it"
            ),
            "branch_exists",
        ));
    }
    if !clear {
        return Err(refused(
            "the project folder waits on something that `fld status` lists".to_owned(),
            format!(
                "run `fld status`, settle what it lists, then run `{RENAME_COMMAND} {to}` again"
            ),
            "folder_held",
        ));
    }
    Ok(Step::Rename)
}

fn refused(message: String, hint: String, reason: &'static str) -> FoldError {
    FoldError::new(codes::INVALID_REQUEST, message)
        .with_hint(hint)
        .with_details([("reason", json!(reason))])
}

/// A `[git] remote` key names the remote `origin` on the `[git]` branch, so a rename would move its push target too.
async fn refuse_legacy_remote(paths: &ProjectPaths) -> Result<(), ProjectError> {
    let path = paths.config();
    let text = blocking(move || match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(config_io(&error)),
    })
    .await?;
    let legacy = text
        .as_deref()
        .and_then(|text| text.parse::<DocumentMut>().ok())
        .is_some_and(|document| {
            document
                .get(fold_core::config::GIT_SECTION)
                .and_then(Item::as_table_like)
                .is_some_and(|git| git.contains_key("remote"))
        });
    if legacy {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            "`.fold/config.toml` names its remote with `remote` in [git], which pushes to the [git] branch, so a rename would change the remote branch too",
        )
        .with_hint("move the remote into a [remotes.origin] section with its own `url` and `branch`, then run the rename again")
        .with_details([("reason", json!("legacy_remote"))])
        .into());
    }
    Ok(())
}

/// Sets `[git] branch` in `.fold/config.toml` and keeps every other byte, through a file beside it that is renamed into place, so a stop leaves the old file or the new one. Blocking.
fn write_branch(paths: &ProjectPaths, branch: &str) -> Result<(), FoldError> {
    let path = paths.config();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(config_io(&error)),
    };
    let written = with_branch(&text, branch)?;
    write_atomic(&path, written.as_bytes()).map_err(|error| config_io(&error))
}

/// `text` with `[git] branch` set to `branch`.
fn with_branch(text: &str, branch: &str) -> Result<String, FoldError> {
    let mut document: DocumentMut = text.parse().map_err(|_| {
        FoldError::new(codes::INVALID_CONFIG, "config.toml is not valid TOML")
            .with_hint("fix .fold/config.toml, then run the rename again")
    })?;
    let section = document
        .entry(fold_core::config::GIT_SECTION)
        .or_insert_with(|| Item::Table(Table::new()));
    let Some(git) = section.as_table_like_mut() else {
        return Err(
            FoldError::new(codes::INVALID_CONFIG, "[git] in config.toml is not a table")
                .with_hint("fix .fold/config.toml, then run the rename again"),
        );
    };
    git.insert("branch", value(branch));
    Ok(document.to_string())
}

#[cfg(test)]
mod tests {
    use fold_vcs::git::CommitId;

    use super::*;

    fn found(checked_out: Option<&str>, from_present: bool, to_present: bool) -> Found {
        Found {
            checked_out: checked_out.map(str::to_owned),
            from_present,
            to_present,
            operation: None,
        }
    }

    fn reason(error: &FoldError) -> String {
        error.details().unwrap()["reason"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn a_rename_runs_only_from_a_clear_folder_on_the_branch() {
        let on_main = found(Some("main"), true, false);
        assert_eq!(decide("main", "dev", &on_main, true).unwrap(), Step::Rename);
        assert_eq!(
            reason(&decide("main", "dev", &on_main, false).unwrap_err()),
            "folder_held"
        );
        let taken = found(Some("main"), true, true);
        assert_eq!(
            reason(&decide("main", "dev", &taken, true).unwrap_err()),
            "branch_exists"
        );
        let elsewhere = found(Some("topic"), true, false);
        let error = decide("main", "dev", &elsewhere, true).unwrap_err();
        assert_eq!(reason(&error), "head_moved");
        assert!(error.hint().unwrap().contains("`git switch main`"));
        let gone = found(Some("topic"), false, false);
        let error = decide("main", "dev", &gone, true).unwrap_err();
        assert!(!error.hint().unwrap().contains("git switch main"));
        let mut merging = found(Some("main"), true, false);
        merging.operation = Some("merge");
        assert_eq!(
            reason(&decide("main", "dev", &merging, true).unwrap_err()),
            "folder_held"
        );
    }

    #[test]
    fn a_rename_that_a_stop_cut_off_only_writes_the_config_and_a_done_one_does_nothing() {
        let half = found(Some("dev"), false, true);
        assert_eq!(
            decide("main", "dev", &half, false).unwrap(),
            Step::ConfigOnly
        );
        let done = found(Some("dev"), false, true);
        assert_eq!(decide("dev", "dev", &done, false).unwrap(), Step::Nothing);
        let detached = found(None, true, false);
        assert_eq!(
            reason(&decide("main", "main", &detached, true).unwrap_err()),
            "head_moved"
        );
    }

    #[test]
    fn the_moved_hint_names_git_switch_only_for_a_branch_that_exists() {
        let other = Moved::OtherBranch(Some("dev".to_owned()));
        assert_eq!(
            moved_hint("main", &other, true),
            "run `git switch main` in the project folder"
        );
        let hint = moved_hint("main", &other, false);
        assert!(hint.contains("`fld git branch dev`"), "{hint}");
        assert!(!hint.contains("git switch main"), "{hint}");
        let hint = moved_hint("main", &Moved::OtherBranch(None), false);
        assert!(!hint.contains("git switch main"), "{hint}");
        assert!(hint.contains("`fld git branch <branch>`"), "{hint}");
        let fold: CommitId = "c".repeat(40).parse().unwrap();
        let rewritten = Moved::Rewritten {
            tip: fold.clone(),
            fold,
        };
        assert_eq!(
            moved_hint("main", &rewritten, false),
            rewritten.hint("main")
        );
    }

    #[test]
    fn the_branch_is_set_and_every_other_line_stays() {
        let text = "# my notes\nlease_minutes = 30\n\n[git]\nbranch = \"main\" # local\n\n[remotes.origin]\nurl = \"/x.git\"\nbranch = \"dev\"\n";
        let written = with_branch(text, "dev").unwrap();
        assert!(written.starts_with("# my notes\nlease_minutes = 30\n"));
        let config = fold_core::ProjectConfig::from_toml(&written).unwrap();
        assert_eq!(config.git.branch, "dev");
        assert_eq!(config.remotes["origin"].branch, "dev");
        let added = with_branch("lease_minutes = 30\n", "release/1.0").unwrap();
        assert_eq!(
            fold_core::ProjectConfig::from_toml(&added)
                .unwrap()
                .git
                .branch,
            "release/1.0"
        );
        assert_eq!(
            with_branch("git = 1\n", "dev").unwrap_err().code(),
            codes::INVALID_CONFIG
        );
    }
}
