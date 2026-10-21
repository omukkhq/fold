use std::path::Path;

use fold_core::Timestamp;
use serde_json::Value;

use super::Check;
use super::push::get_json;
use crate::output;

const NAME: &str = "workers";

/// The words in which Claude Code's own messages and the API's errors say that it is not signed in, such as `Not logged in · Please run /login` or `"type":"authentication_error"`, in lower case. Each is a phrase, so a path such as `src/routes/login.tsx` or another program's token in a process's stderr does not match.
const SIGN_IN_WORDS: [&str; 6] = [
    "run /login",
    "not logged in",
    "invalid api key",
    "oauth token has expired",
    "authentication_error",
    "invalid x-api-key",
];

/// Which workers of the project cannot work for a reason outside fold, as the running daemon knows them. `socket` is the socket of a daemon that answers, `slug` the project's registered slug and `started_at` the daemon's start, when it reports one.
pub(super) fn workers_check(
    socket: Option<&Path>,
    slug: Option<&str>,
    started_at: Option<Timestamp>,
) -> Check {
    let Some(slug) = slug else {
        return Check::ok(NAME, "the project is not registered, so it has no workers");
    };
    let Some(socket) = socket else {
        return Check::ok(NAME, "the daemon does not answer, so fold reads no workers");
    };
    workers_verdict(
        get_json(socket, &format!("/api/v1/projects/{slug}/workers")).as_ref(),
        started_at,
    )
}

/// Whether Claude Code's error of a turn says that it is not signed in.
pub(crate) fn is_sign_in_error(text: &str) -> bool {
    let text = text.to_lowercase();
    SIGN_IN_WORDS.iter().any(|words| text.contains(words))
}

/// The workers of one kind of failure: those whose error came after the daemon's start, and those whose error came before it. A worker keeps its error over a restart until a turn ends without one, so an error from before the start may already be fixed.
#[derive(Default)]
struct Failures {
    current: Vec<String>,
    before: Vec<String>,
}

impl Failures {
    fn push(&mut self, name: String, before_start: bool) {
        if before_start {
            self.before.push(name);
        } else {
            self.current.push(name);
        }
    }
}

/// `list` is the daemon's worker list, `None` when the daemon gave none. A stopped worker runs no turn, so it is left out. An error from before `started_at` is stated with that time and gives no fix, since the restart that the fix names has happened. Only the workers' names are shown, never Claude Code's error text.
pub(crate) fn workers_verdict(list: Option<&Value>, started_at: Option<Timestamp>) -> Check {
    let Some(list) = list else {
        return Check::ok(NAME, "the daemon gave no worker list");
    };
    let mut signed_out = Failures::default();
    let mut unreachable = Failures::default();
    for worker in list["workers"].as_array().into_iter().flatten() {
        if worker["state"].as_str() == Some("stopped") {
            continue;
        }
        let Some(name) = worker["name"].as_str().map(output::escape_control) else {
            continue;
        };
        let before_start = started_at.is_some_and(|start| {
            worker["last_error_at"]
                .as_i64()
                .is_some_and(|at| at < start.as_millis())
        });
        if worker["last_error"].as_str().is_some_and(is_sign_in_error) {
            signed_out.push(name, before_start);
        } else if worker["model_unreachable"].as_bool() == Some(true) {
            unreachable.push(name, before_start);
        }
    }
    let mut parts = Vec::new();
    let mut fixes = Vec::new();
    if !signed_out.current.is_empty() {
        parts.push(format!(
            "the last turn of {} failed because Claude Code is not signed in",
            signed_out.current.join(", ")
        ));
        fixes.push(
            "sign the workers in as the daemon claude and daemon environment checks say, then `fld restart`",
        );
    }
    if !unreachable.current.is_empty() {
        parts.push(format!(
            "the turns of {} fail before they reach the model",
            unreachable.current.join(", ")
        ));
        fixes.push("check the Claude Code sign-in, model and usage limit");
    }
    let mut before = Vec::new();
    if !signed_out.before.is_empty() {
        before.push(format!(
            "the last turn of {} failed because Claude Code was not signed in",
            signed_out.before.join(", ")
        ));
    }
    if !unreachable.before.is_empty() {
        before.push(format!(
            "the turns of {} failed before they reached the model",
            unreachable.before.join(", ")
        ));
    }
    if let (false, Some(start)) = (before.is_empty(), started_at) {
        parts.push(format!(
            "before the daemon started at {start}, {}",
            before.join(", and ")
        ));
    }
    if parts.is_empty() {
        return Check::ok(
            NAME,
            "no worker's last turn failed to sign in or to reach the model",
        );
    }
    if fixes.is_empty() {
        return Check::ok(NAME, parts.join("; "));
    }
    Check::warn(NAME, parts.join("; "), fixes.join("; "))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::Status;
    use super::*;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis).unwrap()
    }

    #[test]
    fn claude_codes_sign_in_messages_count_as_sign_in_errors() {
        for text in [
            "Not logged in · Please run /login",
            "Invalid API key · Please run /login",
            "OAuth token revoked · Please run /login",
            "OAuth token has expired. Please obtain a new token or refresh your existing token.",
            r#"API Error: 401 {"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#,
        ] {
            assert!(is_sign_in_error(text), "{text}");
        }
        for text in [
            "API Error: 529 Overloaded",
            "Error: Cannot find module './src/routes/login.tsx'",
            "fetch failed: https://app.example.com/login returned 500",
            "[github] the GitHub OAuth token in GITHUB_TOKEN is missing; run gh auth login",
        ] {
            assert!(!is_sign_in_error(text), "{text}");
        }
    }

    #[test]
    fn workers_that_cannot_sign_in_or_reach_the_model_are_named_without_their_error() {
        assert_eq!(workers_verdict(None, None).status, Status::Ok);
        assert_eq!(
            workers_verdict(None, None).message,
            "the daemon gave no worker list"
        );
        let list = json!({"workers": [
            {"name": "web-worker", "state": "idle", "last_error": "Not logged in · Please run /login", "last_error_at": 2_000, "model_unreachable": true},
            {"name": "api-worker", "state": "failed", "last_error": "API Error: 529", "last_error_at": 2_000, "model_unreachable": true},
            {"name": "old-worker", "state": "stopped", "last_error": "Not logged in · Please run /login", "last_error_at": 2_000, "model_unreachable": false},
            {"name": "fine", "state": "working", "last_error": null, "last_error_at": null, "model_unreachable": false},
        ]});
        let check = workers_verdict(Some(&list), Some(at(1_000)));
        assert_eq!(check.status, Status::Warn);
        assert_eq!(
            check.message,
            "the last turn of web-worker failed because Claude Code is not signed in; the turns of api-worker fail before they reach the model"
        );
        assert!(!check.message.contains("/login"));
        let fix = check.fix.unwrap();
        assert!(
            fix.contains("`fld restart`") && fix.contains("usage limit"),
            "{fix}"
        );
        assert_eq!(workers_verdict(Some(&list), None).status, Status::Warn);
        let calm = json!({"workers": [{"name": "fine", "state": "idle", "last_error": null, "model_unreachable": false}]});
        assert_eq!(workers_verdict(Some(&calm), None).status, Status::Ok);
        assert_eq!(workers_check(None, None, None).status, Status::Ok);
        assert_eq!(
            workers_check(None, Some("demo"), None).message,
            "the daemon does not answer, so fold reads no workers"
        );
    }

    #[test]
    fn an_error_from_before_the_daemons_start_names_that_time_and_gives_no_fix() {
        let start = at(1_790_000_000_000);
        let list = json!({"workers": [
            {"name": "web-worker", "state": "idle", "last_error": "Not logged in · Please run /login", "last_error_at": 1_789_999_999_000_i64, "model_unreachable": true},
            {"name": "api-worker", "state": "idle", "last_error": "API Error: 529", "last_error_at": 1_789_999_999_000_i64, "model_unreachable": true},
        ]});
        let check = workers_verdict(Some(&list), Some(start));
        assert_eq!(check.status, Status::Ok);
        assert_eq!(
            check.message,
            format!(
                "before the daemon started at {start}, the last turn of web-worker failed because Claude Code was not signed in, and the turns of api-worker failed before they reached the model"
            )
        );
        assert_eq!(check.fix, None);

        let mixed = json!({"workers": [
            {"name": "web-worker", "state": "idle", "last_error": "Not logged in · Please run /login", "last_error_at": 1_789_999_999_000_i64, "model_unreachable": false},
            {"name": "api-worker", "state": "idle", "last_error": "Invalid API key · Please run /login", "last_error_at": 1_790_000_000_500_i64, "model_unreachable": false},
        ]});
        let check = workers_verdict(Some(&mixed), Some(start));
        assert_eq!(check.status, Status::Warn);
        assert_eq!(
            check.message,
            format!(
                "the last turn of api-worker failed because Claude Code is not signed in; before the daemon started at {start}, the last turn of web-worker failed because Claude Code was not signed in"
            )
        );
        assert!(check.fix.unwrap().contains("`fld restart`"));
    }
}
