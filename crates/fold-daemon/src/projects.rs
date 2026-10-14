use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use fold_core::{Clock, FoldError, ProjectPaths, codes};
use fold_store::Store;
use fold_store::projects::ProjectRow;

use crate::services::events::EventBus;

/// An open project: its database, its clock and the live state shared by every request. Later domains add their state here.
pub struct ProjectHandle {
    pub slug: String,
    pub name: String,
    pub paths: ProjectPaths,
    pub store: Store,
    /// Services and jobs read the time only through this clock.
    pub clock: Arc<dyn Clock>,
    pub bus: EventBus,
    /// Held for the whole integration procedure.
    pub integration: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for ProjectHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectHandle")
            .field("slug", &self.slug)
            .finish_non_exhaustive()
    }
}

impl ProjectHandle {
    /// Opens the project database, which applies its pending migrations. A missing database is not created: the registry only lists projects that `fold init` set up.
    pub async fn open(row: &ProjectRow, clock: Arc<dyn Clock>) -> Result<Self, FoldError> {
        let paths = ProjectPaths::new(&row.root);
        let present = tokio::fs::try_exists(paths.db()).await.unwrap_or(false);
        if !present {
            tracing::warn!(project = %row.slug, "project database is missing");
            return Err(unavailable(&row.slug));
        }
        let store = Store::open_project(&paths).await.map_err(|error| {
            tracing::warn!(project = %row.slug, code = error.code().unwrap_or("store_error"), %error, "project failed to open");
            unavailable(&row.slug)
        })?;
        Ok(Self {
            slug: row.slug.clone(),
            name: row.name.clone(),
            paths,
            store,
            clock,
            bus: EventBus::new(),
            integration: tokio::sync::Mutex::new(()),
        })
    }
}

fn unavailable(slug: &str) -> FoldError {
    FoldError::new(
        codes::PROJECT_UNAVAILABLE,
        format!("project {slug} is unavailable"),
    )
}

/// What the daemon knows about a registered project.
#[derive(Debug, Clone)]
pub enum Slot {
    Open(Arc<ProjectHandle>),
    /// The open failed; the other projects keep running.
    Unavailable,
}

/// The projects the daemon has tried to open, by slug.
#[derive(Debug, Default)]
pub struct Projects {
    slots: RwLock<HashMap<String, Slot>>,
    /// Serialises opens so one database never gets two writers.
    opening: tokio::sync::Mutex<()>,
}

impl Projects {
    pub fn slot(&self, slug: &str) -> Option<Slot> {
        self.slots
            .read()
            .ok()
            .and_then(|slots| slots.get(slug).cloned())
    }

    pub fn handles(&self) -> Vec<Arc<ProjectHandle>> {
        let Ok(slots) = self.slots.read() else {
            return Vec::new();
        };
        slots
            .values()
            .filter_map(|slot| match slot {
                Slot::Open(handle) => Some(handle.clone()),
                Slot::Unavailable => None,
            })
            .collect()
    }

    /// Opens `row` unless it is already open, in which case the same handle comes back.
    pub async fn open(
        &self,
        row: &ProjectRow,
        clock: Arc<dyn Clock>,
    ) -> Result<Arc<ProjectHandle>, FoldError> {
        let _opening = self.opening.lock().await;
        if let Some(Slot::Open(handle)) = self.slot(&row.slug) {
            return Ok(handle);
        }
        let outcome = ProjectHandle::open(row, clock).await.map(Arc::new);
        let slot = match &outcome {
            Ok(handle) => Slot::Open(handle.clone()),
            Err(_) => Slot::Unavailable,
        };
        if let Ok(mut slots) = self.slots.write() {
            slots.insert(row.slug.clone(), slot);
        }
        outcome
    }
}
