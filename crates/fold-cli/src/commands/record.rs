use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use clap::Args;
use fold_cli::daemon_client::Response;
use serde_json::Value;

use super::confirm::Consent;
use super::remote::{RemoteError, post_to_project, refusal};
use crate::output;

#[derive(Args, Debug)]
pub struct RecordArgs {
    /// Record without asking when the change empties a tracked file, adds many empty files or holds files that changed while a worker worked
    #[arg(long, short = 'y')]
    yes: bool,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

/// The most changed or conflicted paths the command lists; the rest are counted.
const LISTED_PATHS: usize = 20;

/// The code of a record that waits for the human's confirmation.
const NEEDS_CONFIRMATION: &str = "record_needs_confirmation";

/// Asks the daemon to record the human's changes in the project folder on main, starting the daemon when none runs. A change the daemon holds back for a confirmation is named, then recorded with `--yes` or after a yes on the terminal, as [`Consent`] decides; without a terminal or `--yes` it is refused with `needs_confirmation`.
pub fn run_record(args: &RecordArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    record_with(
        |action| post_to_project(args.socket.as_deref(), args.project.as_deref(), action),
        Consent::of(args.yes),
        // Not locked for the whole command: a request can read the human's name from the terminal.
        &mut io::BufReader::new(io::stdin()),
        out,
    )
}

/// [`run_record`] with the daemon's answers from `post`, which takes the action below the project, and the human's yes from `consent`, which reads `input`.
fn record_with(
    mut post: impl FnMut(&str) -> Result<Response, RemoteError>,
    consent: Consent,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<(), RemoteError> {
    let mut response = post("record")?;
    if let Some(tree) = confirmation_tree(&response) {
        for line in confirmation_lines(&response) {
            output::line(out, &line)?;
        }
        if !consent.ask(
            "Record these files as your change?",
            "fld record",
            input,
            out,
        )? {
            output::line(out, "nothing was recorded")?;
            return Ok(());
        }
        response = post(&format!("record?confirm={tree}"))?;
    }
    for line in summary(&response)? {
        output::line(out, &line)?;
    }
    Ok(())
}

/// The tree a `record_needs_confirmation` refusal asks to confirm.
fn confirmation_tree(response: &Response) -> Option<String> {
    let refused = response.refusal();
    if response.is_success() || refused.code.as_deref() != Some(NEEDS_CONFIRMATION) {
        return None;
    }
    response.body["error"]["details"]["tree"]
        .as_str()
        .map(str::to_owned)
}

/// What the record holds back for a confirmation: the daemon's sentence, then the files of each reason, each list cut at [`LISTED_PATHS`] and counted.
fn confirmation_lines(response: &Response) -> Vec<String> {
    let details = &response.body["error"]["details"];
    let mut lines = vec![format!(
        "fold asks first: {}",
        response
            .refusal()
            .message
            .unwrap_or_else(|| "the change needs a confirmation".to_owned())
    )];
    for (key, title) in [
        ("emptied", "emptied"),
        ("empty_added", "new and empty"),
        ("agent_changed", "changed while a worker worked"),
    ] {
        let paths: Vec<&str> = details[key]
            .as_array()
            .map(|paths| paths.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let count = details[format!("{key}_count")]
            .as_u64()
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(paths.len());
        if count == 0 {
            continue;
        }
        lines.push(format!("{title}:"));
        for path in paths.iter().take(LISTED_PATHS) {
            lines.push(format!("  {path}"));
        }
        if count > LISTED_PATHS.min(paths.len()) {
            lines.push(format!(
                "  and {} more",
                count - LISTED_PATHS.min(paths.len())
            ));
        }
    }
    lines
}

/// The lines `fld record` prints for the daemon's answer, or the error it carries: one line, then one per listed path, touched workspace and skipped file.
fn summary(response: &Response) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    let body = &response.body;
    let text = |key: &str| body[key].as_str().ok_or(RemoteError::Answer);
    let list = |key: &str| body[key].as_array().ok_or(RemoteError::Answer);
    let short = |commit: &str| commit.get(..7).unwrap_or(commit).to_owned();
    let revision = text("revision")?;
    let mut lines = super::stash::finished_unstash_lines(body)?;
    lines.extend(match text("outcome")? {
        "up_to_date" => vec![format!(
            "nothing to record: the project folder has no change that main does not hold (main is {revision})"
        )],
        "recorded" => {
            let files = list("files")?;
            let commit = body["commit"]
                .as_str()
                .map(|commit| format!(", Git commit {}", short(commit)))
                .unwrap_or_default();
            let mut lines = vec![format!(
                "recorded your changes in main as {revision}{commit}: {}; the project folder follows main",
                output::count(files.len(), "file", "files")
            )];
            push_paths(
                &mut lines,
                files.iter().map(|file| file.as_str()),
                files.len(),
            )?;
            lines
        }
        "conflicted" => {
            let conflicted = list("conflicted")?;
            let paths = output::count(conflicted.len(), "path", "paths");
            let mut lines = vec![if body["created"].as_bool().ok_or(RemoteError::Answer)? {
                format!(
                    "your changes conflict with main {revision} in {paths}; main did not move. Your files stay as they are and the change waits in Needs you; ask the main agent to explain the conflict and prepare the merge for your OK"
                )
            } else {
                format!(
                    "your changes wait for the main agent already ({paths} conflicted); nothing new was recorded"
                )
            }];
            for conflict in conflicted.iter().take(LISTED_PATHS) {
                let (Some(path), Some(kind)) =
                    (conflict["path"].as_str(), conflict["kind"].as_str())
                else {
                    return Err(RemoteError::Answer);
                };
                lines.push(format!("  {path} ({kind})"));
            }
            if conflicted.len() > LISTED_PATHS {
                lines.push(format!("  and {} more", conflicted.len() - LISTED_PATHS));
            }
            lines
        }
        _ => return Err(RemoteError::Answer),
    });
    for touched in list("touched")? {
        let (Some(task), Some(workspace)) =
            (touched["task"].as_str(), touched["workspace"].as_str())
        else {
            return Err(RemoteError::Answer);
        };
        lines.push(format!(
            "{task} changes the same files in {workspace}; fold tells its lead and the main agent"
        ));
    }
    for skipped in list("skipped")? {
        let (Some(path), Some(reason)) = (skipped["path"].as_str(), skipped["reason"].as_str())
        else {
            return Err(RemoteError::Answer);
        };
        lines.push(format!(
            "left out {path} ({reason}), as an import leaves it out"
        ));
    }
    Ok(lines)
}

/// One line per path, up to [`LISTED_PATHS`], then a count of the rest.
fn push_paths<'a>(
    lines: &mut Vec<String>,
    paths: impl Iterator<Item = Option<&'a str>>,
    count: usize,
) -> Result<(), RemoteError> {
    for path in paths.take(LISTED_PATHS) {
        lines.push(format!("  {}", path.ok_or(RemoteError::Answer)?));
    }
    if count > LISTED_PATHS {
        lines.push(format!("  and {} more", count - LISTED_PATHS));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn reply(status: u16, body: serde_json::Value) -> Response {
        Response { status, body }
    }

    fn lines(body: serde_json::Value) -> Vec<String> {
        summary(&reply(200, body)).unwrap()
    }

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn each_outcome_reads_as_lines() {
        assert_eq!(
            lines(
                json!({"outcome": "up_to_date", "commit": null, "revision": "r3", "files": [], "conflicted": [], "touched": [], "skipped": [], "created": false})
            ),
            [
                "nothing to record: the project folder has no change that main does not hold (main is r3)"
            ]
        );
        assert_eq!(
            lines(json!({
                "outcome": "recorded", "commit": COMMIT, "revision": "r4", "files": ["README.md", "docs/plan.md"],
                "conflicted": [], "touched": [{"task": "T-2", "workspace": "ws-3"}],
                "skipped": [{"path": "big.bin", "reason": "too_large"}], "created": true,
            })),
            [
                "recorded your changes in main as r4, Git commit 0123456: 2 files; the project folder follows main",
                "  README.md",
                "  docs/plan.md",
                "T-2 changes the same files in ws-3; fold tells its lead and the main agent",
                "left out big.bin (too_large), as an import leaves it out",
            ]
        );
        assert_eq!(
            lines(json!({
                "outcome": "recorded", "commit": null, "revision": "r4", "files": ["a.txt"],
                "conflicted": [], "touched": [], "skipped": [], "created": true,
            }))[0],
            "recorded your changes in main as r4: 1 file; the project folder follows main"
        );
        let conflicted = lines(json!({
            "outcome": "conflicted", "commit": null, "revision": "r5", "files": [],
            "conflicted": [{"path": "a.txt", "kind": "both_modified_text"}], "touched": [], "skipped": [], "created": true,
        }));
        assert!(
            conflicted[0]
                .starts_with("your changes conflict with main r5 in 1 path; main did not move.")
        );
        assert!(conflicted[0].contains("main agent"));
        assert_eq!(conflicted[1], "  a.txt (both_modified_text)");
        let waiting = lines(json!({
            "outcome": "conflicted", "commit": null, "revision": "r5", "files": [],
            "conflicted": [{"path": "a.txt", "kind": "both_modified_text"}], "touched": [], "skipped": [], "created": false,
        }));
        assert_eq!(
            waiting[0],
            "your changes wait for the main agent already (1 path conflicted); nothing new was recorded"
        );
    }

    #[test]
    fn a_record_names_the_unstash_it_finished_first() {
        assert_eq!(
            lines(json!({
                "outcome": "up_to_date", "commit": null, "revision": "r3", "files": [], "conflicted": [], "touched": [], "skipped": [], "created": false,
                "unstash": {"outcome": "conflicted", "revision": "r2", "files": ["a.txt"], "conflicted": [{"path": "a.txt", "kind": "both_modified_text", "sidecar": null}], "entries": 1},
            })),
            [
                "first finished the unstash that stopped part-way: stash@{0} conflicts with the project folder (r2) in 1 path. fold wrote conflict markers and keeps the entry; the project folder does not follow main until you fix the files and run fld record",
                "  a.txt (both_modified_text)",
                "nothing to record: the project folder has no change that main does not hold (main is r3)",
            ]
        );
    }

    fn held_back() -> Response {
        reply(
            409,
            json!({"error": {"code": "record_needs_confirmation", "message": "the change empties 1 tracked file, and holds 27 files that changed while reviewer-35 worked; nothing was recorded", "details": {
                "hint": "check the files", "tree": "abc", "emptied": ["plan.md"], "emptied_count": 1,
                "empty_added": [], "empty_added_count": 0,
                "agent_changed": (0..25).map(|n| format!("f{n}")).collect::<Vec<_>>(), "agent_changed_count": 27,
                "agents": ["reviewer-35"],
            }}}),
        )
    }

    fn recorded() -> Response {
        reply(
            200,
            json!({"outcome": "recorded", "commit": null, "revision": "r4", "files": ["plan.md"], "conflicted": [], "touched": [], "skipped": [], "created": true}),
        )
    }

    /// Runs a record whose first answer is `first` and whose confirmed one records, with `typed` as the human's answer; returns the actions sent and the output.
    fn run(
        first: Response,
        yes: bool,
        interactive: bool,
        typed: &str,
    ) -> (Vec<String>, String, Result<(), RemoteError>) {
        let mut sent = Vec::new();
        let mut first = Some(first);
        let mut out = Vec::new();
        let result = record_with(
            |action| {
                sent.push(action.to_owned());
                Ok(first.take().unwrap_or_else(recorded))
            },
            Consent { yes, interactive },
            &mut typed.as_bytes(),
            &mut out,
        );
        (sent, String::from_utf8(out).unwrap(), result)
    }

    #[test]
    fn a_held_back_record_names_the_files_and_goes_on_only_after_a_yes() {
        let (sent, out, result) = run(held_back(), true, false, "");
        result.unwrap();
        assert_eq!(sent, ["record", "record?confirm=abc"]);
        assert!(
            out.starts_with("fold asks first: the change empties 1 tracked file"),
            "{out}"
        );
        assert!(out.contains("emptied:\n  plan.md\n"), "{out}");
        assert!(
            out.contains("changed while a worker worked:\n  f0\n"),
            "{out}"
        );
        assert!(out.contains("  f19\n  and 7 more\n"), "{out}");
        assert!(!out.contains("new and empty"), "{out}");
        assert!(out.contains("recorded your changes in main as r4"), "{out}");

        let (sent, out, result) = run(held_back(), false, true, "y\n");
        result.unwrap();
        assert_eq!(sent.len(), 2);
        assert!(out.contains("[y/N] "), "{out}");

        let (sent, out, result) = run(held_back(), false, true, "\n");
        result.unwrap();
        assert_eq!(sent, ["record"]);
        assert!(out.ends_with("nothing was recorded\n"), "{out}");

        // Without a terminal the question cannot be answered, so the record stops with the reason the other commands that ask give.
        let (sent, out, result) = run(held_back(), false, false, "y\n");
        assert_eq!(sent, ["record"]);
        assert!(out.contains("emptied:\n  plan.md\n"), "{out}");
        let RemoteError::Fold(refused) = result.unwrap_err() else {
            panic!("a local refusal");
        };
        assert_eq!(refused.code(), "invalid_request");
        assert_eq!(refused.details().unwrap()["reason"], "needs_confirmation");
        assert!(
            refused
                .hint()
                .unwrap()
                .contains("`fld record` in a terminal")
        );

        let (sent, _, result) = run(recorded(), false, false, "");
        result.unwrap();
        assert_eq!(sent, ["record"]);
    }

    #[test]
    fn a_long_list_is_counted_and_a_bad_answer_or_refusal_is_an_error() {
        let files: Vec<String> = (0..25).map(|n| format!("f{n}")).collect();
        let many = lines(json!({
            "outcome": "recorded", "commit": COMMIT, "revision": "r4", "files": files,
            "conflicted": [], "touched": [], "skipped": [], "created": true,
        }));
        assert_eq!(many.len(), 1 + LISTED_PATHS + 1);
        assert_eq!(many.last().unwrap(), "  and 5 more");
        assert!(matches!(
            summary(&reply(200, json!({"outcome": "strange", "revision": "r1"}))),
            Err(RemoteError::Answer)
        ));
        let refused = json!({"error": {"code": "human_required", "message": "only a human records the changes of the project folder", "details": {"hint": "ask a human"}}});
        let error = summary(&reply(403, refused)).unwrap_err();
        assert_eq!(error.code(), Some("human_required"));
    }
}
