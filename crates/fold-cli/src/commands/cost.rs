use std::io::Write;
use std::path::PathBuf;
use std::time::SystemTime;

use chrono::{DateTime, Days, NaiveDate, TimeZone};
use clap::{Args, ValueEnum};
use fold_cli::daemon_client::Response;
use fold_core::{TaskId, Timestamp};
use hyper::Method;
use serde_json::Value;

use super::remote::{RemoteError, refusal, request_project};
use crate::output;

const HOUR_MILLIS: i64 = 60 * 60 * 1000;

/// How many tasks the summary lists.
const TOP_TASKS: usize = 10;
/// `fld cost` counts its days in the terminal's time zone, as the web app counts them in the viewer's.
const DAYS_NOTE: &str = "Today and the last 7 days start at midnight in your time zone.";

/// The cost view's note: the main agent's session is outside fold's view, and fold guesses no number for it. A project whose `[main] supervised` setting is on has fold run the main agent as a worker, so its cost is in the numbers and the note is left out.
const MAIN_NOTE: &str = "The main agent runs in your own Claude Code session, which fold does not supervise, so its cost is not in these numbers.";

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Split {
    Role,
    Area,
    Model,
    Worker,
    Task,
}

#[derive(Args, Debug)]
pub struct CostArgs {
    /// Count only the turns since a time: today, 12h, 7d, a local date such as 2026-10-01, or an RFC 3339 time; days start at local midnight
    #[arg(long, value_name = "WHEN")]
    since: Option<String>,
    /// Print one split in full instead of the summary: role, area, model, worker or task
    #[arg(long, value_enum, value_name = "SPLIT")]
    by: Option<Split>,
    /// Print what one task cost, per worker that built or reviewed it, such as T-12
    #[arg(long, value_name = "TASK", conflicts_with_all = ["since", "by"])]
    task: Option<String>,
    /// Print the daemon's answer as JSON
    #[arg(long)]
    json: bool,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

fn now() -> Timestamp {
    Timestamp::from_system_time(SystemTime::now()).unwrap_or(Timestamp::EPOCH)
}

/// The date that holds `at` in `zone`.
fn local_date<Zone: TimeZone>(at: Timestamp, zone: &Zone) -> Option<NaiveDate> {
    DateTime::from_timestamp_millis(at.as_millis())
        .map(|time| time.with_timezone(zone).date_naive())
}

/// The first instant of `date` in `zone`: its midnight, the earlier one when a clock change repeats it, or the first minute after the gap when a clock change skips it.
fn day_start<Zone: TimeZone>(date: NaiveDate, zone: &Zone) -> Option<Timestamp> {
    (0..24 * 60).find_map(|minute| {
        let local = date.and_hms_opt(minute / 60, minute % 60, 0)?;
        let start = zone.from_local_datetime(&local).earliest()?;
        Timestamp::from_millis(start.timestamp_millis()).ok()
    })
}

/// The first instant in `zone` of the day `days` days before the one that holds `now`.
fn days_ago<Zone: TimeZone>(now: Timestamp, days: u64, zone: &Zone) -> Option<Timestamp> {
    let date = local_date(now, zone)?.checked_sub_days(Days::new(days))?;
    day_start(date, zone)
}

/// The start of `--since` in `zone`: `today` is local midnight, `Nd` local midnight of the day N-1 days ago so `7d` is the last 7 days as the web app counts them, `Nh` N hours ago, a date its local midnight, and an RFC 3339 time itself.
fn since<Zone: TimeZone>(
    text: &str,
    now: Timestamp,
    zone: &Zone,
) -> Result<Timestamp, RemoteError> {
    let invalid = || {
        RemoteError::Fold(
            fold_core::FoldError::new(
                fold_core::codes::INVALID_REQUEST,
                format!(
                    "--since {} is not a time fld reads",
                    output::escape_control(text)
                ),
            )
            .with_hint("pass today, 12h, 7d, a date such as 2026-10-01 or an RFC 3339 time"),
        )
    };
    if text == "today" {
        return days_ago(now, 0, zone).ok_or_else(invalid);
    }
    let count = |suffix: char| {
        text.strip_suffix(suffix)
            .and_then(|digits| digits.parse::<u32>().ok())
            .filter(|count| (1..=100_000).contains(count))
    };
    if let Some(days) = count('d') {
        return days_ago(now, u64::from(days - 1), zone).ok_or_else(invalid);
    }
    if let Some(hours) = count('h') {
        return Timestamp::from_millis(
            now.as_millis()
                .saturating_sub(i64::from(hours) * HOUR_MILLIS),
        )
        .map_err(|_| invalid());
    }
    if text.len() == 10 {
        return NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .ok()
            .and_then(|date| day_start(date, zone))
            .ok_or_else(invalid);
    }
    Timestamp::parse_rfc3339(text).map_err(|_| invalid())
}

/// Prints the project's cost, or one task's, as `fld cost` describes it, starting the daemon when none runs.
pub fn run_cost(args: &CostArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let ask = |action: &str| {
        request_project(
            args.socket.as_deref(),
            args.project.as_deref(),
            Method::GET,
            action,
        )
    };
    let lines = if let Some(task) = &args.task {
        let task: TaskId = task.parse().map_err(fold_core::FoldError::from)?;
        let response = ask(&format!("cost/tasks/{task}"))?;
        if args.json {
            json_lines(&response)?
        } else {
            task_lines(&response, main_supervised(&ask("workers")?)?)?
        }
    } else {
        let now = now();
        match &args.since {
            Some(text) => {
                let start = since(text, now, &chrono::Local)?;
                let response = ask(&format!("cost?since={}", start.as_millis()))?;
                if args.json {
                    json_lines(&response)?
                } else {
                    range_lines(
                        &response,
                        start,
                        args.by,
                        main_supervised(&ask("workers")?)?,
                        &output::local_time,
                    )?
                }
            }
            None => {
                let all = ask("cost")?;
                if args.json {
                    json_lines(&all)?
                } else {
                    let range = |days| {
                        let start = days_ago(now, days, &chrono::Local).unwrap_or(now);
                        ask(&format!("cost?since={}", start.as_millis()))
                    };
                    let day = range(0)?;
                    let week = range(6)?;
                    summary_lines(
                        &all,
                        &day,
                        &week,
                        args.by,
                        main_supervised(&ask("workers")?)?,
                        &output::local_time,
                    )?
                }
            }
        }
    };
    for line in lines {
        output::line(out, &line)?;
    }
    Ok(())
}

fn json_lines(response: &Response) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    serde_json::to_string_pretty(&response.body)
        .map(|text| vec![text])
        .map_err(|_| RemoteError::Answer)
}

/// `$5.87`; the daemon counts millionths of a US dollar.
fn dollars(micro: u64) -> String {
    let cents = micro.saturating_add(5_000) / 10_000;
    format!("${}.{:02}", cents / 100, cents % 100)
}

/// `980`, `64.5k` or `1.2M` tokens.
fn tokens_text(count: u64) -> String {
    let tenths = |unit: u64| (u128::from(count) * 10 + u128::from(unit) / 2) / u128::from(unit);
    if count < 1_000 {
        return count.to_string();
    }
    let thousands = tenths(1_000);
    if thousands < 10_000 {
        return format!("{}.{}k", thousands / 10, thousands % 10);
    }
    let millions = tenths(1_000_000);
    format!("{}.{}M", millions / 10, millions % 10)
}

/// The total of the `tokens` of `value`.
fn token_total(value: &Value) -> Result<u64, RemoteError> {
    number(&value["tokens"], "total")
}

/// The line of the range's tokens: all of them, then the cache reads and writes, the output and the input the cache did not hold.
fn tokens_line(report: &Value) -> Result<String, RemoteError> {
    let tokens = &report["total"]["tokens"];
    Ok(format!(
        "TOKENS  {} · {} cache read · {} cache write · {} output · {} input",
        tokens_text(number(tokens, "total")?),
        tokens_text(number(tokens, "cache_read")?),
        tokens_text(number(tokens, "cache_write")?),
        tokens_text(number(tokens, "output")?),
        tokens_text(number(tokens, "input")?)
    ))
}

fn number(value: &Value, key: &str) -> Result<u64, RemoteError> {
    value[key].as_u64().ok_or(RemoteError::Answer)
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value[key].as_str()
}

fn list<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, RemoteError> {
    value[key].as_array().ok_or(RemoteError::Answer)
}

fn share(part: u64, total: u64) -> String {
    if total == 0 {
        return "0%".to_owned();
    }
    let percent = (u128::from(part) * 100 + u128::from(total) / 2) / u128::from(total);
    format!("{percent}%")
}

fn role_name(role: &str) -> &str {
    match role {
        "area_lead" => "Area leads",
        "review_lead" => "Review lead",
        "task_worker" => "Task workers",
        "reviewer" => "Reviewers",
        other => other,
    }
}

fn one_role(role: &str) -> &str {
    match role {
        "area_lead" => "area lead",
        "review_lead" => "review lead",
        "task_worker" => "task worker",
        other => other,
    }
}

/// Rows of cells, each column padded to its widest cell; the cells of `right` columns are aligned to the right.
fn table(rows: &[Vec<String>], right: &[usize]) -> Vec<String> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    rows.iter()
        .map(|row| {
            let cells: Vec<String> = row
                .iter()
                .enumerate()
                .map(|(column, cell)| {
                    let width = widths[column];
                    if right.contains(&column) {
                        format!("{cell:>width$}")
                    } else {
                        format!("{cell:<width$}")
                    }
                })
                .collect();
            format!("  {}", cells.join("   ").trim_end())
        })
        .collect()
}

/// A table whose first row is its heading, which starts at the margin as the section headings do; its first cell keeps the column's width.
fn headed_table(rows: &[Vec<String>], right: &[usize]) -> Vec<String> {
    let mut lines = table(rows, right);
    let width = rows
        .iter()
        .filter_map(|row| row.first())
        .map(|cell| cell.chars().count())
        .max()
        .unwrap_or(0);
    if let Some(heading) = lines.first_mut()
        && let (Some(first), Some(rest)) = (heading.get(2..2 + width), heading.get(2 + width..))
    {
        *heading = format!("{first}  {rest}").trim_end().to_owned();
    }
    lines
}

fn workers_text(count: u64) -> String {
    output::count(
        usize::try_from(count).unwrap_or(usize::MAX),
        "worker",
        "workers",
    )
}

fn turns_text(count: u64) -> String {
    output::count(
        usize::try_from(count).unwrap_or(usize::MAX),
        "turn",
        "turns",
    )
}

/// The line under the project line: when the first turn in the range ended, as `time` reads it, how many workers ran turns and how many turns there were.
fn since_line(report: &Value, time: &dyn Fn(Timestamp) -> String) -> Result<String, RemoteError> {
    let total = &report["total"];
    let counts = format!(
        "{} · {}",
        workers_text(number(total, "workers")?),
        turns_text(number(total, "turns")?)
    );
    Ok(match report["first_turn_at"].as_i64() {
        Some(first) => {
            let first = Timestamp::from_millis(first).map_err(|_| RemoteError::Answer)?;
            format!("since {} · {counts}", time(first))
        }
        None => counts,
    })
}

fn by_role(report: &Value) -> Result<Vec<String>, RemoteError> {
    let total = number(&report["total"], "cost_micro_usd")?;
    let mut rows = Vec::new();
    for entry in list(report, "by_role")? {
        let cost = number(entry, "cost_micro_usd")?;
        rows.push(vec![
            role_name(text(entry, "role").ok_or(RemoteError::Answer)?).to_owned(),
            dollars(cost),
            share(cost, total),
            format!("{} tokens", tokens_text(token_total(entry)?)),
            workers_text(number(entry, "workers")?),
        ]);
    }
    let mut lines = vec!["BY ROLE".to_owned()];
    lines.extend(table(&rows, &[1, 2, 3]));
    Ok(lines)
}

fn by_area(report: &Value) -> Result<Vec<String>, RemoteError> {
    let total = number(&report["total"], "cost_micro_usd")?;
    let mut rows = Vec::new();
    for entry in list(report, "by_area")? {
        let cost = number(entry, "cost_micro_usd")?;
        rows.push(vec![
            text(entry, "area")
                .map_or_else(|| "Reviews (all areas)".to_owned(), output::escape_control),
            dollars(cost),
            share(cost, total),
        ]);
    }
    let mut lines = vec!["BY AREA".to_owned()];
    if rows.is_empty() {
        lines.push("  no turns".to_owned());
    }
    lines.extend(table(&rows, &[1, 2]));
    Ok(lines)
}

fn by_model(report: &Value) -> Result<Vec<String>, RemoteError> {
    let total = number(&report["total"], "cost_micro_usd")?;
    let mut rows = Vec::new();
    for entry in list(report, "by_model")? {
        let cost = number(entry, "cost_micro_usd")?;
        rows.push(vec![
            text(entry, "model").map_or_else(|| "unknown".to_owned(), output::escape_control),
            turns_text(number(entry, "turns")?),
            dollars(cost),
            share(cost, total),
        ]);
    }
    let mut lines = vec!["BY MODEL".to_owned()];
    if rows.is_empty() {
        lines.push("  no turns".to_owned());
    }
    lines.extend(table(&rows, &[2, 3]));
    Ok(lines)
}

fn by_worker(report: &Value) -> Result<Vec<String>, RemoteError> {
    let mut rows = vec![
        ["WORKER", "ROLE", "AREA", "MODEL", "TURNS", "COST"]
            .map(str::to_owned)
            .to_vec(),
    ];
    for entry in list(report, "workers")? {
        rows.push(vec![
            output::escape_control(text(entry, "name").ok_or(RemoteError::Answer)?),
            one_role(text(entry, "role").ok_or(RemoteError::Answer)?).to_owned(),
            text(entry, "area").map_or_else(|| "-".to_owned(), output::escape_control),
            text(entry, "model").map_or_else(|| "-".to_owned(), output::escape_control),
            number(entry, "turns")?.to_string(),
            dollars(number(entry, "cost_micro_usd")?),
        ]);
    }
    let mut lines = headed_table(&rows, &[4, 5]);
    if rows.len() == 1 {
        lines.push("  no turns".to_owned());
    }
    lines.push("Stopped, restarted and failed workers keep their turns.".to_owned());
    Ok(lines)
}

/// The tasks table, the most expensive first; `limit` leaves out the rest with a line that says how to list them.
fn by_task(report: &Value, limit: Option<usize>) -> Result<Vec<String>, RemoteError> {
    let tasks = list(report, "tasks")?;
    let heading = if limit.is_some() {
        "TOP TASKS"
    } else {
        "TASKS"
    };
    let mut rows = vec![
        [heading, "BUILD", "REVIEW", "TOTAL", "TOKENS"]
            .map(str::to_owned)
            .to_vec(),
    ];
    for entry in tasks.iter().take(limit.unwrap_or(usize::MAX)) {
        let review = number(entry, "review_micro_usd")?;
        rows.push(vec![
            format!(
                "{} {}",
                text(entry, "task").ok_or(RemoteError::Answer)?,
                output::escape_control(text(entry, "title").unwrap_or_default())
            ),
            dollars(number(entry, "build_micro_usd")?),
            if review == 0 {
                "-".to_owned()
            } else {
                dollars(review)
            },
            dollars(number(entry, "total_micro_usd")?),
            tokens_text(token_total(entry)?),
        ]);
    }
    let mut lines = headed_table(&rows, &[1, 2, 3, 4]);
    if tasks.is_empty() {
        lines.push("  no task has a turn yet".to_owned());
    }
    if let Some(limit) = limit
        && tasks.len() > limit
    {
        lines.push(format!(
            "  {} more; fld cost --by task lists every task",
            tasks.len() - limit
        ));
    }
    Ok(lines)
}

/// The sections after the project lines: the summary of role, area and top tasks, or the one split of `--by`.
fn sections(
    report: &Value,
    by: Option<Split>,
    main_supervised: bool,
) -> Result<Vec<String>, RemoteError> {
    let mut lines = Vec::new();
    let mut section = |part: Vec<String>| {
        lines.push(String::new());
        lines.extend(part);
    };
    match by {
        None => {
            section(by_role(report)?);
            section(by_area(report)?);
            section(by_task(report, Some(TOP_TASKS))?);
        }
        Some(Split::Role) => section(by_role(report)?),
        Some(Split::Area) => section(by_area(report)?),
        Some(Split::Model) => section(by_model(report)?),
        Some(Split::Worker) => section(by_worker(report)?),
        Some(Split::Task) => section(by_task(report, None)?),
    }
    main_note(&mut lines, main_supervised);
    Ok(lines)
}

/// Ends `lines` with the main agent's note when the main agent is the user's own session.
fn main_note(lines: &mut Vec<String>, main_supervised: bool) {
    if !main_supervised {
        lines.push(String::new());
        lines.push(MAIN_NOTE.to_owned());
    }
}

/// Whether the project's workers list says that fold runs the main agent as a supervised worker.
fn main_supervised(workers: &Response) -> Result<bool, RemoteError> {
    refusal(workers)?;
    workers.body["main_supervised"]
        .as_bool()
        .ok_or(RemoteError::Answer)
}

/// What `fld cost` prints without `--since`: the project's total of all time, today and the last 7 days, then the splits of all time.
fn summary_lines(
    all: &Response,
    today: &Response,
    week: &Response,
    by: Option<Split>,
    main_supervised: bool,
    time: &dyn Fn(Timestamp) -> String,
) -> Result<Vec<String>, RemoteError> {
    for response in [all, today, week] {
        refusal(response)?;
    }
    let cost = |response: &Response| number(&response.body["total"], "cost_micro_usd");
    let mut lines = vec![
        format!(
            "COST  {} all time · {} today · {} last 7 days",
            dollars(cost(all)?),
            dollars(cost(today)?),
            dollars(cost(week)?)
        ),
        since_line(&all.body, time)?,
        tokens_line(&all.body)?,
    ];
    lines.extend(sections(&all.body, by, main_supervised)?);
    lines.push(DAYS_NOTE.to_owned());
    Ok(lines)
}

/// What `fld cost --since` prints: the total since `start`, then the splits of that range; `time` writes the times.
fn range_lines(
    response: &Response,
    start: Timestamp,
    by: Option<Split>,
    main_supervised: bool,
    time: &dyn Fn(Timestamp) -> String,
) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    let report = &response.body;
    let mut lines = vec![
        format!(
            "COST  {} since {}",
            dollars(number(&report["total"], "cost_micro_usd")?),
            time(start)
        ),
        since_line(report, time)?,
        tokens_line(report)?,
    ];
    lines.extend(sections(report, by, main_supervised)?);
    Ok(lines)
}

/// What `fld cost --task` prints: the task's total with its build and review parts, then one line per worker.
fn task_lines(response: &Response, main_supervised: bool) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    let report = &response.body;
    let mut lines = vec![format!(
        "{}  {}  build {} · review {} · {}",
        text(report, "task").ok_or(RemoteError::Answer)?,
        dollars(number(report, "total_micro_usd")?),
        dollars(number(report, "build_micro_usd")?),
        dollars(number(report, "review_micro_usd")?),
        turns_text(number(report, "turns")?)
    )];
    let mut rows = Vec::new();
    for entry in list(report, "workers")? {
        let did = match text(entry, "part") {
            Some("review") => {
                let proposals: Vec<&str> = list(entry, "proposals")?
                    .iter()
                    .filter_map(Value::as_str)
                    .collect();
                if proposals.is_empty() {
                    "review".to_owned()
                } else {
                    format!("review of {}", proposals.join(", "))
                }
            }
            Some("build") => "build".to_owned(),
            _ => return Err(RemoteError::Answer),
        };
        rows.push(vec![
            output::escape_control(text(entry, "name").ok_or(RemoteError::Answer)?),
            format!(
                "{} · {} · {} · {did}",
                one_role(text(entry, "role").ok_or(RemoteError::Answer)?),
                text(entry, "model")
                    .map_or_else(|| "unknown model".to_owned(), output::escape_control),
                turns_text(number(entry, "turns")?)
            ),
            dollars(number(entry, "cost_micro_usd")?),
        ]);
    }
    if rows.is_empty() {
        lines.push("  no worker has worked on it yet".to_owned());
    }
    lines.extend(table(&rows, &[2]));
    main_note(&mut lines, main_supervised);
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use chrono::{FixedOffset, LocalResult, NaiveDateTime, Utc};
    use clap::Parser;
    use serde_json::json;

    use super::*;

    const NOW: &str = "2026-10-21T15:30:00.000Z";

    fn utc(at: Timestamp) -> String {
        output::readable_time(at, &chrono::Utc)
    }

    fn at(text: &str) -> Timestamp {
        Timestamp::parse_rfc3339(text).unwrap()
    }

    fn ok(body: Value) -> Response {
        Response { status: 200, body }
    }

    fn tokens(turns: u64) -> Value {
        json!({"input": turns, "output": 10 * turns, "cache_read": 1_000 * turns, "cache_write": 100 * turns, "total": 1_111 * turns})
    }

    fn amount(cost: u64, turns: u64, workers: u64) -> Value {
        json!({"cost_micro_usd": cost, "tokens": tokens(turns), "turns": turns, "workers": workers})
    }

    fn with(mut base: Value, key: &str, value: Value) -> Value {
        base[key] = value;
        base
    }

    fn report(total: u64) -> Value {
        json!({
            "since": null,
            "total": amount(total, 12, 4),
            "first_turn_at": at("2026-10-14T08:00:00Z").as_millis(),
            "by_role": [
                with(with(amount(4_000_000, 3, 1), "role", json!("area_lead")), "x", json!(0)),
                with(amount(0, 0, 0), "role", json!("review_lead")),
                with(amount(5_000_000, 6, 2), "role", json!("task_worker")),
                with(amount(1_000_000, 3, 1), "role", json!("reviewer")),
            ],
            "by_area": [
                with(amount(9_000_000, 9, 3), "area", json!("web")),
                with(amount(1_000_000, 3, 1), "area", Value::Null),
            ],
            "by_model": [
                with(amount(10_000_000, 12, 4), "model", json!("claude-sonnet-5")),
            ],
            "workers": [
                {"name": "web-lead", "role": "area_lead", "area": "web", "state": "idle", "model": "claude-sonnet-5", "turns": 3, "cost_micro_usd": 4_000_000},
                {"name": "reviewer-1", "role": "reviewer", "area": null, "state": "stopped", "model": null, "turns": 3, "cost_micro_usd": 1_000_000},
            ],
            "tasks": [
                {"task": "T-12", "title": "Add the login form", "area": "web", "build_micro_usd": 3_000_000, "review_micro_usd": 1_000_000, "total_micro_usd": 4_000_000, "tokens": {"input": 1, "output": 2, "cache_read": 60_000, "cache_write": 3_997, "total": 64_000}, "turns": 6},
                {"task": "T-3", "title": "Fix\u{1b}[2J", "area": "web", "build_micro_usd": 1_994_999, "review_micro_usd": 0, "total_micro_usd": 1_994_999, "tokens": tokens(1), "turns": 3},
            ],
            "slots": [],
            "slot_minutes": 15,
        })
    }

    #[test]
    fn dollars_round_to_cents_and_shares_to_whole_percents() {
        assert_eq!(dollars(0), "$0.00");
        assert_eq!(dollars(4_999), "$0.00");
        assert_eq!(dollars(5_000), "$0.01");
        assert_eq!(dollars(5_870_000), "$5.87");
        assert_eq!(dollars(1_995_000), "$2.00");
        assert_eq!(dollars(12_345_678_901), "$12345.68");
        assert_eq!(share(1, 3), "33%");
        assert_eq!(share(2, 3), "67%");
        assert_eq!(share(5, 0), "0%");
    }

    #[test]
    fn tokens_read_as_thousands_and_millions() {
        assert_eq!(tokens_text(0), "0");
        assert_eq!(tokens_text(999), "999");
        assert_eq!(tokens_text(1_000), "1.0k");
        assert_eq!(tokens_text(64_546), "64.5k");
        assert_eq!(tokens_text(999_949), "999.9k");
        assert_eq!(tokens_text(999_950), "1.0M");
        assert_eq!(tokens_text(12_345_678), "12.3M");
    }

    fn east(hours: i32) -> FixedOffset {
        FixedOffset::east_opt(hours * 3600).unwrap()
    }

    #[test]
    fn days_start_at_local_midnight_and_hours_and_times_are_exact() {
        let now = at(NOW);
        let dhaka = east(6);
        assert_eq!(
            since("today", now, &dhaka).unwrap(),
            at("2026-10-20T18:00:00Z")
        );
        assert_eq!(
            since("1d", now, &dhaka).unwrap(),
            at("2026-10-20T18:00:00Z")
        );
        assert_eq!(
            since("7d", now, &dhaka).unwrap(),
            at("2026-10-14T18:00:00Z")
        );
        assert_eq!(
            since("2026-10-01", now, &dhaka).unwrap(),
            at("2026-09-30T18:00:00Z")
        );
        let west = east(-9);
        assert_eq!(
            since("today", now, &west).unwrap(),
            at("2026-10-21T09:00:00Z")
        );
        assert_eq!(since("7d", now, &west).unwrap(), at("2026-10-15T09:00:00Z"));
        assert_eq!(
            since("2026-10-01", now, &west).unwrap(),
            at("2026-10-01T09:00:00Z")
        );
        assert_eq!(
            since("today", now, &Utc).unwrap(),
            at("2026-10-21T00:00:00Z")
        );
        for zone in [dhaka, west, east(0)] {
            assert_eq!(
                since("12h", now, &zone).unwrap(),
                at("2026-10-21T03:30:00Z")
            );
            assert_eq!(
                since("2026-10-20T10:00:00+06:00", now, &zone).unwrap(),
                at("2026-10-20T04:00:00Z")
            );
        }
        for wrong in ["0d", "yesterday", "-3h", "2026-13-01", "7 d", ""] {
            let Err(RemoteError::Fold(error)) = since(wrong, now, &dhaka) else {
                panic!("{wrong} is not a time");
            };
            assert_eq!(error.code(), "invalid_request");
        }
    }

    #[test]
    fn the_local_day_is_the_one_the_terminal_is_in_across_the_utc_day_boundary() {
        let late = at("2026-10-21T20:00:00Z");
        assert_eq!(
            since("today", late, &east(6)).unwrap(),
            at("2026-10-21T18:00:00Z")
        );
        assert_eq!(
            days_ago(late, 6, &east(6)).unwrap(),
            at("2026-10-15T18:00:00Z")
        );
        let early = at("2026-10-21T03:00:00Z");
        assert_eq!(
            since("today", early, &east(-5)).unwrap(),
            at("2026-10-20T05:00:00Z")
        );
        assert_eq!(
            days_ago(early, 6, &east(-5)).unwrap(),
            at("2026-10-14T05:00:00Z")
        );
    }

    /// A zone whose clock moves from `before` to `after` at the instant `change`.
    #[derive(Clone, Copy)]
    struct Shifting {
        change: Timestamp,
        before: FixedOffset,
        after: FixedOffset,
    }

    impl TimeZone for Shifting {
        type Offset = FixedOffset;

        fn from_offset(offset: &FixedOffset) -> Self {
            Self {
                change: Timestamp::EPOCH,
                before: *offset,
                after: *offset,
            }
        }

        fn offset_from_local_date(&self, local: &NaiveDate) -> LocalResult<FixedOffset> {
            self.offset_from_local_datetime(&local.and_hms_opt(0, 0, 0).unwrap())
        }

        fn offset_from_local_datetime(&self, local: &NaiveDateTime) -> LocalResult<FixedOffset> {
            let instant = |offset: FixedOffset| (*local - offset).and_utc().timestamp_millis();
            let change = self.change.as_millis();
            match (instant(self.before) < change, instant(self.after) >= change) {
                (true, true) => LocalResult::Ambiguous(self.before, self.after),
                (true, false) => LocalResult::Single(self.before),
                (false, true) => LocalResult::Single(self.after),
                (false, false) => LocalResult::None,
            }
        }

        fn offset_from_utc_date(&self, utc: &NaiveDate) -> FixedOffset {
            self.offset_from_utc_datetime(&utc.and_hms_opt(0, 0, 0).unwrap())
        }

        fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> FixedOffset {
            if utc.and_utc().timestamp_millis() < self.change.as_millis() {
                self.before
            } else {
                self.after
            }
        }
    }

    #[test]
    fn a_clock_change_at_midnight_starts_the_day_at_its_first_instant() {
        let skipped = Shifting {
            change: at("2026-09-06T04:00:00Z"),
            before: east(-4),
            after: east(-3),
        };
        let date = NaiveDate::from_ymd_opt(2026, 9, 6).unwrap();
        assert_eq!(
            day_start(date, &skipped).unwrap(),
            at("2026-09-06T04:00:00Z")
        );
        let noon = at("2026-09-06T15:00:00Z");
        assert_eq!(
            since("today", noon, &skipped).unwrap(),
            at("2026-09-06T04:00:00Z")
        );
        assert_eq!(
            since("2026-09-06", noon, &skipped).unwrap(),
            at("2026-09-06T04:00:00Z")
        );
        assert_eq!(
            since("2d", noon, &skipped).unwrap(),
            at("2026-09-05T04:00:00Z")
        );

        let half_past = Shifting {
            change: at("2026-09-06T03:30:00Z"),
            before: east(-4),
            after: east(-3),
        };
        assert_eq!(
            day_start(date, &half_past).unwrap(),
            at("2026-09-06T03:30:00Z")
        );

        let repeated = Shifting {
            change: at("2026-04-05T04:00:00Z"),
            before: east(-3),
            after: east(-4),
        };
        let date = NaiveDate::from_ymd_opt(2026, 4, 5).unwrap();
        assert_eq!(
            day_start(date, &repeated).unwrap(),
            at("2026-04-05T03:00:00Z")
        );
        let noon = at("2026-04-05T16:00:00Z");
        assert_eq!(
            since("today", noon, &repeated).unwrap(),
            at("2026-04-05T03:00:00Z")
        );
        assert_eq!(
            since("2d", noon, &repeated).unwrap(),
            at("2026-04-04T03:00:00Z")
        );
        assert_eq!(
            since("2026-04-06", noon, &repeated).unwrap(),
            at("2026-04-06T04:00:00Z")
        );
    }

    #[test]
    fn the_summary_names_the_three_totals_the_splits_the_top_tasks_and_the_main_agent() {
        let lines = summary_lines(
            &ok(report(10_000_000)),
            &ok(with(report(0), "total", amount(1_200_000, 1, 1))),
            &ok(with(report(0), "total", amount(5_670_000, 4, 2))),
            None,
            false,
            &utc,
        )
        .unwrap();
        assert_eq!(
            lines,
            [
                "COST  $10.00 all time · $1.20 today · $5.67 last 7 days",
                "since 14 Oct 2026, 08:00 · 4 workers · 12 turns",
                "TOKENS  13.3k · 12.0k cache read · 1.2k cache write · 120 output · 12 input",
                "",
                "BY ROLE",
                "  Area leads     $4.00   40%   3.3k tokens   1 worker",
                "  Review lead    $0.00    0%      0 tokens   0 workers",
                "  Task workers   $5.00   50%   6.7k tokens   2 workers",
                "  Reviewers      $1.00   10%   3.3k tokens   1 worker",
                "",
                "BY AREA",
                "  web                   $9.00   90%",
                "  Reviews (all areas)   $1.00   10%",
                "",
                "TOP TASKS                   BUILD   REVIEW   TOTAL   TOKENS",
                "  T-12 Add the login form   $3.00    $1.00   $4.00    64.0k",
                "  T-3 Fix\\u{1b}[2J          $1.99        -   $1.99     1.1k",
                "",
                MAIN_NOTE,
                DAYS_NOTE,
            ]
        );
    }

    #[test]
    fn a_range_prints_its_own_total_and_one_split_in_full() {
        let start = at("2026-10-20T00:00:00Z");
        let lines = range_lines(
            &ok(report(10_000_000)),
            start,
            Some(Split::Worker),
            false,
            &utc,
        )
        .unwrap();
        assert_eq!(
            lines,
            [
                "COST  $10.00 since 20 Oct 2026, 00:00",
                "since 14 Oct 2026, 08:00 · 4 workers · 12 turns",
                "TOKENS  13.3k · 12.0k cache read · 1.2k cache write · 120 output · 12 input",
                "",
                "WORKER         ROLE        AREA   MODEL             TURNS    COST",
                "  web-lead     area lead   web    claude-sonnet-5       3   $4.00",
                "  reviewer-1   reviewer    -      -                     3   $1.00",
                "Stopped, restarted and failed workers keep their turns.",
                "",
                MAIN_NOTE,
            ]
        );
        let model = range_lines(
            &ok(report(10_000_000)),
            start,
            Some(Split::Model),
            false,
            &utc,
        )
        .unwrap();
        assert_eq!(model[5], "  claude-sonnet-5   12 turns   $10.00   100%");
        let area = range_lines(
            &ok(report(10_000_000)),
            start,
            Some(Split::Area),
            false,
            &utc,
        )
        .unwrap();
        assert_eq!(area[4], "BY AREA");
        let role = range_lines(
            &ok(report(10_000_000)),
            start,
            Some(Split::Role),
            false,
            &utc,
        )
        .unwrap();
        assert_eq!(role[4], "BY ROLE");
        let tasks = range_lines(
            &ok(report(10_000_000)),
            start,
            Some(Split::Task),
            false,
            &utc,
        )
        .unwrap();
        assert!(tasks[4].starts_with("TASKS"));
    }

    #[test]
    fn an_empty_range_says_so_and_a_long_task_list_names_the_rest() {
        let mut empty = report(0);
        empty["first_turn_at"] = Value::Null;
        empty["total"] = amount(0, 0, 0);
        for key in ["by_area", "by_model", "workers", "tasks"] {
            empty[key] = json!([]);
        }
        let start = at(NOW);
        let lines = range_lines(&ok(empty.clone()), start, None, false, &utc).unwrap();
        assert_eq!(lines[1], "0 workers · 0 turns");
        assert!(lines.contains(&"  no turns".to_owned()));
        assert!(lines.contains(&"  no task has a turn yet".to_owned()));
        let workers =
            range_lines(&ok(empty.clone()), start, Some(Split::Worker), false, &utc).unwrap();
        assert!(workers.contains(&"  no turns".to_owned()));
        let models = range_lines(&ok(empty), start, Some(Split::Model), false, &utc).unwrap();
        assert!(models.contains(&"  no turns".to_owned()));

        let task = json!({"task": "T-1", "title": "t", "area": null, "build_micro_usd": 1, "review_micro_usd": 0, "total_micro_usd": 1, "tokens": tokens(1), "turns": 1});
        let many = with(report(1), "tasks", json!(vec![task; TOP_TASKS + 2]));
        let lines = summary_lines(
            &ok(many.clone()),
            &ok(many.clone()),
            &ok(many),
            None,
            false,
            &utc,
        )
        .unwrap();
        assert!(lines.contains(&"  2 more; fld cost --by task lists every task".to_owned()));
    }

    #[test]
    fn a_task_prints_its_parts_and_what_each_worker_did() {
        let body = json!({
            "task": "T-12", "build_micro_usd": 1_200_000, "review_micro_usd": 300_000,
            "total_micro_usd": 1_500_000, "turns": 12,
            "workers": [
                {"name": "web-worker", "role": "task_worker", "part": "build", "model": "claude-sonnet-5", "proposals": [], "turns": 8, "cost_micro_usd": 1_200_000},
                {"name": "reviewer-1", "role": "reviewer", "part": "review", "model": null, "proposals": ["P-3", "P-4"], "turns": 3, "cost_micro_usd": 250_000},
                {"name": "reviewer-2", "role": "reviewer", "part": "review", "model": "opus", "proposals": [], "turns": 1, "cost_micro_usd": 50_000},
            ],
        });
        assert_eq!(
            task_lines(&ok(body.clone()), false).unwrap(),
            [
                "T-12  $1.50  build $1.20 · review $0.30 · 12 turns",
                "  web-worker   task worker · claude-sonnet-5 · 8 turns · build           $1.20",
                "  reviewer-1   reviewer · unknown model · 3 turns · review of P-3, P-4   $0.25",
                "  reviewer-2   reviewer · opus · 1 turn · review                         $0.05",
                "",
                MAIN_NOTE,
            ]
        );
        let none = with(body.clone(), "workers", json!([]));
        assert_eq!(
            task_lines(&ok(none), false).unwrap()[1],
            "  no worker has worked on it yet"
        );
        let strange = with(
            body,
            "workers",
            json!([{"name": "x", "role": "reviewer", "part": "lead", "proposals": [], "turns": 1, "cost_micro_usd": 1}]),
        );
        assert!(matches!(
            task_lines(&ok(strange), false),
            Err(RemoteError::Answer)
        ));
    }

    #[test]
    fn a_refusal_or_a_strange_answer_is_an_error_and_json_is_the_answer() {
        let refused = Response {
            status: 404,
            body: json!({"error": {"code": "task_not_found", "message": "T-9 does not exist"}}),
        };
        assert!(matches!(
            task_lines(&refused, false),
            Err(RemoteError::Refused { code, .. }) if code == "task_not_found"
        ));
        assert!(matches!(
            json_lines(&refused),
            Err(RemoteError::Refused { .. })
        ));
        assert!(matches!(
            summary_lines(
                &ok(json!({})),
                &ok(json!({})),
                &ok(json!({})),
                None,
                false,
                &utc
            ),
            Err(RemoteError::Answer)
        ));
        assert_eq!(
            json_lines(&ok(json!({"total": 1}))).unwrap(),
            ["{\n  \"total\": 1\n}"]
        );
    }

    #[test]
    fn a_supervised_main_is_counted_so_no_note_leaves_it_out() {
        let note = MAIN_NOTE.to_owned();
        let summary = |supervised| {
            summary_lines(
                &ok(report(10_000_000)),
                &ok(report(0)),
                &ok(report(0)),
                None,
                supervised,
                &utc,
            )
            .unwrap()
        };
        assert!(summary(false).contains(&note));
        let counted = summary(true);
        assert!(!counted.contains(&note));
        assert_eq!(counted.last().map(String::as_str), Some(DAYS_NOTE));
        let start = at("2026-10-20T00:00:00Z");
        let range = range_lines(
            &ok(report(10_000_000)),
            start,
            Some(Split::Model),
            true,
            &utc,
        )
        .unwrap();
        assert_eq!(
            range.last().map(String::as_str),
            Some("  claude-sonnet-5   12 turns   $10.00   100%")
        );
        let task = json!({
            "task": "T-12", "build_micro_usd": 1, "review_micro_usd": 0,
            "total_micro_usd": 1, "turns": 1, "workers": [],
        });
        assert_eq!(
            task_lines(&ok(task.clone()), false).unwrap().last(),
            Some(&note)
        );
        assert_eq!(
            task_lines(&ok(task), true).unwrap(),
            [
                "T-12  $0.00  build $0.00 · review $0.00 · 1 turn",
                "  no worker has worked on it yet"
            ]
        );

        assert!(main_supervised(&ok(json!({"main_supervised": true}))).unwrap());
        assert!(!main_supervised(&ok(json!({"main_supervised": false}))).unwrap());
        assert!(matches!(
            main_supervised(&ok(json!({}))),
            Err(RemoteError::Answer)
        ));
        let refused = Response {
            status: 404,
            body: json!({"error": {"code": "project_not_found", "message": "no project"}}),
        };
        assert!(matches!(
            main_supervised(&refused),
            Err(RemoteError::Refused { .. })
        ));
    }

    #[derive(Parser)]
    struct Cost {
        #[command(flatten)]
        args: CostArgs,
    }

    #[test]
    fn the_task_option_stands_alone() {
        let parsed = Cost::try_parse_from(["fld", "--by", "worker", "--since", "7d"])
            .unwrap()
            .args;
        assert_eq!(parsed.by, Some(Split::Worker));
        assert_eq!(parsed.since.as_deref(), Some("7d"));
        assert!(Cost::try_parse_from(["fld", "--task", "T-1", "--since", "7d"]).is_err());
        assert!(Cost::try_parse_from(["fld", "--by", "label"]).is_err());
        let json = Cost::try_parse_from(["fld", "--task", "T-1", "--json"])
            .unwrap()
            .args;
        assert!(json.json);
    }
}
