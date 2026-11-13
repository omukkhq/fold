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

const MAX_ACCOUNT_NAME_LEN: usize = 32;
const MIN_TOKEN_LEN: usize = 20;
const MAX_TOKEN_LEN: usize = 1024;

/// The name of a Claude subscription that the host keeps a token of: 1 to 32 characters of `a-z`, `0-9`, `-` and `_`, starting with a letter or digit. A project names it in its config; the token itself stays in the host's data folder.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct AccountName(String);

impl AccountName {
    pub fn parse(text: &str) -> Result<Self, FoldError> {
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
}
