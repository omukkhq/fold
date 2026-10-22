use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use fold_core::object::ObjectHash;
use fold_core::{
    ChangesetId, CheckpointId, FoldError, IdKind, RevisionId, TaskId, WorkspaceId, codes,
};
use fold_store::Store;
use fold_store::workspaces::{self, WorkspaceState};
use fold_vcs::batch::{BatchConflict, BatchMember, MergedBatch, merged_batch_state};
use fold_vcs::compare::{self as vcs, CarriedState, FileChange, FileDiff, State, Totals};
use fold_vcs::objects::ObjectStore;
use fold_vcs::paths::RepoPath;
use fold_vcs::snapshot::Warning;
use serde_json::json;

use super::{WorkspaceError, WorkspaceService, blocking, capabilities, load_config, open_objects};
use crate::support::lock;

/// A state of the project that can be compared. Working files and recorded states are different kinds, so a caller never mistakes live edits for history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateRef {
    Revision(RevisionId),
    Changeset(ChangesetId),
    Checkpoint(CheckpointId),
    /// The working files of the workspace, compared against its own base revision when no other side is given.
    Workspace(WorkspaceId),
    /// A stored tree; it has no public id, so only the daemon builds one.
    Tree(ObjectHash),
}

impl StateRef {
    /// Reads `r<n>`, `C-<n>`, `ck-<n>` or `ws-<n>`. Any other text, a tree hash included, is `invalid_state_ref`.
    pub fn parse(text: &str) -> Result<Self, FoldError> {
        let parsed = if text.starts_with(IdKind::Revision.prefix()) {
            text.parse().map(Self::Revision)
        } else if text.starts_with(IdKind::Changeset.prefix()) {
            text.parse().map(Self::Changeset)
        } else if text.starts_with(IdKind::Checkpoint.prefix()) {
            text.parse().map(Self::Checkpoint)
        } else if text.starts_with(IdKind::Workspace.prefix()) {
            text.parse().map(Self::Workspace)
        } else {
            return Err(invalid_state_ref());
        };
        parsed.map_err(|_| invalid_state_ref())
    }

    pub fn kind(self) -> StateKind {
        match self {
            Self::Revision(_) => StateKind::Revision,
            Self::Changeset(_) => StateKind::Changeset,
            Self::Checkpoint(_) => StateKind::Checkpoint,
            Self::Workspace(_) => StateKind::Workspace,
            Self::Tree(_) => StateKind::Tree,
        }
    }
}

impl fmt::Display for StateRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Revision(id) => id.fmt(f),
            Self::Changeset(id) => id.fmt(f),
            Self::Checkpoint(id) => id.fmt(f),
            Self::Workspace(id) => id.fmt(f),
            Self::Tree(hash) => hash.fmt(f),
        }
    }
}

fn invalid_state_ref() -> FoldError {
    FoldError::new(
        codes::INVALID_STATE_REF,
        "a state is r<n>, C-<n>, ck-<n> or ws-<n>",
    )
    .with_hint(
        "pass a revision such as r17, a changeset C-40, a checkpoint ck-88 or a workspace ws-9",
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateKind {
    Revision,
    Changeset,
    Checkpoint,
    Workspace,
    Tree,
}

impl StateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Revision => "revision",
            Self::Changeset => "changeset",
            Self::Checkpoint => "checkpoint",
            Self::Workspace => "workspace",
            Self::Tree => "tree",
        }
    }
}

/// One side of a comparison as the daemon resolved it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateSummary {
    pub kind: StateKind,
    /// The public id, such as `r7`, `C-18`, `ck-31` or `ws-7`.
    pub label: String,
    /// The base revision of a workspace; `None` for every other kind.
    pub base: Option<RevisionId>,
    /// The task a changeset or a workspace belongs to.
    pub task: Option<TaskId>,
    /// The worker of a workspace.
    pub worker: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedState {
    pub summary: StateSummary,
    /// The stored tree; `None` for working files, which no tree records.
    pub tree: Option<ObjectHash>,
    pub state: State,
    /// Working files the scan could not read, by path; each keeps its base entry. Empty for a recorded state.
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateComparison {
    pub from: StateSummary,
    pub to: StateSummary,
    pub files: Vec<FileChange>,
    pub totals: Totals,
    /// The warnings of both sides, `from` first.
    pub warnings: Vec<Warning>,
}

/// A stored tree with its change against `base` carried onto the tree `onto` by the three-way merge, such as an earlier proposal version moved onto the base of a later one. All three are stored trees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Carry {
    pub base: ObjectHash,
    pub tree: ObjectHash,
    pub onto: ObjectHash,
}

/// How many carried states a [`CarryCache`] keeps. One view of a review batch carries each member whose base moved, so a bound below the most members of a batch would evict every member before the next view reads it again; the rest leaves room for reviewers who read single proposals at the same time. An entry holds the flat tree of the carried state and only the files that both sides changed, so a few dozen of them stay small next to the project's object store.
pub(crate) const CARRY_CACHE_ENTRIES: usize = 32;

/// How many merged batches a [`BatchMergeCache`] keeps. An entry holds main's flat tree and the batch's merged one, so it is larger than a carried state; a few open batches are read at the same time at most.
pub(crate) const BATCH_MERGE_CACHE_ENTRIES: usize = 8;

/// The states the project's reads last built by a merge, by the stored trees they were merged from, so a file list and each file read of one comparison run one merge between them. The trees are content addresses, so an entry never goes stale; the least recently used one leaves when the cache is full. The merged files stay in memory and never reach the object store.
pub struct MergeCache<K, V> {
    /// The most recently used first.
    entries: Mutex<VecDeque<(K, Arc<V>)>>,
    capacity: usize,
    hits: AtomicU64,
    misses: AtomicU64,
}

/// The carried states of revised proposals, by their three trees.
pub type CarryCache = MergeCache<Carry, CarriedState>;

/// The merged trees of review batches, by main's tree and the members' trees.
pub type BatchMergeCache = MergeCache<BatchMergeKey, BatchMergeView>;

impl Default for CarryCache {
    fn default() -> Self {
        Self::with_capacity(CARRY_CACHE_ENTRIES)
    }
}

impl Default for BatchMergeCache {
    fn default() -> Self {
        Self::with_capacity(BATCH_MERGE_CACHE_ENTRIES)
    }
}

/// The counts only: an entry holds file content, which never goes into a log line.
impl<K, V> fmt::Debug for MergeCache<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MergeCache")
            .field("entries", &lock(&self.entries).len())
            .field("hits", &self.hits())
            .field("misses", &self.misses())
            .finish()
    }
}

impl<K, V> MergeCache<K, V> {
    /// The reads the cache answered without a merge.
    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    /// The reads that ran the merge.
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }
}

impl<K: PartialEq, V> MergeCache<K, V> {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Mutex::new(VecDeque::new()),
            capacity,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// The state of `key`, built by `build` when the cache does not hold it. The lock is free while `build` runs, so a slow merge never holds up another read; two reads that miss the same key at once both build it and keep one. Blocking: call it through `spawn_blocking`.
    pub fn get_or_build(
        &self,
        key: K,
        build: impl FnOnce() -> Result<V, FoldError>,
    ) -> Result<Arc<V>, FoldError> {
        if let Some(found) = self.take(&key) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Ok(found);
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        let built = Arc::new(build()?);
        let mut entries = lock(&self.entries);
        entries.retain(|(held, _)| *held != key);
        entries.push_front((key, Arc::clone(&built)));
        entries.truncate(self.capacity);
        Ok(built)
    }

    /// Keeps `value` as the state of `key` without a read, for a command that built it anyway.
    pub fn insert(&self, key: K, value: V) {
        let mut entries = lock(&self.entries);
        entries.retain(|(held, _)| *held != key);
        entries.push_front((key, Arc::new(value)));
        entries.truncate(self.capacity);
    }

    /// The entry of `key`, moved to the front.
    fn take(&self, key: &K) -> Option<Arc<V>> {
        let mut entries = lock(&self.entries);
        let position = entries.iter().position(|(held, _)| held == key)?;
        let entry = entries.remove(position)?;
        let found = Arc::clone(&entry.1);
        entries.push_front(entry);
        Some(found)
    }
}

/// What a batch's merged tree is built from: main's tree and each member's base and own tree, in merge order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchMergeKey {
    pub main: ObjectHash,
    pub members: Vec<BatchMember>,
}

/// Both sides of a batch's merged change, as [`WorkspaceService::compare_batch_merge`] compares them.
#[derive(Debug)]
pub struct BatchMergeView {
    /// main's tree.
    pub from: State,
    pub merged: MergedBatch,
}

/// See [`WorkspaceService::compare_carried`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarriedComparison {
    pub files: Vec<FileChange>,
    pub totals: Totals,
    /// Where the carried change conflicts with `onto`, in path order; each keeps the carried tree's side.
    pub conflicts: Vec<RepoPath>,
}

/// The change a batch's members make together on main: see [`WorkspaceService::compare_batch_merge`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchMergeComparison {
    pub files: Vec<FileChange>,
    pub totals: Totals,
    /// How many members, from the first, the compared tree holds.
    pub merged: usize,
    /// The first member that does not merge; the tree holds the members before it.
    pub conflict: Option<BatchConflict>,
    /// The compared tree's hash; nothing stores it.
    pub tree: ObjectHash,
}

/// Where a state's entries come from before any file is read.
enum Source {
    Tree(ObjectHash),
    Working {
        workspace: WorkspaceId,
        root: std::path::PathBuf,
        base: ObjectHash,
    },
}

impl WorkspaceService {
    /// Reads the tree of a recorded state from its row, or scans the working files of a workspace with the project's largest file. Unknown numbers give `revision_not_found`, `workspace_not_found`, or `not_found` naming the changeset or checkpoint; an archived, abandoned or missing workspace, or one whose directory is gone, is `workspace_not_active`.
    pub async fn resolve_state(&self, state: StateRef) -> Result<ResolvedState, WorkspaceError> {
        let (summary, source) = self.locate(state).await?;
        let max_file_bytes = match &source {
            Source::Working { .. } => load_config(&self.paths).await?.max_file_bytes,
            Source::Tree(_) => 0,
        };
        let paths = self.paths.clone();
        let probed = Arc::clone(&self.caps);
        let (tree, state, warnings) = blocking(move || {
            let objects = open_objects(&paths)?;
            match source {
                Source::Tree(tree) => Ok((
                    Some(tree),
                    State::recorded(vcs::load_flat(&objects, &tree)?),
                    Vec::new(),
                )),
                Source::Working {
                    workspace,
                    root,
                    base,
                } => {
                    if !root.is_dir() {
                        return Err(inactive_workspace(workspace, WorkspaceState::Missing));
                    }
                    let caps = capabilities(&probed, &paths)?;
                    let index = paths.workspace_index(workspace);
                    let working =
                        vcs::working_state(&objects, &root, &index, &base, caps, max_file_bytes)?;
                    Ok((None, working.state, working.warnings))
                }
            }
        })
        .await?;
        Ok(ResolvedState {
            summary,
            tree,
            state,
            warnings,
        })
    }

    /// The files that differ from `from` to `to`, with line counts. A read: it takes no actor and writes nothing.
    pub async fn compare_states(
        &self,
        from: StateRef,
        to: StateRef,
    ) -> Result<StateComparison, WorkspaceError> {
        let (from, to) = (
            self.resolve_state(from).await?,
            self.resolve_state(to).await?,
        );
        let paths = self.paths.clone();
        let (from_state, to_state) = (from.state, to.state);
        let mut warnings = from.warnings;
        warnings.extend(to.warnings);
        let comparison = blocking(move || {
            let objects = open_objects(&paths)?;
            vcs::compare(&objects, &from_state, &to_state)
        })
        .await?;
        Ok(StateComparison {
            from: from.summary,
            to: to.summary,
            files: comparison.files,
            totals: comparison.totals,
            warnings,
        })
    }

    /// The hunks of one changed file. A path the comparison does not list is `path_not_in_comparison`.
    pub async fn diff_file(
        &self,
        from: StateRef,
        to: StateRef,
        path: &str,
    ) -> Result<FileDiff, WorkspaceError> {
        let path = RepoPath::parse(path.as_bytes()).map_err(FoldError::from)?;
        let (from, to) = (
            self.resolve_state(from).await?,
            self.resolve_state(to).await?,
        );
        let paths = self.paths.clone();
        blocking(move || {
            let objects = open_objects(&paths)?;
            vcs::file_diff(&objects, &from.state, &to.state, &path)
        })
        .await
    }

    /// The files that differ from the carried state `from` to the stored tree `to`, with line counts, and the paths where the carried change conflicts. A read: it writes nothing, the line-merged files stay in memory, and the service's [`CarryCache`] keeps the carried state for the next read of the same carry.
    pub async fn compare_carried(
        &self,
        from: Carry,
        to: ObjectHash,
    ) -> Result<CarriedComparison, WorkspaceError> {
        let paths = self.paths.clone();
        let carries = Arc::clone(&self.carries);
        blocking(move || {
            let objects = open_objects(&paths)?;
            let carried = carries.get_or_build(from, || {
                vcs::carried_state(&objects, &from.base, &from.tree, &from.onto)
            })?;
            let to = State::recorded(vcs::load_flat(&objects, &to)?);
            let comparison = vcs::compare(&objects, &carried.state, &to)?;
            Ok(CarriedComparison {
                files: comparison.files,
                totals: comparison.totals,
                conflicts: carried.conflicts.clone(),
            })
        })
        .await
    }

    /// The hunks of one file of [`WorkspaceService::compare_carried`]. A path the comparison does not list is `path_not_in_comparison`.
    pub async fn diff_file_carried(
        &self,
        from: Carry,
        to: ObjectHash,
        path: &str,
    ) -> Result<FileDiff, WorkspaceError> {
        let path = RepoPath::parse(path.as_bytes()).map_err(FoldError::from)?;
        let paths = self.paths.clone();
        let carries = Arc::clone(&self.carries);
        blocking(move || {
            let objects = open_objects(&paths)?;
            let carried = carries.get_or_build(from, || {
                vcs::carried_state(&objects, &from.base, &from.tree, &from.onto)
            })?;
            let to = State::recorded(vcs::load_flat(&objects, &to)?);
            vcs::file_diff(&objects, &carried.state, &to, &path)
        })
        .await
    }

    /// main's tree `main` against `members` merged onto it in merge order, by the merge a batch checkout is made with. The merged tree is built in memory and nothing is written, so a read never adds objects to the store; the service's [`BatchMergeCache`] keeps it for the file reads of the same change.
    pub async fn compare_batch_merge(
        &self,
        main: ObjectHash,
        members: Vec<BatchMember>,
    ) -> Result<BatchMergeComparison, WorkspaceError> {
        let paths = self.paths.clone();
        let merges = Arc::clone(&self.batch_merges);
        blocking(move || {
            let objects = open_objects(&paths)?;
            let view = batch_merge_view(&merges, &objects, main, members)?;
            let comparison = vcs::compare(&objects, &view.from, &view.merged.state)?;
            Ok(BatchMergeComparison {
                files: comparison.files,
                totals: comparison.totals,
                merged: view.merged.merged,
                conflict: view.merged.conflict.clone(),
                tree: view.merged.tree,
            })
        })
        .await
    }

    /// The hunks of one file of [`WorkspaceService::compare_batch_merge`]. A path the comparison does not list is `path_not_in_comparison`.
    pub async fn diff_file_batch_merge(
        &self,
        main: ObjectHash,
        members: Vec<BatchMember>,
        path: &str,
    ) -> Result<FileDiff, WorkspaceError> {
        let path = RepoPath::parse(path.as_bytes()).map_err(FoldError::from)?;
        let paths = self.paths.clone();
        let merges = Arc::clone(&self.batch_merges);
        blocking(move || {
            let objects = open_objects(&paths)?;
            let view = batch_merge_view(&merges, &objects, main, members)?;
            vcs::file_diff(&objects, &view.from, &view.merged.state, &path)
        })
        .await
    }

    async fn locate(&self, state: StateRef) -> Result<(StateSummary, Source), WorkspaceError> {
        let read = self.store.read();
        let mut summary = StateSummary {
            kind: state.kind(),
            label: state.to_string(),
            base: None,
            task: None,
            worker: None,
        };
        let source = match state {
            StateRef::Revision(id) => {
                Source::Tree(workspaces::get_by_number(read, id).await?.tree_hash)
            }
            StateRef::Changeset(id) => {
                let changeset = changeset(&self.store, id).await?;
                summary.task = changeset.task;
                Source::Tree(changeset.tree)
            }
            StateRef::Checkpoint(id) => {
                let checkpoint = checkpoint(&self.store, id).await?;
                summary.task = workspace(&self.store, checkpoint.workspace).await?.task;
                Source::Tree(checkpoint.tree)
            }
            StateRef::Workspace(id) => {
                let row = workspace(&self.store, id).await?;
                if row.state != WorkspaceState::Active {
                    return Err(inactive_workspace(id, row.state).into());
                }
                let base = workspaces::get_by_number(read, row.base).await?;
                summary.base = Some(row.base);
                summary.task = row.task;
                summary.worker = row.worker.map(|worker| worker.to_string());
                Source::Working {
                    workspace: id,
                    root: self.paths.root().join(&row.path),
                    base: base.tree_hash,
                }
            }
            StateRef::Tree(hash) => Source::Tree(hash),
        };
        Ok((summary, source))
    }
}

async fn workspace(
    store: &Store,
    id: WorkspaceId,
) -> Result<workspaces::WorkspaceRow, WorkspaceError> {
    workspaces::get_workspace(store.read(), id)
        .await?
        .ok_or_else(|| {
            FoldError::new(codes::WORKSPACE_NOT_FOUND, format!("{id} does not exist"))
                .with_details([("workspace", json!(id))])
                .into()
        })
}

/// Only an active workspace with its directory has working files. fold removes a closed workspace's directory once it closes, and a missing one has lost it, so comparing either would show every file as deleted.
fn inactive_workspace(id: WorkspaceId, state: WorkspaceState) -> FoldError {
    let hint = match state {
        WorkspaceState::Missing => {
            "the directory is gone; compare one of its checkpoints ck-<n> instead"
        }
        _ => "compare the workspace's last changeset C-<n> instead",
    };
    FoldError::new(
        codes::WORKSPACE_NOT_ACTIVE,
        format!("{id} is {}", state.as_str()),
    )
    .with_details([("workspace", json!(id)), ("state", json!(state.as_str()))])
    .with_hint(hint)
}

async fn changeset(
    store: &Store,
    id: ChangesetId,
) -> Result<workspaces::Changeset, WorkspaceError> {
    workspaces::get_changeset(store.read(), id)
        .await?
        .ok_or_else(|| {
            FoldError::new(codes::NOT_FOUND, format!("changeset {id} does not exist"))
                .with_details([("changeset", json!(id))])
                .into()
        })
}

async fn checkpoint(
    store: &Store,
    id: CheckpointId,
) -> Result<workspaces::Checkpoint, WorkspaceError> {
    workspaces::get_checkpoint(store.read(), id)
        .await?
        .ok_or_else(|| {
            FoldError::new(codes::NOT_FOUND, format!("checkpoint {id} does not exist"))
                .with_details([("checkpoint", json!(id))])
                .into()
        })
}

/// Both sides of a batch's merged change, from `merges` or merged now. Blocking.
fn batch_merge_view(
    merges: &BatchMergeCache,
    objects: &ObjectStore,
    main: ObjectHash,
    members: Vec<BatchMember>,
) -> Result<Arc<BatchMergeView>, FoldError> {
    let key = BatchMergeKey {
        main,
        members: members.clone(),
    };
    merges.get_or_build(key, || {
        Ok(BatchMergeView {
            from: State::recorded(vcs::load_flat(objects, &main)?),
            merged: merged_batch_state(objects, &main, &members)?,
        })
    })
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use fold_core::object::{ObjectKind, hash_object};
    use fold_core::tree::FlatTree;

    use super::*;

    fn hash(n: usize) -> ObjectHash {
        hash_object(ObjectKind::Blob, n.to_string().as_bytes())
    }

    fn key(n: usize) -> Carry {
        Carry {
            base: hash(0),
            tree: hash(n),
            onto: hash(1),
        }
    }

    fn carried(conflict: &str) -> CarriedState {
        CarriedState {
            state: State::recorded(FlatTree::new()),
            conflicts: vec![RepoPath::parse(conflict.as_bytes()).unwrap()],
        }
    }

    #[test]
    fn a_second_read_of_a_carry_reuses_the_first_merge() {
        let cache = CarryCache::default();
        let merges = Cell::new(0);
        let carry = || {
            merges.set(merges.get() + 1);
            Ok(carried("src/lib.rs"))
        };
        let first = cache.get_or_build(key(2), carry).unwrap();
        let second = cache
            .get_or_build(key(2), || panic!("the second read merged again"))
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(merges.get(), 1);
        assert_eq!((cache.hits(), cache.misses()), (1, 1));
        let other = cache.get_or_build(key(3), carry).unwrap();
        assert!(!Arc::ptr_eq(&first, &other));
        assert_eq!(merges.get(), 2);
        assert_eq!(
            format!("{cache:?}"),
            "MergeCache { entries: 2, hits: 1, misses: 2 }"
        );
    }

    #[test]
    fn a_failed_merge_is_not_kept() {
        let cache = CarryCache::default();
        let failed = cache.get_or_build(key(2), || Err(FoldError::new(codes::NOT_FOUND, "gone")));
        assert_eq!(failed.unwrap_err().code(), codes::NOT_FOUND);
        cache.get_or_build(key(2), || Ok(carried("a"))).unwrap();
        assert_eq!((cache.hits(), cache.misses()), (0, 2));
    }

    #[test]
    fn a_full_cache_lets_the_least_recently_used_carry_go() {
        let cache = CarryCache::default();
        for n in 0..CARRY_CACHE_ENTRIES {
            cache.get_or_build(key(n), || Ok(carried("a"))).unwrap();
        }
        cache
            .get_or_build(key(0), || panic!("the first carry left too early"))
            .unwrap();
        cache
            .get_or_build(key(CARRY_CACHE_ENTRIES), || Ok(carried("a")))
            .unwrap();
        cache
            .get_or_build(key(0), || panic!("the most recently read carry left"))
            .unwrap();
        let merges = Cell::new(0);
        cache
            .get_or_build(key(1), || {
                merges.set(1);
                Ok(carried("a"))
            })
            .unwrap();
        assert_eq!(merges.get(), 1, "the least recently used carry stayed");
        assert_eq!(lock(&cache.entries).len(), CARRY_CACHE_ENTRIES);
    }
}
