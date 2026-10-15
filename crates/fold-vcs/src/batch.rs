use std::collections::{BTreeSet, HashMap};

use fold_core::merge::{Conflict, OriginRun, line_origins, merge_trees};
use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::tree::{EntryKind, FlatEntry, FlatTree, flatten};
use fold_core::{FoldError, codes};

use crate::diff::is_binary;
use crate::objects::ObjectStore;
use crate::revision_store::write_initial_tree;
use crate::snapshot::load_tree;

/// One proposal version to merge: the tree of its base revision and its own tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchMember {
    pub base: ObjectHash,
    pub tree: ObjectHash,
}

/// The members that brought the lines of one changed text file into the merged tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOrigins {
    pub path: String,
    pub runs: Vec<OriginRun>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchMerge {
    /// Every object of the tree is in the store. `origins` lists, in path order, each text file a member changed with the runs of lines each member brought.
    Merged {
        tree_hash: ObjectHash,
        origins: Vec<FileOrigins>,
    },
    /// The member at index `member` does not merge onto main with the members before it. `with` is the earlier member it conflicts with, or `None` when the conflict is with main itself. Nothing was written.
    Conflicted {
        member: usize,
        with: Option<usize>,
        conflicts: Vec<Conflict>,
    },
}

/// Merges `members` one after another onto main's tree `main`, each against its own base, with the three-way tree merge an integration uses, so the result is what integrating them in this order gives. The merges run in memory; only a clean result is written, its merged blobs first and then its trees bottom-up. Blocking: call it through `spawn_blocking`.
pub fn merge_batch(
    objects: &ObjectStore,
    main: &ObjectHash,
    members: &[BatchMember],
) -> Result<BatchMerge, FoldError> {
    let load = |hash: &ObjectHash| load_tree(objects, hash);
    let mut texts = Texts {
        objects,
        merged: HashMap::new(),
        cache: HashMap::new(),
    };
    let mut steps: Vec<FlatTree> = vec![flatten(main, &load)?];
    let mut versions: Vec<(FlatTree, FlatTree)> = Vec::with_capacity(members.len());
    for (index, member) in members.iter().enumerate() {
        let base = flatten(&member.base, &load)?;
        let ours = flatten(&member.tree, &load)?;
        let current = steps.last().cloned().unwrap_or_default();
        let next = if base == current {
            ours.clone()
        } else {
            let merged = &texts.merged;
            let blob = |hash: &ObjectHash| read_blob(objects, merged, hash);
            let merge = merge_trees(&base, &ours, &current, &blob)?;
            if !merge.conflicts.is_empty() {
                let with = conflicting_member(&mut texts, &steps, &versions, &merge.conflicts)?;
                return Ok(BatchMerge::Conflicted {
                    member: index,
                    with,
                    conflicts: merge.conflicts,
                });
            }
            texts.merged.extend(merge.new_blobs);
            merge.merged
        };
        versions.push((base, ours));
        steps.push(next);
    }
    let last = steps.last().cloned().unwrap_or_default();
    let used: BTreeSet<ObjectHash> = last.values().map(|entry| entry.hash).collect();
    for (hash, bytes) in &texts.merged {
        if !used.contains(hash) {
            continue;
        }
        let written = objects.write(ObjectKind::Blob, bytes)?;
        if written != *hash {
            return Err(FoldError::new(
                codes::OBJECT_CORRUPT,
                format!("the merged blob {hash} was stored as {written}"),
            ));
        }
    }
    let origins = origins(&mut texts, &steps, &versions)?;
    let tree_hash = write_initial_tree(objects, last)?;
    Ok(BatchMerge::Merged { tree_hash, origins })
}

/// The text of blobs, from the line merges of this call or from the store, read once each. A binary file has none.
struct Texts<'a> {
    objects: &'a ObjectStore,
    merged: HashMap<ObjectHash, Vec<u8>>,
    cache: HashMap<ObjectHash, Option<String>>,
}

impl Texts<'_> {
    fn of(&mut self, entry: Option<&FlatEntry>) -> Result<Option<String>, FoldError> {
        let Some(entry) = entry.filter(|entry| is_file(entry.kind)) else {
            return Ok(None);
        };
        if let Some(text) = self.cache.get(&entry.hash) {
            return Ok(text.clone());
        }
        let bytes = read_blob(self.objects, &self.merged, &entry.hash)?;
        let text = (!is_binary(&bytes))
            .then(|| String::from_utf8(bytes).ok())
            .flatten();
        self.cache.insert(entry.hash, text.clone());
        Ok(text)
    }

    /// The runs of the last step's `path` that each member brought, through `steps` and the members' own `versions`.
    fn runs(
        &mut self,
        path: &[u8],
        steps: &[FlatTree],
        versions: &[(FlatTree, FlatTree)],
    ) -> Result<Vec<OriginRun>, FoldError> {
        let step_texts = steps
            .iter()
            .map(|step| self.of(step.get(path)))
            .collect::<Result<Vec<_>, _>>()?;
        if step_texts.last().is_none_or(Option::is_none) {
            return Ok(Vec::new());
        }
        let member_texts = versions
            .iter()
            .map(|(_, tree)| self.of(tree.get(path)))
            .collect::<Result<Vec<_>, _>>()?;
        let steps: Vec<Option<&str>> = step_texts.iter().map(Option::as_deref).collect();
        let members: Vec<Option<&str>> = member_texts.iter().map(Option::as_deref).collect();
        Ok(line_origins(&steps, &members))
    }
}

fn read_blob(
    objects: &ObjectStore,
    merged: &HashMap<ObjectHash, Vec<u8>>,
    hash: &ObjectHash,
) -> Result<Vec<u8>, FoldError> {
    match merged.get(hash) {
        Some(bytes) => Ok(bytes.clone()),
        None => Ok(objects.read_expecting(hash, ObjectKind::Blob)?.payload),
    }
}

/// The earlier member a conflict is with: the one that brought the merged lines of a conflicting hunk, else the latest one whose own version changed a conflicted path. `None` means only main changed the paths.
fn conflicting_member(
    texts: &mut Texts<'_>,
    steps: &[FlatTree],
    versions: &[(FlatTree, FlatTree)],
    conflicts: &[Conflict],
) -> Result<Option<usize>, FoldError> {
    for conflict in conflicts
        .iter()
        .filter(|conflict| !conflict.hunks.is_empty())
    {
        let runs = texts.runs(&conflict.path, steps, versions)?;
        for hunk in &conflict.hunks {
            let start = u32::try_from(hunk.theirs_start.saturating_add(1)).unwrap_or(u32::MAX);
            let len = u32::try_from(hunk.theirs.len().max(1)).unwrap_or(u32::MAX);
            let end = start.saturating_add(len - 1);
            if let Some(run) = runs
                .iter()
                .find(|run| run.start_line <= end && start <= run.end_line)
            {
                return Ok(Some(run.member));
            }
        }
    }
    Ok(versions.iter().rposition(|(base, tree)| {
        conflicts
            .iter()
            .any(|conflict| base.get(&conflict.path) != tree.get(&conflict.path))
    }))
}

/// The line origins of each text file that some step changed and the merged tree still has.
fn origins(
    texts: &mut Texts<'_>,
    steps: &[FlatTree],
    versions: &[(FlatTree, FlatTree)],
) -> Result<Vec<FileOrigins>, FoldError> {
    let mut changed: BTreeSet<&Vec<u8>> = BTreeSet::new();
    for pair in steps.windows(2) {
        if let [before, after] = pair {
            let paths = before.keys().chain(after.keys());
            changed.extend(paths.filter(|path| before.get(*path) != after.get(*path)));
        }
    }
    let mut found = Vec::new();
    for path in changed {
        let Ok(name) = String::from_utf8(path.clone()) else {
            continue;
        };
        let runs = texts.runs(path, steps, versions)?;
        if !runs.is_empty() {
            found.push(FileOrigins { path: name, runs });
        }
    }
    Ok(found)
}

fn is_file(kind: EntryKind) -> bool {
    matches!(kind, EntryKind::File | EntryKind::Executable)
}
