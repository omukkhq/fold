use std::fmt;
use std::sync::Arc;

use fold_core::object::ObjectHash;
use fold_core::{
    ChangesetId, CheckpointId, FoldError, IdKind, RevisionId, TaskId, WorkspaceId, codes,
};
use fold_store::{Store, workspaces};
use fold_vcs::compare::{self as vcs, FileChange, FileDiff, State, Totals};
use fold_vcs::paths::RepoPath;
use serde_json::json;

use super::{WorkspaceError, WorkspaceService, blocking, capabilities, open_objects};

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateComparison {
    pub from: StateSummary,
    pub to: StateSummary,
    pub files: Vec<FileChange>,
    pub totals: Totals,
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
    /// Reads the tree of a recorded state from its row, or scans the working files of a workspace. Unknown numbers give `revision_not_found`, `workspace_not_found`, or `not_found` naming the changeset or checkpoint; an archived or abandoned workspace is `workspace_not_active`.
    pub async fn resolve_state(&self, state: StateRef) -> Result<ResolvedState, WorkspaceError> {
        let (summary, source) = self.locate(state).await?;
        let paths = self.paths.clone();
        let probed = Arc::clone(&self.caps);
        let (tree, state) = blocking(move || {
            let objects = open_objects(&paths)?;
            match source {
                Source::Tree(tree) => Ok((
                    Some(tree),
                    State::recorded(vcs::load_flat(&objects, &tree)?),
                )),
                Source::Working {
                    workspace,
                    root,
                    base,
                } => {
                    let caps = capabilities(&probed, &paths)?;
                    let index = paths.workspace_index(workspace);
                    let state = vcs::working_state(&objects, &root, &index, &base, caps)?;
                    Ok((None, state))
                }
            }
        })
        .await?;
        Ok(ResolvedState {
            summary,
            tree,
            state,
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
                summary.task = Some(changeset.task);
                Source::Tree(changeset.tree)
            }
            StateRef::Checkpoint(id) => {
                let checkpoint = checkpoint(&self.store, id).await?;
                summary.task = Some(workspace(&self.store, checkpoint.workspace).await?.task);
                Source::Tree(checkpoint.tree)
            }
            StateRef::Workspace(id) => {
                let row = workspace(&self.store, id).await?;
                if !row.state.is_live() {
                    return Err(closed_workspace(id, row.state).into());
                }
                let base = workspaces::get_by_number(read, row.base).await?;
                summary.base = Some(row.base);
                summary.task = Some(row.task);
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

/// A closed workspace has no working files: its directory is kept only until `fold gc` removes it.
fn closed_workspace(id: WorkspaceId, state: workspaces::WorkspaceState) -> FoldError {
    FoldError::new(
        codes::WORKSPACE_NOT_ACTIVE,
        format!("{id} is {}", state.as_str()),
    )
    .with_details([("workspace", json!(id)), ("state", json!(state.as_str()))])
    .with_hint("compare the workspace's last changeset C-<n> instead")
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
