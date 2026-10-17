use std::ffi::OsString;
use std::fmt;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use fold_cli::daemon_client::{self, ClientError};
use fold_core::HumanName;
use serde::{Deserialize, Serialize};

use crate::output;
use crate::serve::Dirs;

/// The file in the fold config folder that holds the fold name for every project on this device.
pub(crate) const FILE: &str = "human.toml";
const ATTEMPTS: usize = 3;
const QUESTION: &str = "fold needs your name inside fold: it names you in the history, on reviews and in messages, and never goes into a Git commit. fld stores it for every project on this computer.";
const RULE: &str =
    "A fold name is 1 to 32 characters of a-z, 0-9, - and _, starting with a letter or digit.";

#[derive(Debug, Serialize, Deserialize)]
struct Stored {
    name: String,
}

/// Where the fold name of a run comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Source {
    /// `FOLD_HUMAN`, `USER` or `LOGNAME`.
    Variable(&'static str),
    /// The stored name in the fold config folder.
    File(PathBuf),
    /// An answer that fld could not store, so it holds for this run only.
    Answer,
    /// The account name of the process's user.
    Account,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Variable(name) => f.write_str(name),
            Self::File(path) => write!(f, "{}", path.display()),
            Self::Answer => f.write_str("the answer, which fld could not store"),
            Self::Account => f.write_str("the account name"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Found {
    pub(crate) name: String,
    pub(crate) source: Source,
}

/// The variables that decide the fold name, read once.
#[derive(Debug, Default, Clone)]
pub(crate) struct Environment {
    pub(crate) fold_human: Option<OsString>,
    pub(crate) user: Option<OsString>,
    pub(crate) logname: Option<OsString>,
    /// Nobody answers a question in a CI run.
    pub(crate) ci: bool,
}

impl Environment {
    pub(crate) fn from_env() -> Self {
        Self {
            fold_human: std::env::var_os("FOLD_HUMAN"),
            user: std::env::var_os("USER"),
            logname: std::env::var_os("LOGNAME"),
            ci: std::env::var_os("CI").is_some_and(|value| !value.is_empty()),
        }
    }
}

fn set(value: Option<&OsString>) -> Option<String> {
    value
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string_lossy().into_owned())
}

/// `human.toml` in the fold config folder, `$XDG_CONFIG_HOME/fold` or `~/.config/fold`.
pub(crate) fn file(dirs: &Dirs) -> Option<PathBuf> {
    dirs.config_base()
        .ok()
        .map(|base| base.join("fold").join(FILE))
}

/// The stored name, when the file holds one the daemon accepts.
fn read_stored(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let stored: Stored = toml::from_str(&text).ok()?;
    HumanName::parse(&stored.name).ok()?;
    Some(stored.name)
}

fn store(path: &Path, name: &str) -> io::Result<()> {
    let text = toml::to_string(&Stored {
        name: name.to_owned(),
    })
    .map_err(io::Error::other)?;
    fold_vcs::atomic::write_atomic(path, text.as_bytes())
}

/// Asks for a name the daemon accepts, offering `default` for an empty answer. `None` after too many bad answers or at the end of input.
fn ask(
    input: &mut impl BufRead,
    out: &mut impl Write,
    default: Option<&str>,
) -> io::Result<Option<String>> {
    output::line(out, QUESTION)?;
    let prompt = match default {
        Some(default) => format!("fold name [{default}]: "),
        None => "fold name: ".to_owned(),
    };
    for _ in 0..ATTEMPTS {
        write!(out, "{prompt}")?;
        out.flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            writeln!(out)?;
            return Ok(None);
        }
        let answer = match (line.trim(), default) {
            ("", Some(default)) => default,
            (answer, _) => answer,
        };
        if HumanName::parse(answer).is_ok() {
            return Ok(Some(answer.to_owned()));
        }
        output::line(out, RULE)?;
    }
    Ok(None)
}

/// The fold name from the environment alone: `USER`, else `LOGNAME`, else the account name.
fn fallback(env: &Environment) -> Result<Found, ClientError> {
    for (variable, value) in [("USER", &env.user), ("LOGNAME", &env.logname)] {
        if let Some(name) = set(value.as_ref()) {
            return Ok(Found {
                name,
                source: Source::Variable(variable),
            });
        }
    }
    daemon_client::os_account()
        .map(|name| Found {
            name,
            source: Source::Account,
        })
        .ok_or(ClientError::HumanUnknown)
}

/// The human's name inside fold, which is not the Git identity. `FOLD_HUMAN` wins, then the name stored in `file`. Without either, a person on a terminal outside CI is asked once for a name, with `USER` as the default when the daemon accepts it, and the answer is stored in `file` for every project on the device. Otherwise `USER`, `LOGNAME` or the account name stands in, as it does when the question gets no answer.
pub(crate) fn resolve(
    env: &Environment,
    file: Option<&Path>,
    interactive: bool,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<Found, ClientError> {
    if let Some(name) = set(env.fold_human.as_ref()) {
        return Ok(Found {
            name,
            source: Source::Variable("FOLD_HUMAN"),
        });
    }
    if let Some(path) = file {
        if let Some(name) = read_stored(path) {
            return Ok(Found {
                name,
                source: Source::File(path.to_owned()),
            });
        }
        if interactive && !env.ci {
            let default = [&env.user, &env.logname]
                .into_iter()
                .find_map(|value| set(value.as_ref()))
                .filter(|name| HumanName::parse(name).is_ok());
            if let Ok(Some(name)) = ask(input, out, default.as_deref()) {
                let source = match store(path, &name) {
                    Ok(()) => Source::File(path.to_owned()),
                    Err(_) => {
                        let _ = output::line(
                            out,
                            &format!(
                                "fld cannot store the fold name in {}; it asks again next time",
                                path.display()
                            ),
                        );
                        Source::Answer
                    }
                };
                return Ok(Found { name, source });
            }
        }
    }
    fallback(env)
}

/// What the fold name of this device is, without a question.
pub(crate) fn known(dirs: &Dirs) -> Result<Found, ClientError> {
    resolve(
        &Environment::from_env(),
        file(dirs).as_deref(),
        false,
        &mut io::empty(),
        &mut io::sink(),
    )
}

/// The fold name of the human who runs this command, settled once per process: the first command that needs it asks on the terminal when no name is stored, as [`resolve`] describes.
pub(crate) fn current() -> Option<String> {
    static NAME: OnceLock<Option<String>> = OnceLock::new();
    NAME.get_or_init(|| {
        let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
        resolve(
            &Environment::from_env(),
            file(&Dirs::from_env()).as_deref(),
            interactive,
            &mut io::stdin().lock(),
            &mut io::stdout(),
        )
        .ok()
        .map(|found| found.name)
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(fold_human: Option<&str>, user: Option<&str>) -> Environment {
        Environment {
            fold_human: fold_human.map(OsString::from),
            user: user.map(OsString::from),
            logname: None,
            ci: false,
        }
    }

    struct Run {
        found: Found,
        out: String,
    }

    fn run(env: &Environment, file: Option<&Path>, interactive: bool, typed: &str) -> Run {
        let mut out = Vec::new();
        let found = resolve(env, file, interactive, &mut typed.as_bytes(), &mut out).unwrap();
        Run {
            found,
            out: String::from_utf8(out).unwrap(),
        }
    }

    fn stored_in(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("config/fold").join(FILE)
    }

    #[test]
    fn fold_human_wins_over_the_stored_name_and_asks_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = stored_in(&dir);
        store(&path, "ana").unwrap();
        let run = run(&env(Some("lead"), Some("bo")), Some(&path), true, "x\n");
        assert_eq!(
            run.found,
            Found {
                name: "lead".to_owned(),
                source: Source::Variable("FOLD_HUMAN"),
            }
        );
        assert_eq!(run.out, "");
    }

    #[test]
    fn a_stored_name_holds_for_every_later_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = stored_in(&dir);
        store(&path, "ana").unwrap();
        let run = run(&env(Some(""), Some("bo")), Some(&path), true, "x\n");
        assert_eq!(
            run.found,
            Found {
                name: "ana".to_owned(),
                source: Source::File(path.clone()),
            }
        );
        assert_eq!(run.out, "");
    }

    #[test]
    fn a_terminal_is_asked_once_and_the_answer_is_stored() {
        let dir = tempfile::tempdir().unwrap();
        let path = stored_in(&dir);
        let first = run(&env(None, Some("bo")), Some(&path), true, "ana\n");
        assert!(first.out.contains("fold name [bo]: "), "{}", first.out);
        assert_eq!(first.found.name, "ana");
        assert_eq!(first.found.source, Source::File(path.clone()));
        assert_eq!(read_stored(&path).as_deref(), Some("ana"));
        let second = run(&env(None, Some("bo")), Some(&path), true, "other\n");
        assert_eq!(second.found.name, "ana");
        assert_eq!(second.out, "");
    }

    #[test]
    fn an_empty_answer_takes_the_user_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = stored_in(&dir);
        let run = run(&env(None, Some("bo")), Some(&path), true, "\n");
        assert_eq!(run.found.name, "bo");
        assert_eq!(read_stored(&path).as_deref(), Some("bo"));
    }

    #[test]
    fn a_user_name_the_daemon_refuses_is_no_default_and_a_bad_answer_is_asked_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = stored_in(&dir);
        let run = run(
            &env(None, Some("Ana.Lima")),
            Some(&path),
            true,
            "\nAna\nana-lima\n",
        );
        assert!(run.out.contains("fold name: "), "{}", run.out);
        assert!(!run.out.contains("[Ana.Lima]"));
        assert_eq!(run.out.matches(RULE).count(), 2);
        assert_eq!(run.found.name, "ana-lima");
    }

    #[test]
    fn no_answer_falls_back_to_the_user_name_and_stores_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = stored_in(&dir);
        let closed = run(&env(None, Some("bo")), Some(&path), true, "");
        assert_eq!(
            closed.found,
            Found {
                name: "bo".to_owned(),
                source: Source::Variable("USER"),
            }
        );
        let wrong = run(&env(None, Some("bo")), Some(&path), true, "A\nB\nC\n");
        assert_eq!(wrong.found.name, "bo");
        assert!(!path.exists());
    }

    #[test]
    fn without_a_terminal_or_in_ci_nothing_is_asked() {
        let dir = tempfile::tempdir().unwrap();
        let path = stored_in(&dir);
        let quiet = run(&env(None, Some("bo")), Some(&path), false, "ana\n");
        assert_eq!(quiet.found.source, Source::Variable("USER"));
        assert_eq!(quiet.out, "");
        let ci = Environment {
            ci: true,
            ..env(None, Some("bo"))
        };
        let in_ci = run(&ci, Some(&path), true, "ana\n");
        assert_eq!(in_ci.found.name, "bo");
        assert_eq!(in_ci.out, "");
        let logname = Environment {
            logname: Some("lo".into()),
            ..env(None, Some(""))
        };
        assert_eq!(
            run(&logname, None, true, "ana\n").found.source,
            Source::Variable("LOGNAME")
        );
        assert!(!path.exists());
    }

    #[test]
    fn a_broken_stored_file_is_asked_again_and_an_unstorable_answer_holds_for_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = stored_in(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "name = \"Not Valid\"\n").unwrap();
        assert_eq!(read_stored(&path), None);
        let fixed = run(&env(None, None), Some(&path), true, "ana\n");
        assert_eq!(fixed.found.source, Source::File(path.clone()));
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, "").unwrap();
        let unstorable = blocked.join(FILE);
        let run = run(&env(None, None), Some(&unstorable), true, "ana\n");
        assert_eq!(run.found.source, Source::Answer);
        assert!(
            run.out.contains("cannot store the fold name"),
            "{}",
            run.out
        );
    }

    #[test]
    fn sources_name_where_the_name_comes_from() {
        assert_eq!(Source::Variable("FOLD_HUMAN").to_string(), "FOLD_HUMAN");
        assert_eq!(
            Source::File(PathBuf::from("/c/fold/human.toml")).to_string(),
            "/c/fold/human.toml"
        );
        assert_eq!(Source::Account.to_string(), "the account name");
        assert!(Source::Answer.to_string().contains("could not store"));
        let dirs = Dirs {
            config_home: Some(PathBuf::from("/c")),
            ..Dirs::default()
        };
        assert_eq!(file(&dirs), Some(PathBuf::from("/c/fold/human.toml")));
        assert_eq!(file(&Dirs::default()), None);
    }
}
