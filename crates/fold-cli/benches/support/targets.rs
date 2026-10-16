/// The five runtime targets the bench measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    IdleMemory,
    FullStatScan,
    ChangeVisibility,
    WorkspaceCreation,
    Integration,
}

impl Target {
    pub fn name(self) -> &'static str {
        match self {
            Target::IdleMemory => "idle_memory",
            Target::FullStatScan => "full_stat_scan",
            Target::ChangeVisibility => "change_visibility",
            Target::WorkspaceCreation => "workspace_creation",
            Target::Integration => "integration",
        }
    }

    pub fn unit(self) -> &'static str {
        match self {
            Target::IdleMemory => "bytes",
            _ => "s",
        }
    }
}
