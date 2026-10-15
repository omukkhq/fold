use std::path::PathBuf;

use fold_core::object::ObjectHash;
use fold_core::tree::FlatTree;
use fold_core::{ProposalId, RevisionId, TaskId, Timestamp};
use fold_store::workspaces::{self, IntegrationKind, RevisionRow};
use fold_vcs::browse::{self, DirEntry, FileContent};
use fold_vcs::compare::{self as vcs, FileChange, State, Totals};

use super::{WorkspaceError, WorkspaceService, blocking, open_objects};

/// How many revisions the repository state lists.
pub const RECENT_REVISIONS: usize = 10;

/// How many revisions one page lists when the caller names no limit.
pub const DEFAULT_REVISION_PAGE: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionOrigin {
    /// `r1`, from `fold init` or an import.
    Initial,
    Proposal,
    /// A revision `fold pull` made from a remote commit.
    Pull,
}

impl RevisionOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::Proposal => "proposal",
            Self::Pull => "pull",
        }
    }
}

/// One main revision with public ids only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionSummary {
    pub number: RevisionId,
    pub parent: Option<RevisionId>,
    pub created_at: Timestamp,
    pub tree: ObjectHash,
    pub origin: RevisionOrigin,
    pub task: Option<TaskId>,
    pub proposal: Option<ProposalId>,
    /// The title of the integrated proposal.
    pub title: Option<String>,
    /// The agent whose proposal was integrated, or the human who pulled.
    pub author: Option<String>,
    /// The remote commit a pull brought in.
    pub pull: Option<String>,
}

impl From<RevisionRow> for RevisionSummary {
    fn from(row: RevisionRow) -> Self {
        let integration = row.integration;
        let origin = match integration.as_ref().map(|info| info.kind) {
            None => RevisionOrigin::Initial,
            Some(IntegrationKind::Proposal) => RevisionOrigin::Proposal,
            Some(IntegrationKind::Pull) => RevisionOrigin::Pull,
        };
        let proposal = integration.as_ref().and_then(|info| info.proposal.clone());
        let author = match (&proposal, &integration) {
            (Some(proposal), _) => Some(proposal.agent.clone()),
            (None, Some(info)) if info.kind == IntegrationKind::Pull => Some(info.actor.clone()),
            _ => None,
        };
        Self {
            number: row.number,
            parent: row.parent,
            created_at: row.created_at,
            tree: row.tree_hash,
            origin,
            task: proposal.as_ref().map(|proposal| proposal.task),
            proposal: proposal.as_ref().map(|proposal| proposal.number),
            title: proposal.map(|proposal| proposal.title),
            author,
            pull: integration.and_then(|info| info.remote_commit),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryState {
    /// `None` only before `r1` exists.
    pub head: Option<RevisionId>,
    /// The project's read-only copy of main.
    pub main_path: PathBuf,
    /// The newest revisions, newest first.
    pub recent: Vec<RevisionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionPage {
    pub revisions: Vec<RevisionSummary>,
    /// Pass it as `before` for the next page; `None` once the page reaches `r1`.
    pub before: Option<RevisionId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionDetail {
    pub summary: RevisionSummary,
    /// The files changed against the parent, or against the empty tree for `r1`, without hunks.
    pub files: Vec<FileChange>,
    pub totals: Totals,
}

impl WorkspaceService {
    pub async fn get_repository_state(&self) -> Result<RepositoryState, WorkspaceError> {
        let read = self.store.read();
        let head = workspaces::head(read).await?.map(|row| row.number);
        let recent = workspaces::list_recent(read, RECENT_REVISIONS, None).await?;
        Ok(RepositoryState {
            head,
            main_path: self.paths.main_dir(),
            recent: recent.into_iter().map(RevisionSummary::from).collect(),
        })
    }

    /// Newest first, at most `limit` (50 when `None`, at least 1, at most 1000), below `before` when it is given.
    pub async fn list_revisions(
        &self,
        limit: Option<usize>,
        before: Option<RevisionId>,
    ) -> Result<RevisionPage, WorkspaceError> {
        // A page of none would end the paging with a null `before` while older revisions remain.
        let limit = limit
            .unwrap_or(DEFAULT_REVISION_PAGE)
            .clamp(1, workspaces::MAX_PAGE);
        let rows = workspaces::list_recent(self.store.read(), limit, before).await?;
        let revisions: Vec<RevisionSummary> = rows.into_iter().map(RevisionSummary::from).collect();
        // Every revision's parent is numbered one lower, so the page ends at r1 exactly when its last number is 1.
        let before = revisions
            .last()
            .map(|last| last.number)
            .filter(|last| last.number() > 1);
        Ok(RevisionPage { revisions, before })
    }

    pub async fn get_revision(&self, number: RevisionId) -> Result<RevisionDetail, WorkspaceError> {
        let read = self.store.read();
        let row = workspaces::get_by_number(read, number).await?;
        let parent_tree = match row.parent {
            Some(parent) => Some(workspaces::get_by_number(read, parent).await?.tree_hash),
            None => None,
        };
        let summary = RevisionSummary::from(row);
        let tree = summary.tree;
        let paths = self.paths.clone();
        let comparison = blocking(move || {
            let objects = open_objects(&paths)?;
            let from = match parent_tree {
                Some(parent) => vcs::load_flat(&objects, &parent)?,
                None => FlatTree::new(),
            };
            let to = vcs::load_flat(&objects, &tree)?;
            vcs::compare(&objects, &State::recorded(from), &State::recorded(to))
        })
        .await?;
        Ok(RevisionDetail {
            summary,
            files: comparison.files,
            totals: comparison.totals,
        })
    }

    /// The entries of one directory of a revision; an empty `path` lists the root.
    pub async fn read_tree(
        &self,
        number: RevisionId,
        path: &str,
    ) -> Result<Vec<DirEntry>, WorkspaceError> {
        let path = browse::parse_path(path)?;
        let tree = workspaces::get_by_number(self.store.read(), number)
            .await?
            .tree_hash;
        let paths = self.paths.clone();
        blocking(move || browse::read_dir(&open_objects(&paths)?, &tree, path.as_ref())).await
    }

    /// One file of a revision. A symlink gives its target text.
    pub async fn read_file(
        &self,
        number: RevisionId,
        path: &str,
    ) -> Result<FileContent, WorkspaceError> {
        let path = browse::parse_path(path)?.ok_or_else(|| {
            fold_core::FoldError::new(fold_core::codes::INVALID_PATH, "a file path is required")
                .with_details([("path", serde_json::json!(""))])
        })?;
        let tree = workspaces::get_by_number(self.store.read(), number)
            .await?
            .tree_hash;
        let paths = self.paths.clone();
        blocking(move || browse::read_file(&open_objects(&paths)?, &tree, &path)).await
    }
}
