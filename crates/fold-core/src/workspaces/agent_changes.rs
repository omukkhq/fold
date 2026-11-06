use std::collections::BTreeSet;
use std::time::Duration;

use crate::agents::AgentName;
use crate::layout::MCP_SERVER_NAME;
use crate::time::Timestamp;
use crate::workspaces::object::{ObjectHash, ObjectKind, hash_object};
use crate::workspaces::tree::{EntryKind, FlatTree};

/// How many empty files one record may add before it asks for a confirmation. A person adds one or two at a time, such as an `__init__.py` or a `.gitkeep`; a shell command that opens each word of a list as a file, as an unquoted `$(...)` in a zsh redirect does, adds one per word, and the smallest such slip seen made 26.
pub const MANY_EMPTY_FILES: usize = 10;

/// What Needs you and `fld status` name as the next step for a change that a worker made in the project folder.
pub const AGENT_CHANGE_NEXT: &str = "run `fld record` to keep it as your change, `fld agent-changes restore` to put it aside in the stash, or `fld agent-changes ignore` to treat it as your own unrecorded change";

/// How far a file time may lie outside a tool call and still count as made during it: the supervisor reads a tool call's start and end from the worker's output a moment after Claude Code writes them, and some file systems keep times to the second.
pub const WINDOW_GRACE: Duration = Duration::from_secs(1);

/// The tools that only read or ask, whose calls never write the project folder. `AskUserQuestion` can wait a long time for a person, who may edit the folder meanwhile.
const READ_ONLY_TOOLS: [&str; 12] = [
    "Read",
    "Glob",
    "Grep",
    "LS",
    "WebFetch",
    "WebSearch",
    "TodoWrite",
    "ToolSearch",
    "AskUserQuestion",
    "ListAgents",
    "EnterPlanMode",
    "ExitPlanMode",
];

/// Whether a call of `tool` can write files, so the project folder's changes during it count as the worker's. fold's own tools write only through fold's commands.
pub fn may_write_files(tool: &str) -> bool {
    let fold_prefix = format!("mcp__{MCP_SERVER_NAME}__");
    !READ_ONLY_TOOLS.contains(&tool) && !tool.starts_with(&fold_prefix)
}

/// One tool call of a supervised worker, by the system clock that file times use; `end` is `None` while the call runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolWindow {
    pub agent: AgentName,
    pub start: Timestamp,
    pub end: Option<Timestamp>,
}

impl ToolWindow {
    fn covers(&self, at: Timestamp) -> bool {
        let start = self
            .start
            .checked_sub(WINDOW_GRACE)
            .unwrap_or(Timestamp::EPOCH);
        let end = self.end.and_then(|end| end.checked_add(WINDOW_GRACE));
        at >= start && end.is_none_or(|end| at <= end)
    }
}

/// The agents whose tool calls ran when a file changed at `at`, in name order: none means a person made the change, as far as fold can tell.
pub fn agents_at(windows: &[ToolWindow], at: Timestamp) -> Vec<AgentName> {
    windows
        .iter()
        .filter(|window| window.covers(at))
        .map(|window| window.agent.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The parts of a change of the project folder that make a record ask first, each in path order: tracked files it empties and empty files it adds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordRisk {
    /// Files the record holds with content that the folder holds empty.
    pub emptied: Vec<String>,
    /// Files the record does not hold that the folder holds empty.
    pub empty_added: Vec<String>,
}

impl RecordRisk {
    /// Whether the record asks for a confirmation: any emptied file, or [`MANY_EMPTY_FILES`] new empty files or more.
    pub fn needs_confirmation(&self) -> bool {
        !self.emptied.is_empty() || self.empty_added.len() >= MANY_EMPTY_FILES
    }
}

fn empty_blob() -> ObjectHash {
    hash_object(ObjectKind::Blob, b"")
}

fn is_file(kind: EntryKind) -> bool {
    matches!(kind, EntryKind::File | EntryKind::Executable)
}

/// What a record of `folder` over `record` empties and adds empty. Paths that are not UTF-8 are named lossily.
pub fn record_risk(record: &FlatTree, folder: &FlatTree) -> RecordRisk {
    let empty = empty_blob();
    let mut risk = RecordRisk::default();
    for (path, entry) in folder {
        if !is_file(entry.kind) || entry.hash != empty {
            continue;
        }
        let name = String::from_utf8_lossy(path).into_owned();
        match record.get(path) {
            None => risk.empty_added.push(name),
            Some(old) if is_file(old.kind) && old.hash != empty => risk.emptied.push(name),
            Some(_) => {}
        }
    }
    risk
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspaces::tree::FlatEntry;

    fn file(text: &str) -> FlatEntry {
        FlatEntry {
            kind: EntryKind::File,
            hash: hash_object(ObjectKind::Blob, text.as_bytes()),
        }
    }

    fn tree(entries: &[(&str, FlatEntry)]) -> FlatTree {
        entries
            .iter()
            .map(|(path, entry)| (path.as_bytes().to_vec(), *entry))
            .collect()
    }

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis).unwrap()
    }

    fn agent(name: &str) -> AgentName {
        AgentName::parse(name).unwrap()
    }

    #[test]
    fn an_emptied_file_asks_at_once_and_new_empty_files_ask_from_the_threshold() {
        let record = tree(&[
            ("plan.md", file("the plan")),
            ("empty.txt", file("")),
            ("keep.md", file("kept")),
        ]);
        let mut folder = tree(&[
            ("plan.md", file("")),
            ("empty.txt", file("")),
            ("keep.md", file("kept")),
            ("pkg/__init__.py", file("")),
        ]);
        let risk = record_risk(&record, &folder);
        assert_eq!(risk.emptied, ["plan.md"]);
        assert_eq!(risk.empty_added, ["pkg/__init__.py"]);
        assert!(risk.needs_confirmation());

        folder.insert(b"plan.md".to_vec(), file("a new plan"));
        let one = record_risk(&record, &folder);
        assert!(one.emptied.is_empty());
        assert!(!one.needs_confirmation());

        for n in 1..MANY_EMPTY_FILES {
            folder.insert(format!("junk{n}").into_bytes(), file(""));
        }
        let many = record_risk(&record, &folder);
        assert_eq!(many.empty_added.len(), MANY_EMPTY_FILES);
        assert!(many.needs_confirmation());
        folder.remove(b"junk1".as_slice());
        assert!(!record_risk(&record, &folder).needs_confirmation());
    }

    #[test]
    fn a_symlink_never_counts_as_an_empty_file() {
        let link = FlatEntry {
            kind: EntryKind::Symlink,
            hash: empty_blob(),
        };
        let record = tree(&[("a", file("text"))]);
        let folder = tree(&[("a", link), ("b", link)]);
        assert_eq!(record_risk(&record, &folder), RecordRisk::default());
    }

    #[test]
    fn a_change_belongs_to_the_agents_whose_calls_ran_then_with_the_grace_on_each_side() {
        let windows = [
            ToolWindow {
                agent: agent("reviewer-35"),
                start: at(10_000),
                end: Some(at(20_000)),
            },
            ToolWindow {
                agent: agent("backend"),
                start: at(15_000),
                end: None,
            },
            ToolWindow {
                agent: agent("reviewer-35"),
                start: at(16_000),
                end: Some(at(17_000)),
            },
        ];
        assert!(agents_at(&windows, at(8_999)).is_empty());
        assert_eq!(agents_at(&windows, at(9_000)), [agent("reviewer-35")]);
        assert_eq!(
            agents_at(&windows, at(16_500)),
            [agent("backend"), agent("reviewer-35")]
        );
        assert_eq!(
            agents_at(&windows, at(21_000)),
            [agent("backend"), agent("reviewer-35")]
        );
        assert_eq!(agents_at(&windows, at(21_001)), [agent("backend")]);
        assert_eq!(agents_at(&windows, at(99_000)), [agent("backend")]);
        assert!(agents_at(&windows[..1], at(21_001)).is_empty());
        assert!(agents_at(&[], at(1)).is_empty());
    }

    #[test]
    fn only_tools_that_can_write_files_count() {
        for tool in [
            "Bash",
            "Write",
            "Edit",
            "NotebookEdit",
            "Task",
            "mcp__other__run",
        ] {
            assert!(may_write_files(tool), "{tool}");
        }
        for tool in ["Read", "Grep", "AskUserQuestion", "mcp__fold__get_task"] {
            assert!(!may_write_files(tool), "{tool}");
        }
    }
}
