use axum::Json;
use axum::extract::Path;
use fold_core::tree::EntryKind;
use fold_core::{FoldError, ProposalId, RevisionId, TaskId, Timestamp};
use fold_vcs::browse::{DirEntry, FileContent};
use fold_vcs::compare::{ChangeKind, FileChange, Totals};
use fold_vcs::git::Git;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::Project;
use super::error::{ApiError, ApiQuery, ErrorEnvelope};
use crate::daemon::AppState;
use crate::services::workspaces::{
    RepositoryState, RevisionDetail, RevisionOrigin, RevisionPage, RevisionSummary,
    WorkspaceService,
};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_repository_state))
        .routes(routes!(list_revisions))
        .routes(routes!(get_revision))
        .routes(routes!(read_revision_tree))
        .routes(routes!(read_revision_file))
}

fn service(project: &Project) -> WorkspaceService {
    WorkspaceService::for_project(&project.0, Git::default())
}

fn revision(text: &str) -> Result<RevisionId, ApiError> {
    Ok(text.parse::<RevisionId>().map_err(FoldError::from)?)
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RepositoryView {
    /// The revision `main` names; null only before `r1` exists.
    #[schema(value_type = Option<String>, required = true, example = "r17")]
    pub head: Option<RevisionId>,
    /// The project's read-only copy of main.
    pub main_path: String,
    /// The newest 10 revisions, newest first.
    pub recent: Vec<RevisionView>,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RevisionOriginView {
    Initial,
    Proposal,
    Pull,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RevisionView {
    #[schema(value_type = String, example = "r17")]
    pub id: RevisionId,
    #[schema(value_type = Option<String>, required = true, example = "r16")]
    pub parent: Option<RevisionId>,
    /// UTC milliseconds.
    #[schema(value_type = i64)]
    pub created_at: Timestamp,
    /// The hash of the revision's root tree.
    pub tree: String,
    pub origin: RevisionOriginView,
    #[schema(value_type = Option<String>, required = true, example = "T-12")]
    pub task: Option<TaskId>,
    #[schema(value_type = Option<String>, required = true, example = "P-3")]
    pub proposal: Option<ProposalId>,
    /// The title of the integrated proposal.
    #[schema(required = true)]
    pub title: Option<String>,
    /// The agent whose proposal was integrated, or the human who pulled.
    #[schema(required = true)]
    pub author: Option<String>,
    /// The remote commit a pull brought in.
    #[schema(required = true)]
    pub pull: Option<String>,
}

impl From<RevisionSummary> for RevisionView {
    fn from(summary: RevisionSummary) -> Self {
        Self {
            id: summary.number,
            parent: summary.parent,
            created_at: summary.created_at,
            tree: summary.tree.to_string(),
            origin: match summary.origin {
                RevisionOrigin::Initial => RevisionOriginView::Initial,
                RevisionOrigin::Proposal => RevisionOriginView::Proposal,
                RevisionOrigin::Pull => RevisionOriginView::Pull,
            },
            task: summary.task,
            proposal: summary.proposal,
            title: summary.title,
            author: summary.author,
            pull: summary.pull,
        }
    }
}

impl From<RepositoryState> for RepositoryView {
    fn from(state: RepositoryState) -> Self {
        Self {
            head: state.head,
            main_path: state.main_path.to_string_lossy().into_owned(),
            recent: state.recent.into_iter().map(RevisionView::from).collect(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RevisionPageView {
    /// Newest first.
    pub revisions: Vec<RevisionView>,
    /// Pass it as `before` for the next page; null once the page reaches `r1`.
    #[schema(value_type = Option<String>, required = true, example = "r40")]
    pub before: Option<RevisionId>,
}

impl From<RevisionPage> for RevisionPageView {
    fn from(page: RevisionPage) -> Self {
        Self {
            revisions: page.revisions.into_iter().map(RevisionView::from).collect(),
            before: page.before,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    ModeChanged,
}

impl From<ChangeKind> for FileChangeKind {
    fn from(kind: ChangeKind) -> Self {
        match kind {
            ChangeKind::Added => Self::Added,
            ChangeKind::Modified => Self::Modified,
            ChangeKind::Deleted => Self::Deleted,
            ChangeKind::Renamed => Self::Renamed,
            ChangeKind::ModeChanged => Self::ModeChanged,
        }
    }
}

/// One changed file of a comparison. A binary file is compared by hash only, so its line counts are 0; a hash is null on the side where the file does not exist.
#[derive(Debug, Serialize, ToSchema)]
pub struct ComparedFile {
    pub path: String,
    /// The path before a rename; null for every other change.
    #[schema(required = true)]
    pub previous_path: Option<String>,
    pub change: FileChangeKind,
    pub binary: bool,
    #[schema(required = true)]
    pub old_hash: Option<String>,
    #[schema(required = true)]
    pub new_hash: Option<String>,
    pub added_lines: u32,
    pub removed_lines: u32,
}

impl From<FileChange> for ComparedFile {
    fn from(file: FileChange) -> Self {
        Self {
            path: file.path.to_string(),
            previous_path: file.old_path.map(|path| path.to_string()),
            change: file.change.into(),
            binary: file.binary,
            old_hash: file.old_hash.map(|hash| hash.to_string()),
            new_hash: file.new_hash.map(|hash| hash.to_string()),
            added_lines: file.added_lines,
            removed_lines: file.removed_lines,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ComparisonTotals {
    pub files: u32,
    pub added_lines: u32,
    pub removed_lines: u32,
}

impl From<Totals> for ComparisonTotals {
    fn from(totals: Totals) -> Self {
        Self {
            files: totals.files,
            added_lines: totals.added_lines,
            removed_lines: totals.removed_lines,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RevisionDetailView {
    pub revision: RevisionView,
    /// The files changed against the parent, or every file as added for `r1`.
    pub files: Vec<ComparedFile>,
    pub totals: ComparisonTotals,
}

impl From<RevisionDetail> for RevisionDetailView {
    fn from(detail: RevisionDetail) -> Self {
        Self {
            revision: detail.summary.into(),
            files: detail.files.into_iter().map(ComparedFile::from).collect(),
            totals: detail.totals.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TreeEntryKind {
    Directory,
    File,
    Executable,
    Symlink,
}

impl From<EntryKind> for TreeEntryKind {
    fn from(kind: EntryKind) -> Self {
        match kind {
            EntryKind::Directory => Self::Directory,
            EntryKind::File => Self::File,
            EntryKind::Executable => Self::Executable,
            EntryKind::Symlink => Self::Symlink,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TreeEntryView {
    pub name: String,
    /// Relative to the revision's root.
    pub path: String,
    pub kind: TreeEntryKind,
    pub hash: String,
    /// Bytes of a file, or of a symlink's target text; null for a directory.
    #[schema(required = true)]
    pub size: Option<u64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TreeView {
    /// The directory listed; empty for the root.
    pub path: String,
    /// In name order.
    pub entries: Vec<TreeEntryView>,
}

fn tree_view(path: String, entries: Vec<DirEntry>) -> TreeView {
    let entries = entries
        .into_iter()
        .map(|entry| {
            let name = String::from_utf8_lossy(&entry.name).into_owned();
            TreeEntryView {
                path: if path.is_empty() {
                    name.clone()
                } else {
                    format!("{path}/{name}")
                },
                name,
                kind: entry.kind.into(),
                hash: entry.hash.to_string(),
                size: entry.size,
            }
        })
        .collect();
    TreeView { path, entries }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct FileContentView {
    pub path: String,
    pub kind: TreeEntryKind,
    /// The blob hash; the repository screen shows it for a binary file.
    pub hash: String,
    pub size: u64,
    pub binary: bool,
    /// The file is larger than 1 MiB, so only its size and hash are given; it is not checked for binary content.
    pub too_large: bool,
    /// The content as UTF-8, or a symlink's target text. Absent for a binary or too-large file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

fn file_view(path: String, content: FileContent) -> FileContentView {
    let size = content.size;
    let text = if content.binary || content.too_large {
        None
    } else {
        String::from_utf8(content.bytes).ok()
    };
    FileContentView {
        path,
        kind: content.kind.into(),
        hash: content.hash.to_string(),
        size,
        binary: content.binary,
        too_large: content.too_large,
        text,
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RevisionListQuery {
    /// At most this many revisions; 50 when absent, at least 1 and at most 1000.
    pub limit: Option<usize>,
    /// Only revisions numbered below this one, such as `r40`.
    pub before: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PathQuery {
    /// A path relative to the revision's root, such as `src/lib.rs`; empty or absent for the root.
    pub path: Option<String>,
}

/// The head of main, the main directory and the newest 10 revisions.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/repository",
    operation_id = "get_repository_state",
    params(("slug" = String, Path, description = "The project slug")),
    responses(
        (status = 200, description = "The repository state", body = RepositoryView),
        (status = 404, description = "The slug is not registered", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn get_repository_state(project: Project) -> Result<Json<RepositoryView>, ApiError> {
    Ok(Json(service(&project).get_repository_state().await?.into()))
}

/// Main revisions, newest first, one page at a time.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/revisions",
    operation_id = "list_revisions",
    params(("slug" = String, Path, description = "The project slug"), RevisionListQuery),
    responses(
        (status = 200, description = "One page of revisions", body = RevisionPageView),
        (status = 400, description = "The query is not valid", body = ErrorEnvelope),
        (status = 404, description = "The slug is not registered", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn list_revisions(
    project: Project,
    ApiQuery(query): ApiQuery<RevisionListQuery>,
) -> Result<Json<RevisionPageView>, ApiError> {
    let before = query.before.as_deref().map(revision).transpose()?;
    let page = service(&project)
        .list_revisions(query.limit, before)
        .await?;
    Ok(Json(page.into()))
}

/// One revision and the files it changed against its parent, without hunks.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/revisions/{r}",
    operation_id = "get_revision",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("r" = String, Path, description = "The revision, such as r17"),
    ),
    responses(
        (status = 200, description = "The revision", body = RevisionDetailView),
        (status = 400, description = "The revision id is not valid", body = ErrorEnvelope),
        (status = 404, description = "The revision or the slug does not exist", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn get_revision(
    project: Project,
    Path((_slug, r)): Path<(String, String)>,
) -> Result<Json<RevisionDetailView>, ApiError> {
    let detail = service(&project).get_revision(revision(&r)?).await?;
    Ok(Json(detail.into()))
}

/// The entries of one directory of a revision.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/revisions/{r}/tree",
    operation_id = "read_revision_tree",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("r" = String, Path, description = "The revision, such as r17"),
        PathQuery,
    ),
    responses(
        (status = 200, description = "The directory's entries", body = TreeView),
        (status = 400, description = "The revision id or the path is not valid", body = ErrorEnvelope),
        (status = 404, description = "The revision, the directory or the slug does not exist", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn read_revision_tree(
    project: Project,
    Path((_slug, r)): Path<(String, String)>,
    ApiQuery(query): ApiQuery<PathQuery>,
) -> Result<Json<TreeView>, ApiError> {
    let path = query.path.unwrap_or_default();
    let entries = service(&project).read_tree(revision(&r)?, &path).await?;
    Ok(Json(tree_view(path, entries)))
}

/// One file of a revision. A symlink gives its target text and is never followed.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/revisions/{r}/file",
    operation_id = "read_revision_file",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("r" = String, Path, description = "The revision, such as r17"),
        PathQuery,
    ),
    responses(
        (status = 200, description = "The file", body = FileContentView),
        (status = 400, description = "The revision id or the path is not valid", body = ErrorEnvelope),
        (status = 404, description = "The revision, the file or the slug does not exist", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn read_revision_file(
    project: Project,
    Path((_slug, r)): Path<(String, String)>,
    ApiQuery(query): ApiQuery<PathQuery>,
) -> Result<Json<FileContentView>, ApiError> {
    let path = query.path.unwrap_or_default();
    let content = service(&project).read_file(revision(&r)?, &path).await?;
    Ok(Json(file_view(path, content)))
}

#[cfg(test)]
mod tests {
    use fold_core::object::{ObjectKind, hash_object};

    use super::*;

    #[test]
    fn a_too_large_file_answers_with_its_size_and_no_text() {
        let content = FileContent {
            kind: EntryKind::File,
            hash: hash_object(ObjectKind::Blob, b"x"),
            size: 5 * 1024 * 1024,
            bytes: Vec::new(),
            binary: false,
            too_large: true,
        };
        let view = file_view("big.txt".into(), content);
        assert!(view.too_large && !view.binary);
        assert_eq!(view.size, 5 * 1024 * 1024);
        assert_eq!(view.text, None);
        let body = serde_json::to_value(&view).unwrap();
        assert!(body.get("text").is_none());
    }
}
