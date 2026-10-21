use std::path::Path;

use serde_json::Value;

use super::Check;
use super::push::get_json;
use crate::output;

const NAME: &str = "workers";

/// The words in which Claude Code's own messages say that it is not signed in, such as `Not logged in · Please run /login`, in lower case.
const SIGN_IN_WORDS: [&str; 4] = ["/login", "not logged in", "invalid api key", "oauth token"];

/// Which workers of the project cannot work for a reason outside fold, as the running daemon knows them. `socket` is the daemon's socket, `slug` the project's registered slug.
pub(super) fn workers_check(socket: Option<&Path>, slug: Option<&str>) -> Check {
    let Some(slug) = slug else {
        return Check::ok(NAME, "the project is not registered, so it has no workers");
    };
    workers_verdict(
        socket
            .and_then(|socket| get_json(socket, &format!("/api/v1/projects/{slug}/workers")))
            .as_ref(),
    )
}

/// Whether Claude Code's error of a turn says that it is not signed in.
pub(crate) fn is_sign_in_error(text: &str) -> bool {
    let text = text.to_lowercase();
    SIGN_IN_WORDS.iter().any(|words| text.contains(words))
}

/// `list` is the daemon's worker list, `None` when no daemon answered. A stopped worker runs no turn, so it is left out. Only the workers' names are shown, never Claude Code's error text.
pub(crate) fn workers_verdict(list: Option<&Value>) -> Check {
    let Some(list) = list else {
        return Check::ok(NAME, "the daemon does not answer, so fold reads no workers");
    };
    let mut signed_out = Vec::new();
    let mut unreachable = Vec::new();
    for worker in list["workers"].as_array().into_iter().flatten() {
        if worker["state"].as_str() == Some("stopped") {
            continue;
        }
        let Some(name) = worker["name"].as_str().map(output::escape_control) else {
            continue;
        };
        if worker["last_error"].as_str().is_some_and(is_sign_in_error) {
            signed_out.push(name);
        } else if worker["model_unreachable"].as_bool() == Some(true) {
            unreachable.push(name);
        }
    }
    let mut parts = Vec::new();
    let mut fixes = Vec::new();
    if !signed_out.is_empty() {
        parts.push(format!(
            "the last turn of {} failed because Claude Code is not signed in",
            signed_out.join(", ")
        ));
        fixes.push(
            "sign the workers in as the daemon claude and daemon environment checks say, then `fld restart`",
        );
    }
    if !unreachable.is_empty() {
        parts.push(format!(
            "the turns of {} fail before they reach the model",
            unreachable.join(", ")
        ));
        fixes.push("check the Claude Code sign-in, model and usage limit");
    }
    if parts.is_empty() {
        return Check::ok(
            NAME,
            "no worker's last turn failed to sign in or to reach the model",
        );
    }
    Check::warn(NAME, parts.join("; "), fixes.join("; "))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::Status;
    use super::*;

    #[test]
    fn claude_codes_sign_in_messages_count_as_sign_in_errors() {
        for text in [
            "Not logged in · Please run /login",
            "Invalid API key · Please run /login",
            "OAuth token has expired",
        ] {
            assert!(is_sign_in_error(text), "{text}");
        }
        assert!(!is_sign_in_error("API Error: 529 Overloaded"));
    }

    #[test]
    fn workers_that_cannot_sign_in_or_reach_the_model_are_named_without_their_error() {
        assert_eq!(workers_verdict(None).status, Status::Ok);
        let list = json!({"workers": [
            {"name": "web-worker", "state": "idle", "last_error": "Not logged in · Please run /login", "model_unreachable": true},
            {"name": "api-worker", "state": "failed", "last_error": "API Error: 529", "model_unreachable": true},
            {"name": "old-worker", "state": "stopped", "last_error": "Not logged in · Please run /login", "model_unreachable": false},
            {"name": "fine", "state": "working", "last_error": null, "model_unreachable": false},
        ]});
        let check = workers_verdict(Some(&list));
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
        let calm = json!({"workers": [{"name": "fine", "state": "idle", "last_error": null, "model_unreachable": false}]});
        assert_eq!(workers_verdict(Some(&calm)).status, Status::Ok);
        assert_eq!(workers_check(None, None).status, Status::Ok);
    }
}
