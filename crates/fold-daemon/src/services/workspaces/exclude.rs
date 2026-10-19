use fold_core::policy::{self, Starter};
use fold_core::{Actor, FoldError, codes};
use fold_store::{StoreError, agents};
use fold_vcs::git_remote::{add_exclude_patterns as append_patterns, exclude_fold_dir};
use fold_vcs::ignore::{brings_back_fold_dir, is_ignore_pattern};
use serde_json::json;

use super::{WorkspaceError, WorkspaceService, blocking};

/// The most patterns one call adds.
const MAX_PATTERNS: usize = 50;

/// The longest pattern, in bytes.
const MAX_PATTERN_LEN: usize = 500;

impl WorkspaceService {
    /// Adds `patterns` to the project's `.fold/exclude`, the local rules of what is never recorded, imported or proposed, for the main agent (`policy::can_add_exclude_patterns`). Each pattern is one line of `.gitignore` syntax; one the file lists already is left out. The file keeps fold's own `/.fold/` line, and when the project has its Git repository, `.git/info/exclude` is made the link to it, so Git leaves the same paths out. Every scan reads the file again, so the next scan of a workspace and the next `fld record` follow it, and the live state of every workspace is scanned again at once. Returns the patterns added, in the order given.
    pub async fn add_exclude_patterns(
        &self,
        actor: &Actor,
        patterns: Vec<String>,
    ) -> Result<Vec<String>, WorkspaceError> {
        let supervised = match actor {
            Actor::Agent(agent) => {
                let mut conn = self
                    .store
                    .read()
                    .acquire()
                    .await
                    .map_err(StoreError::from)?;
                agents::find_worker_process(&mut conn, &agent.agent)
                    .await?
                    .map(|row| (row.role, row.area))
            }
            Actor::Human { .. } | Actor::System => None,
        };
        policy::can_add_exclude_patterns(&Starter::of(actor, supervised))
            .map_err(FoldError::from)?;
        if patterns.is_empty() || patterns.len() > MAX_PATTERNS {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("give 1 to {MAX_PATTERNS} patterns"),
            )
            .with_details([("field", json!("patterns"))])
            .into());
        }
        if let Some(at) = patterns
            .iter()
            .position(|pattern| pattern.len() > MAX_PATTERN_LEN || !is_ignore_pattern(pattern))
        {
            return Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!("pattern {} is not one line of .gitignore syntax of at most {MAX_PATTERN_LEN} bytes", at + 1),
        )
        .with_details([("field", json!("patterns")), ("index", json!(at))])
        .with_hint("give each pattern as a .gitignore line, such as build/ or *.log")
        .into());
        }
        if let Some(at) = patterns
            .iter()
            .position(|pattern| brings_back_fold_dir(pattern))
        {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!(
                    "pattern {} would bring fold's data folder .fold back into Git",
                    at + 1
                ),
            )
            .with_details([("field", json!("patterns")), ("index", json!(at))])
            .with_hint("leave out the ! line that matches .fold")
            .into());
        }
        let paths = self.paths.clone();
        let added = blocking(move || {
            let added = append_patterns(&paths.fold_dir(), &patterns)?;
            if paths.git_dir().is_dir() {
                exclude_fold_dir(&paths.git_dir())?;
            }
            Ok(added)
        })
        .await?;
        // No workspace watcher sees the file, which lies outside every workspace, so each live state is scanned again under the new rules.
        if !added.is_empty()
            && let Some(live) = &self.live
        {
            for workspace in live.all() {
                self.live_rescan(workspace.view.workspace).await;
            }
        }
        tracing::info!(added = added.len(), "patterns added to .fold/exclude");
        Ok(added)
    }
}
