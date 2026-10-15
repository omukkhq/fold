use axum::Json;
use fold_vcs::compare::FileDiff;
use fold_vcs::diff::{DiffLine, Hunk, LineKind};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::Project;
use super::error::{ApiError, ApiQuery, ErrorEnvelope};
use super::workspaces::{
    ChangeSource, ComparedFile, ComparisonTotals, FileChangeKind, FileWarning, service,
};
use crate::daemon::AppState;
use crate::services::workspaces::{StateComparison, StateKind, StateRef, StateSummary};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_diff))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DiffQuery {
    /// The old side: a revision `r17`, a changeset `C-40`, a checkpoint `ck-88` or the working files of a workspace `ws-9`.
    pub from: String,
    /// The new side, in the same forms as `from`. A workspace is compared as its working files.
    pub to: String,
    /// One changed file, such as `src/lib.rs`: its new path for a rename, its old path for a deletion. With it the answer holds that file's hunks; without it, the list of changed files.
    pub path: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StateKindView {
    Revision,
    Changeset,
    Checkpoint,
    Workspace,
    /// A stored tree without a public id; no reference of this route names one.
    Tree,
}

impl From<StateKind> for StateKindView {
    fn from(kind: StateKind) -> Self {
        match kind {
            StateKind::Revision => Self::Revision,
            StateKind::Changeset => Self::Changeset,
            StateKind::Checkpoint => Self::Checkpoint,
            StateKind::Workspace => Self::Workspace,
            StateKind::Tree => Self::Tree,
        }
    }
}

/// One side of a comparison as the daemon resolved its reference.
#[derive(Debug, Serialize, ToSchema)]
pub struct ComparedState {
    pub kind: StateKindView,
    /// The public id, such as `r7`, `C-18`, `ck-31` or `ws-7`.
    pub id: String,
    /// The base revision of a workspace; null for every other kind.
    #[schema(required = true, example = "r7")]
    pub base: Option<String>,
    /// The task of a changeset, a checkpoint or a workspace.
    #[schema(required = true, example = "T-12")]
    pub task: Option<String>,
    /// The worker of a workspace.
    #[schema(required = true)]
    pub worker: Option<String>,
}

impl From<StateSummary> for ComparedState {
    fn from(summary: StateSummary) -> Self {
        Self {
            kind: summary.kind.into(),
            id: summary.label,
            base: summary.base.map(|base| base.to_string()),
            task: summary.task.map(|task| task.to_string()),
            worker: summary.worker,
        }
    }
}

/// The changed files of a comparison, without hunks.
#[derive(Debug, Serialize, ToSchema)]
pub struct ComparisonView {
    pub source: ChangeSource,
    pub from: ComparedState,
    pub to: ComparedState,
    /// In path order.
    pub files: Vec<ComparedFile>,
    pub totals: ComparisonTotals,
    /// Working files that could not be read, by path.
    pub warnings: Vec<FileWarning>,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DiffLineView {
    pub kind: DiffLineKind,
    /// The 1-based line number on the old side; null for an added line.
    #[schema(required = true)]
    pub old_no: Option<u32>,
    /// The 1-based line number on the new side; null for a removed line.
    #[schema(required = true)]
    pub new_no: Option<u32>,
    /// The line as the file holds it, with its line terminator.
    pub text: String,
    /// The last line of a file that does not end with a line terminator.
    pub no_newline: bool,
}

/// A unified hunk. A start is the 1-based number of the first line of its side, or of the line before the hunk (0 at the top) when that side has no lines in it.
#[derive(Debug, Serialize, ToSchema)]
pub struct HunkView {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLineView>,
}

/// The hunks of one changed file. A binary or too-large file has its hashes and no hunks.
#[derive(Debug, Serialize, ToSchema)]
pub struct FileDiffView {
    pub source: ChangeSource,
    pub path: String,
    /// The path before a rename; null for every other change.
    #[schema(required = true)]
    pub previous_path: Option<String>,
    pub change: FileChangeKind,
    pub binary: bool,
    /// A side is larger than 1 MiB, so no hunks are built; its content is not checked for binary.
    pub too_large: bool,
    /// The content hash on each side; null on the side where the file does not exist.
    #[schema(required = true)]
    pub old_hash: Option<String>,
    #[schema(required = true)]
    pub new_hash: Option<String>,
    /// Empty for a binary or too-large file and for a rename or mode change, whose content is the same on both sides.
    pub hunks: Vec<HunkView>,
}

/// A file list without `path`, one file's hunks with it; `view` tells them apart.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "view", rename_all = "snake_case")]
pub enum DiffView {
    Files(ComparisonView),
    File(FileDiffView),
}

fn source(from: StateRef, to: StateRef) -> ChangeSource {
    if [from, to]
        .iter()
        .any(|side| side.kind() == StateKind::Workspace)
    {
        ChangeSource::Working
    } else {
        ChangeSource::Recorded
    }
}

fn comparison_view(source: ChangeSource, comparison: StateComparison) -> ComparisonView {
    ComparisonView {
        source,
        from: comparison.from.into(),
        to: comparison.to.into(),
        files: comparison
            .files
            .into_iter()
            .map(ComparedFile::from)
            .collect(),
        totals: comparison.totals.into(),
        warnings: comparison.warnings.iter().map(FileWarning::from).collect(),
    }
}

fn file_view(source: ChangeSource, diff: FileDiff) -> FileDiffView {
    FileDiffView {
        source,
        path: diff.path.to_string(),
        previous_path: diff.old_path.map(|path| path.to_string()),
        change: diff.change.into(),
        binary: diff.binary,
        too_large: diff.too_large,
        old_hash: diff.old_hash.map(|hash| hash.to_string()),
        new_hash: diff.new_hash.map(|hash| hash.to_string()),
        hunks: diff.hunks.into_iter().map(hunk_view).collect(),
    }
}

fn hunk_view(hunk: Hunk) -> HunkView {
    HunkView {
        old_start: hunk.old_start,
        old_lines: hunk.old_lines,
        new_start: hunk.new_start,
        new_lines: hunk.new_lines,
        lines: hunk.lines.into_iter().map(line_view).collect(),
    }
}

fn line_view(line: DiffLine) -> DiffLineView {
    DiffLineView {
        kind: match line.kind {
            LineKind::Context => DiffLineKind::Context,
            LineKind::Added => DiffLineKind::Added,
            LineKind::Removed => DiffLineKind::Removed,
        },
        old_no: line.old_no,
        new_no: line.new_no,
        text: line.text,
        no_newline: line.no_newline,
    }
}

/// Compares two states, calculated on request: the changed files with line counts, or the hunks of one file when `path` is given. A workspace side is its working files, read with the scanner's stable read; a file that keeps changing keeps its base entry and comes back as a warning.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/diff",
    operation_id = "get_diff",
    params(("slug" = String, Path, description = "The project slug"), DiffQuery),
    responses(
        (status = 200, description = "The file list, or one file's hunks", body = DiffView),
        (status = 400, description = "A reference or the path is not valid (`invalid_state_ref`, `invalid_path`)", body = ErrorEnvelope),
        (status = 404, description = "A state, the path in the comparison or the slug does not exist (`revision_not_found`, `workspace_not_found`, `not_found`, `path_not_in_comparison`)", body = ErrorEnvelope),
        (status = 409, description = "A workspace is missing or closed (`workspace_not_active`)", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "A working file could not be read (`workspace_io`), or an internal error", body = ErrorEnvelope),
    )
)]
pub async fn get_diff(
    project: Project,
    ApiQuery(query): ApiQuery<DiffQuery>,
) -> Result<Json<DiffView>, ApiError> {
    let from = StateRef::parse(&query.from)?;
    let to = StateRef::parse(&query.to)?;
    let source = source(from, to);
    let workspaces = service(&project);
    let view = match query.path {
        Some(path) => DiffView::File(file_view(
            source,
            workspaces.diff_file(from, to, &path).await?,
        )),
        None => DiffView::Files(comparison_view(
            source,
            workspaces.compare_states(from, to).await?,
        )),
    };
    Ok(Json(view))
}
