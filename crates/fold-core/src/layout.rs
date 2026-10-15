use std::path::{Path, PathBuf};

use crate::error::{FoldError, codes};
use crate::ids::{ProposalId, TaskId, WorkspaceId};

/// Where the host-wide files live: `<data home>/fold/`. The data home is `XDG_DATA_HOME`, else `<home>/.local/share`. An empty or relative value counts as unset, as the XDG base directory rules require.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPaths {
    fold_dir: PathBuf,
}

impl HostPaths {
    pub fn new(xdg_data_home: Option<&str>, home: Option<&str>) -> Result<Self, FoldError> {
        let data_home = match (absolute(xdg_data_home), absolute(home)) {
            (Some(data), _) => PathBuf::from(data),
            (None, Some(home)) => Path::new(home).join(".local").join("share"),
            (None, None) => {
                return Err(FoldError::new(
                    codes::INVALID_HOST_PATHS,
                    "neither XDG_DATA_HOME nor HOME is set to an absolute path",
                ));
            }
        };
        Ok(Self {
            fold_dir: data_home.join("fold"),
        })
    }

    pub fn registry_db(&self) -> PathBuf {
        self.fold_dir.join("registry.db")
    }
}

fn absolute(value: Option<&str>) -> Option<&str> {
    value.filter(|text| Path::new(text).is_absolute())
}

/// Where the files of one project live. Every method only joins names onto the root; nothing here touches the disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPaths {
    root: PathBuf,
}

impl ProjectPaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn fold_dir(&self) -> PathBuf {
        self.root.join(".fold")
    }

    pub fn db(&self) -> PathBuf {
        self.fold_dir().join("fold.db")
    }

    pub fn objects(&self) -> PathBuf {
        self.fold_dir().join("objects")
    }

    pub fn index(&self) -> PathBuf {
        self.fold_dir().join("index")
    }

    pub fn config(&self) -> PathBuf {
        self.fold_dir().join("config.toml")
    }

    /// The read-only copy of the current main revision.
    pub fn main_dir(&self) -> PathBuf {
        self.root.join("main")
    }

    /// The parent of every workspace and review checkout directory.
    pub fn ws_dir(&self) -> PathBuf {
        self.root.join("ws")
    }

    /// The stat index of one workspace. It is named by the public id, which never repeats, rather than by the directory.
    pub fn workspace_index(&self, workspace: WorkspaceId) -> PathBuf {
        self.index().join(format!("{workspace}.idx"))
    }
}

/// The directory of a task workspace relative to the project root, as `workspaces.path` stores it. `earlier` counts the task workspaces the task had before this one: none gives `ws/t-12`, one gives `ws/t-12-2`. The name carries no worker, so it never changes when the workspace changes hands.
pub fn task_workspace_path(task: TaskId, earlier: u32) -> String {
    let name = task.to_string().to_ascii_lowercase();
    match earlier {
        0 => format!("ws/{name}"),
        _ => format!("ws/{name}-{}", u64::from(earlier) + 1),
    }
}

/// The directory of a review checkout relative to the project root. `earlier` counts the review checkouts the proposal had before this one: none gives `ws/review-p-4`, one gives `ws/review-p-4-2`. Like a task workspace's name it carries no worker and no version, so it never changes when the checkout changes hands.
pub fn review_checkout_path(proposal: ProposalId, earlier: u32) -> String {
    let name = proposal.to_string().to_ascii_lowercase();
    match earlier {
        0 => format!("ws/review-{name}"),
        _ => format!("ws/review-{name}-{}", u64::from(earlier) + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_paths_fall_back_to_home() {
        let xdg = HostPaths::new(Some("/data"), Some("/home/ana")).unwrap();
        assert_eq!(xdg.registry_db(), Path::new("/data/fold/registry.db"));
        let home = HostPaths::new(None, Some("/home/ana")).unwrap();
        assert_eq!(
            home.registry_db(),
            Path::new("/home/ana/.local/share/fold/registry.db")
        );
        let empty = HostPaths::new(Some(""), Some("/home/ana")).unwrap();
        assert_eq!(empty, home);
        let error = HostPaths::new(None, None).unwrap_err();
        assert_eq!(error.code(), "invalid_host_paths");
        assert!(HostPaths::new(Some(""), Some("")).is_err());
        let relative = HostPaths::new(Some("data"), Some("/home/ana")).unwrap();
        assert_eq!(relative, home);
        assert!(HostPaths::new(None, Some("home/ana")).is_err());
    }

    #[test]
    fn every_path_hangs_off_the_root() {
        let paths = ProjectPaths::new("/srv/demo");
        assert_eq!(paths.root(), Path::new("/srv/demo"));
        assert_eq!(paths.fold_dir(), Path::new("/srv/demo/.fold"));
        assert_eq!(paths.db(), Path::new("/srv/demo/.fold/fold.db"));
        assert_eq!(paths.objects(), Path::new("/srv/demo/.fold/objects"));
        assert_eq!(paths.index(), Path::new("/srv/demo/.fold/index"));
        assert_eq!(paths.config(), Path::new("/srv/demo/.fold/config.toml"));
        assert_eq!(paths.main_dir(), Path::new("/srv/demo/main"));
        assert_eq!(paths.ws_dir(), Path::new("/srv/demo/ws"));
        let workspace = WorkspaceId::from_number(9).unwrap();
        assert_eq!(
            paths.workspace_index(workspace),
            Path::new("/srv/demo/.fold/index/ws-9.idx")
        );
    }

    #[test]
    fn task_workspace_paths_count_the_earlier_workspaces() {
        let task = TaskId::from_number(12).unwrap();
        assert_eq!(task_workspace_path(task, 0), "ws/t-12");
        assert_eq!(task_workspace_path(task, 1), "ws/t-12-2");
        assert_eq!(task_workspace_path(task, 2), "ws/t-12-3");
        assert_eq!(
            task_workspace_path(task, u32::MAX),
            format!("ws/t-12-{}", u64::from(u32::MAX) + 1)
        );
    }

    #[test]
    fn review_checkout_paths_count_the_earlier_checkouts() {
        let proposal = ProposalId::from_number(4).unwrap();
        assert_eq!(review_checkout_path(proposal, 0), "ws/review-p-4");
        assert_eq!(review_checkout_path(proposal, 1), "ws/review-p-4-2");
        assert_eq!(review_checkout_path(proposal, 2), "ws/review-p-4-3");
        assert_eq!(
            review_checkout_path(proposal, u32::MAX),
            format!("ws/review-p-4-{}", u64::from(u32::MAX) + 1)
        );
    }
}
