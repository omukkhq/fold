use std::collections::HashSet;
use std::time::Duration;

use regex::Regex;
use serde::Deserialize;

use crate::error::{FoldError, codes};
use crate::tasks::define_names;
use crate::tree::FlatEntry;

/// The checks file at the root of a tree. The checks that run on a tree are the ones its own copy of the file names, so a proposal changes its checks together with its code.
pub const CHECKS_FILE: &str = "fold-checks.toml";

/// How long one setup or check command may run before fold stops its process group.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// The most checks one file may name.
pub const MAX_CHECKS: usize = 64;

/// The largest checks file fold reads, in bytes. A larger file is refused before it is parsed, so a huge file in a tree cannot take the daemon's time and memory.
pub const MAX_FILE_BYTES: usize = 256 * 1024;

const MAX_NAME_CHARS: usize = 64;
const MAX_RUN_CHARS: usize = 4000;
const MAX_PATTERN_CHARS: usize = 1000;
/// A compiled `files` pattern may use at most this much memory, so a hostile pattern cannot take the daemon's memory.
const MAX_PATTERN_BYTES: usize = 1 << 20;

define_names!(
    /// When a check runs: before fold makes a proposal version, or on the merged tree before an integration reaches main.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum Stage {
        Proposal = "proposal",
        Integration = "integration",
    }
);

define_names!(
    /// What asked for a run: a worker's `run_checks`, the gate of `create_proposal`, `update_proposal` or a reviewer's `submit_review_fix`, or an integration.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum Purpose {
        Check = "check",
        CreateProposal = "create_proposal",
        UpdateProposal = "update_proposal",
        Integrate = "integrate",
        ReviewFix = "review_fix",
    }
);

define_names!(
    /// Where a run stands. A run is `running` from its start until one transaction writes its results and its end; `stopped` is a run that ended without a verdict, such as one a daemon stop cut short or one whose integration started again.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum RunState {
        Running = "running",
        Passed = "passed",
        Failed = "failed",
        Stopped = "stopped",
    }
);

define_names!(
    /// Why a run failed or stopped.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum RunReason {
        /// A required check failed or timed out.
        CheckFailed = "check_failed",
        /// The setup command failed or timed out, so no check ran.
        SetupFailed = "setup_failed",
        /// The checks file of the tree is not valid.
        ChecksFileInvalid = "checks_file_invalid",
        /// The recorded files after the run differ from the ones that were checked: the checks or an edit during the run changed them.
        FilesChanged = "files_changed",
        /// The workspace was synced, restored or replaced during the run, or could not be read.
        WorkspaceChanged = "workspace_changed",
        /// The command the run was for is refused now, such as a proposal that another one replaced or a claim that was lost.
        Refused = "refused",
        /// The daemon stopped during the run.
        DaemonStopped = "daemon_stopped",
        /// The integration started again on a newer state, with a run of its own.
        Superseded = "superseded",
        /// fold itself could not run the checks, such as when its check directory could not take the tree or a command could not start: the run says nothing of the change, and the integration can be asked again once the fault is fixed.
        FoldFailed = "fold_failed",
    }
);

define_names!(
    /// What one command of a run did. `skipped` is a check whose `files` pattern matched no changed path; `not_run` is a check after a failed setup.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum Outcome {
        Passed = "passed",
        Failed = "failed",
        TimedOut = "timed_out",
        Skipped = "skipped",
        NotRun = "not_run",
    }
);

impl Outcome {
    /// Whether the command did not pass: it failed or ran out of time.
    pub fn is_failure(self) -> bool {
        matches!(self, Outcome::Failed | Outcome::TimedOut)
    }
}

/// One `[[check]]` of the file. `files` limits the check to trees whose change touches a matching path; `required = false` shows the result and never blocks; `timeout` replaces [`CHECK_TIMEOUT`] for this check.
#[derive(Debug, Clone)]
pub struct Check {
    pub name: String,
    pub run: String,
    pub files: Option<Regex>,
    pub required: bool,
    /// The stages the check runs in, in the order the file lists them; never empty.
    pub stages: Vec<Stage>,
    pub timeout: Option<Duration>,
}

/// A parsed and checked `fold-checks.toml`. `setup` runs before the checks of every run, in the directory where they run.
#[derive(Debug, Clone, Default)]
pub struct ChecksFile {
    pub setup: Option<String>,
    pub checks: Vec<Check>,
}

/// A check of one stage with whether it runs for the changed paths.
#[derive(Debug, Clone, Copy)]
pub struct Planned<'a> {
    pub check: &'a Check,
    /// False when the check has a `files` pattern that no changed path matches.
    pub runs: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    setup: Option<RawSetup>,
    #[serde(default)]
    check: Vec<RawCheck>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSetup {
    run: String,
}

/// `stage` as the file writes it: one name or a list of names.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawStage {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCheck {
    name: String,
    run: String,
    files: Option<String>,
    required: Option<bool>,
    stage: Option<RawStage>,
    timeout: Option<toml::Spanned<String>>,
}

impl ChecksFile {
    /// Parses the file. Every key is known: a misspelt key such as `require` is refused rather than read as a default, because a default here decides whether a check blocks. The error is `checks_file_invalid`, whose message names what is wrong.
    pub fn parse(text: &str) -> Result<ChecksFile, FoldError> {
        if text.len() > MAX_FILE_BYTES {
            return Err(invalid(format!(
                "the file must be at most {} KiB",
                MAX_FILE_BYTES / 1024
            )));
        }
        let raw: RawFile = toml::from_str(text).map_err(|error| {
            let line = error.span().map(|span| line_of(text, span.start));
            match line {
                Some(line) => invalid(format!("line {line}: {}", error.message())),
                None => invalid(error.message().to_owned()),
            }
        })?;
        let setup = raw
            .setup
            .map(|setup| command(setup.run, "[setup] run"))
            .transpose()?;
        if raw.check.len() > MAX_CHECKS {
            return Err(invalid(format!("at most {MAX_CHECKS} checks are allowed")));
        }
        let mut names = HashSet::new();
        let mut checks = Vec::with_capacity(raw.check.len());
        for (index, check) in raw.check.into_iter().enumerate() {
            let at = format!("check {}", index + 1);
            let name = check_name(check.name, &at)?;
            if !names.insert(name.clone()) {
                return Err(invalid(format!("{at}: the name {name} is used twice")));
            }
            let run = command(check.run, &format!("{at} ({name}) run"))?;
            let files = check
                .files
                .map(|pattern| files_pattern(&pattern, &format!("{at} ({name}) files")))
                .transpose()?;
            let stages = parse_stages(check.stage, &format!("{at} ({name})"))?;
            let timeout = check
                .timeout
                .map(|value| {
                    parse_timeout(value.get_ref()).ok_or_else(|| {
                        invalid(format!(
                            "line {}: {at} ({name}) timeout is a whole number and s, m or h, such as \"90s\", \"10m\" or \"2h\", from 1s to {MAX_TIMEOUT_HOURS}h",
                            line_of(text, value.span().start)
                        ))
                    })
                })
                .transpose()?;
            checks.push(Check {
                name,
                run,
                files,
                required: check.required.unwrap_or(true),
                stages,
                timeout,
            });
        }
        Ok(ChecksFile { setup, checks })
    }

    /// The checks of `stage` in file order, each with whether it runs: a check without a pattern always runs, and one with a pattern runs when it matches at least one of `changed`, searched anywhere in the path.
    pub fn plan<'a>(&'a self, stage: Stage, changed: &[String]) -> Vec<Planned<'a>> {
        self.checks
            .iter()
            .filter(|check| check.stages.contains(&stage))
            .map(|check| Planned {
                check,
                runs: check
                    .files
                    .as_ref()
                    .is_none_or(|pattern| changed.iter().any(|path| pattern.is_match(path))),
            })
            .collect()
    }

    /// Whether the file names any check of `stage`, whatever the changed paths.
    pub fn has_stage(&self, stage: Stage) -> bool {
        self.checks
            .iter()
            .any(|check| check.stages.contains(&stage))
    }
}

/// Whether a plan runs anything. A plan with only skipped checks needs no run.
pub fn runs_any(plan: &[Planned<'_>]) -> bool {
    plan.iter().any(|planned| planned.runs)
}

/// Whether a change adds, changes or removes the checks file, from its entry in the base tree and in the changed tree.
pub fn checks_file_changed(base: Option<&FlatEntry>, tree: Option<&FlatEntry>) -> bool {
    base != tree
}

/// The stages of a check from its `stage` value: `proposal` when absent, otherwise one name or a list of distinct names.
fn parse_stages(raw: Option<RawStage>, at: &str) -> Result<Vec<Stage>, FoldError> {
    let names = match raw {
        None => return Ok(vec![Stage::Proposal]),
        Some(RawStage::One(name)) => vec![name],
        Some(RawStage::Many(names)) => names,
    };
    if names.is_empty() {
        return Err(invalid(format!(
            "{at}: stage is not empty; list proposal, integration or both"
        )));
    }
    let mut stages = Vec::with_capacity(names.len());
    for name in &names {
        let stage = Stage::parse(name).ok_or_else(|| {
            invalid(format!(
                "{at}: stage is proposal, integration or a list of both, not \"{name}\""
            ))
        })?;
        if stages.contains(&stage) {
            return Err(invalid(format!(
                "{at}: stage lists {} twice",
                stage.as_str()
            )));
        }
        stages.push(stage);
    }
    Ok(stages)
}

/// The longest `timeout` a check may have, in hours.
const MAX_TIMEOUT_HOURS: u64 = 24;

/// The 1-based line of the byte offset `at` in `text`.
fn line_of(text: &str, at: usize) -> usize {
    text.get(..at).map_or(0, |head| head.matches('\n').count()) + 1
}

/// A duration as a check's `timeout` writes it: a whole number followed by `s`, `m` or `h`, from one second to [`MAX_TIMEOUT_HOURS`] hours.
pub fn parse_timeout(text: &str) -> Option<Duration> {
    let unit = text.chars().last()?;
    let scale = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        _ => return None,
    };
    let number = &text[..text.len() - 1];
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) || number.len() > 6 {
        return None;
    }
    let seconds = number.parse::<u64>().ok()?.checked_mul(scale)?;
    (1..=MAX_TIMEOUT_HOURS * 3600)
        .contains(&seconds)
        .then(|| Duration::from_secs(seconds))
}

fn invalid(problem: String) -> FoldError {
    FoldError::new(
        codes::CHECKS_FILE_INVALID,
        format!("{CHECKS_FILE}: {problem}"),
    )
    .with_hint("fix fold-checks.toml in the workspace, then try again")
}

fn command(run: String, at: &str) -> Result<String, FoldError> {
    if run.trim().is_empty() {
        return Err(invalid(format!("{at} must not be empty")));
    }
    if run.chars().count() > MAX_RUN_CHARS {
        return Err(invalid(format!(
            "{at} must be at most {MAX_RUN_CHARS} characters"
        )));
    }
    Ok(run)
}

fn check_name(name: String, at: &str) -> Result<String, FoldError> {
    let fits = !name.is_empty()
        && name.chars().count() <= MAX_NAME_CHARS
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'));
    if fits {
        Ok(name)
    } else {
        Err(invalid(format!(
            "{at}: a name is 1 to {MAX_NAME_CHARS} letters, digits, '_', '-' or '.'"
        )))
    }
}

fn files_pattern(pattern: &str, at: &str) -> Result<Regex, FoldError> {
    if pattern.chars().count() > MAX_PATTERN_CHARS {
        return Err(invalid(format!(
            "{at} must be at most {MAX_PATTERN_CHARS} characters"
        )));
    }
    regex::RegexBuilder::new(pattern)
        .size_limit(MAX_PATTERN_BYTES)
        .build()
        .map_err(|_| invalid(format!("{at} is not a valid regular expression")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::ObjectHash;
    use crate::tree::EntryKind;
    use proptest::prelude::*;

    const EXAMPLE: &str = r#"
[setup]
run = "bun install --frozen-lockfile"

[[check]]
name  = "test"
run   = "bun run test"
files = "\\.(ts|tsx)$"

[[check]]
name     = "format"
run      = "bunx biome check ."
required = false

[[check]]
name  = "e2e"
run   = "bun run test:e2e"
stage = "integration"
"#;

    fn paths(list: &[&str]) -> Vec<String> {
        list.iter().map(|path| (*path).to_owned()).collect()
    }

    #[test]
    fn the_example_file_parses_with_its_defaults() {
        let file = ChecksFile::parse(EXAMPLE).unwrap();
        assert_eq!(file.setup.as_deref(), Some("bun install --frozen-lockfile"));
        let names: Vec<_> = file.checks.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["test", "format", "e2e"]);
        assert!(file.checks[0].required);
        assert_eq!(file.checks[0].stages, [Stage::Proposal]);
        assert!(!file.checks[1].required);
        assert_eq!(file.checks[2].stages, [Stage::Integration]);
        assert!(file.has_stage(Stage::Integration));
    }

    #[test]
    fn a_pattern_limits_its_check_to_matching_changes() {
        let file = ChecksFile::parse(EXAMPLE).unwrap();
        let plan = file.plan(Stage::Proposal, &paths(&["README.md"]));
        assert_eq!(plan.len(), 2);
        assert!(!plan[0].runs, "no .ts file changed");
        assert!(plan[1].runs, "a check without a pattern always runs");
        let plan = file.plan(Stage::Proposal, &paths(&["web/src/board.tsx"]));
        assert!(plan[0].runs);
        let plan = file.plan(Stage::Integration, &[]);
        assert_eq!(plan.len(), 1);
        assert!(runs_any(&plan));
    }

    #[test]
    fn a_stage_list_runs_the_check_in_each_listed_stage() {
        let file = ChecksFile::parse(
            "[[check]]\nname = \"both\"\nrun = \"x\"\nstage = [\"proposal\", \"integration\"]\n[[check]]\nname = \"one\"\nrun = \"y\"\nstage = [\"integration\"]\n[[check]]\nname = \"text\"\nrun = \"z\"\nstage = \"integration\"\n[[check]]\nname = \"plain\"\nrun = \"w\"\n",
        )
        .unwrap();
        assert_eq!(file.checks[0].stages, [Stage::Proposal, Stage::Integration]);
        assert_eq!(file.checks[1].stages, file.checks[2].stages);
        let name = |stage| -> Vec<&str> {
            file.plan(stage, &[])
                .iter()
                .map(|p| p.check.name.as_str())
                .collect()
        };
        assert_eq!(name(Stage::Proposal), ["both", "plain"]);
        assert_eq!(name(Stage::Integration), ["both", "one", "text"]);
        let only = ChecksFile::parse(
            "[[check]]\nname = \"a\"\nrun = \"x\"\nstage = [\"proposal\", \"integration\"]\n",
        )
        .unwrap();
        assert!(only.has_stage(Stage::Proposal) && only.has_stage(Stage::Integration));
    }

    #[test]
    fn an_empty_unknown_or_repeated_stage_names_its_check() {
        for (stage, part) in [
            ("[]", "not empty"),
            ("[\"later\"]", "not \"later\""),
            ("\"later\"", "not \"later\""),
            ("[\"proposal\", \"proposal\"]", "proposal twice"),
            (
                "[\"integration\", \"proposal\", \"integration\"]",
                "integration twice",
            ),
        ] {
            let text = format!("[[check]]\nname = \"t\"\nrun = \"x\"\nstage = {stage}\n");
            let error = ChecksFile::parse(&text).unwrap_err();
            assert_eq!(error.code(), codes::CHECKS_FILE_INVALID, "{stage}");
            assert!(
                error.message().contains("check 1 (t): stage"),
                "{}",
                error.message()
            );
            assert!(error.message().contains(part), "{}", error.message());
        }
        let error =
            ChecksFile::parse("[[check]]\nname = \"t\"\nrun = \"x\"\nstage = 3\n").unwrap_err();
        assert_eq!(error.code(), codes::CHECKS_FILE_INVALID);
    }

    #[test]
    fn an_empty_file_has_no_checks() {
        let file = ChecksFile::parse("").unwrap();
        assert!(file.setup.is_none());
        assert!(file.checks.is_empty());
        assert!(!runs_any(&file.plan(Stage::Proposal, &paths(&["a"]))));
    }

    #[test]
    fn unknown_keys_and_bad_values_are_refused() {
        for text in [
            "[[check]]\nname = \"t\"\nrun = \"x\"\nrequire = false\n",
            "[[check]]\nname = \"t\"\nrun = \"x\"\nstage = \"later\"\n",
            "[[check]]\nname = \"t\"\nrun = \"  \"\n",
            "[[check]]\nname = \"has space\"\nrun = \"x\"\n",
            "[[check]]\nname = \"\"\nrun = \"x\"\n",
            "[[check]]\nname = \"t\"\nrun = \"x\"\n[[check]]\nname = \"t\"\nrun = \"y\"\n",
            "[[check]]\nname = \"t\"\nrun = \"x\"\nfiles = \"(\"\n",
            "[setup]\nrun = \"\"\n",
            "[setup]\nrun = \"x\"\ncache = true\n",
            "[[check]]\nrun = \"x\"\n",
            "checks = 1\n",
            "[[check]\n",
        ] {
            let error = ChecksFile::parse(text).unwrap_err();
            assert_eq!(error.code(), codes::CHECKS_FILE_INVALID, "{text}");
            assert!(error.message().starts_with(CHECKS_FILE));
        }
        let error = ChecksFile::parse("[[check]]\nname = \"t\"\nrun = \"x\"\nrequire = false\n")
            .unwrap_err();
        assert!(error.message().contains("line 4"), "{}", error.message());
    }

    #[test]
    fn a_timeout_is_a_number_and_a_unit_and_a_bad_one_names_its_line() {
        let file = ChecksFile::parse(
            "[[check]]\nname = \"e2e\"\nrun = \"x\"\ntimeout = \"10m\"\n[[check]]\nname = \"t\"\nrun = \"y\"\n",
        )
        .unwrap();
        assert_eq!(file.checks[0].timeout, Some(Duration::from_secs(600)));
        assert_eq!(file.checks[1].timeout, None);
        assert_eq!(parse_timeout("90s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_timeout("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_timeout("24h"), Some(Duration::from_secs(86_400)));
        for bad in [
            "10", "0s", "25h", "10 m", "ten", "m", "1.5h", "-1s", "", "9999999s",
        ] {
            assert_eq!(parse_timeout(bad), None, "{bad}");
        }
        let error = ChecksFile::parse(
            "[[check]]\nname = \"e2e\"\nrun = \"x\"\ntimeout = \"ten minutes\"\n",
        )
        .unwrap_err();
        assert_eq!(error.code(), codes::CHECKS_FILE_INVALID);
        assert!(error.message().contains("line 4"), "{}", error.message());
        assert!(
            error.message().contains("(e2e) timeout"),
            "{}",
            error.message()
        );
        let number = ChecksFile::parse("[[check]]\nname = \"e2e\"\nrun = \"x\"\ntimeout = 600\n")
            .unwrap_err();
        assert!(number.message().contains("line 4"), "{}", number.message());
    }

    #[test]
    fn too_many_checks_are_refused() {
        let text: String = (0..=MAX_CHECKS)
            .map(|n| format!("[[check]]\nname = \"c{n}\"\nrun = \"true\"\n"))
            .collect();
        assert_eq!(
            ChecksFile::parse(&text).unwrap_err().code(),
            codes::CHECKS_FILE_INVALID
        );
    }

    #[test]
    fn a_file_over_the_size_limit_is_refused() {
        let comment = format!("# {}\n", "x".repeat(MAX_FILE_BYTES));
        let error = ChecksFile::parse(&comment).unwrap_err();
        assert_eq!(error.code(), codes::CHECKS_FILE_INVALID);
        assert!(
            error.message().contains("at most 256 KiB"),
            "{}",
            error.message()
        );
        let fits = format!("# {}\n", "x".repeat(MAX_FILE_BYTES - 4));
        assert!(ChecksFile::parse(&fits).unwrap().checks.is_empty());
    }

    #[test]
    fn a_long_run_or_pattern_is_refused() {
        let run = "x".repeat(MAX_RUN_CHARS + 1);
        let text = format!("[[check]]\nname = \"t\"\nrun = \"{run}\"\n");
        assert!(ChecksFile::parse(&text).is_err());
        let files = "a".repeat(MAX_PATTERN_CHARS + 1);
        let text = format!("[[check]]\nname = \"t\"\nrun = \"x\"\nfiles = \"{files}\"\n");
        assert!(ChecksFile::parse(&text).is_err());
    }

    #[test]
    fn the_names_round_trip() {
        for stage in Stage::ALL {
            assert_eq!(Stage::parse(stage.as_str()), Some(stage));
        }
        assert_eq!(Stage::parse("main"), None);
        for purpose in Purpose::ALL {
            assert_eq!(Purpose::parse(purpose.as_str()), Some(purpose));
        }
        for state in RunState::ALL {
            assert_eq!(RunState::parse(state.as_str()), Some(state));
        }
        for reason in RunReason::ALL {
            assert_eq!(RunReason::parse(reason.as_str()), Some(reason));
        }
        for outcome in Outcome::ALL {
            assert_eq!(Outcome::parse(outcome.as_str()), Some(outcome));
        }
        assert!(Outcome::TimedOut.is_failure() && Outcome::Failed.is_failure());
        assert!(!Outcome::Skipped.is_failure() && !Outcome::NotRun.is_failure());
    }

    #[test]
    fn a_change_of_the_file_entry_is_a_change_of_the_checks() {
        let entry = |byte| FlatEntry {
            kind: EntryKind::File,
            hash: ObjectHash::from_bytes([byte; 32]),
        };
        assert!(!checks_file_changed(None, None));
        assert!(!checks_file_changed(Some(&entry(1)), Some(&entry(1))));
        assert!(checks_file_changed(None, Some(&entry(1))));
        assert!(checks_file_changed(Some(&entry(1)), None));
        assert!(checks_file_changed(Some(&entry(1)), Some(&entry(2))));
    }

    proptest! {
        #[test]
        fn parse_never_panics(text in ".{0,400}") {
            let _ = ChecksFile::parse(&text);
        }

        #[test]
        fn a_generated_file_parses_and_plans_every_check(
            checks in proptest::collection::vec(("[a-z][a-z0-9_-]{0,12}", any::<bool>(), any::<bool>()), 0..8),
            changed in proptest::collection::vec("[a-z]{1,8}\\.(rs|ts|md)", 0..5),
        ) {
            let mut seen = HashSet::new();
            let mut text = String::new();
            for (name, required, integration) in &checks {
                if !seen.insert(name.clone()) {
                    continue;
                }
                let stage = if *integration { "integration" } else { "proposal" };
                text.push_str(&format!(
                    "[[check]]\nname = \"{name}\"\nrun = \"true\"\nrequired = {required}\nstage = \"{stage}\"\n"
                ));
            }
            let file = ChecksFile::parse(&text).unwrap();
            prop_assert_eq!(file.checks.len(), seen.len());
            let proposal = file.plan(Stage::Proposal, &changed);
            let integration = file.plan(Stage::Integration, &changed);
            prop_assert_eq!(proposal.len() + integration.len(), file.checks.len());
            prop_assert!(proposal.iter().chain(&integration).all(|planned| planned.runs));
        }

        #[test]
        fn a_pattern_runs_exactly_when_a_path_matches(
            changed in proptest::collection::vec("[a-z]{1,6}\\.(rs|ts)", 0..6),
        ) {
            let file = ChecksFile::parse(
                "[[check]]\nname = \"rust\"\nrun = \"cargo test\"\nfiles = \"\\\\.rs$\"\n",
            )
            .unwrap();
            let plan = file.plan(Stage::Proposal, &changed);
            prop_assert_eq!(plan[0].runs, changed.iter().any(|path| path.ends_with(".rs")));
        }
    }
}
