use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tasks::define_names;
use crate::{FoldError, codes};

/// The longest summary of an intent, in characters.
pub const MAX_SUMMARY_CHARS: usize = 1200;
/// The most parts an intent splits a change into.
pub const MAX_PARTS: usize = 12;
/// The longest name of a part, in characters.
pub const MAX_PART_NAME_CHARS: usize = 80;
/// The longest text of what a part does, in characters.
pub const MAX_PART_CHARS: usize = 800;
/// The longest entry of a contract kind, of the decisions or of what the change does not change, in characters.
pub const MAX_ENTRY_CHARS: usize = 300;
/// The most entries of one contract kind, of the decisions or of what the change does not change.
pub const MAX_ENTRIES: usize = 20;
/// The most file paths one coverage refusal names; its details count them all.
const MAX_LISTED_FILES: usize = 50;

define_names!(
    /// A kind of contract a change can add or change, in the order a reader sees them.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub enum ContractKind {
        Routes = "routes",
        Database = "database",
        Rights = "rights",
        Config = "config",
        Errors = "errors",
        Events = "events",
        Commands = "commands",
    }
);

/// The intent of a proposal version: how the change works in the code, so a human understands and judges it without reading every line, and the reviewer checks the lines against it. The summary and the parts are always there, and every changed file of the version belongs to exactly one part; the contracts, the decisions and what the change does not change are there only when they apply, so an empty list is a section that does not show.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalIntent {
    /// How the change works in the code: the flow, the states and the rules, in 2 to 4 sentences.
    pub summary: String,
    pub parts: Vec<IntentPart>,
    #[serde(default)]
    pub contracts: Contracts,
    /// The choices the author made and why, and the open questions with the message that asks them.
    #[serde(default)]
    pub decisions: Vec<String>,
    /// What a reader may think the change touches but it does not.
    #[serde(default)]
    pub not_changed: Vec<String>,
}

/// One part of the diff: a short name, what that part of the code does, and its files.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentPart {
    pub name: String,
    pub does: String,
    pub files: Vec<String>,
}

/// The contracts a change adds or changes, by kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contracts {
    #[serde(default)]
    pub routes: Vec<String>,
    #[serde(default)]
    pub database: Vec<String>,
    #[serde(default)]
    pub rights: Vec<String>,
    #[serde(default)]
    pub config: Vec<String>,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub events: Vec<String>,
    #[serde(default)]
    pub commands: Vec<String>,
}

impl Contracts {
    /// Each kind with its entries, in [`ContractKind::ALL`] order, empty kinds included.
    pub fn by_kind(&self) -> [(ContractKind, &[String]); 7] {
        [
            (ContractKind::Routes, &self.routes),
            (ContractKind::Database, &self.database),
            (ContractKind::Rights, &self.rights),
            (ContractKind::Config, &self.config),
            (ContractKind::Errors, &self.errors),
            (ContractKind::Events, &self.events),
            (ContractKind::Commands, &self.commands),
        ]
    }

    pub fn entries_mut(&mut self, kind: ContractKind) -> &mut Vec<String> {
        match kind {
            ContractKind::Routes => &mut self.routes,
            ContractKind::Database => &mut self.database,
            ContractKind::Rights => &mut self.rights,
            ContractKind::Config => &mut self.config,
            ContractKind::Errors => &mut self.errors,
            ContractKind::Events => &mut self.events,
            ContractKind::Commands => &mut self.commands,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.by_kind().iter().all(|(_, entries)| entries.is_empty())
    }
}

impl ProposalIntent {
    /// Every file the parts name, in part order.
    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.parts
            .iter()
            .flat_map(|part| part.files.iter().map(String::as_str))
    }
}

/// Checks the shape of an intent and returns it as it is stored: every text trimmed with each line break as `\n`, so it becomes part of the version's Git commit byte for byte. The summary and what a part does may hold line breaks and tabs; a part's name, its paths and every entry are one line. A refusal is `invalid_intent`, whose details name the `field` (such as `intent.parts[1].does`) and the `reason` and never repeat the text: `empty`, `too_long` (with `max_chars`), `control_character`, `no_parts`, `too_many_parts` or `too_many_entries` (with `max`), `duplicate_part`, or `file_in_two_parts` (with at most 50 of the paths in `files` and their number in `files_count`). `again` names the call that sends it again. Whether the parts cover the version's files is [`intent_coverage`], which needs the diff.
pub fn proposal_intent(intent: ProposalIntent, again: &str) -> Result<ProposalIntent, FoldError> {
    let summary = text(
        &intent.summary,
        "intent.summary",
        MAX_SUMMARY_CHARS,
        true,
        again,
    )?;
    if intent.parts.is_empty() {
        return Err(refusal(again, "intent.parts", "no_parts", []));
    }
    if intent.parts.len() > MAX_PARTS {
        return Err(refusal(
            again,
            "intent.parts",
            "too_many_parts",
            [("max", json!(MAX_PARTS))],
        ));
    }
    let mut parts = Vec::with_capacity(intent.parts.len());
    let mut names = BTreeSet::new();
    let mut owners: BTreeMap<String, usize> = BTreeMap::new();
    let mut twice = BTreeSet::new();
    for (index, part) in intent.parts.into_iter().enumerate() {
        let field = format!("intent.parts[{index}]");
        let name = text(
            &part.name,
            &format!("{field}.name"),
            MAX_PART_NAME_CHARS,
            false,
            again,
        )?;
        if !names.insert(name.clone()) {
            return Err(refusal(
                again,
                &format!("{field}.name"),
                "duplicate_part",
                [],
            ));
        }
        let does = text(
            &part.does,
            &format!("{field}.does"),
            MAX_PART_CHARS,
            true,
            again,
        )?;
        let mut files = Vec::with_capacity(part.files.len());
        for (position, path) in part.files.iter().enumerate() {
            let path = text(
                path,
                &format!("{field}.files[{position}]"),
                usize::MAX,
                false,
                again,
            )?;
            if owners.insert(path.clone(), index).is_some() {
                twice.insert(path.clone());
                continue;
            }
            files.push(path);
        }
        parts.push(IntentPart { name, does, files });
    }
    if !twice.is_empty() {
        let count = twice.len();
        let files: Vec<String> = twice.into_iter().take(MAX_LISTED_FILES).collect();
        return Err(refusal(
            again,
            "intent.parts",
            "file_in_two_parts",
            [("files", json!(files)), ("files_count", json!(count))],
        ));
    }
    let mut contracts = Contracts::default();
    for (kind, entries) in intent.contracts.by_kind() {
        *contracts.entries_mut(kind) = list(entries, &format!("intent.contracts.{kind}"), again)?;
    }
    Ok(ProposalIntent {
        summary,
        parts,
        contracts,
        decisions: list(&intent.decisions, "intent.decisions", again)?,
        not_changed: list(&intent.not_changed, "intent.not_changed", again)?,
    })
}

/// Checks that the parts of a checked intent cover the files the version changes against its base, `changed` as the diff names them (the new path of a rename): every changed file belongs to a part, no part names a file outside the diff, and every part names a file unless the version changes none. A refusal is `invalid_intent` with the reason `part_files_uncovered`, `part_file_outside_diff` or `part_without_files`; its details list both `uncovered` and `outside` paths, at most 50 of each, with `uncovered_count` and `outside_count`, and its hint names the first ones.
pub fn intent_coverage(
    intent: &ProposalIntent,
    changed: &[String],
    again: &str,
) -> Result<(), FoldError> {
    let named: BTreeSet<&str> = intent.files().collect();
    let changed_set: BTreeSet<&str> = changed.iter().map(String::as_str).collect();
    let uncovered: Vec<&str> = changed_set.difference(&named).copied().collect();
    let outside: Vec<&str> = named.difference(&changed_set).copied().collect();
    let reason = if !uncovered.is_empty() {
        "part_files_uncovered"
    } else if !outside.is_empty() {
        "part_file_outside_diff"
    } else if let Some(index) = intent
        .parts
        .iter()
        .position(|part| part.files.is_empty() && !changed.is_empty())
    {
        return Err(refusal(
            again,
            &format!("intent.parts[{index}].files"),
            "part_without_files",
            [],
        ));
    } else {
        return Ok(());
    };
    let listed = |paths: &[&str]| -> Vec<String> {
        paths
            .iter()
            .take(MAX_LISTED_FILES)
            .map(|path| (*path).to_owned())
            .collect()
    };
    let (message, hint) = if reason == "part_files_uncovered" {
        (
            format!(
                "{} changed files belong to no part of the intent",
                uncovered.len()
            ),
            format!(
                "add each of these files to the part whose code it changes, then call {again} again: {}",
                first_paths(&uncovered)
            ),
        )
    } else {
        (
            format!(
                "the intent names {} files that the version does not change",
                outside.len()
            ),
            format!(
                "remove these files from the parts, or change them in the workspace, then call {again} again: {}",
                first_paths(&outside)
            ),
        )
    };
    Err(FoldError::new(codes::INVALID_INTENT, message)
        .with_details([
            ("field", json!("intent.parts")),
            ("reason", json!(reason)),
            ("uncovered", json!(listed(&uncovered))),
            ("uncovered_count", json!(uncovered.len())),
            ("outside", json!(listed(&outside))),
            ("outside_count", json!(outside.len())),
        ])
        .with_hint(hint))
}

/// `invalid_intent` with the reason `missing`: the call left out the intent that a new version needs.
pub fn missing_intent(again: &str) -> FoldError {
    refusal(again, "intent", "missing", [])
}

fn first_paths(paths: &[&str]) -> String {
    const SHOWN: usize = 5;
    let mut text = paths
        .iter()
        .take(SHOWN)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > SHOWN {
        text.push_str(&format!(" and {} more", paths.len() - SHOWN));
    }
    text
}

/// A trimmed text with each line break as `\n`, refused when it is empty, longer than `max` characters or holds a control character: any but a line break or a tab when `lines` allows them, any at all otherwise.
fn text(raw: &str, field: &str, max: usize, lines: bool, again: &str) -> Result<String, FoldError> {
    let normal = raw.trim().replace("\r\n", "\n").replace('\r', "\n");
    let reason = if normal.is_empty() {
        "empty"
    } else if normal.chars().count() > max {
        return Err(refusal(
            again,
            field,
            "too_long",
            [("max_chars", json!(max))],
        ));
    } else if normal
        .chars()
        .any(|c| c.is_control() && !(lines && (c == '\n' || c == '\t')))
    {
        "control_character"
    } else {
        return Ok(normal);
    };
    Err(refusal(again, field, reason, []))
}

fn list(entries: &[String], field: &str, again: &str) -> Result<Vec<String>, FoldError> {
    if entries.len() > MAX_ENTRIES {
        return Err(refusal(
            again,
            field,
            "too_many_entries",
            [("max", json!(MAX_ENTRIES))],
        ));
    }
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            text(
                entry,
                &format!("{field}[{index}]"),
                MAX_ENTRY_CHARS,
                false,
                again,
            )
        })
        .collect()
}

/// An `invalid_intent` refusal whose details name the field, the reason and `extra`, such as the limit the value broke or the paths of a file reason; never the text.
fn refusal<const N: usize>(
    again: &str,
    field: &str,
    reason: &str,
    extra: [(&'static str, serde_json::Value); N],
) -> FoldError {
    let mut details = vec![("field", json!(field)), ("reason", json!(reason))];
    details.extend(extra);
    FoldError::new(
        codes::INVALID_INTENT,
        format!("{field}: {}", problem(reason)),
    )
    .with_details(details)
    .with_hint(format!(
        "call {again} again with `intent`: a summary of how the change works in the code (at most {MAX_SUMMARY_CHARS} characters) and 1 to {MAX_PARTS} parts, each with a name, what that part of the code does and its files, so every changed file is in exactly one part; add contracts, decisions and not_changed only when they apply"
    ))
}

fn problem(reason: &str) -> &'static str {
    match reason {
        "missing" => "a new version needs an intent",
        "empty" => "the text is empty",
        "too_long" => "the text is too long",
        "control_character" => {
            "the text holds a control character (only a summary or what a part does may hold line breaks and tabs)"
        }
        "no_parts" => "the intent needs at least one part",
        "too_many_parts" => "the intent has too many parts",
        "too_many_entries" => "the list has too many entries",
        "duplicate_part" => "two parts have the same name",
        "file_in_two_parts" => "a file belongs to more than one part",
        _ => "every changed file must belong to one part",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(name: &str, files: &[&str]) -> IntentPart {
        IntentPart {
            name: name.to_owned(),
            does: format!("{name} does its work."),
            files: files.iter().map(|path| (*path).to_owned()).collect(),
        }
    }

    fn intent(parts: Vec<IntentPart>) -> ProposalIntent {
        ProposalIntent {
            summary: "The board reads the cards and shows them by status.".to_owned(),
            parts,
            ..ProposalIntent::default()
        }
    }

    fn details(error: &FoldError) -> serde_json::Value {
        serde_json::Value::Object(error.details().unwrap().clone())
    }

    fn changed(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|path| (*path).to_owned()).collect()
    }

    #[test]
    fn a_checked_intent_is_trimmed_and_keeps_only_what_it_holds() {
        let raw = ProposalIntent {
            summary: "  Reads the cards.\r\n\r\nShows them by status.\r".to_owned(),
            parts: vec![IntentPart {
                name: " Board query ".to_owned(),
                does: "Reads:\n\t- the cards ".to_owned(),
                files: vec![" src/board.rs ".to_owned()],
            }],
            contracts: Contracts {
                routes: vec![" GET /board ".to_owned()],
                ..Contracts::default()
            },
            decisions: vec!["Status comes from the cache.".to_owned()],
            not_changed: Vec::new(),
        };
        let checked = proposal_intent(raw, "create_proposal").unwrap();
        assert_eq!(checked.summary, "Reads the cards.\n\nShows them by status.");
        assert_eq!(checked.parts[0].name, "Board query");
        assert_eq!(checked.parts[0].does, "Reads:\n\t- the cards");
        assert_eq!(checked.parts[0].files, ["src/board.rs"]);
        assert_eq!(checked.contracts.routes, ["GET /board"]);
        assert!(checked.contracts.database.is_empty());
        assert!(
            checked.not_changed.is_empty(),
            "an empty section stays empty"
        );
        let longest = ProposalIntent {
            summary: "é".repeat(MAX_SUMMARY_CHARS),
            parts: vec![IntentPart {
                name: "n".repeat(MAX_PART_NAME_CHARS),
                does: "d".repeat(MAX_PART_CHARS),
                files: vec!["a".to_owned()],
            }],
            decisions: vec!["x".repeat(MAX_ENTRY_CHARS); MAX_ENTRIES],
            ..ProposalIntent::default()
        };
        assert_eq!(
            proposal_intent(longest.clone(), "create_proposal").unwrap(),
            longest
        );
        let most = intent(
            (0..MAX_PARTS)
                .map(|n| part(&format!("p{n}"), &[]))
                .collect(),
        );
        assert_eq!(proposal_intent(most, "x").unwrap().parts.len(), MAX_PARTS);
    }

    #[test]
    fn a_misshapen_intent_names_the_field_and_the_reason_never_the_text() {
        let base = || intent(vec![part("Board", &["src/board.rs"])]);
        let cases: Vec<(ProposalIntent, serde_json::Value)> = vec![
            (
                ProposalIntent {
                    summary: " \n ".to_owned(),
                    ..base()
                },
                json!({"field": "intent.summary", "reason": "empty"}),
            ),
            (
                ProposalIntent {
                    summary: "x".repeat(MAX_SUMMARY_CHARS + 1),
                    ..base()
                },
                json!({"field": "intent.summary", "reason": "too_long", "max_chars": MAX_SUMMARY_CHARS}),
            ),
            (
                ProposalIntent {
                    summary: "Reads the \u{1b}[31mcards".to_owned(),
                    ..base()
                },
                json!({"field": "intent.summary", "reason": "control_character"}),
            ),
            (
                intent(Vec::new()),
                json!({"field": "intent.parts", "reason": "no_parts"}),
            ),
            (
                intent(
                    (0..=MAX_PARTS)
                        .map(|n| part(&format!("p{n}"), &[]))
                        .collect(),
                ),
                json!({"field": "intent.parts", "reason": "too_many_parts", "max": MAX_PARTS}),
            ),
            (
                intent(vec![part("Board\nquery", &[])]),
                json!({"field": "intent.parts[0].name", "reason": "control_character"}),
            ),
            (
                intent(vec![IntentPart {
                    does: "d".repeat(MAX_PART_CHARS + 1),
                    ..part("Board", &[])
                }]),
                json!({"field": "intent.parts[0].does", "reason": "too_long", "max_chars": MAX_PART_CHARS}),
            ),
            (
                intent(vec![part("Board", &["a"]), part("Board", &["b"])]),
                json!({"field": "intent.parts[1].name", "reason": "duplicate_part"}),
            ),
            (
                intent(vec![part("Board", &["a", " "])]),
                json!({"field": "intent.parts[0].files[1]", "reason": "empty"}),
            ),
            (
                intent(vec![part("Board", &["a", "b"]), part("Cards", &["b", "a"])]),
                json!({"field": "intent.parts", "reason": "file_in_two_parts", "files": ["a", "b"], "files_count": 2}),
            ),
            (
                ProposalIntent {
                    contracts: Contracts {
                        errors: vec!["case_open".to_owned(); MAX_ENTRIES + 1],
                        ..Contracts::default()
                    },
                    ..base()
                },
                json!({"field": "intent.contracts.errors", "reason": "too_many_entries", "max": MAX_ENTRIES}),
            ),
            (
                ProposalIntent {
                    decisions: vec!["ok".to_owned(), "x".repeat(MAX_ENTRY_CHARS + 1)],
                    ..base()
                },
                json!({"field": "intent.decisions[1]", "reason": "too_long", "max_chars": MAX_ENTRY_CHARS}),
            ),
            (
                ProposalIntent {
                    not_changed: vec!["Sign-in\nscreens".to_owned()],
                    ..base()
                },
                json!({"field": "intent.not_changed[0]", "reason": "control_character"}),
            ),
        ];
        for (raw, expected) in cases {
            let error = proposal_intent(raw, "update_proposal").unwrap_err();
            assert_eq!(error.code(), codes::INVALID_INTENT);
            assert_eq!(details(&error), expected);
            assert!(
                error
                    .hint()
                    .unwrap()
                    .starts_with("call update_proposal again with `intent`")
            );
        }
        let missing = missing_intent("submit_review_fix");
        assert_eq!(
            details(&missing),
            json!({"field": "intent", "reason": "missing"})
        );
        assert!(
            missing
                .hint()
                .unwrap()
                .starts_with("call submit_review_fix again")
        );
    }

    #[test]
    fn every_changed_file_belongs_to_exactly_one_part() {
        let two = intent(vec![
            part("Routes", &["src/routes.rs"]),
            part("Tests", &["tests/routes.rs"]),
        ]);
        assert_eq!(
            intent_coverage(&two, &changed(&["tests/routes.rs", "src/routes.rs"]), "x"),
            Ok(())
        );
        let missing = intent_coverage(
            &two,
            &changed(&["src/routes.rs", "tests/routes.rs", "web/api.ts"]),
            "create_proposal",
        )
        .unwrap_err();
        assert_eq!(
            details(&missing),
            json!({"field": "intent.parts", "reason": "part_files_uncovered", "uncovered": ["web/api.ts"], "uncovered_count": 1, "outside": [], "outside_count": 0})
        );
        assert!(
            missing
                .hint()
                .unwrap()
                .ends_with("call create_proposal again: web/api.ts")
        );
        let extra =
            intent_coverage(&two, &changed(&["src/routes.rs"]), "update_proposal").unwrap_err();
        assert_eq!(
            details(&extra),
            json!({"field": "intent.parts", "reason": "part_file_outside_diff", "uncovered": [], "uncovered_count": 0, "outside": ["tests/routes.rs"], "outside_count": 1})
        );
        let empty = intent(vec![part("Routes", &["src/routes.rs"]), part("Docs", &[])]);
        let error = intent_coverage(&empty, &changed(&["src/routes.rs"]), "x").unwrap_err();
        assert_eq!(
            details(&error),
            json!({"field": "intent.parts[1].files", "reason": "part_without_files"})
        );
        let nothing = intent(vec![part("Nothing", &[])]);
        assert_eq!(
            intent_coverage(&nothing, &[], "x"),
            Ok(()),
            "a version that changes no file"
        );
    }

    #[test]
    fn a_coverage_refusal_lists_at_most_fifty_paths_and_counts_them_all() {
        let one = intent(vec![part("Board", &[])]);
        let many: Vec<String> = (0..60).map(|n| format!("src/f{n:02}.rs")).collect();
        let error = intent_coverage(&one, &many, "create_proposal").unwrap_err();
        let details = details(&error);
        assert_eq!(details["uncovered"].as_array().unwrap().len(), 50);
        assert_eq!(details["uncovered_count"], 60);
        assert!(error.hint().unwrap().ends_with("and 55 more"));
        let twice = proposal_intent(
            intent(vec![
                part(
                    "Board",
                    &many.iter().map(String::as_str).collect::<Vec<_>>(),
                ),
                part(
                    "Cards",
                    &many.iter().map(String::as_str).collect::<Vec<_>>(),
                ),
            ]),
            "create_proposal",
        )
        .unwrap_err();
        let listed = self::details(&twice);
        assert_eq!(listed["reason"], "file_in_two_parts");
        assert_eq!(listed["files"].as_array().unwrap().len(), 50);
        assert_eq!(listed["files_count"], 60);
    }

    #[test]
    fn an_intent_round_trips_through_json_and_leaves_out_nothing() {
        let full = ProposalIntent {
            contracts: Contracts {
                rights: vec!["can_verify".to_owned()],
                ..Contracts::default()
            },
            decisions: vec!["One case per kind.".to_owned()],
            not_changed: vec!["Sign-in.".to_owned()],
            ..intent(vec![part("Board", &["a"])])
        };
        let text = serde_json::to_string(&full).unwrap();
        assert_eq!(serde_json::from_str::<ProposalIntent>(&text).unwrap(), full);
        let short: ProposalIntent =
            serde_json::from_value(json!({"summary": "s", "parts": []})).unwrap();
        assert!(short.contracts.is_empty() && short.decisions.is_empty());
        assert_eq!(ContractKind::parse("events"), Some(ContractKind::Events));
        assert_eq!(ContractKind::ALL.len(), full.contracts.by_kind().len());
    }
}
