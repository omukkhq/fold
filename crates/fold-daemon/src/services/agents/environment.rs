use std::ffi::OsString;
use std::path::PathBuf;

use fold_core::layout::{AGENT_ENV, AGENT_TAGS_ENV};

use crate::support::SESSION_ENV;

/// Where the environment that the daemon gives its workers came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EnvSource {
    /// The user's login shell, read once at the daemon's start.
    Shell,
    /// The daemon's own environment, which the service that `fld setup` installed gives it from the environment of that setup.
    Setup,
    /// The daemon's own environment, from whatever started it.
    #[default]
    Daemon,
}

impl EnvSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Setup => "setup",
            Self::Daemon => "daemon",
        }
    }
}

/// Why the daemon did not take the login shell's environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellProblem {
    /// Neither `SHELL` nor the user's entry in the password file names an absolute path.
    NoShell,
    /// The shell could not start, exited with an error or printed no environment fld could read.
    Failed,
    /// The shell did not finish within the time limit and was stopped.
    TimedOut,
    /// The shell's `PATH` holds no `claude`.
    NoClaude,
}

impl ShellProblem {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoShell => "no_shell",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::NoClaude => "no_claude",
        }
    }
}

/// What the daemon found at its start about the environment of its workers. It holds paths and variable names, never a variable's value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerEnvironment {
    pub source: EnvSource,
    /// Why the login shell's environment was not used; `None` when it was, or when nobody read it.
    pub shell_problem: Option<ShellProblem>,
    /// The absolute path of `claude` on the chosen `PATH`.
    pub claude: Option<PathBuf>,
    /// The absolute path of `git` on the chosen `PATH`.
    pub git: Option<PathBuf>,
    /// The names, sorted, of the variables of the workers' environment that decide how Claude Code signs in, as [`is_sign_in_variable`] picks them.
    pub sign_in: Vec<String>,
}

/// The variables that fold sets or removes itself on every worker process, so the shell's value never replaces fold's.
const FOLD_SET: [&str; 4] = [
    AGENT_ENV,
    AGENT_TAGS_ENV,
    "CLAUDE_CODE_PROMPT_CACHE_TTL",
    "CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL",
];

/// The variables that decide which daemon, socket and configuration a worker's `fld mcp` reaches. A worker keeps the daemon's own, set or not, so it always reaches the daemon that started it.
const DAEMON_ROUTE: [&str; 4] = [
    "FOLD_SOCKET",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_RUNTIME_DIR",
];

/// The state of the shell that printed its environment, which says nothing about the user's setup.
const SHELL_STATE: [&str; 4] = ["_", "OLDPWD", "PWD", "SHLVL"];

/// The login shell's variables that a worker takes over the daemon's own: all of them except those fold sets itself, those that route `fld mcp` to its daemon, the shell's own state and the variables of a Claude Code session around the daemon. A name or value that is not UTF-8 is left out.
pub fn worker_variables(
    shell: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(String, String)> {
    let mut kept: Vec<(String, String)> = shell
        .into_iter()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .filter(|(name, _)| {
            let name = name.as_str();
            !name.is_empty()
                && !FOLD_SET.contains(&name)
                && !DAEMON_ROUTE.contains(&name)
                && !SHELL_STATE.contains(&name)
                && !SESSION_ENV.contains(&name)
        })
        .collect();
    kept.sort();
    kept.dedup_by(|a, b| a.0 == b.0);
    kept
}

/// The variables whose presence decides how Claude Code finds its sign-in: its home and configuration folder, an API key, token or gateway, and the cloud providers it can sign in through.
pub fn is_sign_in_variable(name: &str) -> bool {
    const EXACT: [&str; 11] = [
        "AWS_ACCESS_KEY_ID",
        "AWS_BEARER_TOKEN_BEDROCK",
        "AWS_PROFILE",
        "AWS_REGION",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CONFIG_DIR",
        "CLOUD_ML_REGION",
        "GOOGLE_APPLICATION_CREDENTIALS",
        "HOME",
    ];
    EXACT.contains(&name) || name.starts_with("ANTHROPIC_") || name.starts_with("CLAUDE_CODE_USE_")
}

/// The names among `names` that [`is_sign_in_variable`] picks, sorted and each once.
pub fn sign_in_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut picked: Vec<String> = names
        .into_iter()
        .filter(|name| is_sign_in_variable(name))
        .map(str::to_owned)
        .collect();
    picked.sort();
    picked.dedup();
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    }

    #[test]
    fn a_worker_takes_the_shells_variables_but_not_those_fold_decides() {
        let shell = vars(&[
            ("PATH", "/home/ana/.local/bin:/usr/bin"),
            ("ANTHROPIC_API_KEY", "secret"),
            ("NVM_DIR", "/home/ana/.nvm"),
            ("FOLD_AGENT", "main"),
            ("FOLD_AGENT_TAGS", "main"),
            ("CLAUDE_CODE_PROMPT_CACHE_TTL", "1h"),
            ("CLAUDECODE", "1"),
            ("XDG_RUNTIME_DIR", "/run/other"),
            ("XDG_CONFIG_HOME", "/cfg"),
            ("FOLD_SOCKET", "/tmp/other.sock"),
            ("XDG_CACHE_HOME", "/cache"),
            ("PWD", "/home/ana"),
            ("SHLVL", "2"),
            ("_", "/usr/bin/fld"),
            ("", "empty"),
        ]);
        let kept = worker_variables(shell);
        let names: Vec<&str> = kept.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            ["ANTHROPIC_API_KEY", "NVM_DIR", "PATH", "XDG_CACHE_HOME"]
        );
        assert_eq!(kept[2].1, "/home/ana/.local/bin:/usr/bin");
    }

    #[test]
    fn a_name_or_value_that_is_not_utf8_stays_out_and_a_repeated_name_counts_once() {
        use std::os::unix::ffi::OsStringExt;
        let shell = vec![
            (OsString::from("A"), OsString::from_vec(vec![0xff])),
            (OsString::from_vec(vec![0xfe]), OsString::from("x")),
            (OsString::from("B"), OsString::from("1")),
            (OsString::from("B"), OsString::from("2")),
        ];
        assert_eq!(worker_variables(shell), [("B".to_owned(), "1".to_owned())]);
    }

    #[test]
    fn the_sign_in_variables_are_the_ones_claude_code_reads_for_its_account() {
        for name in [
            "HOME",
            "CLAUDE_CONFIG_DIR",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_BASE_URL",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "AWS_PROFILE",
        ] {
            assert!(is_sign_in_variable(name), "{name}");
        }
        for name in ["PATH", "NVM_DIR", "CLAUDECODE", "XDG_CONFIG_HOME"] {
            assert!(!is_sign_in_variable(name), "{name}");
        }
        assert_eq!(
            sign_in_names(["PATH", "HOME", "ANTHROPIC_API_KEY", "HOME"]),
            ["ANTHROPIC_API_KEY", "HOME"]
        );
    }

    #[test]
    fn sources_and_problems_have_stable_names() {
        assert_eq!(EnvSource::default(), EnvSource::Daemon);
        let sources = [EnvSource::Shell, EnvSource::Setup, EnvSource::Daemon];
        assert_eq!(sources.map(EnvSource::as_str), ["shell", "setup", "daemon"]);
        let problems = [
            ShellProblem::NoShell,
            ShellProblem::Failed,
            ShellProblem::TimedOut,
            ShellProblem::NoClaude,
        ];
        assert_eq!(
            problems.map(ShellProblem::as_str),
            ["no_shell", "failed", "timed_out", "no_claude"]
        );
    }
}
