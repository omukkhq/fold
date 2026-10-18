use std::collections::{BTreeMap, BTreeSet};

use fold_core::object::ObjectHash;
use fold_core::{AgentName, LabelName, TaskId, WorkspaceId};
use fold_store::{agents, tasks, workspaces};
use fold_vcs::compare::load_flat;
use fold_vcs::scan::changes;
use serde_json::json;

use super::{WorkspaceError, WorkspaceService, blocking, open_objects};

/// The most paths a human-change event lists; the rest are counted.
const LISTED_PATHS: usize = 50;

/// An open task workspace whose changes touch a file the human changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TouchedTask {
    pub task: TaskId,
    pub workspace: WorkspaceId,
}

/// What a human change touches when it reaches main or waits for the main agent: the files it changes, the open task workspaces whose own changes touch them, and the leads of their tasks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HumanChangeReach {
    /// In path order.
    pub files: Vec<String>,
    pub touched: Vec<TouchedTask>,
    pub leads: BTreeSet<AgentName>,
}

impl HumanChangeReach {
    /// What every human-change event carries for its notices: the human's Git commit, `null` for a change read from the project folder, the changed files, the open workspaces they touch and the leads of their tasks.
    pub(crate) fn event_data(&self, commit: Option<&str>) -> serde_json::Value {
        json!({
            "commit": commit,
            "files": self.files.iter().take(LISTED_PATHS).collect::<Vec<_>>(),
            "file_count": self.files.len(),
            "tasks": self.touched.iter().map(|touched| touched.task.to_string()).collect::<Vec<_>>(),
            "workspaces": self.touched.iter().map(|touched| touched.workspace.to_string()).collect::<Vec<_>>(),
            "leads": self.leads.iter().map(AgentName::as_str).collect::<Vec<_>>(),
        })
    }
}

impl WorkspaceService {
    /// What the human change from `base` to `tree` touches.
    pub(crate) async fn human_change_reach(
        &self,
        base: ObjectHash,
        tree: ObjectHash,
    ) -> Result<HumanChangeReach, WorkspaceError> {
        let files = self.changed_paths(base, tree).await?;
        let touched = self.touched_tasks(&files).await?;
        let leads = self.leads_of(&touched).await?;
        Ok(HumanChangeReach {
            files: files
                .iter()
                .map(|path| String::from_utf8_lossy(path).into_owned())
                .collect(),
            touched,
            leads,
        })
    }

    /// Whether `tree` holds every change of the human change from `base` to `human`: each path the human changed has the human's entry in `tree`, or is gone from it as the human removed it.
    pub(crate) async fn holds_human_change(
        &self,
        base: ObjectHash,
        human: ObjectHash,
        tree: ObjectHash,
    ) -> Result<bool, WorkspaceError> {
        let paths = self.paths.clone();
        blocking(move || {
            let objects = open_objects(&paths)?;
            let (human, tree, base) = (
                load_flat(&objects, &human)?,
                load_flat(&objects, &tree)?,
                load_flat(&objects, &base)?,
            );
            Ok(changes(&base, &human)
                .iter()
                .all(|change| human.get(&change.path) == tree.get(&change.path)))
        })
        .await
    }

    async fn changed_paths(
        &self,
        from: ObjectHash,
        to: ObjectHash,
    ) -> Result<Vec<Vec<u8>>, WorkspaceError> {
        let paths = self.paths.clone();
        blocking(move || {
            let objects = open_objects(&paths)?;
            Ok(
                changes(&load_flat(&objects, &from)?, &load_flat(&objects, &to)?)
                    .into_iter()
                    .map(|change| change.path)
                    .collect(),
            )
        })
        .await
    }

    /// The live task workspaces whose own changes touch one of `files`, by workspace: the changed files of the live state, which can lag a moment behind the files, and of the workspace's newest changeset against its base.
    async fn touched_tasks(&self, files: &[Vec<u8>]) -> Result<Vec<TouchedTask>, WorkspaceError> {
        let files: BTreeSet<&[u8]> = files.iter().map(Vec::as_slice).collect();
        let mut touched = Vec::new();
        for row in workspaces::list_live_task_workspaces(self.store.read()).await? {
            let Some(task) = row.task else {
                continue;
            };
            let mut own: Vec<Vec<u8>> = self
                .live
                .as_ref()
                .and_then(|live| live.get(row.id))
                .map(|live| {
                    live.view
                        .files
                        .iter()
                        .flat_map(|file| [Some(&file.path), file.previous_path.as_ref()])
                        .flatten()
                        .map(|path| path.as_bytes().to_vec())
                        .collect()
                })
                .unwrap_or_default();
            if let Some(changeset) = workspaces::latest_changeset(self.store.read(), row.id).await?
            {
                let base = workspaces::get_by_number(self.store.read(), changeset.base_revision)
                    .await?
                    .tree_hash;
                own.extend(self.changed_paths(base, changeset.tree).await?);
            }
            if own.iter().any(|path| files.contains(path.as_slice())) {
                touched.push(TouchedTask {
                    task,
                    workspace: row.id,
                });
            }
        }
        Ok(touched)
    }

    /// The leads of the areas whose label one of the touched tasks carries.
    async fn leads_of(
        &self,
        touched: &[TouchedTask],
    ) -> Result<BTreeSet<AgentName>, WorkspaceError> {
        if touched.is_empty() {
            return Ok(BTreeSet::new());
        }
        let mut by_label: BTreeMap<LabelName, AgentName> = BTreeMap::new();
        for area in agents::list_areas(self.store.read()).await? {
            if let Some(lead) = area.lead {
                by_label.insert(area.label, lead);
            }
        }
        let mut leads = BTreeSet::new();
        for touched in touched {
            for label in tasks::labels_for(self.store.read(), touched.task).await? {
                if let Some(lead) = by_label.get(&label.name) {
                    leads.insert(lead.clone());
                }
            }
        }
        Ok(leads)
    }
}
