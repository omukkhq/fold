use std::path::{Path, PathBuf};

use fold_core::accounts::AccountName;
use fold_core::config::ACCOUNT_KEY;
use fold_core::events::types;
use fold_core::{Actor, FoldError, NewEvent, ProjectConfig, codes, policy};
use fold_vcs::atomic::EditLock;
use serde_json::json;
use toml_edit::{DocumentMut, value};

use super::super::{AgentError, AgentService, accounts};
use super::limits::{REWRITE, config_io, joined, replace, restore};
use crate::services::events::EventBuffer;

/// The project's account and the accounts of the host it can choose, by name only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectAccount {
    pub account: Option<AccountName>,
    pub accounts: Vec<AccountName>,
}

/// The text of `config.toml` with the top-level `account` set to `account`, or without it for `None`, every other byte kept; `None` when the file names that account already. A missing file reads as empty. Also returns the account the file named before.
fn edit(
    original: Option<&str>,
    account: Option<&AccountName>,
) -> Result<(Option<AccountName>, Option<String>), FoldError> {
    let text = original.unwrap_or_default();
    let old = ProjectConfig::from_toml(text)?.account;
    if old.as_ref() == account {
        return Ok((old, None));
    }
    let mut document: DocumentMut = text.parse().map_err(|_| {
        FoldError::new(codes::INVALID_CONFIG, "config.toml is not valid TOML")
            .with_hint("fix .fold/config.toml, then choose the account again")
    })?;
    match account {
        Some(name) => {
            let mut replacement = toml_edit::Value::from(name.as_str());
            if let Some(existing) = document.get(ACCOUNT_KEY).and_then(|item| item.as_value()) {
                *replacement.decor_mut() = existing.decor().clone();
            }
            document.insert(ACCOUNT_KEY, value(replacement));
        }
        None => {
            document.remove(ACCOUNT_KEY);
        }
    }
    Ok((old, Some(document.to_string())))
}

fn read_original(path: &Path) -> Result<Option<String>, FoldError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(config_io(&error)),
    }
}

impl AgentService {
    fn accounts_dir(&self) -> Option<PathBuf> {
        self.workers.as_ref()?.accounts_dir()
    }

    /// The account `.fold/config.toml` names and the names of the host's accounts. A file that cannot be read names none; accounts that cannot be read list none.
    pub async fn project_account(&self) -> ProjectAccount {
        let account = self.worker_config().await.and_then(|config| config.account);
        let accounts = match self.accounts_dir() {
            Some(dir) => tokio::task::spawn_blocking(move || accounts::list(&dir))
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default()
                .into_iter()
                .map(|entry| entry.name)
                .collect(),
            None => Vec::new(),
        };
        ProjectAccount { account, accounts }
    }

    /// A named human chooses the project's account, `None` for Claude Code's own login. The name goes into `.fold/config.toml`, which stays the single source, through a flushed temporary file and a rename that keeps every other byte, and `workers.account_changed` with the old and the new name is stored in the same write; a commit that fails puts the old file back. A choice of the account the file names already writes nothing. Once the event is out, each running worker continues in its own session with the new account when it is idle (see [`super::Supervisor::switch_accounts`]); a start reads the file, so a daemon that stops between the rename and the commit starts its workers with the new account, without the event.
    pub async fn set_account(
        &self,
        actor: &Actor,
        account: Option<AccountName>,
    ) -> Result<ProjectAccount, AgentError> {
        let starter = self.starter(actor).await?;
        policy::can_set_account(&starter).map_err(FoldError::from)?;
        if let Some(name) = &account {
            let dir = self
                .accounts_dir()
                .ok_or_else(|| accounts::not_found(name))?;
            let lookup = name.clone();
            let found =
                joined(tokio::task::spawn_blocking(move || accounts::stamp(&dir, &lookup)).await)?;
            if found.is_none() {
                return Err(accounts::not_found(name).into());
            }
        }
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
        let (original, old, text) = {
            let path = path.clone();
            let chosen = account.clone();
            joined(
                tokio::task::spawn_blocking(move || {
                    let original = read_original(&path)?;
                    let (old, text) = edit(original.as_deref(), chosen.as_ref())?;
                    Ok((original, old, text))
                })
                .await,
            )?
        };
        let Some(text) = text else {
            return Ok(self.project_account().await);
        };
        let by = actor.event_actor();
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let mut renamed = false;
        let written = self
            .store
            .write(async |tx| {
                let event =
                    NewEvent::new(types::WORKERS_ACCOUNT_CHANGED, by.clone())?.data(json!({
                        "old": old.as_ref().map(AccountName::as_str),
                        "new": account.as_ref().map(AccountName::as_str),
                    }))?;
                buffer.append(tx, &event, now).await?;
                let path = path.clone();
                joined(tokio::task::spawn_blocking(move || replace(&path, &text)).await)?;
                renamed = true;
                #[cfg(feature = "failpoints")]
                fail::fail_point!("account_before_commit", |_| Err(AgentError::from(
                    FoldError::new(codes::INTERNAL_ERROR, "stopped before the commit")
                )));
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
                        "the account stayed in config.toml without its event"
                    );
                }
            }
            return Err(error);
        }
        self.bus.publish(buffer);
        // The turns at a usage limit run again: at once in a process that runs with the new account already, and in each other one after its switch.
        if let Some(workers) = &self.workers {
            workers.run_limit_turns_now();
        }
        Ok(self.project_account().await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> AccountName {
        AccountName::parse(text).unwrap()
    }

    #[test]
    fn the_account_goes_before_the_first_table_and_every_other_byte_stays() {
        let template = fold_core::CONFIG_TEMPLATE;
        let (old, text) = edit(Some(template), Some(&name("work-a"))).unwrap();
        assert_eq!(old, None);
        let text = text.unwrap();
        assert_eq!(
            text.replace("account = \"work-a\"\n", ""),
            template,
            "{text}"
        );
        assert_eq!(
            ProjectConfig::from_toml(&text).unwrap().account,
            Some(name("work-a"))
        );

        let sections = "# top\nlease_minutes = 9 # mine\n\n[workers]\ntask_worker = 2\n\n[roles.main]\nmodel = \"opus\"\n";
        let (_, text) = edit(Some(sections), Some(&name("work-a"))).unwrap();
        let text = text.unwrap();
        let config = ProjectConfig::from_toml(&text).unwrap();
        assert_eq!(config.account, Some(name("work-a")));
        assert_eq!(config.workers.task_worker, 2);
        assert_eq!(config.lease_minutes, 9);
        let account_at = text.find("account = \"work-a\"").unwrap();
        assert!(account_at < text.find("[workers]").unwrap(), "{text}");

        let (old, switched) = edit(Some(&text), Some(&name("work-b"))).unwrap();
        assert_eq!(old, Some(name("work-a")));
        assert_eq!(
            switched.unwrap(),
            text.replace("account = \"work-a\"", "account = \"work-b\"")
        );
        let (_, cleared) = edit(Some(&text), None).unwrap();
        assert_eq!(cleared.unwrap(), sections);
        assert_eq!(edit(Some(&text), Some(&name("work-a"))).unwrap().1, None);
        assert_eq!(edit(None, None).unwrap(), (None, None));
        assert_eq!(
            edit(None, Some(&name("work-a"))).unwrap().1.as_deref(),
            Some("account = \"work-a\"\n")
        );
    }

    #[test]
    fn a_broken_file_is_refused_before_any_write() {
        let error = edit(Some("lease_minutes = \"x\"\n"), Some(&name("work-a"))).unwrap_err();
        assert_eq!(error.code(), codes::INVALID_CONFIG);
    }
}
