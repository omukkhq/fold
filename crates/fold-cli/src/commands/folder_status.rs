use std::io::Write;
use std::path::PathBuf;

use clap::Args;
use fold_cli::daemon_client::Response;
use fold_core::Timestamp;
use hyper::Method;
use serde_json::Value;

use super::remote::{RemoteError, refusal, request_project};
use crate::output::{self, Item};

/// The most tracked files that ignore rules match the command lists; the rest are counted.
const LISTED_PATHS: usize = 10;

/// Claude Code's settings of one person on one machine, which `.fold/exclude` leaves out of every new project and which stays tracked where main holds it already.
const LOCAL_SETTINGS: &str = fold_core::layout::LOCAL_SETTINGS_FILE;

#[derive(Args, Debug)]
pub struct StatusArgs {
    /// Print the daemon's answer as JSON, with times in UTC milliseconds
    #[arg(long)]
    json: bool,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

/// Shows the project folder against main, starting the daemon when none runs. It records nothing and changes nothing.
pub fn run_status(args: &StatusArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let response = request_project(
        args.socket.as_deref(),
        args.project.as_deref(),
        Method::GET,
        "folder",
    )?;
    let lines = if args.json {
        refusal(&response)?;
        vec![serde_json::to_string_pretty(&response.body).map_err(|_| RemoteError::Answer)?]
    } else {
        status_lines(&response, &output::local_time)?
    };
    for line in lines {
        output::line(out, &line)?;
    }
    Ok(())
}

fn text<'a>(body: &'a Value, key: &str) -> Result<&'a str, RemoteError> {
    body[key].as_str().ok_or(RemoteError::Answer)
}

fn list<'a>(body: &'a Value, key: &str) -> Result<&'a Vec<Value>, RemoteError> {
    body[key].as_array().ok_or(RemoteError::Answer)
}

fn paths(body: &Value) -> Result<Vec<String>, RemoteError> {
    list(body, "paths")?
        .iter()
        .map(|path| path.as_str().map(output::escape_control))
        .collect::<Option<_>>()
        .ok_or(RemoteError::Answer)
}

/// The lines `fld status` prints for the daemon's answer, or the error it carries: the folder against main, what `fld record` would record, each blocker with its next step, the stash, then a summary line. A folder with nothing to record or to show says so on one head line, with the Git branch below it while Git sync is on. Tracked files that ignore rules match are counted on the head item, and listed only when one of them has a change, since a project may keep such files tracked on purpose. Paths are escaped, since agents name files too.
fn status_lines(
    response: &Response,
    time: &dyn Fn(Timestamp) -> String,
) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    let body = &response.body;
    let head = text(body, "head")?;
    let folder = text(body, "folder")?;
    let at_main = body["at_main"].as_bool().ok_or(RemoteError::Answer)?;
    let changes = list(body, "changes")?;
    let skipped = list(body, "skipped")?;
    let ignored_tracked = list(body, "ignored_tracked")?
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<&str>>>()
        .ok_or(RemoteError::Answer)?;
    let blockers = list(body, "blockers")?;
    let stash = list(body, "stash")?;
    let synced = body["branch"].as_str().map(|branch| {
        format!(
            "Git: main is synced with the branch {}",
            output::escape_control(branch)
        )
    });
    let ignored_changed = changes
        .iter()
        .filter_map(|change| change["path"].as_str())
        .any(|path| ignored_tracked.contains(&path));
    let ignored_note = (!ignored_tracked.is_empty() && !ignored_changed).then(|| {
        let (files, verb) = if ignored_tracked.len() == 1 {
            ("1 tracked file".to_owned(), "matches")
        } else {
            (format!("{} tracked files", ignored_tracked.len()), "match")
        };
        format!("{files} {verb} ignore rules; `fld status --json` lists them")
    });
    let skills_note = skills_note(&ignored_tracked);
    let settings_note = ignored_tracked.contains(&LOCAL_SETTINGS).then(|| {
        format!("{LOCAL_SETTINGS} holds your own Claude Code settings; `fld untrack {LOCAL_SETTINGS}` stops tracking it and leaves it on disk")
    });
    if at_main
        && changes.is_empty()
        && skipped.is_empty()
        && blockers.is_empty()
        && stash.is_empty()
    {
        let mut clean = Item::new(format!(
            "Nothing to record: the project folder is at main {head}"
        ));
        if let Some(synced) = &synced {
            clean.detail(synced);
        }
        if let Some(note) = &ignored_note {
            clean.detail(note);
        }
        if let Some(note) = &skills_note {
            clean.detail(note);
        }
        if let Some(note) = &settings_note {
            clean.detail(note);
        }
        return Ok(output::items(vec![clean]));
    }

    let mut items = Vec::new();
    let mut place = if at_main {
        Item::new(format!("Project folder at main {head}"))
    } else {
        let mut place = Item::new(format!("Project folder at {folder}, behind main {head}"));
        place.detail(format!(
            "Why: {}",
            match body["behind_because"].as_str() {
                Some(kind) => behind_because(kind)?,
                None => "fold has not brought it to main yet; it does so on its next follow",
            }
        ));
        place
    };
    if let Some(synced) = &synced {
        place.detail(synced);
    }
    if let Some(note) = &ignored_note {
        place.detail(note);
    }
    if let Some(note) = &skills_note {
        place.detail(note);
    }
    if let Some(note) = &settings_note {
        place.detail(note);
    }
    items.push(place);

    let mut held = 0;
    if !changes.is_empty() {
        let mut record = Item::new(format!(
            "To record: {}",
            output::count(changes.len(), "file", "files")
        ));
        for change in changes {
            let path = output::escape_control(text(change, "path")?);
            let entry = text(change, "entry")?;
            let mut line = match text(change, "kind")? {
                "added" => format!("added: {path}"),
                "modified" => format!("modified: {path}"),
                "deleted" => format!("deleted: {path}"),
                "type_changed" => format!(
                    "type changed: {path} (now {})",
                    if entry == "symlink" {
                        "a symlink"
                    } else {
                        "a file"
                    }
                ),
                _ => return Err(RemoteError::Answer),
            };
            if text(change, "kind")? != "type_changed" && entry != "file" {
                line.push_str(&format!(" ({entry})"));
            }
            if change["held"].as_bool().ok_or(RemoteError::Answer)? {
                held += 1;
                line.push_str(", held by main");
            }
            record.detail(line);
        }
        record.detail("Next: `fld record` records them on main; `fld stash` puts them aside");
        items.push(record);
    }

    if !skipped.is_empty() {
        let mut left = Item::new(format!(
            "Left out: {}, as `fld record` leaves them out",
            output::count(skipped.len(), "file", "files")
        ));
        for file in skipped {
            let path = output::escape_control(text(file, "path")?);
            let mut line = format!("{path}: {}", skipped_reason(text(file, "reason")?));
            if file["tracked"].as_bool().ok_or(RemoteError::Answer)? {
                line.push_str(&format!(
                    "; main keeps the version it recorded, and `fld untrack {path}` removes it from main"
                ));
            }
            left.detail(line);
        }
        items.push(left);
    }

    if ignored_changed {
        let mut tracked = Item::new(format!(
            "Tracked although ignored: {}, which `fld record` records because main tracks them",
            output::count(ignored_tracked.len(), "file", "files")
        ));
        for path in ignored_tracked.iter().take(LISTED_PATHS) {
            tracked.detail(output::escape_control(path));
        }
        if ignored_tracked.len() > LISTED_PATHS {
            tracked.detail(format!("and {} more", ignored_tracked.len() - LISTED_PATHS));
        }
        tracked.detail("Next: `fld untrack <path>` stops tracking a file and leaves it on disk");
        items.push(tracked);
    }

    let mut waits = Vec::new();
    for blocker in blockers {
        let kind = text(blocker, "kind")?;
        let paths = paths(blocker)?;
        let counted = output::count(paths.len(), "path", "paths");
        let (head, short) = match kind {
            "head_moved" => (
                "Git branch moved: the branch or HEAD shows a change fold did not make, so fold does not update the folder".to_owned(),
                "Git branch moved",
            ),
            "commits_waiting" => (
                "Git commits wait: the branch holds commits of yours that fold has not recorded yet".to_owned(),
                "Git commits wait",
            ),
            "unstash_open" => (
                format!("Unstash stopped part-way: it writes {counted} once it is finished"),
                "unstash stopped part-way",
            ),
            "unstash_conflicted" => (
                format!("Unstash conflict: conflict markers of `fld unstash` wait in {counted}"),
                "unstash conflict",
            ),
            "human_change" => (
                format!("Waits for the main agent: your change conflicts with main in {counted}"),
                "waits for the main agent",
            ),
            "folder_held" => (
                format!("Held by main: changes of yours on {counted} that main changed too"),
                "",
            ),
            _ => return Err(RemoteError::Answer),
        };
        if !short.is_empty() {
            waits.push(short);
        }
        let mut item = Item::new(head);
        for path in &paths {
            item.detail(path);
        }
        item.detail(format!(
            "Next: {}",
            output::escape_control(text(blocker, "next")?)
        ));
        items.push(item);
    }

    if !stash.is_empty() {
        let mut entries = Item::new(format!(
            "Stash: {}",
            output::count(stash.len(), "entry", "entries")
        ));
        for entry in stash {
            let at = entry["created_at"]
                .as_i64()
                .and_then(|millis| Timestamp::from_millis(millis).ok())
                .ok_or(RemoteError::Answer)?;
            let files = entry["files"]
                .as_u64()
                .and_then(|files| usize::try_from(files).ok())
                .ok_or(RemoteError::Answer)?;
            let state = text(entry, "state")?;
            let (state, next) = match state {
                "stashed" => ("", "`fld unstash` applies it next"),
                "conflicted" => (", its unstash conflicts", ""),
                "unstashing" => (
                    ", its unstash stopped part-way",
                    "`fld unstash` finishes it next",
                ),
                _ => return Err(RemoteError::Answer),
            };
            entries.detail(format!(
                "{}: {}{state}; stashed on {} by {}, {}",
                text(entry, "stash")?,
                output::count(files, "file", "files"),
                text(entry, "base")?,
                output::person(&output::escape_control(text(entry, "actor")?)),
                time(at)
            ));
            if entry["next"].as_bool().ok_or(RemoteError::Answer)? && !next.is_empty() {
                entries.deeper(next);
            }
        }
        items.push(entries);
    }

    let mut summary = vec![if changes.is_empty() {
        "nothing to record".to_owned()
    } else {
        format!(
            "{} to record",
            output::count(changes.len(), "file", "files")
        )
    }];
    if held > 0 {
        summary.push(format!("{held} held by main"));
    }
    summary.extend(waits.iter().map(|wait| (*wait).to_owned()));
    if !stash.is_empty() {
        summary.push(output::count(stash.len(), "stash entry", "stash entries"));
    }
    items.push(Item::new(summary.join(" · ")));
    Ok(output::items(items))
}

/// Why the folder holds an older revision than main, by the kind of the blocker that keeps it there.
fn behind_because(kind: &str) -> Result<&'static str, RemoteError> {
    Ok(match kind {
        "head_moved" => "the Git branch or HEAD shows a change fold did not make",
        "commits_waiting" => "the Git branch holds commits of yours that fold has not recorded yet",
        "unstash_open" => "an unstash stopped part-way and waits to be finished",
        "unstash_conflicted" => "the conflict markers of `fld unstash` wait in the files",
        "human_change" => "your change conflicts with main and waits for the main agent",
        "folder_held" => "paths main changed hold changes of yours that no revision records",
        _ => return Err(RemoteError::Answer),
    })
}

/// Why fold leaves a file out, as words.
fn skipped_reason(reason: &str) -> &str {
    match reason {
        "too_large" => "larger than the limit",
        "special" => "not a file, a directory or a symlink",
        "unreadable" => "not readable",
        other => other,
    }
}

/// The note for tracked files of fold's own skills that the ignore rules leave out: the daemon stops tracking them at its next start of the project, and `fld untrack` with their folders does it at once. It names each folder once.
fn skills_note(ignored_tracked: &[&str]) -> Option<String> {
    let skills = fold_core::layout::SKILLS_DIR;
    let mut folders: Vec<String> = ignored_tracked
        .iter()
        .filter(|path| fold_core::layout::is_fold_skill_file(path.as_bytes()))
        .filter_map(|path| {
            path.strip_prefix(skills)?
                .strip_prefix('/')?
                .split('/')
                .next()
        })
        .map(|name| output::escape_control(&format!("{skills}/{name}")))
        .collect();
    folders.sort();
    folders.dedup();
    if folders.is_empty() {
        return None;
    }
    let folders = folders.join(" ");
    Some(format!(
        "main still tracks fold's skills in {folders}; the daemon's next start stops tracking them, or `fld untrack {folders}` does it now and leaves them on disk"
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const AT: i64 = 1_792_775_503_162;

    fn utc(at: Timestamp) -> String {
        output::readable_time(at, &chrono::Utc)
    }

    fn lines(body: Value) -> Vec<String> {
        status_lines(&Response { status: 200, body }, &utc).unwrap()
    }

    fn answer(fields: Value) -> Value {
        let mut body = json!({
            "head": "r12", "folder": "r12", "at_main": true, "behind_because": null, "branch": null,
            "changes": [], "skipped": [], "ignored_tracked": [], "blockers": [], "stash": [],
        });
        for (key, value) in fields.as_object().unwrap() {
            body[key] = value.clone();
        }
        body
    }

    fn change(path: &str, kind: &str, entry: &str, held: bool) -> Value {
        json!({"path": path, "kind": kind, "entry": entry, "held": held})
    }

    fn entry(stash: &str, state: &str, files: u64, next: bool) -> Value {
        json!({"stash": stash, "state": state, "created_at": AT, "base": "r8", "actor": "human:ana", "files": files, "next": next})
    }

    #[test]
    fn a_clean_folder_says_so_in_one_line_with_the_git_branch_while_git_sync_is_on() {
        assert_eq!(
            lines(answer(json!({}))),
            ["Nothing to record: the project folder is at main r12"]
        );
        assert_eq!(
            lines(answer(json!({"branch": "trunk"}))),
            [
                "Nothing to record: the project folder is at main r12",
                "  ↳ Git: main is synced with the branch trunk",
            ]
        );
    }

    #[test]
    fn changes_are_listed_in_path_order_with_their_kind_and_the_next_step() {
        assert_eq!(
            lines(answer(json!({
                "branch": "main",
                "changes": [
                    change("a.txt", "modified", "file", false),
                    change("bin/run", "added", "executable", false),
                    change("docs", "type_changed", "symlink", false),
                    change("link", "type_changed", "file", false),
                    change("old.md", "deleted", "file", false),
                    change("tip", "added", "symlink", false),
                ],
                "skipped": [{"path": "video.mp4", "reason": "too_large", "tracked": false}],
            }))),
            [
                "Project folder at main r12",
                "  ↳ Git: main is synced with the branch main",
                "",
                "To record: 6 files",
                "  ↳ modified: a.txt",
                "  ↳ added: bin/run (executable)",
                "  ↳ type changed: docs (now a symlink)",
                "  ↳ type changed: link (now a file)",
                "  ↳ deleted: old.md",
                "  ↳ added: tip (symlink)",
                "  ↳ Next: `fld record` records them on main; `fld stash` puts them aside",
                "",
                "Left out: 1 file, as `fld record` leaves them out",
                "  ↳ video.mp4: larger than the limit",
                "",
                "6 files to record",
            ]
        );
    }

    #[test]
    fn a_held_folder_is_behind_main_and_names_its_paths_and_the_next_step() {
        assert_eq!(
            lines(answer(json!({
                "folder": "r10", "at_main": false, "behind_because": "folder_held",
                "changes": [change("README.md", "modified", "file", true), change("notes.md", "added", "file", false)],
                "blockers": [{"kind": "folder_held", "paths": ["README.md"], "next": "run `fld record`, or take the changes back"}],
                "stash": [entry("stash@{0}", "stashed", 3, true), entry("stash@{1}", "stashed", 1, false)],
            }))),
            [
                "Project folder at r10, behind main r12",
                "  ↳ Why: paths main changed hold changes of yours that no revision records",
                "",
                "To record: 2 files",
                "  ↳ modified: README.md, held by main",
                "  ↳ added: notes.md",
                "  ↳ Next: `fld record` records them on main; `fld stash` puts them aside",
                "",
                "Held by main: changes of yours on 1 path that main changed too",
                "  ↳ README.md",
                "  ↳ Next: run `fld record`, or take the changes back",
                "",
                "Stash: 2 entries",
                "  ↳ stash@{0}: 3 files; stashed on r8 by @ana, 23 Oct 2026, 17:11",
                "    ↳ `fld unstash` applies it next",
                "  ↳ stash@{1}: 1 file; stashed on r8 by @ana, 23 Oct 2026, 17:11",
                "",
                "2 files to record · 1 held by main · 2 stash entries",
            ]
        );
    }

    #[test]
    fn tracked_fold_skills_name_their_folders_and_the_untrack_that_stops_tracking_them() {
        let answer = answer(json!({"ignored_tracked": [
            ".claude/skills/fold-main/SKILL.md",
            ".claude/skills/fold/ERRORS.md",
            ".claude/skills/fold/SKILL.md",
            ".claude/skills/fold-custom/SKILL.md",
            "app.env",
        ]}));
        assert_eq!(
            lines(answer),
            [
                "Nothing to record: the project folder is at main r12",
                "  ↳ 5 tracked files match ignore rules; `fld status --json` lists them",
                "  ↳ main still tracks fold's skills in .claude/skills/fold .claude/skills/fold-main; the daemon's next start stops tracking them, or `fld untrack .claude/skills/fold .claude/skills/fold-main` does it now and leaves them on disk",
            ]
        );
        assert_eq!(
            skills_note(&["app.env", ".claude/skills/fold-custom/a.md"]),
            None
        );
    }

    #[test]
    fn a_tracked_local_settings_file_names_the_untrack_that_stops_tracking_it() {
        let hint = "  ↳ .claude/settings.local.json holds your own Claude Code settings; `fld untrack .claude/settings.local.json` stops tracking it and leaves it on disk";
        assert_eq!(
            lines(answer(
                json!({"ignored_tracked": [".claude/settings.local.json"]})
            )),
            [
                "Nothing to record: the project folder is at main r12",
                "  ↳ 1 tracked file matches ignore rules; `fld status --json` lists them",
                hint,
            ]
        );
        let busy = lines(answer(json!({
            "ignored_tracked": [".claude/settings.local.json", "app.env"],
            "changes": [change("notes.md", "added", "file", false)],
        })));
        assert_eq!(
            busy[..3],
            [
                "Project folder at main r12",
                "  ↳ 2 tracked files match ignore rules; `fld status --json` lists them",
                hint
            ]
        );
        let other = lines(answer(json!({"ignored_tracked": ["app.env"]})));
        assert!(
            !other
                .iter()
                .any(|line| line.contains("settings.local.json")),
            "{other:?}"
        );
    }

    #[test]
    fn a_folder_behind_main_with_nothing_in_the_way_follows_it_next() {
        assert_eq!(
            lines(answer(json!({"folder": "r11", "at_main": false})))[..2],
            [
                "Project folder at r11, behind main r12",
                "  ↳ Why: fold has not brought it to main yet; it does so on its next follow",
            ]
        );
    }

    #[test]
    fn an_unstash_conflict_and_a_waiting_change_name_their_files_and_steps() {
        let unstash = lines(answer(json!({
            "changes": [change("README.md", "modified", "file", false)],
            "blockers": [{"kind": "unstash_conflicted", "paths": ["README.md"], "next": "fix the conflict markers in these files, then run `fld record`"}],
            "stash": [entry("stash@{0}", "conflicted", 1, false)],
        })));
        assert_eq!(
            unstash[6..],
            [
                "Unstash conflict: conflict markers of `fld unstash` wait in 1 path",
                "  ↳ README.md",
                "  ↳ Next: fix the conflict markers in these files, then run `fld record`",
                "",
                "Stash: 1 entry",
                "  ↳ stash@{0}: 1 file, its unstash conflicts; stashed on r8 by @ana, 23 Oct 2026, 17:11",
                "",
                "1 file to record · unstash conflict · 1 stash entry",
            ]
        );
        let waiting = lines(answer(json!({
            "folder": "r10", "at_main": false, "behind_because": "human_change",
            "changes": [change("a.txt", "modified", "file", true)],
            "blockers": [{"kind": "human_change", "paths": ["a.txt"], "next": "ask the main agent"}],
        })));
        assert_eq!(
            waiting[1],
            "  ↳ Why: your change conflicts with main and waits for the main agent"
        );
        assert_eq!(
            waiting[7..],
            [
                "Waits for the main agent: your change conflicts with main in 1 path",
                "  ↳ a.txt",
                "  ↳ Next: ask the main agent",
                "",
                "1 file to record · 1 held by main · waits for the main agent",
            ]
        );
    }

    #[test]
    fn an_open_unstash_and_a_moved_branch_name_their_steps() {
        assert_eq!(
            lines(answer(json!({
                "branch": "main",
                "blockers": [
                    {"kind": "head_moved", "paths": [], "next": "run `git switch main` in the project folder"},
                    {"kind": "commits_waiting", "paths": [], "next": "fold records them on its next run"},
                    {"kind": "unstash_open", "paths": ["a.txt", "b.txt"], "next": "run `fld unstash` to finish it"},
                ],
                "stash": [entry("stash@{0}", "unstashing", 2, true)],
            }))),
            [
                "Project folder at main r12",
                "  ↳ Git: main is synced with the branch main",
                "",
                "Git branch moved: the branch or HEAD shows a change fold did not make, so fold does not update the folder",
                "  ↳ Next: run `git switch main` in the project folder",
                "",
                "Git commits wait: the branch holds commits of yours that fold has not recorded yet",
                "  ↳ Next: fold records them on its next run",
                "",
                "Unstash stopped part-way: it writes 2 paths once it is finished",
                "  ↳ a.txt",
                "  ↳ b.txt",
                "  ↳ Next: run `fld unstash` to finish it",
                "",
                "Stash: 1 entry",
                "  ↳ stash@{0}: 2 files, its unstash stopped part-way; stashed on r8 by @ana, 23 Oct 2026, 17:11",
                "    ↳ `fld unstash` finishes it next",
                "",
                "nothing to record · Git branch moved · Git commits wait · unstash stopped part-way · 1 stash entry",
            ]
        );
        assert_eq!(
            lines(answer(
                json!({"folder": "r9", "at_main": false, "behind_because": "head_moved", "blockers": [{"kind": "head_moved", "paths": [], "next": "x"}]})
            ))[1],
            "  ↳ Why: the Git branch or HEAD shows a change fold did not make"
        );
    }

    #[test]
    fn tracked_files_that_rules_match_are_counted_and_listed_only_when_one_changed() {
        assert_eq!(
            lines(answer(
                json!({"branch": "main", "ignored_tracked": ["app.env"]})
            )),
            [
                "Nothing to record: the project folder is at main r12",
                "  ↳ Git: main is synced with the branch main",
                "  ↳ 1 tracked file matches ignore rules; `fld status --json` lists them",
            ]
        );
        let many: Vec<String> = (0..12).map(|n| format!("gen/{n:02}.rs")).collect();
        assert_eq!(
            lines(answer(json!({
                "skipped": [{"path": "data.bin", "reason": "too_large", "tracked": true}],
                "ignored_tracked": many,
            }))),
            [
                "Project folder at main r12",
                "  ↳ 12 tracked files match ignore rules; `fld status --json` lists them",
                "",
                "Left out: 1 file, as `fld record` leaves them out",
                "  ↳ data.bin: larger than the limit; main keeps the version it recorded, and `fld untrack data.bin` removes it from main",
                "",
                "nothing to record",
            ]
        );
        assert_eq!(
            lines(answer(json!({
                "changes": [change("gen/03.rs", "modified", "file", false)],
                "ignored_tracked": many,
            }))),
            [
                "Project folder at main r12",
                "",
                "To record: 1 file",
                "  ↳ modified: gen/03.rs",
                "  ↳ Next: `fld record` records them on main; `fld stash` puts them aside",
                "",
                "Tracked although ignored: 12 files, which `fld record` records because main tracks them",
                "  ↳ gen/00.rs",
                "  ↳ gen/01.rs",
                "  ↳ gen/02.rs",
                "  ↳ gen/03.rs",
                "  ↳ gen/04.rs",
                "  ↳ gen/05.rs",
                "  ↳ gen/06.rs",
                "  ↳ gen/07.rs",
                "  ↳ gen/08.rs",
                "  ↳ gen/09.rs",
                "  ↳ and 2 more",
                "  ↳ Next: `fld untrack <path>` stops tracking a file and leaves it on disk",
                "",
                "1 file to record",
            ]
        );
        assert!(matches!(
            status_lines(
                &Response {
                    status: 200,
                    body: answer(json!({"ignored_tracked": [7]})),
                },
                &utc,
            ),
            Err(RemoteError::Answer)
        ));
    }

    #[test]
    fn agent_text_is_escaped_and_no_line_holds_an_escape() {
        let all = lines(answer(json!({
            "changes": [change("evil\u{1b}[2J.txt", "added", "file", false)],
        })));
        assert_eq!(all[3], "  ↳ added: evil\\u{1b}[2J.txt");
        assert!(all.iter().all(|line| !line.contains('\u{1b}')));
    }

    #[test]
    fn a_strange_or_refused_answer_is_an_error() {
        let strange = |fields: Value| {
            status_lines(
                &Response {
                    status: 200,
                    body: answer(fields),
                },
                &utc,
            )
        };
        assert!(matches!(
            strange(json!({"changes": [change("a", "renamed", "file", false)]})),
            Err(RemoteError::Answer)
        ));
        assert!(matches!(
            strange(json!({"blockers": [{"kind": "strange", "paths": [], "next": "x"}]})),
            Err(RemoteError::Answer)
        ));
        assert!(matches!(
            strange(json!({"stash": [entry("stash@{0}", "applied", 1, false)]})),
            Err(RemoteError::Answer)
        ));
        assert!(matches!(
            strange(json!({"at_main": false, "behind_because": "strange"})),
            Err(RemoteError::Answer)
        ));
        assert!(matches!(
            strange(json!({"head": null})),
            Err(RemoteError::Answer)
        ));
        let refused = Response {
            status: 404,
            body: json!({"error": {"code": "project_not_found", "message": "no project"}}),
        };
        assert!(matches!(
            status_lines(&refused, &utc),
            Err(RemoteError::Refused { code, .. }) if code == "project_not_found"
        ));
        assert_eq!(
            skipped_reason("special"),
            "not a file, a directory or a symlink"
        );
        assert_eq!(skipped_reason("unreadable"), "not readable");
        assert_eq!(skipped_reason("other"), "other");
    }
}
