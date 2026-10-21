use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use fold_cli::daemon_client::{ClientConfig, DaemonClient};
use fold_daemon::services::agents::claude::{ClaudeVersion, MIN_CLAUDE_VERSION};
use fold_daemon::services::agents::environment::is_sign_in_variable;
use hyper::Method;
use serde::Deserialize;

use super::Check;
use crate::shell_env::SHELL_TIMEOUT;

/// How long doctor waits for the daemon's answer, which runs `claude` twice with a limit of 10 seconds each, at the same time.
const DAEMON_TIMEOUT: Duration = Duration::from_secs(15);

const CLAUDE_NAME: &str = "daemon claude";
const ENV_NAME: &str = "daemon environment";
const MODELS_NAME: &str = "models";

/// The daemon's answer at `/api/v1/worker-environment`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct WorkerEnvironment {
    pub(crate) source: String,
    pub(crate) shell_problem: Option<String>,
    pub(crate) claude: Option<String>,
    pub(crate) git: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) signed_in: Option<bool>,
    pub(crate) sign_in_variables: Vec<String>,
}

/// What the running daemon said about its workers' environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DaemonReport {
    /// No daemon answers on the socket.
    NotRunning,
    /// A daemon answers but does not report the environment, as one of an older fld does.
    Unknown,
    Report(WorkerEnvironment),
}

/// Asks the daemon on `socket` how it starts Claude Code for its workers. It never starts a daemon.
pub(crate) fn ask_daemon(
    socket: &Path,
    human: Option<String>,
    runtime: &tokio::runtime::Runtime,
) -> DaemonReport {
    let client = DaemonClient::new(
        ClientConfig::running(socket.to_owned()),
        Some(human.unwrap_or_else(|| "doctor".to_owned())),
    );
    let answer = runtime.block_on(async {
        tokio::time::timeout(
            DAEMON_TIMEOUT,
            client.request(Method::GET, "/api/v1/worker-environment", None),
        )
        .await
    });
    match answer {
        Ok(Ok(response)) if response.status == 200 => serde_json::from_value(response.body)
            .map_or(DaemonReport::Unknown, DaemonReport::Report),
        Ok(Ok(_)) => DaemonReport::Unknown,
        Ok(Err(_)) => DaemonReport::NotRunning,
        Err(_) => DaemonReport::Unknown,
    }
}

/// The words for where the workers' environment came from.
fn source_text(source: &str) -> &'static str {
    match source {
        "shell" => "your login shell's environment",
        "setup" => "the environment that fld setup saved",
        _ => "the daemon's own environment",
    }
}

/// The fix when the daemon's `PATH` lacks a program: the daemon reads the login shell again at its next start.
const PATH_FIX: &str = "install Claude Code, or add its folder to PATH in your shell's startup files (such as ~/.profile or ~/.zprofile), then `fld restart`";

/// Why the daemon did not take the login shell's environment, in words, from the problem name the daemon reports.
pub(crate) fn shell_problem_words(problem: &str) -> String {
    match problem {
        "timed_out" => format!(
            "your login shell took longer than {} s",
            SHELL_TIMEOUT.as_secs()
        ),
        "no_claude" => "your login shell's PATH has no claude".to_owned(),
        "failed" => {
            "your login shell exited with an error or printed nothing fold could read".to_owned()
        }
        "no_shell" => "fold found no login shell".to_owned(),
        other => format!(
            "of the shell problem {}",
            crate::output::escape_control(other)
        ),
    }
}

/// Whether the daemon runs a `claude` that its workers can use: one on the `PATH` it gives them, recent enough and signed in there. `terminal` holds the sign-in variables of this terminal, by name, so a signed-out claude names those the workers lack.
pub(crate) fn daemon_claude_verdict(report: &DaemonReport, terminal: &BTreeSet<String>) -> Check {
    let environment = match report {
        DaemonReport::NotRunning => {
            return Check::ok(
                CLAUDE_NAME,
                "no daemon answers on the socket; this check runs while one does",
            );
        }
        DaemonReport::Unknown => {
            return Check::warn(
                CLAUDE_NAME,
                "the running daemon does not say which claude its workers run",
                "`fld restart`",
            );
        }
        DaemonReport::Report(environment) => environment,
    };
    let source = source_text(&environment.source);
    let shell = environment
        .shell_problem
        .as_deref()
        .map(|problem| {
            format!(
                "; the workers lack your login shell's environment, because {}",
                shell_problem_words(problem)
            )
        })
        .unwrap_or_default();
    let Some(claude) = &environment.claude else {
        return Check::fail(
            CLAUDE_NAME,
            format!(
                "the daemon finds no claude on the PATH of {source}, so no worker can start{shell}"
            ),
            PATH_FIX,
        );
    };
    let version = match environment
        .version
        .as_deref()
        .and_then(ClaudeVersion::parse)
    {
        None => {
            return Check::warn(
                CLAUDE_NAME,
                format!("{claude} --version gave the daemon no version fold can read"),
                "claude update",
            );
        }
        Some(version) if version < MIN_CLAUDE_VERSION => {
            return Check::warn(
                CLAUDE_NAME,
                format!(
                    "the daemon runs claude {version} at {claude}, older than {MIN_CLAUDE_VERSION}, the oldest version fold drives"
                ),
                "claude update",
            );
        }
        Some(version) => version,
    };
    let git = match &environment.git {
        Some(git) => format!("git at {git}"),
        None => "no git on its PATH".to_owned(),
    };
    match environment.signed_in {
        Some(true) => Check::ok(
            CLAUDE_NAME,
            format!(
                "workers run claude {version} at {claude}, signed in, with {source}{shell}; {git}"
            ),
        ),
        Some(false) => Check::fail(
            CLAUDE_NAME,
            format!(
                "claude {version} at {claude} is not signed in with {source}, so every worker turn fails{}{shell}",
                lacking(environment, terminal)
            ),
            "`claude auth login`, or give the daemon the sign-in variables of your shell (see daemon environment), then `fld restart`",
        ),
        None => Check::warn(
            CLAUDE_NAME,
            format!(
                "claude {version} at {claude}: claude auth status gave the daemon no answer fold can read"
            ),
            "`claude auth login`, then `fld restart`",
        ),
    }
}

/// The sign-in variables that `terminal` sets and the workers' environment lacks, as a clause of the check's message; empty when it lacks none.
fn lacking(environment: &WorkerEnvironment, terminal: &BTreeSet<String>) -> String {
    let workers = sign_in_names(environment.sign_in_variables.iter().map(String::as_str));
    let lacks: Vec<&str> = terminal.difference(&workers).map(String::as_str).collect();
    if lacks.is_empty() {
        return String::new();
    }
    format!(
        "; the workers lack {}, which this terminal sets",
        lacks.join(", ")
    )
}

/// The sorted sign-in variable names among `names`.
pub(crate) fn sign_in_names<'a>(names: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    names
        .into_iter()
        .filter(|name| is_sign_in_variable(name))
        .map(str::to_owned)
        .collect()
}

/// Compares, by name only, the variables that decide how Claude Code signs in: those of the workers' environment and `shell`, those of this terminal. A variable that one sets and the other does not can make Claude Code sign in differently, or not at all, in the workers.
pub(crate) fn daemon_env_verdict(
    report: &DaemonReport,
    shell: &BTreeSet<String>,
    linux: bool,
) -> Option<Check> {
    let DaemonReport::Report(environment) = report else {
        return None;
    };
    let daemon: BTreeSet<String> =
        sign_in_names(environment.sign_in_variables.iter().map(String::as_str));
    let lacks: Vec<&str> = shell.difference(&daemon).map(String::as_str).collect();
    let extra: Vec<&str> = daemon.difference(shell).map(String::as_str).collect();
    if lacks.is_empty() && extra.is_empty() {
        let names: Vec<&str> = shell.iter().map(String::as_str).collect();
        return Some(Check::ok(
            ENV_NAME,
            format!(
                "the workers and this shell set the same sign-in variables: {}",
                names.join(", ")
            ),
        ));
    }
    let mut parts = Vec::new();
    if !lacks.is_empty() {
        parts.push(format!(
            "the workers' environment lacks {}, which this shell sets",
            lacks.join(", ")
        ));
    }
    if !extra.is_empty() {
        parts.push(format!(
            "it sets {}, which this shell does not",
            extra.join(", ")
        ));
    }
    let manager = if linux {
        "`systemctl --user set-environment NAME=value`"
    } else {
        "`launchctl setenv NAME value`"
    };
    Some(Check::warn(
        ENV_NAME,
        parts.join("; "),
        format!(
            "set each variable in your login shell's startup files and run `fld restart`, which reads them again; or run `fld setup` again, or {manager}, then `fld restart`"
        ),
    ))
}

/// The model aliases that Claude Code takes for `--model`, each also with the `[1m]` suffix of its long context.
const MODEL_ALIASES: [&str; 7] = [
    "default", "best", "sonnet", "opus", "haiku", "fable", "opusplan",
];

/// Whether `model` has a form that Claude Code takes: an alias, or a model id that names Claude, as the ids of the Claude API (`claude-sonnet-5-5`), of Amazon Bedrock (`anthropic.claude-…`, or an ARN) and of Vertex AI (`claude-…@…`) all do. It reads the form only; whether the account may use the model shows at the first turn.
pub(crate) fn is_claude_model(model: &str) -> bool {
    let alias = model.strip_suffix("[1m]").unwrap_or(model);
    MODEL_ALIASES.contains(&alias) || model.to_ascii_lowercase().contains("claude")
}

/// Warns about each model of the project's role settings and area profiles, given as `(where, model)`, that has no form Claude Code takes. A gateway that names its own models can take others, so it is a warning.
pub(crate) fn model_names_verdict(models: &[(String, String)]) -> Check {
    let odd: Vec<String> = models
        .iter()
        .filter(|(_, model)| !is_claude_model(model))
        .map(|(place, model)| format!("{model} ({place})"))
        .collect();
    if odd.is_empty() {
        return Check::ok(
            MODELS_NAME,
            match models.len() {
                0 => "no role or area names a model".to_owned(),
                1 => "the one model name has a form claude takes".to_owned(),
                count => format!("all {count} model names have a form claude takes"),
            },
        );
    }
    Check::warn(
        MODELS_NAME,
        format!(
            "{} is neither a Claude Code alias nor a Claude model id, so claude may refuse every turn",
            odd.join(", ")
        ),
        "use an alias such as sonnet or opus, or a model id such as claude-sonnet-5-5, in .fold/config.toml or the area's profile",
    )
}

#[cfg(test)]
mod tests {
    use super::super::Status;
    use super::*;

    fn report(change: impl FnOnce(&mut WorkerEnvironment)) -> DaemonReport {
        let mut environment = WorkerEnvironment {
            source: "shell".to_owned(),
            shell_problem: None,
            claude: Some("/home/ana/.local/bin/claude".to_owned()),
            git: Some("/usr/bin/git".to_owned()),
            version: Some("2.1.301".to_owned()),
            signed_in: Some(true),
            sign_in_variables: vec!["ANTHROPIC_API_KEY".to_owned(), "HOME".to_owned()],
        };
        change(&mut environment);
        DaemonReport::Report(environment)
    }

    #[test]
    fn a_daemon_whose_workers_can_run_claude_is_ok_and_names_where_it_looked() {
        let check = daemon_claude_verdict(&report(|_| {}), &BTreeSet::new());
        assert_eq!(check.status, Status::Ok, "{check:?}");
        assert_eq!(
            check.message,
            "workers run claude 2.1.301 at /home/ana/.local/bin/claude, signed in, with your login shell's environment; git at /usr/bin/git"
        );
        let fallback = daemon_claude_verdict(
            &report(|environment| {
                environment.source = "setup".to_owned();
                environment.shell_problem = Some("timed_out".to_owned());
                environment.git = None;
            }),
            &BTreeSet::new(),
        );
        assert_eq!(fallback.status, Status::Ok);
        assert!(
            fallback.message.contains(
                "with the environment that fld setup saved; the workers lack your login shell's environment, because your login shell took longer than 10 s; no git on its PATH"
            ),
            "{}",
            fallback.message
        );
    }

    #[test]
    fn a_daemon_without_claude_or_signed_out_fails_and_an_old_or_odd_claude_warns() {
        let none = BTreeSet::new();
        let missing = daemon_claude_verdict(
            &report(|environment| {
                environment.source = "daemon".to_owned();
                environment.shell_problem = Some("no_claude".to_owned());
                environment.claude = None;
            }),
            &none,
        );
        assert_eq!(missing.status, Status::Fail);
        assert_eq!(
            missing.message,
            "the daemon finds no claude on the PATH of the daemon's own environment, so no worker can start; the workers lack your login shell's environment, because your login shell's PATH has no claude"
        );
        assert!(missing.fix.unwrap().contains("fld restart"));
        let signed_out = daemon_claude_verdict(
            &report(|environment| {
                environment.signed_in = Some(false);
            }),
            &none,
        );
        assert_eq!(signed_out.status, Status::Fail);
        assert_eq!(
            signed_out.message,
            "claude 2.1.301 at /home/ana/.local/bin/claude is not signed in with your login shell's environment, so every worker turn fails"
        );
        for (version, signed_in) in [
            (None, Some(true)),
            (Some("2.0.1"), Some(true)),
            (Some("2.1.301"), None),
        ] {
            let check = daemon_claude_verdict(
                &report(|environment| {
                    environment.version = version.map(str::to_owned);
                    environment.signed_in = signed_in;
                }),
                &none,
            );
            assert_eq!(check.status, Status::Warn, "{version:?} {signed_in:?}");
        }
        assert_eq!(
            daemon_claude_verdict(&DaemonReport::NotRunning, &none).status,
            Status::Ok
        );
        let old = daemon_claude_verdict(&DaemonReport::Unknown, &none);
        assert_eq!(old.status, Status::Warn);
        assert_eq!(old.fix.as_deref(), Some("`fld restart`"));
    }

    #[test]
    fn a_signed_out_claude_names_the_variables_the_workers_lack_and_why_the_shell_was_not_used() {
        let terminal = sign_in_names(["HOME", "CLAUDE_CODE_OAUTH_TOKEN", "PATH"]);
        let check = daemon_claude_verdict(
            &report(|environment| {
                environment.source = "setup".to_owned();
                environment.shell_problem = Some("timed_out".to_owned());
                environment.signed_in = Some(false);
                environment.sign_in_variables = vec!["HOME".to_owned()];
            }),
            &terminal,
        );
        assert_eq!(check.status, Status::Fail);
        assert_eq!(
            check.message,
            "claude 2.1.301 at /home/ana/.local/bin/claude is not signed in with the environment that fld setup saved, so every worker turn fails; the workers lack CLAUDE_CODE_OAUTH_TOKEN, which this terminal sets; the workers lack your login shell's environment, because your login shell took longer than 10 s"
        );
        assert!(!check.message.contains("PATH"));
    }

    #[test]
    fn each_shell_problem_reads_as_words() {
        assert_eq!(
            shell_problem_words("timed_out"),
            "your login shell took longer than 10 s"
        );
        assert_eq!(
            shell_problem_words("no_claude"),
            "your login shell's PATH has no claude"
        );
        assert_eq!(
            shell_problem_words("failed"),
            "your login shell exited with an error or printed nothing fold could read"
        );
        assert_eq!(shell_problem_words("no_shell"), "fold found no login shell");
        assert_eq!(
            shell_problem_words("new\u{1b}"),
            "of the shell problem new\\u{1b}"
        );
    }

    #[test]
    fn the_sign_in_variables_are_compared_by_name_only() {
        let same = sign_in_names(["HOME", "PATH", "ANTHROPIC_API_KEY"]);
        let check = daemon_env_verdict(&report(|_| {}), &same, true).unwrap();
        assert_eq!(check.status, Status::Ok);
        assert_eq!(
            check.message,
            "the workers and this shell set the same sign-in variables: ANTHROPIC_API_KEY, HOME"
        );
        let shell = sign_in_names(["HOME", "CLAUDE_CONFIG_DIR"]);
        let check = daemon_env_verdict(&report(|_| {}), &shell, true).unwrap();
        assert_eq!(check.status, Status::Warn);
        assert_eq!(
            check.message,
            "the workers' environment lacks CLAUDE_CONFIG_DIR, which this shell sets; it sets ANTHROPIC_API_KEY, which this shell does not"
        );
        assert!(
            check
                .fix
                .unwrap()
                .contains("systemctl --user set-environment")
        );
        let mac = daemon_env_verdict(&report(|_| {}), &shell, false).unwrap();
        assert!(mac.fix.unwrap().contains("launchctl setenv"));
        assert_eq!(
            daemon_env_verdict(&DaemonReport::NotRunning, &shell, true),
            None
        );
        assert_eq!(
            daemon_env_verdict(&DaemonReport::Unknown, &shell, true),
            None
        );
    }

    #[test]
    fn a_model_name_passes_as_an_alias_or_a_claude_id_and_anything_else_warns() {
        for model in [
            "sonnet",
            "opus[1m]",
            "opusplan",
            "default",
            "claude-sonnet-5-5",
            "anthropic.claude-opus-5-5",
            "arn:aws:bedrock:us-east-1:1:inference-profile/us.anthropic.claude-sonnet-5-5",
            "claude-opus-4-5@20251101",
        ] {
            assert!(is_claude_model(model), "{model}");
        }
        for model in ["gpt-5", "sonet", "sonnet[2m]", "llama"] {
            assert!(!is_claude_model(model), "{model}");
        }
        assert_eq!(model_names_verdict(&[]).status, Status::Ok);
        let models = [
            ("roles.task_worker".to_owned(), "sonnet".to_owned()),
            ("area web".to_owned(), "sonet".to_owned()),
        ];
        let check = model_names_verdict(&models);
        assert_eq!(check.status, Status::Warn);
        assert!(
            check.message.starts_with("sonet (area web) is neither"),
            "{}",
            check.message
        );
        assert_eq!(
            model_names_verdict(&models[..1]).message,
            "the one model name has a form claude takes"
        );
    }
}
