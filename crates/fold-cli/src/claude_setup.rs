use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;

use fold_cli::claude_files::ClaudeFileError;
use fold_cli::claude_user::{self, UserFiles};
use serde::{Deserialize, Serialize};

use crate::output;
use crate::serve::Dirs;

/// The command that adds fold to the user's Claude Code configuration without asking.
pub(crate) const ADD_COMMAND: &str = "fld setup --claude";
const ASK: &str = "Add fold to Claude Code for your user? [Y/n] ";

/// Which command makes the offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Offer {
    /// `fld setup`, which names the way to add fold also after the user declined.
    Setup,
    /// `fld claude`, which says nothing more once the user declined.
    Start,
}

/// The user's answer, kept in `claude.toml` of the fold config folder.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Answer {
    declined: bool,
}

fn answer_file(dirs: &Dirs) -> Option<PathBuf> {
    dirs.config_base()
        .ok()
        .map(|base| base.join("fold").join("claude.toml"))
}

pub(crate) fn declined(dirs: &Dirs) -> bool {
    answer_file(dirs)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| toml::from_str::<Answer>(&text).ok())
        .is_some_and(|answer| answer.declined)
}

/// A failed record only means that the question can come again.
fn record(dirs: &Dirs, declined: bool) {
    let Some(path) = answer_file(dirs) else {
        return;
    };
    if let Ok(text) = toml::to_string(&Answer { declined }) {
        let _ = fold_vcs::atomic::write_atomic(&path, text.as_bytes());
    }
}

fn not_added_line() -> String {
    format!("fold is not in your Claude Code config; run {ADD_COMMAND} to add it")
}

fn question(files: &UserFiles) -> String {
    format!(
        "fld can add the fold MCP server to {}, and allow the fold tools and add the fold hooks in {}. They do nothing in a Claude Code session outside a fold project.",
        files.config.display(),
        files.settings.display()
    )
}

/// The one-time question whether fold may set itself up in the user's Claude Code configuration. It never stops the command that asks: an error prints its lines and the question can come again. Without a terminal, it changes nothing and names [`ADD_COMMAND`]. Returns whether the configuration holds fold's setup afterwards.
pub(crate) fn offer(
    dirs: &Dirs,
    files: Option<&UserFiles>,
    offer: Offer,
    interactive: bool,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> bool {
    let Some(files) = files else {
        return false;
    };
    if claude_user::state(files).is_complete() {
        return true;
    }
    if declined(dirs) {
        if offer == Offer::Setup {
            let _ = output::line(out, &not_added_line());
        }
        return false;
    }
    if !interactive {
        let _ = output::line(out, &not_added_line());
        return false;
    }
    if output::line(out, &question(files)).is_err() {
        return false;
    }
    match read_answer(input, out) {
        Ok(Some(true)) => match claude_user::add(files) {
            Ok(_) => {
                record(dirs, false);
                let _ = output::line(out, &added_line(files));
                true
            }
            Err(error) => {
                let _ = output::line(out, &error_lines(&error));
                false
            }
        },
        Ok(Some(false)) => {
            record(dirs, true);
            let _ = output::line(
                out,
                &format!(
                    "fold stays out of your Claude Code config; run {ADD_COMMAND} to add it later"
                ),
            );
            false
        }
        Ok(None) | Err(_) => false,
    }
}

/// [`offer`] on the real terminal: it asks only when stdin and stdout are both terminals.
pub(crate) fn offer_on_terminal(
    dirs: &Dirs,
    files: Option<&UserFiles>,
    offer_by: Offer,
    out: &mut impl Write,
) -> bool {
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    offer(
        dirs,
        files,
        offer_by,
        interactive,
        &mut io::stdin().lock(),
        out,
    )
}

/// Yes for Enter, `y` or `yes`, no for `n` or `no`, in any case; `None` when the input ends or two answers are neither.
fn read_answer(input: &mut impl BufRead, out: &mut impl Write) -> io::Result<Option<bool>> {
    for _ in 0..2 {
        write!(out, "{ASK}")?;
        out.flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            writeln!(out)?;
            return Ok(None);
        }
        match line.trim().to_ascii_lowercase().as_str() {
            "" | "y" | "yes" => return Ok(Some(true)),
            "n" | "no" => return Ok(Some(false)),
            _ => {}
        }
    }
    Ok(None)
}

fn added_line(files: &UserFiles) -> String {
    format!(
        "added fold to Claude Code in {} and {}",
        files.config.display(),
        files.settings.display()
    )
}

fn error_lines(error: &ClaudeFileError) -> String {
    let mut text = match error.code() {
        Some(code) => format!("{code}: {error}"),
        None => error.to_string(),
    };
    if let Some(hint) = error.hint() {
        text.push_str("\nhint: ");
        text.push_str(hint);
    }
    text
}

/// `fld setup --claude`: adds fold's setup without asking, and forgets an earlier no.
pub(crate) fn add(dirs: &Dirs, files: &UserFiles, out: &mut impl Write) -> Result<(), SetupError> {
    claude_user::add(files)?;
    record(dirs, false);
    output::line(out, &added_line(files))?;
    Ok(())
}

/// `fld setup --remove`: takes fold's entries out again and remembers the removal as a no, so `fld claude` does not ask again.
pub(crate) fn remove(
    dirs: &Dirs,
    files: &UserFiles,
    out: &mut impl Write,
) -> Result<(), SetupError> {
    let changed = claude_user::remove(files)?;
    record(dirs, true);
    let text = if changed {
        format!(
            "removed fold from Claude Code in {} and {}",
            files.config.display(),
            files.settings.display()
        )
    } else {
        "your Claude Code config holds no fold entries".to_owned()
    };
    output::line(out, &text)?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error(transparent)]
    Files(#[from] ClaudeFileError),
    #[error("cannot write the output: {0}")]
    Write(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Value, json};

    use super::*;

    fn dirs(root: &Path) -> Dirs {
        Dirs {
            home: Some(root.to_owned()),
            config_home: Some(root.join("config")),
            data_home: None,
            runtime_dir: None,
            library_runtime: false,
        }
    }

    fn files(root: &Path) -> UserFiles {
        UserFiles::from_vars(|name| (name == "HOME").then(|| root.as_os_str().to_owned())).unwrap()
    }

    fn ask(root: &Path, offer_by: Offer, interactive: bool, typed: &str) -> (bool, String) {
        let mut out = Vec::new();
        let set = offer(
            &dirs(root),
            Some(&files(root)),
            offer_by,
            interactive,
            &mut typed.as_bytes(),
            &mut out,
        );
        (set, String::from_utf8(out).unwrap())
    }

    fn json_at(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn a_yes_adds_fold_and_the_question_never_comes_again() {
        let home = tempfile::tempdir().unwrap();
        let (set, shown) = ask(home.path(), Offer::Start, true, "\n");
        assert!(set);
        assert!(shown.contains("Add fold to Claude Code for your user? [Y/n] "));
        assert!(shown.contains("added fold to Claude Code"));
        let config = json_at(&home.path().join(".claude.json"));
        assert_eq!(
            config["mcpServers"]["fold"],
            json!({"type": "stdio", "command": "fld", "args": ["mcp"], "env": {}})
        );
        assert!(claude_user::state(&files(home.path())).is_complete());
        let (again, shown) = ask(home.path(), Offer::Setup, true, "n\n");
        assert!(again);
        assert_eq!(shown, "");
    }

    #[test]
    fn a_no_is_remembered_and_changes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let (set, shown) = ask(home.path(), Offer::Start, true, "no\n");
        assert!(!set);
        assert!(shown.contains("fold stays out of your Claude Code config"));
        assert!(!home.path().join(".claude.json").exists());
        assert!(!home.path().join(".claude").exists());
        let (_, start) = ask(home.path(), Offer::Start, true, "y\n");
        assert_eq!(start, "", "fld claude asks once only");
        let (set, setup) = ask(home.path(), Offer::Setup, true, "y\n");
        assert!(!set);
        assert_eq!(setup, format!("{}\n", not_added_line()));
        assert!(!home.path().join(".claude.json").exists());
    }

    #[test]
    fn without_a_terminal_nothing_changes_and_the_command_is_named() {
        let home = tempfile::tempdir().unwrap();
        let (set, shown) = ask(home.path(), Offer::Start, false, "y\n");
        assert!(!set);
        assert_eq!(
            shown,
            "fold is not in your Claude Code config; run fld setup --claude to add it\n"
        );
        assert!(!home.path().join(".claude.json").exists());
        assert!(!declined(&dirs(home.path())));
    }

    #[test]
    fn an_answer_that_is_neither_asks_once_more_and_an_end_is_no_answer() {
        let home = tempfile::tempdir().unwrap();
        let (set, shown) = ask(home.path(), Offer::Start, true, "maybe\nlater\n");
        assert!(!set);
        assert_eq!(shown.matches("[Y/n]").count(), 2);
        let (set, _) = ask(home.path(), Offer::Start, true, "");
        assert!(!set);
        assert!(!declined(&dirs(home.path())));
        let (set, _) = ask(home.path(), Offer::Start, true, "Y\n");
        assert!(set);
    }

    #[test]
    fn a_malformed_user_file_is_refused_with_a_hint_and_kept() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(".claude.json"), "{").unwrap();
        let (set, shown) = ask(home.path(), Offer::Setup, true, "\n");
        assert!(!set);
        assert!(shown.contains("invalid_request: "), "{shown}");
        assert!(shown.contains("hint: correct or move the file, then run `fld setup --claude`"));
        assert_eq!(
            std::fs::read_to_string(home.path().join(".claude.json")).unwrap(),
            "{"
        );
        assert!(!home.path().join(".claude/settings.json").exists());
        let mut out = Vec::new();
        let error = add(&dirs(home.path()), &files(home.path()), &mut out).unwrap_err();
        assert!(matches!(
            error,
            SetupError::Files(ClaudeFileError::Malformed(_))
        ));
    }

    #[test]
    fn an_explicit_add_forgets_the_no_and_a_removal_remembers_one() {
        let home = tempfile::tempdir().unwrap();
        let dirs = dirs(home.path());
        let files = files(home.path());
        record(&dirs, true);
        let mut out = Vec::new();
        add(&dirs, &files, &mut out).unwrap();
        assert!(!declined(&dirs));
        assert!(claude_user::state(&files).is_complete());
        remove(&dirs, &files, &mut out).unwrap();
        assert!(declined(&dirs));
        assert!(!claude_user::state(&files).is_complete());
        remove(&dirs, &files, &mut out).unwrap();
        let shown = String::from_utf8(out).unwrap();
        assert!(shown.contains("removed fold from Claude Code"));
        assert!(shown.ends_with("your Claude Code config holds no fold entries\n"));
    }
}
