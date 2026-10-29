use std::sync::Arc;

use fold_core::layout::{SKILLS_DIR, is_fold_skill_file};
use fold_core::object::ObjectHash;
use fold_core::{Actor, FoldError, RevisionId, codes, policy};
use fold_store::StoreError;
use fold_store::workspaces::{
    self, StashState, conflicted_human_change, open_unstash, stash_stack,
};
use fold_vcs::compare::{load_flat, load_flat_below};
use fold_vcs::git::{CommitId, Git};
use fold_vcs::git_remote::{BranchState, exclude_claude_files};
use fold_vcs::git_sync_map::SyncMap;
use fold_vcs::ignore::IgnoreRules;
#[cfg(doc)]
use fold_vcs::ignore::{EXCLUDE_NOTE, claude_excludes};
use fold_vcs::objects::ObjectStore;
use fold_vcs::paths::RepoPath;
use fold_vcs::revision_store::write_initial_tree;
use serde_json::json;

use super::folder::FolderFollowed;
use super::pull::project_error;
use super::record::{HumanFolderChange, OnConflict, RecordOutcome, integrate_change, sync};
use super::remote::blocking;
use super::stash::{AfterUnstash, Finish, finish_open_unstash, refuse_branch, spawned};
use super::{ProjectError, TouchedTask, load_config};
use crate::projects::ProjectHandle;
use crate::services::reviews::{PullOutcome, RecordAuthor, RecordIntegration, ReviewService};
use crate::services::workspaces::WorkspaceService;

/// What `fld untrack` recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UntrackOutcome {
    /// The new revision, which no longer holds the paths.
    pub revision: RevisionId,
    /// The Git commit of `revision` while Git sync is on.
    pub commit: Option<CommitId>,
    /// The tracked paths the revision removes, in path order.
    pub files: Vec<String>,
    /// The paths of `files` that no ignore rule matches, so the next `fld record` records them again.
    pub unignored: Vec<String>,
    pub touched: Vec<TouchedTask>,
}

/// Stops tracking `paths` in the project folder, as `git rm --cached` and a commit would: one revision of the named human removes every path main tracks at or below each of them, and the files stay in the folder as they are. A directory stands for every tracked path below it.
///
/// The command runs as `fld record` does, under the project's remote lock and folder lock: the folder follows main first, an unstash that a stop left part-way is finished, and a branch or `HEAD` that shows a change fold did not make, or commits of the human that wait for the main agent, refuse it. The change goes from the checkout record to the record without the paths, so the folder's other changes that no revision records stay unrecorded, and it merges onto main under the integration mutex as an integration of kind `record`, with `human_change.recorded` in its transaction. The record then holds the change laid over it, so the follow after the integration leaves the files where they are; while Git sync is on, the revision's Git commit is written before the command returns: the `[git]` branch moves to it and the files' entries leave Git's index through `git update-index`, which no ignore rule affects, and the files in the work tree are never written, so Git sees them as untracked. When the index holds a change the human staged on such a path, the branch and the index stay where they are, as for any revision. A path that main changed since the folder's revision conflicts with the removal, and the command is refused with `reason` `main_changed` instead of waiting for the main agent; a path main does not track is refused with `path_not_tracked`. Older revisions keep the files.
pub async fn untrack(
    handle: &Arc<ProjectHandle>,
    actor: &Actor,
    git: &Git,
    paths: &[String],
) -> Result<AfterUnstash<UntrackOutcome>, ProjectError> {
    policy::can_record(actor).map_err(FoldError::from)?;
    let Actor::Human { .. } = actor else {
        return Err(FoldError::new(
            codes::HUMAN_REQUIRED,
            "only a human stops tracking files of the project folder",
        )
        .with_hint("ask a human to run `fld untrack`")
        .into());
    };
    let wanted = parse_paths(paths)?;
    let (handle, actor, git) = (Arc::clone(handle), actor.clone(), git.clone());
    spawned(async move { run(&handle, &actor, &git, wanted).await }).await
}

fn parse_paths(paths: &[String]) -> Result<Vec<RepoPath>, FoldError> {
    if paths.is_empty() {
        return Err(
            FoldError::new(codes::INVALID_REQUEST, "name at least one path")
                .with_details([("field", json!("paths"))]),
        );
    }
    paths
        .iter()
        .map(|path| {
            RepoPath::parse(path.trim_end_matches('/').as_bytes()).map_err(|_| {
                FoldError::new(
                    codes::INVALID_PATH,
                    format!("{path:?} is not a path inside the project folder"),
                )
                .with_hint("name a file or directory relative to the project folder")
                .with_details([("field", json!("paths")), ("path", json!(path))])
            })
        })
        .collect()
}

async fn run(
    handle: &Arc<ProjectHandle>,
    actor: &Actor,
    git: &Git,
    wanted: Vec<RepoPath>,
) -> Result<AfterUnstash<UntrackOutcome>, ProjectError> {
    let Actor::Human { name: human } = actor else {
        return Err(FoldError::new(codes::HUMAN_REQUIRED, "only a human untracks").into());
    };
    let config = load_config(&handle.paths).await?;
    let _remote = handle.remote_lock.clone().lock_owned().await;
    let _folder = handle.folder_lock.clone().lock_owned().await;
    let mut unstash = None;
    loop {
        let (followed, _) = sync(handle, git, false).await?;
        refuse_branch(&config.git.branch, &followed)?;
        match finish_open_unstash(handle, config.max_file_bytes).await? {
            Finish::Nothing => break,
            Finish::Abandoned => {}
            Finish::Finished(outcome) => unstash = Some(outcome),
        }
    }
    let record_tree = workspaces::checkout(handle.store.read())
        .await?
        .ok_or(StoreError::Corrupt("checkout"))?
        .tree;
    let head = workspaces::head(handle.store.read())
        .await?
        .ok_or(StoreError::Corrupt("refs.main"))?;
    let objects_guard = handle.objects.clone().read_owned().await;
    let paths = handle.paths.clone();
    let head_tree = head.tree_hash;
    let (tree, files, unignored) = blocking(move || {
        let _objects = objects_guard;
        let objects = ObjectStore::open_existing(&paths.fold_dir())?;
        let mut record = load_flat(&objects, &record_tree)?;
        let main = load_flat(&objects, &head_tree)?;
        let untracked: Vec<String> = wanted
            .iter()
            .filter(|path| path.at_or_below(&main).next().is_none())
            .map(ToString::to_string)
            .collect();
        if !untracked.is_empty() {
            return Err(not_tracked(&untracked));
        }
        // Main tracks the path but the folder's record does not yet: a follow that waits holds main's version back.
        let behind: Vec<String> = wanted
            .iter()
            .filter(|path| path.at_or_below(&record).next().is_none())
            .map(ToString::to_string)
            .collect();
        if !behind.is_empty() {
            return Err(main_changed(&behind));
        }
        let mut files = Vec::new();
        for path in &wanted {
            let below: Vec<Vec<u8>> = path
                .at_or_below(&record)
                .map(|(found, _)| found.clone())
                .collect();
            for found in below {
                record.remove(&found);
                files.push(String::from_utf8_lossy(&found).into_owned());
            }
        }
        files.sort();
        files.dedup();
        let rules = IgnoreRules::new(paths.root());
        let unignored = files
            .iter()
            .filter(|file| {
                RepoPath::parse(file.as_bytes()).is_ok_and(|path| !rules.is_ignored(&path, false))
            })
            .cloned()
            .collect();
        let tree = write_initial_tree(&objects, record)?;
        Ok((tree, files, unignored))
    })
    .await?;
    let change = HumanFolderChange {
        human,
        base_tree: record_tree,
        tree,
        commit: None,
        title: Some(title(&files)),
    };
    let RecordOutcome::Recorded {
        revision, touched, ..
    } = integrate_change(handle, actor, &change, OnConflict::Refuse).await?
    else {
        return Err(FoldError::new(
            codes::INTERNAL_ERROR,
            "the removal neither reached main nor was refused",
        )
        .into());
    };
    sync(handle, git, false).await?;
    let fold_dir = handle.paths.fold_dir();
    let commit = blocking(move || SyncMap::load(&fold_dir))
        .await
        .ok()
        .and_then(|map| map.commit_of(revision).cloned());
    tracing::info!(revision = %revision, files = files.len(), "the human stopped tracking files");
    Ok(AfterUnstash {
        outcome: UntrackOutcome {
            revision,
            commit,
            files,
            unignored,
            touched,
        },
        unstash,
    })
}

/// The title of fold's own revision that stops tracking its skill files.
pub const FOLD_SKILLS_TITLE: &str = "Stop tracking fold's skill files";

/// What [`keep_fold_skills_out`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FoldSkills {
    /// Main tracks none of fold's skill files that the ignore rules leave out.
    Untracked,
    /// fold's revision stopped tracking `files`, in path order.
    Stopped {
        revision: RevisionId,
        /// The Git commit of `revision` while Git sync is on.
        commit: Option<CommitId>,
        files: Vec<String>,
    },
    /// The project folder is not ready for the revision, for the reason named; main still tracks the files and the next open tries again.
    Waits(&'static str),
}

/// Keeps the Claude Code files that belong to no revision out of main, as fold itself when the daemon opens a project (`policy::can_untrack_fold_skills`). `.fold/exclude` first gets [`EXCLUDE_NOTE`] at its top when it does not hold it, and each line of [`claude_excludes`] it does not list after the lines already there, so a line a person removed comes back at the next open. Then, when main tracks files below a folder of fold's own skills (`layout::is_fold_skill_file`), which reads only main's `.claude/skills` folder, that the ignore rules leave out now, so not one that a `!` line of a `.foldignore` brings back, fold makes one revision titled [`FOLD_SKILLS_TITLE`] that stops tracking them, as `fld untrack` stops tracking the files a human names: under the project's remote lock and folder lock, after the folder follows main, the change goes from the checkout record to the record without the files and merges onto main through the integration procedure as an integration of kind `record` of `fold`. No human made it, so it records no human change; its transaction gives the checkout record the change's tree instead, so the follow after it leaves the files in the folder, and while Git sync is on, the revision's commit removes them from the branch and the index only. A tracked `.claude/settings.local.json` or any other file of `.claude` stays tracked.
///
/// A folder that is not ready is left as it is, with [`FoldSkills::Waits`]: a branch or `HEAD` that shows a change fold did not make, commits of the human that wait, paths held by main, a stash conflict or an unstash that stopped part-way, a human change that waits for the main agent, a folder that did not follow, or a file whose entry in the record is not main's. Once main tracks none of the files, the next open finds nothing to do.
pub async fn keep_fold_skills_out(
    handle: &Arc<ProjectHandle>,
    actor: &Actor,
    git: &Git,
) -> Result<FoldSkills, ProjectError> {
    policy::can_untrack_fold_skills(actor).map_err(FoldError::from)?;
    let fold_dir = handle.paths.fold_dir();
    let added = blocking(move || exclude_claude_files(&fold_dir)).await?;
    if !added.is_empty() {
        tracing::info!(
            added = added.len(),
            "fold's Claude Code lines added to .fold/exclude"
        );
    }
    let head = workspaces::head(handle.store.read())
        .await?
        .ok_or(StoreError::Corrupt("refs.main"))?;
    if fold_skill_files(handle, head.tree_hash).await?.is_empty() {
        return Ok(FoldSkills::Untracked);
    }
    let (handle, git) = (Arc::clone(handle), git.clone());
    spawned(async move { stop_tracking_fold_skills(&handle, &git).await }).await
}

/// The files of main's tree `tree` below a folder of fold's own skills that the folder's ignore rules leave out.
async fn fold_skill_files(
    handle: &ProjectHandle,
    tree: ObjectHash,
) -> Result<Vec<Vec<u8>>, ProjectError> {
    let objects_guard = handle.objects.clone().read_owned().await;
    let paths = handle.paths.clone();
    blocking(move || {
        let _objects = objects_guard;
        let objects = ObjectStore::open_existing(&paths.fold_dir())?;
        let skills = load_flat_below(&objects, &tree, SKILLS_DIR.as_bytes())?;
        let rules = IgnoreRules::new(paths.root());
        Ok(skills
            .keys()
            .filter(|path| is_fold_skill_file(path))
            .filter(|path| RepoPath::parse(path).is_ok_and(|path| rules.is_ignored(&path, false)))
            .cloned()
            .collect())
    })
    .await
}

async fn stop_tracking_fold_skills(
    handle: &Arc<ProjectHandle>,
    git: &Git,
) -> Result<FoldSkills, ProjectError> {
    let _remote = handle.remote_lock.clone().lock_owned().await;
    let _folder = handle.folder_lock.clone().lock_owned().await;
    let (followed, _) = sync(handle, git, false).await?;
    if let Some(waits) = folder_waits(handle, &followed).await? {
        return Ok(FoldSkills::Waits(waits));
    }
    let checkout = workspaces::checkout(handle.store.read())
        .await?
        .ok_or(StoreError::Corrupt("checkout"))?;
    let head = workspaces::head(handle.store.read())
        .await?
        .ok_or(StoreError::Corrupt("refs.main"))?;
    let wanted = fold_skill_files(handle, head.tree_hash).await?;
    if wanted.is_empty() {
        return Ok(FoldSkills::Untracked);
    }
    let objects_guard = handle.objects.clone().read_owned().await;
    let paths = handle.paths.clone();
    let (record_tree, head_tree) = (checkout.tree, head.tree_hash);
    let removal = blocking(move || {
        let _objects = objects_guard;
        let objects = ObjectStore::open_existing(&paths.fold_dir())?;
        let mut record = load_flat(&objects, &record_tree)?;
        let main = load_flat(&objects, &head_tree)?;
        // The removal is based on the record, so it removes main's version of each file only where the record holds that version.
        if wanted.iter().any(|path| record.get(path) != main.get(path)) {
            return Ok(None);
        }
        for path in &wanted {
            record.remove(path);
        }
        let files: Vec<String> = wanted
            .iter()
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .collect();
        Ok(Some((write_initial_tree(&objects, record)?, files)))
    })
    .await?;
    let Some((tree, files)) = removal else {
        return Ok(FoldSkills::Waits("main_changed"));
    };
    let reviews = ReviewService::new(
        handle.store.clone(),
        handle.bus.clone(),
        handle.clock.clone(),
        WorkspaceService::for_project(handle),
        handle.integration.clone(),
    );
    let request = RecordIntegration {
        commit: None,
        base_tree: record_tree,
        tree,
        by: RecordAuthor::Fold { checkout },
        title: Some(FOLD_SKILLS_TITLE.to_owned()),
    };
    let revision = match reviews
        .integrate_record(&Actor::System, request)
        .await
        .map_err(project_error)?
    {
        PullOutcome::Succeeded { revision } => revision,
        PullOutcome::Conflicted(_) => return Ok(FoldSkills::Waits("main_changed")),
    };
    sync(handle, git, false).await?;
    let fold_dir = handle.paths.fold_dir();
    let commit = blocking(move || SyncMap::load(&fold_dir))
        .await
        .ok()
        .and_then(|map| map.commit_of(revision).cloned());
    tracing::info!(revision = %revision, files = files.len(), "fold stopped tracking its skill files");
    Ok(FoldSkills::Stopped {
        revision,
        commit,
        files,
    })
}

/// Why the project folder is not ready for fold's own change after its follow, if it is not.
async fn folder_waits(
    handle: &ProjectHandle,
    followed: &FolderFollowed,
) -> Result<Option<&'static str>, ProjectError> {
    match &followed.branch {
        Some(BranchState::Moved(_)) => return Ok(Some("head_moved")),
        Some(BranchState::Ahead { .. }) => return Ok(Some("commits_waiting")),
        _ => {}
    }
    if !followed.held.is_empty() {
        return Ok(Some("folder_held"));
    }
    if followed.at.is_none() {
        return Ok(Some("folder_behind"));
    }
    let mut conn = handle
        .store
        .read()
        .acquire()
        .await
        .map_err(StoreError::from)?;
    if open_unstash(&mut conn).await?.is_some()
        || stash_stack(&mut conn)
            .await?
            .iter()
            .any(|entry| entry.state == StashState::Conflicted)
    {
        return Ok(Some("unstash"));
    }
    if conflicted_human_change(&mut conn).await?.is_some() {
        return Ok(Some("human_change"));
    }
    Ok(None)
}

/// The longest title that names the files; a longer list is counted.
const TITLE_CHARS: usize = 72;

/// The revision's title, which the Git commit, `fld log` and the web app show: the files when they fit on one short line, else their count.
fn title(files: &[String]) -> String {
    let named = format!("Stop tracking {}", files.join(", "));
    if named.chars().count() <= TITLE_CHARS && !named.chars().any(char::is_control) {
        named
    } else if files.len() == 1 {
        "Stop tracking 1 file".to_owned()
    } else {
        format!("Stop tracking {} files", files.len())
    }
}

fn main_changed(paths: &[String]) -> FoldError {
    FoldError::new(
        codes::INVALID_REQUEST,
        format!(
            "the project folder does not hold main's version of {} yet",
            paths.join(", ")
        ),
    )
    .with_hint("run `fld status` to see what keeps the project folder from main; once it follows main, run `fld untrack` again")
    .with_details([("reason", json!("main_changed")), ("paths", json!(paths))])
}

fn not_tracked(paths: &[String]) -> FoldError {
    FoldError::new(
        codes::PATH_NOT_TRACKED,
        format!("main does not track {}", paths.join(", ")),
    )
    .with_hint("name a file or directory that main records; `fld status` lists the tracked files that ignore rules match")
    .with_details([("paths", json!(paths))])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_names_the_files_while_they_fit_and_counts_them_after() {
        let named = |files: &[&str]| {
            title(
                &files
                    .iter()
                    .map(|file| (*file).to_owned())
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(named(&["a.txt", ".env"]), "Stop tracking a.txt, .env");
        let long: Vec<String> = (0..12).map(|n| format!("generated/file-{n}.rs")).collect();
        assert_eq!(title(&long), "Stop tracking 12 files");
        assert_eq!(named(&[&"x".repeat(80)]), "Stop tracking 1 file");
        assert_eq!(named(&["a\nb"]), "Stop tracking 1 file");
    }
}
