use std::io::{self, BufRead, Write};
use std::path::Path;

use fold_core::{FoldError, codes};
use fold_vcs::git::Git;

use crate::output;

const NAME_KEY: &str = "user.name";
const EMAIL_KEY: &str = "user.email";
const ATTEMPTS: usize = 3;

/// The values given on the command line.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Given {
    pub name: Option<String>,
    pub email: Option<String>,
}

/// What the environment already settles, read once. Git takes author and committer from separate variables, so a value counts as set only when both are.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Environment {
    pub name_set: bool,
    pub email_set: bool,
    /// Nobody answers a question in a CI run.
    pub ci: bool,
}

impl Environment {
    pub fn from_env() -> Self {
        Self::from_vars(|name| std::env::var_os(name))
    }

    fn from_vars(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Self {
        let set = |name: &str| var(name).is_some_and(|value| !value.is_empty());
        Self {
            name_set: set("GIT_AUTHOR_NAME") && set("GIT_COMMITTER_NAME"),
            email_set: set("EMAIL") || (set("GIT_AUTHOR_EMAIL") && set("GIT_COMMITTER_EMAIL")),
            ci: set("CI"),
        }
    }
}

/// A name Git accepts and a person can read: not blank, no control characters.
pub fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && !name.contains(char::is_control)
}

/// One `@` with text on both sides, and no spaces or control characters.
pub fn valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && !email.contains(|c: char| c.is_whitespace() || c.is_control())
}

/// Refuses a flag value that Git would store as a broken identity.
pub fn check_given(given: &Given) -> Result<(), FoldError> {
    let refuse = |flag: &str, reason: &str| {
        FoldError::new(codes::INVALID_REQUEST, format!("{flag} {reason}")).with_hint(format!(
            "give {flag} a value such as the one in your other repositories"
        ))
    };
    if given.name.as_deref().is_some_and(|name| !valid_name(name)) {
        return Err(refuse(
            "--git-name",
            "must not be empty or hold control characters",
        ));
    }
    if given
        .email
        .as_deref()
        .is_some_and(|email| !valid_email(email))
    {
        return Err(refuse("--git-email", "is not an email address"));
    }
    Ok(())
}

/// Which of the two values still need an answer.
#[derive(Debug, Default, PartialEq, Eq)]
struct Missing {
    name: bool,
    email: bool,
}

fn configured(git: &Git, root: &Path, key: &str) -> bool {
    git.config_get(root, key)
        .ok()
        .flatten()
        .is_some_and(|value| !value.trim().is_empty())
}

/// Reads one required line, `None` after too many bad answers or at the end of input.
fn read_value(
    input: &mut impl BufRead,
    out: &mut impl Write,
    prompt: &str,
    valid: fn(&str) -> bool,
    complaint: &str,
) -> io::Result<Option<String>> {
    for _ in 0..ATTEMPTS {
        write!(out, "{prompt}")?;
        out.flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            writeln!(out)?;
            return Ok(None);
        }
        let value = line.trim();
        if valid(value) {
            return Ok(Some(value.to_owned()));
        }
        output::line(out, complaint)?;
    }
    Ok(None)
}

fn warning(missing: &Missing) -> String {
    let keys: Vec<&str> = [
        missing.name.then_some("git config user.name"),
        missing.email.then_some("git config user.email"),
    ]
    .into_iter()
    .flatten()
    .collect();
    format!(
        "Git has no {} for commits in this project; run {} in the project folder",
        if missing.name && missing.email {
            "name and email"
        } else if missing.name {
            "name"
        } else {
            "email"
        },
        keys.join(" and ")
    )
}

fn store(git: &Git, root: &Path, key: &str, value: &str, warn: &mut impl Write) {
    if git.config_set_local(root, key, value).is_err() {
        let _ = output::line(
            warn,
            &format!(
                "cannot write {key} into the project's Git config; run git config {key} in the project folder"
            ),
        );
    }
}

/// How `ensure` decides: the flag values, the environment, and whether a person can answer.
#[derive(Debug, Clone, Copy)]
pub struct Plan<'a> {
    pub given: &'a Given,
    pub env: &'a Environment,
    pub interactive: bool,
}

/// Gives the repository at `root`, which `git init` just created, a name and an email for the human's own commits. Values from the flags are written as they are. A value that Git already resolves, from any config file or the environment, is left alone. A missing value is asked for on a terminal and otherwise named in a warning on `warn`; init goes on either way. Values are written to the repository's own config and never logged.
pub fn ensure(
    git: &Git,
    root: &Path,
    plan: Plan<'_>,
    input: &mut impl BufRead,
    out: &mut impl Write,
    warn: &mut impl Write,
) {
    let Plan {
        given,
        env,
        interactive,
    } = plan;
    let mut name = given.name.clone();
    let mut email = given.email.clone();
    let mut missing = Missing {
        name: name.is_none() && !env.name_set && !configured(git, root, NAME_KEY),
        email: email.is_none() && !env.email_set && !configured(git, root, EMAIL_KEY),
    };
    if (missing.name || missing.email) && interactive && !env.ci {
        // A broken terminal leaves the unanswered values marked missing, which the warning names.
        let _ = ask(input, out, &mut missing, &mut name, &mut email);
    }
    if let Some(name) = &name {
        store(git, root, NAME_KEY, name, warn);
    }
    if let Some(email) = &email {
        store(git, root, EMAIL_KEY, email, warn);
    }
    if missing.name || missing.email {
        let _ = output::line(warn, &warning(&missing));
    }
}

/// Asks for what is missing. A value left unanswered stays marked missing.
fn ask(
    input: &mut impl BufRead,
    out: &mut impl Write,
    missing: &mut Missing,
    name: &mut Option<String>,
    email: &mut Option<String>,
) -> io::Result<()> {
    output::line(
        out,
        "Git needs a name and an email for your own commits in this project. fld stores them in this project's Git config only.",
    )?;
    if missing.name {
        *name = read_value(
            input,
            out,
            "Git user name: ",
            valid_name,
            "A name is required.",
        )?;
        missing.name = name.is_none();
    }
    if missing.email {
        *email = read_value(
            input,
            out,
            "Git user email: ",
            valid_email,
            "That is not an email address.",
        )?;
        missing.email = email.is_none();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;

    /// A repository and a `git` that ignores the machine's global and system config.
    struct Fixture {
        dir: tempfile::TempDir,
        git: Git,
    }

    impl Fixture {
        fn new(global: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let config = dir.path().join("gitconfig");
            std::fs::write(&config, global).unwrap();
            let script = dir.path().join("git-isolated");
            std::fs::write(
                &script,
                format!(
                    "#!/bin/sh\nGIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL='{}' exec git \"$@\"\n",
                    config.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            let git = Git::new(script);
            git.init_checkout(&Self::root_in(dir.path()), "main")
                .unwrap_or_else(|_| {
                    std::fs::create_dir(Self::root_in(dir.path())).unwrap();
                    git.init_checkout(&Self::root_in(dir.path()), "main")
                        .unwrap();
                });
            Self { dir, git }
        }

        fn root_in(dir: &Path) -> PathBuf {
            dir.join("repo")
        }

        fn root(&self) -> PathBuf {
            Self::root_in(self.dir.path())
        }

        fn value(&self, key: &str) -> Option<String> {
            self.git.config_get(&self.root(), key).unwrap()
        }

        fn run(&self, given: &Given, env: Environment, interactive: bool, typed: &str) -> Run {
            let (mut out, mut warn) = (Vec::new(), Vec::new());
            ensure(
                &self.git,
                &self.root(),
                Plan {
                    given,
                    env: &env,
                    interactive,
                },
                &mut typed.as_bytes(),
                &mut out,
                &mut warn,
            );
            Run {
                out: String::from_utf8(out).unwrap(),
                warn: String::from_utf8(warn).unwrap(),
            }
        }
    }

    struct Run {
        out: String,
        warn: String,
    }

    #[test]
    fn names_and_emails_are_checked() {
        assert!(valid_name("Ana Lima"));
        assert!(!valid_name("  "));
        assert!(!valid_name("a\u{7}b"));
        assert!(valid_email("ana@example.com"));
        assert!(valid_email("ana@localhost"));
        for bad in ["ana", "@example.com", "ana@", "a@b@c", "a na@x.com", ""] {
            assert!(!valid_email(bad), "{bad}");
        }
        let bad_name = Given {
            name: Some(" ".to_owned()),
            email: None,
        };
        assert!(check_given(&bad_name).is_err());
        let bad_email = Given {
            name: None,
            email: Some("x".to_owned()),
        };
        assert!(check_given(&bad_email).is_err());
        assert!(check_given(&Given::default()).is_ok());
    }

    #[test]
    fn the_environment_needs_author_and_committer() {
        let vars = |pairs: &'static [&str]| {
            Environment::from_vars(move |name| {
                pairs.contains(&name).then(|| std::ffi::OsString::from("x"))
            })
        };
        assert!(!vars(&["GIT_AUTHOR_NAME"]).name_set);
        assert!(vars(&["GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME"]).name_set);
        assert!(!vars(&["GIT_COMMITTER_EMAIL"]).email_set);
        assert!(vars(&["GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL"]).email_set);
        assert!(vars(&["EMAIL"]).email_set);
        assert!(vars(&["CI"]).ci);
        assert_eq!(vars(&[]), Environment::default());
    }

    #[test]
    fn a_terminal_is_asked_for_both_values() {
        let fixture = Fixture::new("");
        let run = fixture.run(
            &Given::default(),
            Environment::default(),
            true,
            "Ana Lima\nana@example.com\n",
        );
        assert!(run.out.contains("Git user name: "));
        assert!(run.out.contains("Git user email: "));
        assert_eq!(run.warn, "");
        assert_eq!(fixture.value("user.name").as_deref(), Some("Ana Lima"));
        assert_eq!(
            fixture.value("user.email").as_deref(),
            Some("ana@example.com")
        );
    }

    #[test]
    fn a_wrong_answer_is_asked_again() {
        let fixture = Fixture::new("");
        let run = fixture.run(
            &Given::default(),
            Environment::default(),
            true,
            "\nAna\nnot-an-email\nana@example.com\n",
        );
        assert!(run.out.contains("A name is required."));
        assert!(run.out.contains("That is not an email address."));
        assert_eq!(fixture.value("user.name").as_deref(), Some("Ana"));
        assert_eq!(
            fixture.value("user.email").as_deref(),
            Some("ana@example.com")
        );
    }

    #[test]
    fn three_wrong_answers_or_a_closed_input_leave_a_warning() {
        let fixture = Fixture::new("");
        let run = fixture.run(
            &Given::default(),
            Environment::default(),
            true,
            "\n\n\nana@example.com\n",
        );
        assert!(run.warn.contains("git config user.name"));
        assert!(!run.warn.contains("git config user.email"));
        assert_eq!(fixture.value("user.name"), None);
        assert_eq!(
            fixture.value("user.email").as_deref(),
            Some("ana@example.com")
        );
        let closed = Fixture::new("");
        let run = closed.run(&Given::default(), Environment::default(), true, "");
        assert!(run.warn.contains("name and email"));
    }

    #[test]
    fn a_set_value_is_not_asked_for() {
        let fixture = Fixture::new("[user]\n\tname = Global\n");
        let run = fixture.run(
            &Given::default(),
            Environment::default(),
            true,
            "ana@example.com\n",
        );
        assert!(!run.out.contains("Git user name"));
        assert!(run.out.contains("Git user email: "));
        assert_eq!(
            fixture
                .git
                .config_get(&fixture.root(), "user.name")
                .unwrap()
                .as_deref(),
            Some("Global")
        );
    }

    #[test]
    fn nothing_is_asked_without_a_terminal_or_in_ci() {
        let fixture = Fixture::new("");
        let run = fixture.run(&Given::default(), Environment::default(), false, "a\nb\n");
        assert_eq!(run.out, "");
        assert!(run.warn.contains("name and email"));
        let ci = Environment {
            ci: true,
            ..Environment::default()
        };
        let run = fixture.run(&Given::default(), ci, true, "a\nb\n");
        assert_eq!(run.out, "");
        assert!(run.warn.contains("git config user.name"));
        assert_eq!(fixture.value("user.name"), None);
    }

    #[test]
    fn flags_win_over_a_global_value_and_skip_the_question() {
        let fixture = Fixture::new("[user]\n\tname = Global\n\temail = g@example.com\n");
        let given = Given {
            name: Some("Local".to_owned()),
            email: Some("local@example.com".to_owned()),
        };
        let run = fixture.run(&given, Environment::default(), true, "");
        assert_eq!((run.out.as_str(), run.warn.as_str()), ("", ""));
        assert_eq!(fixture.value("user.name").as_deref(), Some("Local"));
    }

    #[test]
    fn a_failed_write_is_a_warning_that_names_no_value() {
        let fixture = Fixture::new("");
        let given = Given {
            name: Some("Ana".to_owned()),
            email: Some("ana@example.com".to_owned()),
        };
        let gone = fixture.dir.path().join("missing");
        let mut warn = Vec::new();
        ensure(
            &fixture.git,
            &gone,
            Plan {
                given: &given,
                env: &Environment::default(),
                interactive: false,
            },
            &mut "".as_bytes(),
            &mut Vec::new(),
            &mut warn,
        );
        let text = String::from_utf8(warn).unwrap();
        assert!(text.contains("cannot write user.name"), "{text}");
        assert!(!text.contains("ana@example.com") && !text.contains("Ana"));
    }
}
