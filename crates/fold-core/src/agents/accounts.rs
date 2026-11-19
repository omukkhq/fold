use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{FoldError, codes};
use crate::time::Timestamp;

/// The environment variable that gives Claude Code a long-lived OAuth token of a subscription, made by `claude setup-token`. It ranks below `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_API_KEY`, `apiKeyHelper` and a cloud provider's variables, and above the login that `/login` stored.
pub const OAUTH_TOKEN_ENV: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// Set beside [`OAUTH_TOKEN_ENV`] when fold gave a process the token of an account, so a daemon that such a process starts drops both: the token belongs to the project of that session, never to every project of the daemon.
pub const ACCOUNT_ENV: &str = "FOLD_ACCOUNT";

/// The variables of a worker's environment that Claude Code takes before an account's token, so an account has no effect while one of them is set.
pub const OVERRIDING_ENV: [&str; 5] = [
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
];

/// A token from `claude setup-token` lasts one year, so fold names one that was added more than 11 months ago.
pub const OLD_TOKEN_AGE: Duration = Duration::from_secs(334 * 24 * 60 * 60);

/// The account that stands for Claude Code's own login, as `claude login` stored it on the host: first in every list while that login is there, and never holding a token of fold's. A project on it gives its workers no token, and a config without the key `account` means it.
pub const DEFAULT_ACCOUNT: &str = "default";

/// The name [`DEFAULT_ACCOUNT`] had before, which a config or a request may still hold; it reads as `default`.
const OLD_DEFAULT_ACCOUNT: &str = "system-default";

const MAX_ACCOUNT_NAME_LEN: usize = 32;
const MIN_TOKEN_LEN: usize = 20;
const MAX_TOKEN_LEN: usize = 1024;

/// The name of a Claude subscription that the host keeps a token of: 1 to 32 characters of `a-z`, `0-9`, `-` and `_`, starting with a letter or digit. A project names it in its config; the token itself stays in the host's data folder.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct AccountName(String);

impl AccountName {
    pub fn parse(text: &str) -> Result<Self, FoldError> {
        if text == OLD_DEFAULT_ACCOUNT {
            return Ok(Self::default_account());
        }
        let mut bytes = text.bytes();
        let valid = bytes
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
            && text.len() <= MAX_ACCOUNT_NAME_LEN
            && bytes
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'));
        if valid {
            Ok(Self(text.to_owned()))
        } else {
            Err(FoldError::new(
                codes::INVALID_ACCOUNT_NAME,
                "an account name is 1 to 32 characters of a-z, 0-9, - and _, starting with a letter or digit",
            )
            .with_hint("choose a name such as work-a"))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// [`DEFAULT_ACCOUNT`], Claude Code's own login.
    pub fn default_account() -> Self {
        Self(DEFAULT_ACCOUNT.to_owned())
    }

    pub fn is_default(&self) -> bool {
        self.0 == DEFAULT_ACCOUNT
    }

    /// The account a choice of this name stores: `None` for [`DEFAULT_ACCOUNT`], which a config states by leaving the key out.
    pub fn stored(self) -> Option<Self> {
        (!self.is_default()).then_some(self)
    }

    /// Refuses [`DEFAULT_ACCOUNT`] for a change of the host's accounts: it holds no token, and nobody adds, updates or removes it.
    pub fn check_not_reserved(&self) -> Result<(), FoldError> {
        if self.is_default() {
            return Err(FoldError::new(
                codes::ACCOUNT_RESERVED,
                "default is Claude Code's own login, which fold keeps no token of; it cannot be added, updated or removed",
            )
            .with_hint("sign in with `claude auth login` to change that login, or add your token under another name, such as `fld account add work-a`")
            .with_details([("account", serde_json::json!(DEFAULT_ACCOUNT))]));
        }
        Ok(())
    }
}

impl fmt::Display for AccountName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for AccountName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(|error| serde::de::Error::custom(error.message()))
    }
}

/// The token of an account. It has no `Display` and a redacted `Debug`, so no log line, error or answer holds it by formatting; only the start of a Claude Code process reads it.
#[derive(Clone, PartialEq, Eq)]
pub struct AccountToken(String);

impl AccountToken {
    /// The token as Claude Code takes it in [`OAUTH_TOKEN_ENV`].
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AccountToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccountToken(<redacted>)")
    }
}

/// Why a text is not a token, in the refusal's `reason` detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenRefusal {
    Empty,
    SeveralLines,
    NotAToken,
}

impl TokenRefusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::SeveralLines => "several_lines",
            Self::NotAToken => "not_a_token",
        }
    }
}

/// The token in `text`, as `claude setup-token` prints it and a person pastes it: surrounding whitespace and one final line break go, and what is left must be one word of 20 to 1024 visible ASCII characters. Claude Code documents no token format, so fold checks only this shape. The refusal never repeats the text.
pub fn check_token(text: &str) -> Result<AccountToken, FoldError> {
    let trimmed = text.trim();
    let refusal = if trimmed.is_empty() {
        Some(TokenRefusal::Empty)
    } else if trimmed.contains(['\n', '\r']) {
        Some(TokenRefusal::SeveralLines)
    } else if !(MIN_TOKEN_LEN..=MAX_TOKEN_LEN).contains(&trimmed.len())
        || !trimmed.bytes().all(|b| b.is_ascii_graphic())
    {
        Some(TokenRefusal::NotAToken)
    } else {
        None
    };
    match refusal {
        None => Ok(AccountToken(trimmed.to_owned())),
        Some(reason) => Err(token_refused(reason)),
    }
}

fn token_refused(reason: TokenRefusal) -> FoldError {
    let message = match reason {
        TokenRefusal::Empty => "fld account add read no token from its input",
        TokenRefusal::SeveralLines => "the input holds more than one line, and a token is one line",
        TokenRefusal::NotAToken => {
            "the input is not a token of claude setup-token: one word of 20 to 1024 visible characters"
        }
    };
    FoldError::new(codes::INVALID_ACCOUNT_TOKEN, message)
        .with_hint("run `claude setup-token`, then give its token on stdin, such as `fld account add work-a < token.txt`")
        .with_details([("reason", serde_json::json!(reason.as_str()))])
}

/// Whether a token added at `added_at` is older than [`OLD_TOKEN_AGE`] at `now`.
pub fn token_is_old(added_at: Timestamp, now: Timestamp) -> bool {
    now.duration_since(added_at)
        .is_some_and(|age| age > OLD_TOKEN_AGE)
}

/// The account a process runs with: its name and when its token was added, so a token given again counts as another account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStamp {
    pub name: AccountName,
    pub added_at: Timestamp,
}

/// The prompt of the one small turn with which `fld account add` and `update` check a new token: an answer of the model shows that the API takes the token.
pub const TOKEN_CHECK_PROMPT: &str = "Reply with the word ok.";

/// The longest that check waits for its turn.
pub const TOKEN_CHECK_TIMEOUT: Duration = Duration::from_secs(60);

/// The result of the check's turn as Claude Code's `--output-format json` gives it: whether it is an error, its text, and whether any token of the turn reached the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenCheckRun {
    pub is_error: bool,
    pub text: Option<String>,
    pub reached_model: bool,
}

/// What the check's turn showed of a new token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenCheck {
    /// The model answered the turn.
    Works,
    /// The API refused the token: the turn reached no model and Claude Code's text is a refused login, as a worker's turn that waits for a working login ends.
    Rejected,
    /// fold could not tell now, and stores the token with a warning.
    Unchecked(Unchecked),
}

/// Why the check could not tell whether the API takes a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unchecked {
    /// Claude Code could not reach the model.
    Network,
    /// The account reached a usage limit, so the API took the token but the turn did not run.
    UsageLimit,
    /// Claude Code did not run, did not end within [`TOKEN_CHECK_TIMEOUT`], or gave no result that fold reads.
    NoAnswer,
}

impl Unchecked {
    /// Why fold could not check the token, after `fold could not check the token now: `.
    pub fn words(self) -> &'static str {
        match self {
            Self::Network => "Claude Code could not reach the model",
            Self::UsageLimit => "the account is at a usage limit",
            Self::NoAnswer => "claude gave no answer that fold reads",
        }
    }
}

/// Reads the check's turn, `None` when Claude Code gave no result. Only positive evidence decides: a refused login rejects the token, and only a turn that ended well and reached the model shows that it works; anything else leaves it unchecked, so a host where `claude` cannot run still keeps the token.
pub fn token_check(run: Option<&TokenCheckRun>) -> TokenCheck {
    let Some(run) = run else {
        return TokenCheck::Unchecked(Unchecked::NoAnswer);
    };
    let wait = run
        .text
        .as_deref()
        .and_then(crate::turn_errors::error_text)
        .as_deref()
        .and_then(crate::turn_waits::turn_wait);
    match wait {
        _ if !run.is_error && run.reached_model => TokenCheck::Works,
        Some(_) if run.reached_model => TokenCheck::Unchecked(Unchecked::NoAnswer),
        Some(crate::turn_waits::TurnWait::Login) => TokenCheck::Rejected,
        Some(crate::turn_waits::TurnWait::Network) => TokenCheck::Unchecked(Unchecked::Network),
        Some(crate::turn_waits::TurnWait::UsageLimit { .. }) => {
            TokenCheck::Unchecked(Unchecked::UsageLimit)
        }
        None => TokenCheck::Unchecked(Unchecked::NoAnswer),
    }
}

/// The refusal of a new token of the account `name` that the API refused in the check's turn. Nothing was stored; `update` says whether the token was to replace the account's token, which stays.
pub fn token_rejected(name: &AccountName, update: bool) -> FoldError {
    let (command, kept) = if update {
        ("update", format!("the account {name} keeps its token"))
    } else {
        ("add", "fold stored nothing".to_owned())
    };
    FoldError::new(
        codes::ACCOUNT_TOKEN_REJECTED,
        format!("Claude Code's login with the new token of {name} was refused; {kept}"),
    )
    .with_hint(format!(
        "run `claude setup-token` for a new token, then `fld account {command} {name}` with it on stdin"
    ))
    .with_details([("account", serde_json::json!(name.as_str()))])
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "sk-ant-oat01-abcdefghijklmnopqrstuvwxyz0123456789";

    #[test]
    fn a_name_follows_the_agent_name_pattern() {
        for good in ["work-a", "b", "0x", "a_b-c", &"a".repeat(32)] {
            assert_eq!(AccountName::parse(good).unwrap().as_str(), good);
        }
        for bad in ["", "-a", "Work", "a b", "a.b", &"a".repeat(33), "ä"] {
            let error = AccountName::parse(bad).unwrap_err();
            assert_eq!(error.code(), codes::INVALID_ACCOUNT_NAME);
            assert!(error.hint().is_some());
        }
        let parsed: AccountName = serde_json::from_str("\"work-a\"").unwrap();
        assert_eq!(parsed.to_string(), "work-a");
        assert!(serde_json::from_str::<AccountName>("\"Work\"").is_err());
    }

    #[test]
    fn default_is_a_name_that_stores_as_none_and_is_reserved() {
        let default = AccountName::parse(DEFAULT_ACCOUNT).unwrap();
        assert_eq!(default, AccountName::default_account());
        assert!(default.is_default());
        assert_eq!(
            AccountName::parse("system-default").unwrap(),
            default,
            "the old name reads as default"
        );
        assert_eq!(default.clone().stored(), None);
        let refused = default.check_not_reserved().unwrap_err();
        assert_eq!(refused.code(), codes::ACCOUNT_RESERVED);
        assert!(refused.hint().unwrap().contains("fld account add"));
        let work = AccountName::parse("work-a").unwrap();
        assert_eq!(work.clone().stored(), Some(work.clone()));
        assert!(work.check_not_reserved().is_ok());
    }

    #[test]
    fn a_token_is_one_word_and_a_refusal_never_repeats_it() {
        let token = check_token(&format!("  {TOKEN}\n")).unwrap();
        assert!(token.expose() == TOKEN);
        assert_eq!(format!("{token:?}"), "AccountToken(<redacted>)");
        for (text, reason) in [
            ("", "empty"),
            (" \n\n", "empty"),
            (
                "sk-ant-oat01-aaaaaaaaaaaaaaaa\nsk-ant-oat01-bbbbbbbbbbbbbbbb",
                "several_lines",
            ),
            ("short", "not_a_token"),
            ("sk-ant-oat01 aaaaaaaaaaaaaaaaaaaaaaaa", "not_a_token"),
            ("sk-ant-oat01-ääääääääääääääääää", "not_a_token"),
        ] {
            let error = check_token(text).unwrap_err();
            assert_eq!(error.code(), codes::INVALID_ACCOUNT_TOKEN);
            assert_eq!(error.details().unwrap()["reason"], reason, "{text:?}");
            assert!(error.hint().unwrap().contains("claude setup-token"));
            if !text.trim().is_empty() {
                assert!(!error.message().contains(text.trim()));
            }
        }
        assert!(check_token(&"a".repeat(1025)).is_err());
        assert!(check_token(&"a".repeat(1024)).is_ok());
    }

    #[test]
    fn a_token_is_old_after_eleven_months() {
        let added = Timestamp::from_millis(1_700_000_000_000).unwrap();
        let day = Duration::from_secs(24 * 60 * 60);
        assert!(!token_is_old(added, added));
        assert!(!token_is_old(added, added.checked_add(day * 334).unwrap()));
        assert!(token_is_old(
            added,
            added
                .checked_add(day * 334 + Duration::from_millis(1))
                .unwrap()
        ));
        assert!(!token_is_old(added, added.checked_sub(day).unwrap()));
    }

    fn run(is_error: bool, text: &str, reached_model: bool) -> TokenCheckRun {
        TokenCheckRun {
            is_error,
            text: Some(text.to_owned()),
            reached_model,
        }
    }

    #[test]
    fn only_a_refused_login_rejects_a_token_and_only_an_answer_shows_it_works() {
        assert_eq!(
            token_check(Some(&run(false, "ok", true))),
            TokenCheck::Works
        );
        for refused in [
            "Invalid API key · Please run /login",
            "API Error: 401 Invalid authentication credentials",
            "Failed to authenticate: OAuth token revoked",
            "OAuth token has expired. Please obtain a new token or refresh your existing token.",
        ] {
            assert_eq!(
                token_check(Some(&run(true, refused, false))),
                TokenCheck::Rejected,
                "{refused}"
            );
        }
        assert_eq!(
            token_check(Some(&run(
                true,
                "API Error: Can't reach the API server",
                false
            ))),
            TokenCheck::Unchecked(Unchecked::Network)
        );
        assert_eq!(
            token_check(Some(&run(true, "You've hit your weekly limit", false))),
            TokenCheck::Unchecked(Unchecked::UsageLimit)
        );
        for unclear in [
            run(true, "API Error: 529 Overloaded", false),
            run(true, "Invalid API key", true),
            run(false, "ok", false),
            TokenCheckRun {
                is_error: true,
                text: None,
                reached_model: false,
            },
        ] {
            assert_eq!(
                token_check(Some(&unclear)),
                TokenCheck::Unchecked(Unchecked::NoAnswer),
                "{unclear:?}"
            );
        }
        assert_eq!(
            token_check(None),
            TokenCheck::Unchecked(Unchecked::NoAnswer)
        );
        for why in [
            Unchecked::Network,
            Unchecked::UsageLimit,
            Unchecked::NoAnswer,
        ] {
            assert!(!why.words().is_empty());
        }
    }

    #[test]
    fn a_rejected_token_names_the_account_and_the_command_that_gives_a_new_one() {
        let name = AccountName::parse("work-a").unwrap();
        let added = token_rejected(&name, false);
        assert_eq!(added.code(), codes::ACCOUNT_TOKEN_REJECTED);
        assert!(added.message().contains("fold stored nothing"));
        assert!(added.hint().unwrap().contains("`fld account add work-a`"));
        let updated = token_rejected(&name, true);
        assert!(updated.message().contains("keeps its token"));
        assert!(
            updated
                .hint()
                .unwrap()
                .contains("`fld account update work-a`")
        );
        assert_eq!(updated.details().unwrap()["account"], "work-a");
    }
}
