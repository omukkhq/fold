use std::io::Write;
use std::path::PathBuf;

use clap::Args;
use fold_cli::daemon_client::Response;
use fold_core::{EventActor, Timestamp};
use hyper::Method;
use serde_json::Value;

use super::remote::{RemoteError, refusal, request_project};
use crate::output::{self, Item};

/// The most revisions the daemon lists in one answer.
const MAX_COUNT: u64 = 1000;

#[derive(Args, Debug)]
pub struct LogArgs {
    /// List the newest N revisions of main; at most 1000
    #[arg(short = 'n', long = "count", value_name = "N", default_value_t = 20, value_parser = clap::value_parser!(u64).range(1..=1000))]
    count: u64,
    /// One line per revision: its id, its title and the head mark
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
    for line in log_lines(&response, args.count, args.oneline, &output::local_time)? {
        output::line(out, &line)?;
    }
    Ok(())
}

/// The lines `fld log` prints for one page of revisions, newest first, or the error the answer carries: one item per revision, its id and title on the head line and its details below, or with `oneline` the head line alone. The page starts at main's head, because it is asked for without `before`. `time` writes a revision's time.
fn log_lines(
    response: &Response,
    count: u64,
    oneline: bool,
    time: &dyn Fn(Timestamp) -> String,
) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    let revisions = response.body["revisions"]
        .as_array()
        .ok_or(RemoteError::Answer)?;
    if revisions.is_empty() {
        return Ok(vec!["main has no revisions yet".to_owned()]);
    }
    let mut items = Vec::new();
    let mut lines = Vec::new();
    for (index, revision) in revisions.iter().enumerate() {
        let described = describe(revision, time)?;
        let head = index == 0;
        if oneline {
            let mark = if head { " (head of main)" } else { "" };
            lines.push(format!("{}  {}{mark}", described.id, described.title));
            continue;
        }
        let mut item = Item::new(format!("{}  {}", described.id, described.title));
        if head {
            item.detail("head of main");
        }
        item.detail(&described.what).detail(&described.when);
        items.push(item);
    }
    let footer = (!response.body["before"].is_null()).then(|| {
        if count < MAX_COUNT {
            format!(
                "older revisions are not listed; fld log -n {} lists more",
                (count * 2).min(MAX_COUNT)
            )
        } else {
            format!("older revisions are not listed; fld log lists at most {MAX_COUNT}")
        }
    });
    if oneline {
        lines.extend(footer);
        return Ok(lines);
    }
    items.extend(footer.map(Item::new));
    Ok(output::items(items))
}

/// What one revision shows: its id, its title, what it is and who made it, and when with its Git commit.
#[derive(Debug, PartialEq, Eq)]
struct Described {
    id: String,
    title: String,
    what: String,
    when: String,
}

/// The title and the details by origin, in the words of the web app: a proposal names itself, its task and its author; a pull, a direct change, a record and the genesis say what they are, and fold's own record says fold made it. Agent and remote text is escaped.
fn describe(
    revision: &Value,
    time: &dyn Fn(Timestamp) -> String,
) -> Result<Described, RemoteError> {
    let text = |key: &str| revision[key].as_str().map(output::escape_control);
    let id = text("id").ok_or(RemoteError::Answer)?;
    let created_at = revision["created_at"]
        .as_i64()
        .and_then(|millis| Timestamp::from_millis(millis).ok())
        .ok_or(RemoteError::Answer)?;
    let by = text("author").map(|author| format!(" by {}", output::person(&author)));
    let by = by.as_deref().unwrap_or_default();
    let short = |commit: &str| commit.get(..7).unwrap_or(commit).to_owned();
    let (title, what) = match revision["origin"].as_str().ok_or(RemoteError::Answer)? {
        "proposal" => {
            let name = text("proposal").unwrap_or_else(|| "a proposal".to_owned());
            let what = match text("task") {
                Some(task) => format!("{name} for {task}{by}"),
                None => format!("{name}{by}"),
            };
            let title = text("title")
                .or_else(|| text("proposal"))
                .unwrap_or_else(|| id.clone());
            (title, what)
        }
        "pull" => (
            match text("pull") {
                Some(commit) => format!("Pull of the remote commit {}", short(&commit)),
                None => "Pull from a Git remote".to_owned(),
            },
            format!("pulled from a Git remote{by}"),
        ),
        "direct" => (
            text("title").unwrap_or_else(|| id.clone()),
            format!("a direct change on main{by}, approved by the user"),
        ),
        "record" if text("author").as_deref() == Some(EventActor::FOLD) => (
            text("title").unwrap_or_else(|| id.clone()),
            "a change of main made by fold".to_owned(),
        ),
        "record" => (
            text("title").unwrap_or_else(|| "Changes of the project folder".to_owned()),
            format!("recorded from the project folder{by}"),
        ),
        "initial" => (
            text("title").unwrap_or_else(|| "Genesis revision".to_owned()),
            format!("the start of main's history{by}"),
        ),
        _ => return Err(RemoteError::Answer),
    };
    let when = match text("commit") {
        Some(commit) => format!("{} · Git {}", time(created_at), short(&commit)),
        None => time(created_at),
    };
    Ok(Described {
        id,
        title,
        what,
        when,
    })
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use serde_json::json;

    use super::*;

    const AT: i64 = 1_792_576_800_000;
    const TIME: &str = "21 Oct 2026, 10:00";

    fn utc(at: Timestamp) -> String {
        output::readable_time(at, &chrono::Utc)
    }

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

    fn lines(response: &Response, count: u64, oneline: bool) -> Vec<String> {
        log_lines(response, count, oneline, &utc).unwrap()
    }

    fn described(revision: &Value) -> (String, String, String) {
        let described = describe(revision, &utc).unwrap();
        (described.title, described.what, described.when)
    }

    fn three(title: &str, what: &str, when: &str) -> (String, String, String) {
        (title.to_owned(), what.to_owned(), when.to_owned())
    }

    #[test]
    fn each_origin_says_what_it_is() {
        let proposal = revision(
            "r17",
            "proposal",
            json!({"proposal": "P-3", "title": "Add the login form", "task": "T-12", "author": "frontend", "commit": "1a2b3c4d5e6f"}),
        );
        assert_eq!(
            described(&proposal),
            three(
                "Add the login form",
                "P-3 for T-12 by @frontend",
                &format!("{TIME} · Git 1a2b3c4")
            )
        );
        assert_eq!(
            described(&revision("r9", "proposal", json!({}))),
            three("r9", "a proposal", TIME)
        );
        assert_eq!(
            described(&revision(
                "r8",
                "proposal",
                json!({"proposal": "P-2", "author": "backend"})
            )),
            three("P-2", "P-2 by @backend", TIME)
        );

        let pull = revision(
            "r16",
            "pull",
            json!({"pull": "9f8e7d6c5b4a", "author": "human:ana"}),
        );
        assert_eq!(
            described(&pull),
            three(
                "Pull of the remote commit 9f8e7d6",
                "pulled from a Git remote by @ana",
                TIME
            )
        );
        assert_eq!(
            described(&revision("r15", "pull", json!({}))).0,
            "Pull from a Git remote"
        );

        let direct = revision(
            "r14",
            "direct",
            json!({"title": "Fix the build", "author": "main"}),
        );
        assert_eq!(
            described(&direct),
            three(
                "Fix the build",
                "a direct change on main by @main, approved by the user",
                TIME
            )
        );
        assert_eq!(described(&revision("r13", "direct", json!({}))).0, "r13");

        let record = revision("r12", "record", json!({"author": "human:ana"}));
        assert_eq!(
            described(&record),
            three(
                "Changes of the project folder",
                "recorded from the project folder by @ana",
                TIME
            )
        );
        assert_eq!(
            described(&revision("r11", "record", json!({"title": "Tidy"}))).0,
            "Tidy"
        );
        assert_eq!(
            described(&revision(
                "r14",
                "record",
                json!({"author": "fold", "title": "Stop tracking fold's skill files"})
            )),
            three(
                "Stop tracking fold's skill files",
                "a change of main made by fold",
                TIME
            )
        );

        assert_eq!(
            described(&revision("r0", "initial", json!({}))),
            three("Genesis revision", "the start of main's history", TIME)
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
        assert_eq!(described(&sneaky).0, "Fix\\u{1b}[2J\\nr4 forged");
        let all = lines(&page(vec![sneaky], None), 20, false);
        assert!(all.iter().all(|line| !line.contains('\u{1b}')));
    }

    #[test]
    fn each_revision_is_an_item_with_the_head_marked_and_a_blank_line_between() {
        let answer = page(
            vec![
                revision(
                    "r99",
                    "proposal",
                    json!({"proposal": "P-95", "title": "Add PIN sign-in", "task": "T-18", "author": "backend-worker-23", "commit": "a1b2c3d4e5"}),
                ),
                revision("r98", "record", json!({"author": "human:ana"})),
            ],
            None,
        );
        assert_eq!(
            lines(&answer, 20, false),
            [
                "r99  Add PIN sign-in",
                "  ↳ head of main",
                "  ↳ P-95 for T-18 by @backend-worker-23",
                &format!("  ↳ {TIME} · Git a1b2c3d"),
                "",
                "r98  Changes of the project folder",
                "  ↳ recorded from the project folder by @ana",
                &format!("  ↳ {TIME}"),
            ]
        );
        assert_eq!(
            lines(&answer, 20, true),
            [
                "r99  Add PIN sign-in (head of main)",
                "r98  Changes of the project folder",
            ]
        );
    }

    #[test]
    fn a_page_that_stops_before_the_genesis_names_how_to_list_more() {
        let answer = page(vec![revision("r9", "initial", json!({}))], Some("r9"));
        assert_eq!(
            lines(&answer, 1, false),
            [
                "r9  Genesis revision",
                "  ↳ head of main",
                "  ↳ the start of main's history",
                &format!("  ↳ {TIME}"),
                "",
                "older revisions are not listed; fld log -n 2 lists more",
            ]
        );
        assert_eq!(
            lines(&answer, 1, true),
            [
                "r9  Genesis revision (head of main)",
                "older revisions are not listed; fld log -n 2 lists more",
            ]
        );
        assert_eq!(
            lines(&answer, 600, true)[1],
            "older revisions are not listed; fld log -n 1000 lists more"
        );
        assert_eq!(
            lines(&answer, 1000, true)[1],
            "older revisions are not listed; fld log lists at most 1000"
        );
    }

    #[test]
    fn an_empty_history_says_so_and_a_strange_or_refused_answer_is_an_error() {
        assert_eq!(
            lines(&page(vec![], None), 20, false),
            ["main has no revisions yet"]
        );
        assert!(matches!(
            log_lines(
                &page(vec![revision("r1", "merge", json!({}))], None),
                20,
                false,
                &utc
            ),
            Err(RemoteError::Answer)
        ));
        assert!(matches!(
            log_lines(
                &page(vec![json!({"id": "r1", "origin": "initial"})], None),
                20,
                false,
                &utc
            ),
            Err(RemoteError::Answer)
        ));
        let refused = Response {
            status: 404,
            body: json!({"error": {"code": "project_not_found", "message": "no project"}}),
        };
        assert!(matches!(
            log_lines(&refused, 20, false, &utc),
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
