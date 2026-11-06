use std::path::Path;

use fold_core::policy::{self, Starter};
use fold_core::{Actor, FoldError, codes};
use fold_store::{StoreError, agents};
use fold_vcs::git_remote::{add_exclude_patterns, remove_exclude_patterns};
use fold_vcs::ignore::IgnoreRules;
use fold_vcs::paths::RepoPath;
use fold_vcs::shared::{
    NotShareable, added_excludes, check_shareable, link_shared, read_shared, unshared_ignored,
    write_added_excludes, write_shared,
};
use serde_json::json;

use super::{WorkspaceError, WorkspaceService, blocking};

/// The most paths one call names.
const MAX_PATHS: usize = 50;

/// The most ignored paths that [`SharedState::unseen`] names; the rest are counted.
pub const LISTED_UNSEEN: usize = 10;

/// The shared reference paths and the ignored paths that no worker sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedState {
    /// The shared paths, in path order.
    pub shared: Vec<String>,
    /// The first [`LISTED_UNSEEN`] ignored files and folders of the project folder that no workspace holds or links to, in path order; a folder ends with `/`.
    pub unseen: Vec<String>,
    /// How many there are in all.
    pub unseen_count: usize,
}

impl WorkspaceService {
    /// Shares `paths` of the project folder with every workspace, read-only, for a human or the main agent (`policy::can_share_paths`). Each path is relative to the project folder and must be a file or folder there, reached without a symbolic link, that an ignore rule leaves out, so main never tracks it; a path within or around another shared one is refused, so no link lies inside another. A folder that only a folder pattern leaves out gets an anchored line in `.fold/exclude`, so its link, which is no folder, is never recorded either. The list goes to `.fold/shared`; each workspace gets its links at its next create or sync. Returns the paths added, in the order given; one the list holds already is left out.
    pub async fn share_paths(
        &self,
        actor: &Actor,
        paths: Vec<String>,
    ) -> Result<Vec<String>, WorkspaceError> {
        self.may_share(actor).await?;
        let parsed = parse_paths(&paths)?;
        let project = self.paths.clone();
        let added = blocking(move || {
            let _writing = crate::support::lock(&SHARED_WRITES);
            let root = project.root();
            let fold_dir = project.fold_dir();
            let rules = IgnoreRules::new(root);
            let mut shared = read_shared(&fold_dir).map_err(io_error)?;
            let mut added = Vec::new();
            let mut excludes = Vec::new();
            for (at, path) in parsed.into_iter().enumerate() {
                if shared.contains(&path) {
                    continue;
                }
                let shareable = check_shareable(root, &rules, &shared, path.clone())
                    .map_err(|reason| refusal(at, &path, reason))?;
                if shareable.needs_exclude {
                    excludes.push(format!("/{}", shareable.path));
                }
                added.push(shareable.path.to_string());
                shared.push(shareable.path);
            }
            if !excludes.is_empty() {
                let made = add_exclude_patterns(&fold_dir, &excludes)?;
                let mut own = added_excludes(&fold_dir).map_err(io_error)?;
                own.extend(
                    made.into_iter()
                        .filter(|line| !own.contains(line))
                        .collect::<Vec<_>>(),
                );
                write_added_excludes(&fold_dir, &own).map_err(io_error)?;
            }
            if !added.is_empty() {
                shared.sort();
                write_shared(&fold_dir, &shared).map_err(io_error)?;
            }
            Ok(added)
        })
        .await?;
        tracing::info!(added = added.len(), "paths shared with the workspaces");
        Ok(added)
    }

    /// Takes `paths` off the shared list, for a human or the main agent (`policy::can_share_paths`), with the `.fold/exclude` line that sharing a path added; a line of the user's stays. A workspace drops its link at its next sync; its files never change otherwise. Returns the paths removed; one the list does not hold is left out.
    pub async fn unshare_paths(
        &self,
        actor: &Actor,
        paths: Vec<String>,
    ) -> Result<Vec<String>, WorkspaceError> {
        self.may_share(actor).await?;
        let parsed = parse_paths(&paths)?;
        let fold_dir = self.paths.fold_dir();
        let removed = blocking(move || {
            let _writing = crate::support::lock(&SHARED_WRITES);
            let mut shared = read_shared(&fold_dir).map_err(io_error)?;
            let mut removed = Vec::new();
            for path in parsed {
                if let Some(at) = shared.iter().position(|held| held == &path) {
                    shared.remove(at);
                    removed.push(path.to_string());
                }
            }
            if !removed.is_empty() {
                write_shared(&fold_dir, &shared).map_err(io_error)?;
                // Only the lines fold added for these paths leave `.fold/exclude`.
                let mut own = added_excludes(&fold_dir).map_err(io_error)?;
                let lines: Vec<String> = removed
                    .iter()
                    .map(|path| format!("/{path}"))
                    .filter(|line| own.contains(line))
                    .collect();
                if !lines.is_empty() {
                    remove_exclude_patterns(&fold_dir, &lines)?;
                    own.retain(|line| !lines.contains(line));
                    write_added_excludes(&fold_dir, &own).map_err(io_error)?;
                }
            }
            Ok(removed)
        })
        .await?;
        tracing::info!(removed = removed.len(), "paths no longer shared");
        Ok(removed)
    }

    /// The shared paths, and the ignored files and folders of the project folder that workers cannot see, by name only.
    pub async fn shared_state(&self) -> Result<SharedState, WorkspaceError> {
        let paths = self.paths.clone();
        blocking(move || {
            let shared = read_shared(&paths.fold_dir()).map_err(io_error)?;
            let rules = IgnoreRules::new(paths.root());
            let (unseen, unseen_count) =
                unshared_ignored(paths.root(), &rules, &shared, LISTED_UNSEEN).map_err(io_error)?;
            Ok(SharedState {
                shared: shared.iter().map(ToString::to_string).collect(),
                unseen,
                unseen_count,
            })
        })
        .await
    }

    /// The names of the shared paths, which a worker reads through the links in its workspace.
    pub async fn shared_names(&self) -> Result<Vec<String>, WorkspaceError> {
        let fold_dir = self.paths.fold_dir();
        blocking(move || {
            Ok(read_shared(&fold_dir)
                .map_err(io_error)?
                .iter()
                .map(ToString::to_string)
                .collect())
        })
        .await
    }

    async fn may_share(&self, actor: &Actor) -> Result<(), WorkspaceError> {
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
        policy::can_share_paths(&Starter::of(actor, supervised)).map_err(FoldError::from)?;
        Ok(())
    }
}

/// Writes of `.fold/shared` take turns, so two shares never lose one of their paths.
static SHARED_WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Makes the links of the project's shared paths in the workspace directory `dir`, as [`link_shared`] does, at a create or a sync. A link that cannot be made never fails the create or the sync: the worker reads the path through the fold tools' errors instead, and the log names only counts. Blocking.
pub(super) fn link_shared_in(root: &Path, dir: &Path) {
    let fold_dir = root.join(".fold");
    let shared = match read_shared(&fold_dir) {
        Ok(shared) => shared,
        Err(error) => {
            tracing::warn!(kind = %error.kind(), "could not read .fold/shared");
            return;
        }
    };
    let root = root.canonicalize().unwrap_or_else(|_| root.to_owned());
    let rules = IgnoreRules::new(&root);
    match link_shared(&root, &rules, dir, &shared) {
        Ok(linked) if linked.skipped > 0 => {
            tracing::info!(
                linked = linked.linked,
                skipped = linked.skipped,
                "some shared paths got no link in a workspace"
            );
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(kind = %error.kind(), "could not link the shared paths in a workspace")
        }
    }
}

fn parse_paths(paths: &[String]) -> Result<Vec<RepoPath>, WorkspaceError> {
    if paths.is_empty() || paths.len() > MAX_PATHS {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!("give 1 to {MAX_PATHS} paths"),
        )
        .with_details([("field", json!("paths"))])
        .into());
    }
    paths
        .iter()
        .enumerate()
        .map(|(at, path)| {
            RepoPath::parse(path.trim_end_matches('/').as_bytes()).map_err(|_| {
                FoldError::new(
                    codes::INVALID_PATH,
                    format!("path {} is no relative, normalised path inside the project folder", at + 1),
                )
                .with_details([("field", json!("paths")), ("index", json!(at))])
                .with_hint("give each path relative to the project folder, such as design/ or docs/spec.pdf, without .. and outside .fold and .git")
                .into()
            })
        })
        .collect()
}

fn refusal(at: usize, path: &RepoPath, reason: NotShareable) -> FoldError {
    let (name, message, hint) = match reason {
        NotShareable::Missing => (
            "missing",
            format!("{path} is not in the project folder"),
            "share a file or folder that is there",
        ),
        NotShareable::Link => (
            "link",
            format!("{path} or a folder above it is a symbolic link"),
            "share the file or folder the link leads to, inside the project folder",
        ),
        NotShareable::NotIgnored => (
            "not_ignored",
            format!("{path} is tracked: no ignore rule leaves it out"),
            "every workspace holds a tracked file already; share only a path that .gitignore, .foldignore or .fold/exclude leaves out",
        ),
        NotShareable::Overlaps => (
            "overlaps",
            format!("{path} lies within or holds another shared path"),
            "run fld share --list, and unshare the other path first",
        ),
    };
    FoldError::new(codes::INVALID_REQUEST, message)
        .with_details([
            ("field", json!("paths")),
            ("index", json!(at)),
            ("reason", json!(name)),
        ])
        .with_hint(hint)
}

fn io_error(error: std::io::Error) -> FoldError {
    FoldError::new(
        codes::WORKSPACE_IO,
        format!("cannot read or write .fold/shared: {}", error.kind()),
    )
}
