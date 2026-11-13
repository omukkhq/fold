use std::path::Path;

use fold_core::accounts::{AccountName, OVERRIDING_ENV, token_is_old};
use fold_core::{FoldError, ProjectConfig, ProjectPaths, Timestamp};
use fold_daemon::services::agents::accounts::AccountEntry;

use super::Check;
use super::environment::DaemonReport;
use crate::output;

const NAME: &str = "accounts";
const PROJECT_NAME: &str = "account";

/// The host's accounts: whether each token is still young enough, and whether the workers' environment sets a variable that Claude Code takes before an account's token while a project names an account. `used` names the accounts that registered projects name.
pub(crate) fn accounts_verdict(
    listed: &Result<Vec<AccountEntry>, FoldError>,
    used: &[AccountName],
    report: &DaemonReport,
    now: Timestamp,
) -> Check {
    let accounts = match listed {
        Ok(accounts) => accounts,
        Err(error) => {
            return Check::warn(
                NAME,
                format!("fold cannot read the host's accounts: {}", error.message()),
                error
                    .hint()
                    .unwrap_or("check fold's data folder accounts/")
                    .to_owned(),
            );
        }
    };
    if accounts.is_empty() {
        return Check::ok(
            NAME,
            "no accounts: every project uses Claude Code's own login",
        );
    }
    let overriding: Vec<&str> = match report {
        DaemonReport::Report(environment) if !used.is_empty() => OVERRIDING_ENV
            .into_iter()
            .filter(|name| environment.sign_in_variables.iter().any(|set| set == name))
            .collect(),
        _ => Vec::new(),
    };
    if !overriding.is_empty() {
        let names: Vec<&str> = used.iter().map(AccountName::as_str).collect();
        return Check::warn(
            NAME,
            format!(
                "the workers' environment sets {}, which Claude Code uses before an account's token, so the account {} has no effect",
                overriding.join(", "),
                names.join(", ")
            ),
            "remove these variables from the environment the daemon gives its workers (see daemon environment), then `fld restart`",
        );
    }
    let old: Vec<&AccountEntry> = accounts
        .iter()
        .filter(|account| token_is_old(account.added_at, now))
        .collect();
    if let Some(first) = old.first() {
        let words: Vec<String> = old
            .iter()
            .map(|account| {
                format!(
                    "{} (added {})",
                    account.name,
                    output::local_time(account.added_at)
                )
            })
            .collect();
        return Check::warn(
            NAME,
            format!(
                "the token of {} is older than 11 months, and a token of claude setup-token lasts one year",
                words.join(", ")
            ),
            format!(
                "run `claude setup-token`, then `fld account add {}` with the new token on stdin",
                first.name
            ),
        );
    }
    let names: Vec<&str> = accounts
        .iter()
        .map(|account| account.name.as_str())
        .collect();
    Check::ok(NAME, format!("accounts: {}", names.join(", ")))
}

/// The account `.fold/config.toml` of the project at `root` names, if the config can be read.
pub(crate) fn named_account(root: &Path) -> Option<AccountName> {
    let text = std::fs::read_to_string(ProjectPaths::new(root).config()).ok()?;
    ProjectConfig::from_toml(&text).ok()?.account
}

/// Whether the account the project names is one of the host's: a worker of a project whose account is missing cannot start.
pub(crate) fn project_account_verdict(
    named: Option<&AccountName>,
    listed: &Result<Vec<AccountEntry>, FoldError>,
) -> Check {
    let Some(name) = named else {
        return Check::ok(PROJECT_NAME, "the workers use Claude Code's own login");
    };
    match listed {
        Ok(accounts) if accounts.iter().any(|account| &account.name == name) => Check::ok(
            PROJECT_NAME,
            format!("the workers and fld claude use the account {name}"),
        ),
        Ok(_) => Check::fail(
            PROJECT_NAME,
            format!(
                "the project names the account {name}, which this host does not have, so no worker can start"
            ),
            format!(
                "run `claude setup-token`, then `fld account add {name}` with the token on stdin, or `fld account use <name>` in the project folder"
            ),
        ),
        Err(_) => Check::warn(
            PROJECT_NAME,
            format!(
                "the project names the account {name}, and fold cannot read the host's accounts"
            ),
            "check fold's data folder accounts/",
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::Status;
    use super::super::environment::WorkerEnvironment;
    use super::*;

    fn entry(name: &str, added_at: Timestamp) -> AccountEntry {
        AccountEntry {
            name: AccountName::parse(name).unwrap(),
            added_at,
        }
    }

    fn report(variables: &[&str]) -> DaemonReport {
        DaemonReport::Report(WorkerEnvironment {
            sign_in_variables: variables.iter().map(|name| (*name).to_owned()).collect(),
            ..WorkerEnvironment::default()
        })
    }

    #[test]
    fn doctor_names_an_old_token_and_a_variable_that_overrides_the_account() {
        let now = Timestamp::from_millis(1_800_000_000_000).unwrap();
        let day = Duration::from_secs(24 * 60 * 60);
        let young = now.checked_sub(day * 30).unwrap();
        let old = now.checked_sub(day * 340).unwrap();
        let none = accounts_verdict(&Ok(Vec::new()), &[], &report(&[]), now);
        assert_eq!(none.status, Status::Ok);
        let fine = accounts_verdict(&Ok(vec![entry("work-a", young)]), &[], &report(&[]), now);
        assert_eq!(
            (fine.status, fine.message.as_str()),
            (Status::Ok, "accounts: work-a")
        );
        let aged = accounts_verdict(
            &Ok(vec![entry("work-a", young), entry("work-b", old)]),
            &[],
            &report(&[]),
            now,
        );
        assert_eq!(aged.status, Status::Warn);
        assert!(aged.message.contains("work-b (added "), "{}", aged.message);
        assert!(!aged.message.contains("work-a"));
        assert!(aged.fix.unwrap().contains("fld account add work-b"));
        let used = [AccountName::parse("work-a").unwrap()];
        let accounts = Ok(vec![entry("work-a", young)]);
        for variable in ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"] {
            let overridden = accounts_verdict(&accounts, &used, &report(&[variable, "HOME"]), now);
            assert_eq!(overridden.status, Status::Warn);
            assert!(
                overridden.message.contains(variable),
                "{}",
                overridden.message
            );
            assert!(overridden.message.contains("account work-a has no effect"));
            assert!(overridden.fix.unwrap().contains("fld restart"));
        }
        let unused = accounts_verdict(&accounts, &[], &report(&["ANTHROPIC_API_KEY"]), now);
        assert_eq!(unused.status, Status::Ok, "no project uses an account");
        let unreadable = accounts_verdict(
            &Err(FoldError::new(fold_core::codes::ACCOUNTS_IO, "x").with_hint("check accounts/")),
            &[],
            &report(&[]),
            now,
        );
        assert_eq!(unreadable.status, Status::Warn);
    }

    #[test]
    fn a_project_that_names_a_missing_account_fails() {
        let now = Timestamp::from_millis(1_800_000_000_000).unwrap();
        let listed = Ok(vec![entry("work-a", now)]);
        let work_a = AccountName::parse("work-a").unwrap();
        let gone = AccountName::parse("gone").unwrap();
        assert_eq!(project_account_verdict(None, &listed).status, Status::Ok);
        assert_eq!(
            project_account_verdict(Some(&work_a), &listed).status,
            Status::Ok
        );
        let missing = project_account_verdict(Some(&gone), &listed);
        assert_eq!(missing.status, Status::Fail);
        assert!(missing.fix.unwrap().contains("fld account add gone"));
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".fold")).unwrap();
        assert_eq!(named_account(dir.path()), None);
        std::fs::write(
            dir.path().join(".fold/config.toml"),
            "account = \"work-a\"\n",
        )
        .unwrap();
        assert_eq!(named_account(dir.path()), Some(work_a));
    }
}
