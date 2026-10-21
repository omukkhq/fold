use std::io::Write;
use std::path::PathBuf;

use clap::Args;
use fold_cli::daemon_client::Response;
use fold_core::Timestamp;
use hyper::Method;
use serde_json::Value;

use super::remote::{RemoteError, refusal, request_project};
use crate::output;

/// The most revisions the daemon lists in one answer.
const MAX_COUNT: u64 = 1000;

#[derive(Args, Debug)]
pub struct LogArgs {
    /// List the newest N revisions of main; at most 1000
    #[arg(short = 'n', long = "count", value_name = "N", default_value_t = 20, value_parser = clap::value_parser!(u64).range(1..=1000))]
    count: u64,
    /// One line per revision: its id, the head mark and its title
    #[arg(long)]
    oneline: bool,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

/// Lists main's revisions, newest first, as the web app's repository screen describes them, starting the daemon when none runs.
pub fn run_log(args: &LogArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let response = request_project(
        args.socket.as_deref(),
        args.project.as_deref(),
        Method::GET,
        &format!("revisions?limit={}", args.count),
    )?;
    for line in log_lines(&response, args.count, args.oneline)? {
        output::line(out, &line)?;
    }
    Ok(())
}

/// The lines `fld log` prints for one page of revisions, newest first, or the error the answer carries. The page starts at main's head, because it is asked for without `before`.
fn log_lines(response: &Response, count: u64, oneline: bool) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    let revisions = response.body["revisions"]
        .as_array()
        .ok_or(RemoteError::Answer)?;
    if revisions.is_empty() {
        return Ok(vec!["main has no revisions yet".to_owned()]);
    }
    let mut lines = Vec::new();
    for (index, revision) in revisions.iter().enumerate() {
        let described = describe(revision)?;
        let head = if index == 0 { " (head of main)" } else { "" };
        lines.push(format!("{}{head} {}", described.id, described.title));
        if !oneline {
            lines.push(format!("  {}", described.meta));
        }
    }
    if !response.body["before"].is_null() {
        lines.push(if count < MAX_COUNT {
            format!(
                "older revisions are not listed; fld log -n {} lists more",
                (count * 2).min(MAX_COUNT)
            )
        } else {
            format!("older revisions are not listed; fld log lists at most {MAX_COUNT}")
        });
    }
    Ok(lines)
}

/// What one revision shows: its id, its title and its meta line of task, author, time and Git commit.
#[derive(Debug, PartialEq, Eq)]
struct Described {
    id: String,
    title: String,
    meta: String,
}

/// The title and the meta line by origin, in the words of the web app: a proposal names itself and its task, a pull the remote commit, a direct change its commit subject and the user's approval, a record the human's change of the project folder, and the genesis the start of the history. Agent and remote text is escaped.
fn describe(revision: &Value) -> Result<Described, RemoteError> {
    let text = |key: &str| revision[key].as_str().map(output::escape_control);
    let id = text("id").ok_or(RemoteError::Answer)?;
    let created_at = revision["created_at"]
        .as_i64()
        .and_then(|millis| Timestamp::from_millis(millis).ok())
        .ok_or(RemoteError::Answer)?;
    let who = text("author").map_or_else(|| "unknown".to_owned(), |author| output::person(&author));
    let at = created_at.to_string();
    let (title, meta) = match revision["origin"].as_str().ok_or(RemoteError::Answer)? {
        "proposal" => {
            let name = text("proposal").unwrap_or_else(|| id.clone());
            let title = match text("title") {
                Some(title) => format!("{name} · {title}"),
                None => name,
            };
            let meta = match text("task") {
                Some(task) => format!("{task} · {who} · {at}"),
                None => format!("{who} · {at}"),
            };
            (title, meta)
        }
        "pull" => (
            format!("pull · {}", text("pull").unwrap_or_default()),
            format!("{who} · {at}"),
        ),
        "direct" => (
            format!("direct · {}", text("title").unwrap_or_else(|| id.clone())),
            format!("{who} · approved by the user · {at}"),
        ),
        "record" => (
            format!(
                "record · {}",
                text("title").unwrap_or_else(|| "changes of the project folder".to_owned())
            ),
            format!("{who} · {at}"),
        ),
        "initial" => (
            text("title").unwrap_or_else(|| "Genesis revision".to_owned()),
            format!("{who} · {at}"),
        ),
        _ => return Err(RemoteError::Answer),
    };
    let meta = match text("commit") {
        Some(commit) => format!("{meta} · {}", commit.get(..7).unwrap_or(&commit)),
        None => meta,
    };
    Ok(Described { id, title, meta })
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use serde_json::json;

    use super::*;

    const AT: i64 = 1_792_576_800_000;
    const TIME: &str = "2026-10-21T10:00:00.000Z";

    fn revision(id: &str, origin: &str, fields: Value) -> Value {
        let mut revision = json!({
            "id": id, "parent": null, "created_at": AT, "tree": "t", "origin": origin,
            "task": null, "proposal": null, "title": null, "author": null, "pull": null, "commit": null,
        });
        for (key, value) in fields.as_object().unwrap() {
            revision[key] = value.clone();
        }
        revision
    }

    fn page(revisions: Vec<Value>, before: Option<&str>) -> Response {
        Response {
            status: 200,
            body: json!({"revisions": revisions, "before": before}),
        }
    }

    fn described(revision: &Value) -> (String, String) {
        let described = describe(revision).unwrap();
        (described.title, described.meta)
    }

    fn pair(title: &str, meta: &str) -> (String, String) {
        (title.to_owned(), meta.to_owned())
    }

    #[test]
    fn the_time_is_utc() {
        assert_eq!(Timestamp::from_millis(AT).unwrap().to_string(), TIME);
    }

    #[test]
    fn each_origin_reads_as_the_repository_screen_says_it() {
        let proposal = revision(
            "r17",
            "proposal",
            json!({"proposal": "P-3", "title": "Add the login form", "task": "T-12", "author": "frontend", "commit": "1a2b3c4d5e6f"}),
        );
        assert_eq!(
            described(&proposal),
            pair(
                "P-3 · Add the login form",
                &format!("T-12 · @frontend · {TIME} · 1a2b3c4")
            )
        );
        let bare = revision("r9", "proposal", json!({}));
        assert_eq!(described(&bare), pair("r9", &format!("unknown · {TIME}")));

        let pull = revision(
            "r16",
            "pull",
            json!({"pull": "9f8e7d6c", "author": "human:ana"}),
        );
        assert_eq!(
            described(&pull),
            pair("pull · 9f8e7d6c", &format!("@ana · {TIME}"))
        );
        assert_eq!(described(&revision("r15", "pull", json!({}))).0, "pull · ");

        let direct = revision(
            "r14",
            "direct",
            json!({"title": "Fix the build", "author": "main"}),
        );
        assert_eq!(
            described(&direct),
            pair(
                "direct · Fix the build",
                &format!("@main · approved by the user · {TIME}")
            )
        );
        assert_eq!(
            described(&revision("r13", "direct", json!({}))).0,
            "direct · r13"
        );

        let record = revision("r12", "record", json!({"author": "human:ana"}));
        assert_eq!(
            described(&record),
            pair(
                "record · changes of the project folder",
                &format!("@ana · {TIME}")
            )
        );
        assert_eq!(
            described(&revision("r11", "record", json!({"title": "Tidy"}))).0,
            "record · Tidy"
        );

        assert_eq!(
            described(&revision("r1", "initial", json!({}))),
            pair("Genesis revision", &format!("unknown · {TIME}"))
        );
        assert_eq!(
            described(&revision(
                "r1",
                "initial",
                json!({"title": "Initial commit"})
            ))
            .0,
            "Initial commit"
        );
    }

    #[test]
    fn agent_text_is_printed_with_its_control_characters_escaped() {
        let sneaky = revision(
            "r5",
            "proposal",
            json!({"proposal": "P-1", "title": "Fix\u{1b}[2J\nr4 forged", "author": "frontend"}),
        );
        assert_eq!(described(&sneaky).0, "P-1 · Fix\\u{1b}[2J\\nr4 forged");
    }

    #[test]
    fn the_head_is_marked_and_each_revision_has_its_meta_line() {
        let answer = page(
            vec![
                revision(
                    "r2",
                    "record",
                    json!({"author": "human:ana", "commit": "abcdef0123"}),
                ),
                revision("r1", "initial", json!({})),
            ],
            None,
        );
        assert_eq!(
            log_lines(&answer, 20, false).unwrap(),
            [
                "r2 (head of main) record · changes of the project folder",
                &format!("  @ana · {TIME} · abcdef0"),
                "r1 Genesis revision",
                &format!("  unknown · {TIME}"),
            ]
        );
        assert_eq!(
            log_lines(&answer, 20, true).unwrap(),
            [
                "r2 (head of main) record · changes of the project folder",
                "r1 Genesis revision",
            ]
        );
    }

    #[test]
    fn a_page_that_stops_before_r1_says_older_revisions_are_not_listed() {
        let answer = page(vec![revision("r9", "initial", json!({}))], Some("r9"));
        assert_eq!(
            log_lines(&answer, 1, true).unwrap(),
            [
                "r9 (head of main) Genesis revision",
                "older revisions are not listed; fld log -n 2 lists more",
            ]
        );
        assert_eq!(
            log_lines(&answer, 600, true).unwrap()[1],
            "older revisions are not listed; fld log -n 1000 lists more"
        );
        assert_eq!(
            log_lines(&answer, 1000, true).unwrap()[1],
            "older revisions are not listed; fld log lists at most 1000"
        );
    }

    #[test]
    fn an_empty_history_says_so_and_a_strange_or_refused_answer_is_an_error() {
        assert_eq!(
            log_lines(&page(vec![], None), 20, false).unwrap(),
            ["main has no revisions yet"]
        );
        assert!(matches!(
            log_lines(
                &page(vec![revision("r1", "merge", json!({}))], None),
                20,
                false
            ),
            Err(RemoteError::Answer)
        ));
        assert!(matches!(
            log_lines(
                &page(vec![json!({"id": "r1", "origin": "initial"})], None),
                20,
                false
            ),
            Err(RemoteError::Answer)
        ));
        let refused = Response {
            status: 404,
            body: json!({"error": {"code": "project_not_found", "message": "no project"}}),
        };
        assert!(matches!(
            log_lines(&refused, 20, false),
            Err(RemoteError::Refused { code, .. }) if code == "project_not_found"
        ));
    }

    #[derive(Parser)]
    struct Log {
        #[command(flatten)]
        args: LogArgs,
    }

    #[test]
    fn the_count_defaults_to_20_and_stays_within_what_the_daemon_lists() {
        let parsed = Log::try_parse_from(["fld"]).unwrap().args;
        assert_eq!((parsed.count, parsed.oneline), (20, false));
        let parsed = Log::try_parse_from(["fld", "-n", "5", "--oneline"])
            .unwrap()
            .args;
        assert_eq!((parsed.count, parsed.oneline), (5, true));
        assert!(Log::try_parse_from(["fld", "-n", "0"]).is_err());
        assert!(Log::try_parse_from(["fld", "-n", "1001"]).is_err());
    }
}
