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

/// What `decide` reads from the environment, once. The commits fold writes take the identity from the repository's config files, which never see Git's identity variables, so a value from them has to go into the config to count.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Environment {
    /// The name Git takes from `GIT_AUTHOR_NAME`, else `GIT_COMMITTER_NAME`; a value [`valid_name`] refuses counts as unset.
    pub name: Option<String>,
    /// The email Git takes from `GIT_AUTHOR_EMAIL`, else `GIT_COMMITTER_EMAIL`, else `EMAIL`; a value [`valid_email`] refuses counts as unset.
    pub email: Option<String>,
    /// Nobody answers a question in a CI run.
    pub ci: bool,
}

impl Environment {
    pub fn from_env() -> Self {
        Self::from_vars(|name| std::env::var_os(name))
    }

    fn from_vars(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Self {
        let first = |names: &[&str], valid: fn(&str) -> bool| {
            names.iter().find_map(|name| {
                var(name)
                    .and_then(|value| value.into_string().ok())
                    .map(|value| value.trim().to_owned())
                    .filter(|value| valid(value))
            })
        };
        Self {
            name: first(&["GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME"], valid_name),
            email: first(
                &["GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL", "EMAIL"],
                valid_email,
            ),
            ci: var("CI").is_some_and(|value| !value.is_empty()),
        }
    }
}

/// A name a commit's author line can carry and a person can read: not blank, no control characters, no `<` or `>`.
pub fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && !name.contains(|c: char| c.is_control() || matches!(c, '<' | '>'))
}

/// One `@` with text on both sides, and no spaces, control characters, `<` or `>`.
pub fn valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && !email.contains(|c: char| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>'))
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
            "must not be empty or hold control characters, < or >",
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

/// Whether Git resolves `key` from the global and system config and their includes. A new project has no config of its own yet, so any directory outside a repository answers the same.
fn configured(git: &Git, key: &str) -> bool {
    git.config_get(Path::new("/"), key)
        .ok()
        .flatten()
        .is_some_and(|value| !value.trim().is_empty())
}

/// Whether a plain init at `dir` gets to create a Git repository there: `dir` holds no `.git` and no working tree holds it. Init refuses a folder at or inside a repository, and a stopped init that made its repository already writes no config into it, so neither needs the question.
pub fn creates_repository(git: &Git, dir: &Path) -> bool {
    let Ok(dir) = std::path::absolute(dir) else {
        return true;
    };
    if std::fs::symlink_metadata(dir.join(".git")).is_ok() {
        return false;
    }
    // `fld init DIR` creates DIR itself, so the nearest folder that exists tells whether a working tree would hold it.
    !dir.ancestors()
        .find(|folder| folder.is_dir())
        .is_some_and(|folder| {
            git.enclosing_repository(folder, false)
                .ok()
                .flatten()
                .is_some()
        })
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

/// How `decide` works: the flag values, the environment, whether a person can answer, and the folder init makes the project in.
#[derive(Debug, Clone, Copy)]
pub struct Plan<'a> {
    pub given: &'a Given,
    pub env: &'a Environment,
    pub interactive: bool,
    pub dir: &'a Path,
}

/// The values `decide` settled for the repository's own config: the flag values and the answers.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: Option<String>,
    pub email: Option<String>,
}

/// Settles the name and email for the commits in a project whose repository does not exist yet, before init writes the first commit, which takes them. A folder where init creates no repository (see [`creates_repository`]) settles nothing and asks nothing. Values from the flags are used as they are. A value that a Git config file already sets is left alone and never copied. A value that only Git's identity variables give is taken from them, so it goes into the repository's own config. A value still missing is asked for on a terminal and otherwise named in a warning on `warn`; init goes on either way. Nothing is written and no value is logged.
pub fn decide(
    git: &Git,
    plan: Plan<'_>,
    input: &mut impl BufRead,
    out: &mut impl Write,
    warn: &mut impl Write,
) -> Identity {
    let Plan {
        given,
        env,
        interactive,
        dir,
    } = plan;
    if !creates_repository(git, dir) {
        return Identity::default();
    }
    let (mut name, missing_name) = settle(&given.name, &env.name, || configured(git, NAME_KEY));
    let (mut email, missing_email) =
        settle(&given.email, &env.email, || configured(git, EMAIL_KEY));
    let mut missing = Missing {
        name: missing_name,
        email: missing_email,
    };
    if (missing.name || missing.email) && interactive && !env.ci {
        // A broken terminal leaves the unanswered values marked missing, which the warning names.
        let _ = ask(input, out, &mut missing, &mut name, &mut email);
    }
    if missing.name || missing.email {
        let _ = output::line(warn, &warning(&missing));
    }
    Identity { name, email }
}

/// One value for the repository's own config and whether it is still missing: the flag value, else nothing when a config file sets it, else the environment's value.
fn settle(
    given: &Option<String>,
    from_env: &Option<String>,
    configured: impl FnOnce() -> bool,
) -> (Option<String>, bool) {
    match given {
        Some(value) => (Some(value.clone()), false),
        None if configured() => (None, false),
        None => (from_env.clone(), from_env.is_none()),
    }
}

/// Writes the settled values into the own config of the repository at `root`, which `git init` just created. A failed write is a warning that names the key and no value.
pub fn store_in(git: &Git, root: &Path, identity: &Identity, warn: &mut impl Write) {
    if let Some(name) = &identity.name {
        store(git, root, NAME_KEY, name, warn);
    }
    if let Some(email) = &identity.email {
        store(git, root, EMAIL_KEY, email, warn);
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
            let identity = decide(
                &self.git,
                Plan {
                    given,
                    env: &env,
                    interactive,
                    dir: &self.dir.path().join("new"),
                },
                &mut typed.as_bytes(),
                &mut out,
                &mut warn,
            );
            store_in(&self.git, &self.root(), &identity, &mut warn);
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
        assert!(!valid_name("Ana <ana>"));
        assert!(!valid_email("<ana@example.com>"));
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
    fn identity_variables_follow_git_and_skip_invalid_values() {
        let vars = |pairs: &[(&str, &str)]| {
            let pairs: Vec<(String, String)> = pairs
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect();
            Environment::from_vars(move |name| {
                pairs
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| std::ffi::OsString::from(value))
            })
        };
        let all = vars(&[
            ("GIT_AUTHOR_NAME", "Author"),
            ("GIT_COMMITTER_NAME", "Committer"),
            ("GIT_AUTHOR_EMAIL", "author@example.com"),
            ("GIT_COMMITTER_EMAIL", "committer@example.com"),
            ("EMAIL", "email@example.com"),
        ]);
        assert_eq!(all.name.as_deref(), Some("Author"));
        assert_eq!(all.email.as_deref(), Some("author@example.com"));
        let later = vars(&[
            ("GIT_AUTHOR_NAME", "Bad <name>"),
            ("GIT_COMMITTER_NAME", "Committer"),
            ("GIT_AUTHOR_EMAIL", ""),
            ("GIT_COMMITTER_EMAIL", "not-an-email"),
            ("EMAIL", "email@example.com"),
        ]);
        assert_eq!(later.name.as_deref(), Some("Committer"));
        assert_eq!(later.email.as_deref(), Some("email@example.com"));
        let invalid = vars(&[("GIT_AUTHOR_NAME", " "), ("EMAIL", "a@b@c")]);
        assert_eq!((invalid.name, invalid.email), (None, None));
        assert!(vars(&[("CI", "1")]).ci);
        assert_eq!(vars(&[]), Environment::default());
    }

    #[test]
    fn identity_variables_go_into_the_repository_config_without_a_question() {
        let env = Environment {
            name: Some("Env Name".to_owned()),
            email: Some("env@example.com".to_owned()),
            ci: false,
        };
        let fixture = Fixture::new("");
        let run = fixture.run(&Given::default(), env.clone(), true, "typed\n");
        assert_eq!((run.out.as_str(), run.warn.as_str()), ("", ""));
        assert_eq!(fixture.value("user.name").as_deref(), Some("Env Name"));
        assert_eq!(
            fixture.value("user.email").as_deref(),
            Some("env@example.com")
        );

        let configured = Fixture::new("[user]\n\tname = Global\n\temail = g@example.com\n");
        let run = configured.run(&Given::default(), env.clone(), true, "typed\n");
        assert_eq!((run.out.as_str(), run.warn.as_str()), ("", ""));
        assert_eq!(configured.value("user.name").as_deref(), Some("Global"));
        let own = std::fs::read_to_string(configured.root().join(".git/config")).unwrap();
        assert!(!own.contains("[user]"), "{own}");

        let flags = Fixture::new("");
        let given = Given {
            name: Some("Flag".to_owned()),
            email: None,
        };
        flags.run(&given, env, false, "");
        assert_eq!(flags.value("user.name").as_deref(), Some("Flag"));
        assert_eq!(
            flags.value("user.email").as_deref(),
            Some("env@example.com")
        );
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
    fn only_a_folder_outside_every_repository_gets_a_new_one() {
        let fixture = Fixture::new("");
        let root = fixture.root();
        assert!(!creates_repository(&fixture.git, &root));
        std::fs::create_dir(root.join("sub")).unwrap();
        assert!(!creates_repository(&fixture.git, &root.join("sub")));
        assert!(!creates_repository(
            &fixture.git,
            &root.join("sub/new/deeper")
        ));
        let outside = fixture.dir.path().join("outside");
        assert!(creates_repository(&fixture.git, &outside));
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join(".git"), "not a repository").unwrap();
        assert!(!creates_repository(&fixture.git, &outside));
        let (mut out, mut warn) = (Vec::new(), Vec::new());
        let given = Given {
            name: Some("Ana".to_owned()),
            email: None,
        };
        let identity = decide(
            &fixture.git,
            Plan {
                given: &given,
                env: &Environment::default(),
                interactive: true,
                dir: &root.join("sub"),
            },
            &mut "ana@example.com\n".as_bytes(),
            &mut out,
            &mut warn,
        );
        assert_eq!(identity, Identity::default());
        assert!(out.is_empty() && warn.is_empty());
    }

    #[test]
    fn a_failed_write_is_a_warning_that_names_no_value() {
        let fixture = Fixture::new("");
        let identity = Identity {
            name: Some("Ana".to_owned()),
            email: Some("ana@example.com".to_owned()),
        };
        let gone = fixture.dir.path().join("missing");
        let mut warn = Vec::new();
        store_in(&fixture.git, &gone, &identity, &mut warn);
        let text = String::from_utf8(warn).unwrap();
        assert!(text.contains("cannot write user.name"), "{text}");
        assert!(!text.contains("ana@example.com") && !text.contains("Ana"));
    }
}
