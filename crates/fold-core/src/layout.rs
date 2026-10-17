use std::path::{Path, PathBuf};

use crate::agents::AgentName;
use crate::error::{FoldError, codes};
use crate::ids::{ProposalId, ReviewBatchId, TaskId, WorkspaceId};
use crate::roles::Role;

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

    /// The project's own Git repository: one commit per integration on the `[git]` branch, which the project folder has checked out, so the folder's files are main. Every reader and writer of the repository takes its place from here.
    pub fn git_dir(&self) -> PathBuf {
        self.root.join(".git")
    }

    /// The parent of every workspace and review checkout directory: [`WS_DIR`] below the root, inside fold's data folder, so the project's Git repository never sees a workspace.
    pub fn ws_dir(&self) -> PathBuf {
        self.root.join(WS_DIR)
    }

    /// A project's own prompt for a worker role, which replaces the one that ships with fold.
    pub fn role_prompt(&self, role: Role) -> PathBuf {
        self.fold_dir()
            .join("roles")
            .join(format!("{}.md", role.as_str()))
    }

    /// A project's own prompt for the main agent, which replaces the one that ships with fold.
    pub fn main_role_prompt(&self) -> PathBuf {
        self.fold_dir().join("roles").join("main.md")
    }

    /// Where `fld claude` writes the shipped main agent prompt that it passes to `claude`. It sits beside the session records, and its `.` keeps it apart from them, as no agent name holds one.
    pub fn main_prompt_file(&self) -> PathBuf {
        self.fold_dir().join("sessions").join("prompt.md")
    }

    /// The files fold writes for one worker agent's Claude Code process: its MCP configuration and its role prompt.
    pub fn worker_dir(&self, agent: &AgentName) -> PathBuf {
        self.fold_dir().join("workers").join(agent.as_str())
    }

    /// The Claude Code session id of the last session that `fld claude` or `fld new` started as `agent`, which `fld claude --name <agent>` resumes. It holds that one id only, so each name continues its own session and never that of another.
    pub fn main_session(&self, agent: &AgentName) -> PathBuf {
        self.fold_dir().join("sessions").join(agent.as_str())
    }

    /// The MCP servers of the project that Claude Code reads.
    pub fn mcp_json(&self) -> PathBuf {
        self.root.join(".mcp.json")
    }

    /// The stat index of one workspace. It is named by the public id, which never repeats, rather than by the directory.
    pub fn workspace_index(&self, workspace: WorkspaceId) -> PathBuf {
        self.index().join(format!("{workspace}.idx"))
    }
}

/// The environment variable that names the agent of a fold Claude Code session: `fld claude` sets it for the main agent, the worker supervisor for each worker, and `fld mcp` and `fld hook` read it.
pub const AGENT_ENV: &str = "FOLD_AGENT";

/// The environment variable that lists the tags a session's agent registers with; `fld claude` sets `main` in it.
pub const AGENT_TAGS_ENV: &str = "FOLD_AGENT_TAGS";

/// The key of the servers object in an MCP configuration such as `.mcp.json`.
pub const MCP_SERVERS_KEY: &str = "mcpServers";

/// The key of fold's own server below `mcpServers`, in the project's `.mcp.json`, in a worker's MCP configuration and in a role's `mcp` list.
pub const MCP_SERVER_NAME: &str = "fold";

/// The `fld` subcommand that fold's MCP server entry runs: the stdio adapter to the daemon.
pub const MCP_COMMAND: &str = "mcp";

/// The parent of every workspace directory relative to the project root.
pub const WS_DIR: &str = ".fold/ws";

/// The directory of a task workspace relative to the project root, as `workspaces.path` stores it. `earlier` counts the task workspaces the task had before this one: none gives `.fold/ws/t-12`, one gives `.fold/ws/t-12-2`. The name carries no worker, so it never changes when the workspace changes hands.
pub fn task_workspace_path(task: TaskId, earlier: u32) -> String {
    let name = task.to_string().to_ascii_lowercase();
    match earlier {
        0 => format!("{WS_DIR}/{name}"),
        _ => format!("{WS_DIR}/{name}-{}", u64::from(earlier) + 1),
    }
}

/// The directory of a review checkout relative to the project root. `earlier` counts the review checkouts the proposal had before this one: none gives `.fold/ws/review-p-4`, one gives `.fold/ws/review-p-4-2`. Like a task workspace's name it carries no worker and no version, so it never changes when the checkout changes hands.
pub fn review_checkout_path(proposal: ProposalId, earlier: u32) -> String {
    let name = proposal.to_string().to_ascii_lowercase();
    match earlier {
        0 => format!("{WS_DIR}/review-{name}"),
        _ => format!("{WS_DIR}/review-{name}-{}", u64::from(earlier) + 1),
    }
}

/// The directory of a review batch's checkout, relative to the project root: `.fold/ws/review-rb-3`, then `.fold/ws/review-rb-3-2` when `earlier` checkouts of the batch exist, by the naming rule of a proposal's checkout.
pub fn review_batch_checkout_path(batch: ReviewBatchId, earlier: u32) -> String {
    let name = batch.to_string();
    match earlier {
        0 => format!("{WS_DIR}/review-{name}"),
        _ => format!("{WS_DIR}/review-{name}-{}", u64::from(earlier) + 1),
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
        assert_eq!(paths.git_dir(), Path::new("/srv/demo/.git"));
        assert_eq!(paths.ws_dir(), Path::new("/srv/demo/.fold/ws"));
        assert_eq!(
            paths.role_prompt(Role::TaskCoder),
            Path::new("/srv/demo/.fold/roles/task_coder.md")
        );
        assert_eq!(
            paths.main_role_prompt(),
            Path::new("/srv/demo/.fold/roles/main.md")
        );
        assert_eq!(
            paths.main_prompt_file(),
            Path::new("/srv/demo/.fold/sessions/prompt.md")
        );
        assert_eq!(
            paths.worker_dir(&AgentName::parse("web-lead").unwrap()),
            Path::new("/srv/demo/.fold/workers/web-lead")
        );
        assert_eq!(paths.mcp_json(), Path::new("/srv/demo/.mcp.json"));
        assert_eq!(
            paths.main_session(&AgentName::parse("main").unwrap()),
            Path::new("/srv/demo/.fold/sessions/main")
        );
        let workspace = WorkspaceId::from_number(9).unwrap();
        assert_eq!(
            paths.workspace_index(workspace),
            Path::new("/srv/demo/.fold/index/ws-9.idx")
        );
    }

    #[test]
    fn task_workspace_paths_count_the_earlier_workspaces() {
        let task = TaskId::from_number(12).unwrap();
        assert_eq!(task_workspace_path(task, 0), ".fold/ws/t-12");
        assert_eq!(task_workspace_path(task, 1), ".fold/ws/t-12-2");
        assert_eq!(task_workspace_path(task, 2), ".fold/ws/t-12-3");
        assert_eq!(
            task_workspace_path(task, u32::MAX),
            format!(".fold/ws/t-12-{}", u64::from(u32::MAX) + 1)
        );
    }

    #[test]
    fn review_batch_checkout_paths_count_the_earlier_checkouts() {
        let batch = ReviewBatchId::from_number(3).unwrap();
        assert_eq!(review_batch_checkout_path(batch, 0), ".fold/ws/review-rb-3");
        assert_eq!(
            review_batch_checkout_path(batch, 1),
            ".fold/ws/review-rb-3-2"
        );
    }

    #[test]
    fn review_checkout_paths_count_the_earlier_checkouts() {
        let proposal = ProposalId::from_number(4).unwrap();
        assert_eq!(review_checkout_path(proposal, 0), ".fold/ws/review-p-4");
        assert_eq!(review_checkout_path(proposal, 1), ".fold/ws/review-p-4-2");
        assert_eq!(review_checkout_path(proposal, 2), ".fold/ws/review-p-4-3");
        assert_eq!(
            review_checkout_path(proposal, u32::MAX),
            format!(".fold/ws/review-p-4-{}", u64::from(u32::MAX) + 1)
        );
    }
}
