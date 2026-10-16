use fold_core::requests::{attachment_too_large, check_attachment_count};
use fold_core::{Actor, FoldError, WorkspaceId, codes};
use fold_vcs::paths::RepoPath;
use fold_vcs::scan::{WorkingFile, read_working_file};
use serde_json::json;

use super::{WorkspaceError, WorkspaceService, blocking};

/// A file read from a workspace to attach elsewhere: its file name and its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

impl WorkspaceService {
    /// The files at `paths` in the actor's own active workspace, in order, so an agent can attach what it has at hand. No paths read nothing; paths without a workspace are `invalid_request` with `details.reason = "attachments_need_ws"`. More paths than a request holds are refused before anything is read. Each path must be relative and normalised, and each file a regular file of at most `max_bytes` bytes, read one directory handle at a time with no symlink followed. A file larger than the limit is `invalid_request` with `details.reason = "attachment_too_large"` and its 1-based `position`.
    pub async fn read_own_files(
        &self,
        actor: &Actor,
        workspace: Option<WorkspaceId>,
        paths: &[String],
        max_bytes: u64,
    ) -> Result<Vec<WorkspaceFile>, WorkspaceError> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let workspace = workspace.ok_or_else(|| {
            FoldError::new(codes::INVALID_REQUEST, "attachments need your workspace ws")
                .with_details([("reason", json!("attachments_need_ws"))])
                .with_hint("call create_request again with ws, the workspace that holds the files")
        })?;
        check_attachment_count(paths.len())?;
        let parsed = paths
            .iter()
            .map(|path| {
                RepoPath::parse(path.as_bytes()).map_err(|error| {
                    FoldError::from(error).with_hint(
                        "attach paths relative to your workspace root, such as logs/run.log",
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let row = self.owned(actor, workspace).await?;
        let root = self.paths.root().join(&row.path);
        blocking(move || {
            parsed
                .iter()
                .enumerate()
                .map(|(index, path)| {
                    let refused = |code: &'static str, message: &str| {
                        FoldError::new(code, format!("{path} {message} in {workspace}"))
                            .with_details([("path", json!(path.to_string()))])
                    };
                    let bytes = match read_working_file(&root, path, max_bytes)? {
                        WorkingFile::Read(bytes) => bytes,
                        WorkingFile::Missing => {
                            return Err(refused(codes::NOT_FOUND, "does not exist")
                                .with_hint("attach a file that exists in your workspace"));
                        }
                        WorkingFile::NotAFile => {
                            return Err(refused(codes::INVALID_PATH, "is not a regular file")
                                .with_hint("attach regular files; a symlink is never followed"));
                        }
                        WorkingFile::Unstable => {
                            return Err(refused(codes::WORKSPACE_IO, "kept changing while read")
                                .with_hint("attach the file again once it stops changing"));
                        }
                        WorkingFile::TooLarge(_) => return Err(attachment_too_large(index + 1)),
                    };
                    let name = path
                        .components()
                        .last()
                        .map(|name| String::from_utf8_lossy(name).into_owned())
                        .unwrap_or_default();
                    Ok(WorkspaceFile { name, bytes })
                })
                .collect()
        })
        .await
    }
}
