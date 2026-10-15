use std::fmt;

use serde::Deserialize;

/// The oldest Claude Code that fold drives: the version the headless probe passed on. `CLAUDE_CODE_PROMPT_CACHE_TTL` needs 2.1.242 or later.
pub const MIN_CLAUDE_VERSION: ClaudeVersion = ClaudeVersion::new(2, 1, 301);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClaudeVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl ClaudeVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Reads the output of `claude --version`, such as `2.1.301 (Claude Code)`: the first word is the version, and a pre-release or build suffix after the patch number is ignored.
    pub fn parse(output: &str) -> Option<Self> {
        let word = output.split_whitespace().next()?;
        let word = word.strip_prefix('v').unwrap_or(word);
        let mut parts = word.splitn(3, '.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let rest = parts.next()?;
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .map_or(rest, |end| &rest[..end]);
        let patch = digits.parse().ok()?;
        Some(Self::new(major, minor, patch))
    }
}

impl fmt::Display for ClaudeVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The part of `claude auth status --json` fold reads. The answer also names the account's email and organisation; they are never deserialized, so they cannot reach a log or a message.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AuthStatus {
    #[serde(rename = "loggedIn")]
    pub logged_in: bool,
    #[serde(rename = "authMethod", default)]
    pub auth_method: Option<String>,
}

impl AuthStatus {
    pub fn parse(output: &str) -> Option<Self> {
        serde_json::from_str(output.trim()).ok()
    }
}
