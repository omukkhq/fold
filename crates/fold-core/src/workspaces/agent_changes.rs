use std::collections::BTreeSet;
use std::time::Duration;

use crate::agents::AgentName;
use crate::layout::{FILE_TOOLS, MCP_SERVER_NAME};
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

/// Whether a call of `tool` can write files where no guard looks, so the project folder's changes during it count as the worker's: a shell command, a subagent or another server's tool. The file tools of [`FILE_TOOLS`] are left out, because the edit guard refuses their writes outside the workspaces before they run, and so are fold's own tools, which write only through fold's commands. A call fold leaves out never holds a person's edit made meanwhile.
pub fn may_write_files(tool: &str) -> bool {
    let fold_prefix = format!("mcp__{MCP_SERVER_NAME}__");
    !READ_ONLY_TOOLS.contains(&tool)
        && !FILE_TOOLS.split('|').any(|file_tool| file_tool == tool)
        && !tool.starts_with(&fold_prefix)
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

/// The paths of the open marks of workers' changes that a follow of the project folder settles: those its read no longer shows changed, among the marks placed before that read began. Each mark of `open` comes with its number, which grows in the order the marks were placed, and `placed_before_read` is the newest number when the follow began to read the folder; a mark placed while the follow read, for a file its read may not show, stays open, so a worker's change never passes as the human's for a stale read. A path at or below one of `unread`, which the read left out, such as a file over the size limit, an unreadable one or one that kept changing, stays open too: the read keeps its recorded entry without seeing the file, so it says nothing about what the worker left there.
pub fn settled_by_follow<'a>(
    open: impl IntoIterator<Item = (i64, &'a [u8])>,
    still_changed: &BTreeSet<&[u8]>,
    unread: &[&[u8]],
    placed_before_read: i64,
) -> BTreeSet<Vec<u8>> {
    open.into_iter()
        .filter(|(number, path)| {
            *number <= placed_before_read
                && !still_changed.contains(path)
                && !unread.iter().any(|left_out| at_or_below(path, left_out))
        })
        .map(|(_, path)| path.to_vec())
        .collect()
}

/// Whether `path` is `ancestor` or a path below it.
fn at_or_below(path: &[u8], ancestor: &[u8]) -> bool {
    path.strip_prefix(ancestor)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(b"/"))
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
    fn a_follow_settles_only_the_marks_placed_before_its_read() {
        let changed: BTreeSet<&[u8]> = [b"kept.md".as_slice()].into();
        let open = [
            (1, b"kept.md".as_slice()),
            (2, b"back.md".as_slice()),
            (3, b"new.md".as_slice()),
        ];
        let settled = settled_by_follow(open, &changed, &[], 2);
        assert_eq!(settled, [b"back.md".to_vec()].into());
        assert!(settled_by_follow(open, &changed, &[], 0).is_empty());
        assert_eq!(settled_by_follow(open, &BTreeSet::new(), &[], 3).len(), 3);
    }

    #[test]
    fn a_follow_never_settles_a_path_its_read_left_out() {
        let open = [
            (1, b"big.bin".as_slice()),
            (2, b"docs/huge.pdf".as_slice()),
            (3, b"docs2/a.md".as_slice()),
            (4, b"back.md".as_slice()),
        ];
        let settled = settled_by_follow(
            open,
            &BTreeSet::new(),
            &[b"big.bin".as_slice(), b"docs".as_slice()],
            4,
        );
        assert_eq!(
            settled,
            [b"back.md".to_vec(), b"docs2/a.md".to_vec()].into()
        );
        assert!(at_or_below(b"docs/huge.pdf", b"docs"));
        assert!(at_or_below(b"docs", b"docs"));
        assert!(!at_or_below(b"docs2/a.md", b"docs"));
    }

    #[test]
    fn only_tools_that_can_write_files_count() {
        for tool in ["Bash", "Monitor", "Task", "Agent", "mcp__other__run"] {
            assert!(may_write_files(tool), "{tool}");
        }
        // The edit guard confines the file tools, and fold's tools write through its commands.
        for tool in [
            "Read",
            "Grep",
            "AskUserQuestion",
            "Write",
            "Edit",
            "MultiEdit",
            "NotebookEdit",
            "mcp__fold__get_task",
        ] {
            assert!(!may_write_files(tool), "{tool}");
        }
    }
}
