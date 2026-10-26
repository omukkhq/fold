use crate::workspaces::tree::{EntryKind, FlatTree};

/// What `fld status` names as the next step for paths main changed that hold changes of the human's no revision records; Needs you shows the same words.
pub const FOLDER_HELD_NEXT: &str = "run `fld record`, or take the changes back";

/// What `fld status` names as the next step while the conflict markers of `fld unstash` wait in the folder; Needs you shows the same words.
pub const UNSTASH_CONFLICTED_NEXT: &str =
    "fix the conflict markers in these files, then run `fld record`";

/// How one path of the project folder differs from the entry fold knows the folder holds there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnrecordedKind {
    Added,
    /// Other content, or the executable bit set or cleared, as Git counts a mode change.
    Modified,
    Deleted,
    /// A symlink where a file was, or a file where a symlink was.
    TypeChanged,
}

impl UnrecordedKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::TypeChanged => "type_changed",
        }
    }
}

/// One path that `fld record` would record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnrecordedChange {
    pub path: Vec<u8>,
    pub kind: UnrecordedKind,
    /// The kind of the folder's entry, or of the record's for a deleted path.
    pub entry: EntryKind,
}

/// The paths where `folder` differs from `record`, in path order: what a record of the folder takes as the human's change. Entries are compared by kind and hash, so a file whose executable bit changed is modified.
pub fn unrecorded_changes(record: &FlatTree, folder: &FlatTree) -> Vec<UnrecordedChange> {
    let mut found = Vec::new();
    for (path, entry) in folder {
        let kind = match record.get(path) {
            None => UnrecordedKind::Added,
            Some(old) if old == entry => continue,
            Some(old) if is_link(old.kind) != is_link(entry.kind) => UnrecordedKind::TypeChanged,
            Some(_) => UnrecordedKind::Modified,
        };
        found.push(UnrecordedChange {
            path: path.clone(),
            kind,
            entry: entry.kind,
        });
    }
    for (path, entry) in record {
        if !folder.contains_key(path) {
            found.push(UnrecordedChange {
                path: path.clone(),
                kind: UnrecordedKind::Deleted,
                entry: entry.kind,
            });
        }
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

fn is_link(kind: EntryKind) -> bool {
    kind == EntryKind::Symlink
}

/// What keeps the project folder from following main, or the human's changes from reaching it, as the daemon read it. Each path list is in path order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderFacts {
    /// The paths main changed since the folder's revision that hold changes of the human's no revision records.
    pub held: Vec<String>,
    /// The conflicted paths of the human's change that waits for the main agent.
    pub human_change: Option<Vec<String>>,
    /// The paths where `fld unstash` wrote conflict markers that wait in the folder.
    pub unstash_conflicted: Option<Vec<String>>,
    /// The paths an unstash that stopped part-way writes once it is finished.
    pub unstash_open: Option<Vec<String>>,
    /// The command that brings back a Git branch or `HEAD` that shows a change fold did not make.
    pub head_moved: Option<String>,
    /// The Git branch holds commits of the human's that fold has not recorded yet.
    pub commits_waiting: bool,
}

/// One state that keeps the project folder where it is, or the human's changes off main, with the step that ends it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocker {
    HeadMoved { next: String },
    CommitsWaiting,
    UnstashOpen { paths: Vec<String> },
    UnstashConflicted { paths: Vec<String> },
    HumanChange { paths: Vec<String> },
    FolderHeld { paths: Vec<String> },
}

impl Blocker {
    /// The name the HTTP view gives the kind.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::HeadMoved { .. } => "head_moved",
            Self::CommitsWaiting => "commits_waiting",
            Self::UnstashOpen { .. } => "unstash_open",
            Self::UnstashConflicted { .. } => "unstash_conflicted",
            Self::HumanChange { .. } => "human_change",
            Self::FolderHeld { .. } => "folder_held",
        }
    }

    /// The paths the blocker is about; none for the Git branch.
    pub fn paths(&self) -> &[String] {
        match self {
            Self::HeadMoved { .. } | Self::CommitsWaiting => &[],
            Self::UnstashOpen { paths }
            | Self::UnstashConflicted { paths }
            | Self::HumanChange { paths }
            | Self::FolderHeld { paths } => paths,
        }
    }

    /// The step that ends the blocker, as a person runs or asks for it.
    pub fn next(&self) -> String {
        match self {
            Self::HeadMoved { next } => next.clone(),
            Self::CommitsWaiting => {
                "fold records them on its next run; `fld record` records them now".to_owned()
            }
            Self::UnstashOpen { .. } => "run `fld unstash` to finish it".to_owned(),
            Self::UnstashConflicted { .. } => UNSTASH_CONFLICTED_NEXT.to_owned(),
            Self::HumanChange { .. } => "ask the main agent to explain the conflict and prepare the merge for your OK, or run `fld stash` to take your change back".to_owned(),
            Self::FolderHeld { .. } => FOLDER_HELD_NEXT.to_owned(),
        }
    }
}

/// The blockers of `facts`, the one that decides first first: a Git branch that moved stops everything, an unstash that stopped part-way is finished before any other command goes on, and conflicts come before paths that merely wait. While a conflicted human change or an unstash conflict waits, it stands for the folder that waits on the human's changes, as in Needs you: it names the files, and its end lets the folder go on.
pub fn blockers(facts: &FolderFacts) -> Vec<Blocker> {
    let mut found = Vec::new();
    if let Some(next) = &facts.head_moved {
        found.push(Blocker::HeadMoved { next: next.clone() });
    }
    if facts.commits_waiting {
        found.push(Blocker::CommitsWaiting);
    }
    if let Some(paths) = &facts.unstash_open {
        found.push(Blocker::UnstashOpen {
            paths: paths.clone(),
        });
    }
    if let Some(paths) = &facts.unstash_conflicted {
        found.push(Blocker::UnstashConflicted {
            paths: paths.clone(),
        });
    }
    if let Some(paths) = &facts.human_change {
        found.push(Blocker::HumanChange {
            paths: paths.clone(),
        });
    }
    let conflicted = facts.unstash_conflicted.is_some() || facts.human_change.is_some();
    if !facts.held.is_empty() && !conflicted {
        found.push(Blocker::FolderHeld {
            paths: facts.held.clone(),
        });
    }
    found
}

/// Where the project folder is against main.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderPlace {
    /// The folder holds main's head, apart from the changes no revision records.
    AtMain,
    /// The folder holds an older revision; `why` is the kind of the first blocker, or `None` when nothing blocks it and fold brings it to main on its next follow.
    Behind { why: Option<&'static str> },
}

/// Whether the folder at `folder` holds main's `head`, and if not, which of `blockers` keeps it back.
pub fn place<R: PartialEq>(head: &R, folder: &R, blockers: &[Blocker]) -> FolderPlace {
    if head == folder {
        FolderPlace::AtMain
    } else {
        FolderPlace::Behind {
            why: blockers.first().map(Blocker::kind),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspaces::object::{ObjectKind, hash_object};
    use crate::workspaces::tree::FlatEntry;

    fn entry(kind: EntryKind, text: &str) -> FlatEntry {
        FlatEntry {
            kind,
            hash: hash_object(ObjectKind::Blob, text.as_bytes()),
        }
    }

    fn tree(entries: &[(&str, FlatEntry)]) -> FlatTree {
        entries
            .iter()
            .map(|(path, entry)| (path.as_bytes().to_vec(), *entry))
            .collect()
    }

    fn paths(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn each_kind_of_change_is_named_in_path_order() {
        let file = |text| entry(EntryKind::File, text);
        let record = tree(&[
            ("a.txt", file("a")),
            ("b.txt", file("b")),
            ("link", entry(EntryKind::Symlink, "a.txt")),
            ("run.sh", file("echo")),
            ("same.txt", file("same")),
            ("z.txt", file("z")),
        ]);
        let folder = tree(&[
            ("a.txt", file("a2")),
            ("c.txt", file("c")),
            ("link", file("a.txt")),
            ("run.sh", entry(EntryKind::Executable, "echo")),
            ("same.txt", file("same")),
            ("z.txt", entry(EntryKind::Symlink, "b.txt")),
        ]);
        let found: Vec<(String, UnrecordedKind, EntryKind)> = unrecorded_changes(&record, &folder)
            .into_iter()
            .map(|change| {
                (
                    String::from_utf8(change.path).unwrap(),
                    change.kind,
                    change.entry,
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                (
                    "a.txt".to_owned(),
                    UnrecordedKind::Modified,
                    EntryKind::File
                ),
                ("b.txt".to_owned(), UnrecordedKind::Deleted, EntryKind::File),
                ("c.txt".to_owned(), UnrecordedKind::Added, EntryKind::File),
                (
                    "link".to_owned(),
                    UnrecordedKind::TypeChanged,
                    EntryKind::File
                ),
                (
                    "run.sh".to_owned(),
                    UnrecordedKind::Modified,
                    EntryKind::Executable
                ),
                (
                    "z.txt".to_owned(),
                    UnrecordedKind::TypeChanged,
                    EntryKind::Symlink
                ),
            ]
        );
        assert!(unrecorded_changes(&record, &record).is_empty());
        assert_eq!(
            [
                UnrecordedKind::Added,
                UnrecordedKind::Modified,
                UnrecordedKind::Deleted,
                UnrecordedKind::TypeChanged
            ]
            .map(UnrecordedKind::as_str),
            ["added", "modified", "deleted", "type_changed"]
        );
    }

    #[test]
    fn a_clean_folder_has_no_blocker_and_is_at_main_or_follows_it() {
        let none = blockers(&FolderFacts::default());
        assert!(none.is_empty());
        assert_eq!(place(&3, &3, &none), FolderPlace::AtMain);
        assert_eq!(place(&4, &3, &none), FolderPlace::Behind { why: None });
    }

    #[test]
    fn the_blockers_come_in_the_order_that_decides_and_a_conflict_stands_for_the_held_paths() {
        let all = FolderFacts {
            held: paths(&["a.txt"]),
            human_change: Some(paths(&["b.txt"])),
            unstash_conflicted: Some(paths(&["c.txt"])),
            unstash_open: Some(paths(&["d.txt"])),
            head_moved: Some("run `git switch main` in the project folder".to_owned()),
            commits_waiting: true,
        };
        let found = blockers(&all);
        assert_eq!(
            found.iter().map(Blocker::kind).collect::<Vec<_>>(),
            [
                "head_moved",
                "commits_waiting",
                "unstash_open",
                "unstash_conflicted",
                "human_change"
            ]
        );
        assert_eq!(
            place(&5, &4, &found),
            FolderPlace::Behind {
                why: Some("head_moved")
            }
        );
        assert_eq!(
            found[0].next(),
            "run `git switch main` in the project folder"
        );
        assert!(found[0].paths().is_empty());
        assert!(found[1].next().contains("`fld record`"));
        assert_eq!(found[2].next(), "run `fld unstash` to finish it");
        assert_eq!(found[2].paths(), ["d.txt"]);
        assert_eq!(found[3].next(), UNSTASH_CONFLICTED_NEXT);
        assert!(found[4].next().contains("main agent"));
        assert!(found[4].next().contains("`fld stash`"));
        assert_eq!(found[4].paths(), ["b.txt"]);

        let held = FolderFacts {
            held: paths(&["a.txt", "b.txt"]),
            ..FolderFacts::default()
        };
        let found = blockers(&held);
        assert_eq!(
            found,
            [Blocker::FolderHeld {
                paths: paths(&["a.txt", "b.txt"])
            }]
        );
        assert_eq!(found[0].next(), FOLDER_HELD_NEXT);
        assert_eq!(
            place(&"r5", &"r4", &found),
            FolderPlace::Behind {
                why: Some("folder_held")
            }
        );
        let conflicted = FolderFacts {
            unstash_conflicted: Some(paths(&["a.txt"])),
            ..held
        };
        assert_eq!(
            blockers(&conflicted)
                .iter()
                .map(Blocker::kind)
                .collect::<Vec<_>>(),
            ["unstash_conflicted"]
        );
    }
}
