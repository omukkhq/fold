use std::collections::{BTreeSet, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use fold_core::projects::notify::{
    DEFAULT_WINDOW_SECONDS, NoticeFilter, NoticeType, OFF_AFTER_FAILURES, TargetKind, check_name,
    check_transport, check_types, check_url, check_window, default_moves_to,
};
use fold_core::{FoldError, HumanName, NotifyTargetId, TaskStatus, Timestamp, codes};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::projects::ProjectHandle;
use crate::push::{read_private, remove_temporary, write_private};
use crate::support::lock;

/// A signing secret or a chat's webhook URL, which is a secret of its own. It has no `Display` and a redacted `Debug`, so it reaches no log line or error by formatting; only a send reads it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub(crate) fn new(text: String) -> Self {
        Self(text)
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Why fold turned a target off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OffReason {
    /// The receiver answered 410 Gone.
    Gone,
    /// [`OFF_AFTER_FAILURES`] tries failed in a row.
    Failures,
    /// A send failed every try of its retry schedule.
    GaveUp,
}

impl OffReason {
    /// What a person does about a target fold turned off for this reason, as its Needs you item says it.
    pub fn advice(self) -> &'static str {
        match self {
            OffReason::Gone => {
                "the receiver answered 410 Gone, so it says the webhook is removed: check the URL or give a new one, then send a test from the Settings tab or with `fld notify test`; a test that gets through turns the target on again"
            }
            OffReason::Failures => {
                "20 tries in a row failed: check the URL and the receiver, then send a test from the Settings tab or with `fld notify test`; a test that gets through turns the target on again"
            }
            OffReason::GaveUp => {
                "a send failed every try for about 8.6 hours: check the URL and the receiver, then send a test from the Settings tab or with `fld notify test`; a test that gets through turns the target on again"
            }
        }
    }
}

/// Whether a target gets notices: `on`; `off`, which a person chose; `gone`, as its receiver answered 410 Gone; or `failing`, as fold turned it off after its sends failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetState {
    On,
    Off,
    Gone,
    Failing,
}

impl TargetState {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetState::On => "on",
            TargetState::Off => "off",
            TargetState::Gone => "gone",
            TargetState::Failing => "failing",
        }
    }
}

/// How a target's sends went, across every project that sends to it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    /// Failed tries since the last one that got through.
    #[serde(default)]
    pub failures: u32,
    #[serde(default)]
    pub off: Option<OffReason>,
    #[serde(default)]
    pub off_at: Option<Timestamp>,
    #[serde(default)]
    pub last_ok_at: Option<Timestamp>,
    #[serde(default)]
    pub last_failed_at: Option<Timestamp>,
}

/// One notice target as its settings file keeps it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub id: NotifyTargetId,
    pub name: String,
    pub kind: TargetKind,
    /// A Slack or Discord URL holds the token of the channel, so only a send reads it.
    pub url: Secret,
    /// The `whsec_` secret of a generic target.
    #[serde(default)]
    pub secret: Option<Secret>,
    /// The secret a rotation replaced, which signs too until `previous_until`.
    #[serde(default)]
    pub previous_secret: Option<Secret>,
    #[serde(default)]
    pub previous_until: Option<Timestamp>,
    pub filter: NoticeFilter,
    pub window_seconds: u32,
    /// A person's switch; a target fold turned off has `health.off` instead.
    pub enabled: bool,
    pub created_at: Timestamp,
    /// The human who added the target.
    pub created_by: String,
    #[serde(default)]
    pub health: Health,
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl Target {
    /// Whether the target gets notices now: a person left it on and fold did not turn it off.
    pub fn active(&self) -> bool {
        self.enabled && self.health.off.is_none()
    }

    /// A target fold turned off shows why, whatever a person's switch says.
    pub fn state(&self) -> TargetState {
        match self.health.off {
            Some(OffReason::Gone) => TargetState::Gone,
            Some(OffReason::Failures | OffReason::GaveUp) => TargetState::Failing,
            None if self.enabled => TargetState::On,
            None => TargetState::Off,
        }
    }

    pub fn window(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.window_seconds))
    }

    /// The secrets every generic send signs with: the current one, and the previous one during a rotation.
    pub fn signing_secrets(&self, now: Timestamp) -> Vec<&str> {
        let mut secrets: Vec<&str> = self.secret.iter().map(Secret::expose).collect();
        if let (Some(previous), Some(until)) = (&self.previous_secret, self.previous_until)
            && now < until
        {
            secrets.push(previous.expose());
        }
        secrets
    }
}

/// Whose targets: the host's, which every project that uses the host targets also sends to, or one project's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Host,
    Project(String),
}

/// What a person sets on a new target or changes on one; a `None` keeps the value, or takes the default on a new target.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct TargetFields {
    pub name: Option<String>,
    pub url: Option<String>,
    pub types: Option<BTreeSet<NoticeType>>,
    pub moves_to: Option<BTreeSet<TaskStatus>>,
    pub window_seconds: Option<u32>,
    pub enabled: Option<bool>,
}

impl std::fmt::Debug for TargetFields {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TargetFields")
            .field("name", &self.name)
            .field("url", &self.url.as_ref().map(|_| "<redacted>"))
            .field("types", &self.types)
            .field("moves_to", &self.moves_to)
            .field("window_seconds", &self.window_seconds)
            .field("enabled", &self.enabled)
            .finish()
    }
}

fn checked_url(url: &str) -> Result<Secret, FoldError> {
    let url = url.trim();
    check_url(url)?;
    check_transport(url)?;
    Ok(Secret(url.to_owned()))
}

fn invalid(message: &str) -> FoldError {
    FoldError::new(codes::INVALID_NOTIFY_TARGET, message)
}

impl TargetFields {
    /// A new target once every field passed its check. A generic target gets `secret`.
    pub fn new_target(
        self,
        kind: TargetKind,
        id: NotifyTargetId,
        secret: Option<String>,
        by: &HumanName,
        now: Timestamp,
    ) -> Result<Target, FoldError> {
        let name = check_name(self.name.as_deref().unwrap_or_default())?;
        let url = checked_url(
            self.url
                .as_deref()
                .ok_or_else(|| invalid("a target needs its URL"))?,
        )?;
        let window_seconds = self.window_seconds.unwrap_or(DEFAULT_WINDOW_SECONDS);
        check_window(window_seconds)?;
        let defaults = NoticeFilter::default();
        let filter = NoticeFilter {
            types: self.types.unwrap_or(defaults.types),
            moves_to: self.moves_to.unwrap_or_else(default_moves_to),
        };
        check_types(&filter.types)?;
        Ok(Target {
            id,
            name,
            kind,
            url,
            secret: (kind == TargetKind::Webhook).then(|| Secret(secret.unwrap_or_default())),
            previous_secret: None,
            previous_until: None,
            filter,
            window_seconds,
            enabled: self.enabled.unwrap_or(true),
            created_at: now,
            created_by: by.to_string(),
            health: Health::default(),
        })
    }

    /// `target` with the fields set here, each checked. A changed target starts with a clean health, as a person looked at it.
    pub fn apply(self, target: &mut Target) -> Result<(), FoldError> {
        if let Some(name) = &self.name {
            target.name = check_name(name)?;
        }
        if let Some(url) = &self.url {
            target.url = checked_url(url)?;
        }
        if let Some(types) = self.types {
            check_types(&types)?;
            target.filter.types = types;
        }
        if let Some(moves_to) = self.moves_to {
            target.filter.moves_to = moves_to;
        }
        if let Some(window) = self.window_seconds {
            check_window(window)?;
            target.window_seconds = window;
        }
        if let Some(enabled) = self.enabled {
            target.enabled = enabled;
        }
        target.health = Health::default();
        Ok(())
    }
}

#[derive(Default, Serialize, Deserialize)]
struct HostFile {
    /// The number the next target of the host or of any project takes.
    #[serde(default)]
    next_number: u32,
    #[serde(default)]
    web_url: Option<String>,
    #[serde(default)]
    targets: Vec<Target>,
}

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    #[serde(default)]
    targets: Vec<Target>,
    /// Whether the host's targets get the project's notices too; on for a project that never set it.
    #[serde(default = "on")]
    use_host_targets: bool,
}

const fn on() -> bool {
    true
}

impl Default for ProjectFile {
    fn default() -> Self {
        Self {
            targets: Vec::new(),
            use_host_targets: true,
        }
    }
}

/// The host file has a folder of its own below the settings folder, as each project file has, because a private write narrows the folder it writes in and that folder's parent to 0700: the parent is then the settings folder, never the daemon's data folder.
const HOST_DIR: &str = "host";
const HOST_FILE: &str = "host.json";
const PROJECTS_DIR: &str = "projects";

fn settings_io(error: &io::Error) -> FoldError {
    FoldError::new(
        codes::NOTIFY_SETTINGS_IO,
        format!(
            "the notice settings could not be read or written: {}",
            error.kind()
        ),
    )
    .with_hint("check the daemon's data folder notices/: it must belong to this user and be readable only by it")
}

fn read_file<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> Result<T, FoldError> {
    match read_private(path).map_err(|error| settings_io(&error))? {
        None => Ok(T::default()),
        Some(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| settings_io(&io::Error::from(io::ErrorKind::InvalidData))),
    }
}

fn write_file<T: Serialize>(dir: &Path, name: &str, value: &T) -> Result<(), FoldError> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|_| settings_io(&io::Error::from(io::ErrorKind::InvalidData)))?;
    bytes.push(b'\n');
    write_private(dir, name, &bytes).map_err(|error| settings_io(&error))
}

/// A target with whose it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applying {
    pub target: Target,
    pub host: bool,
}

/// The notice settings of the host and of each project, in files of mode 0600 below a folder of mode 0700 in the daemon's data folder, outside every project folder, which agents can read. A file holds its targets with their secrets and health; the host file also holds the web app's address for links and the next target number. Only the daemon writes them, one change at a time.
#[derive(Clone)]
pub struct NotifySettings {
    dir: PathBuf,
    writing: Arc<tokio::sync::Mutex<()>>,
    /// Wakes each project's notifier when a person changed a target or asked for a send.
    wakes: Arc<std::sync::Mutex<HashMap<String, Arc<Notify>>>>,
    /// The projects the daemon opened, whose event streams a change of a host target reaches.
    peers: Arc<std::sync::Mutex<HashMap<String, Weak<ProjectHandle>>>>,
}

impl std::fmt::Debug for NotifySettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotifySettings")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

impl NotifySettings {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            writing: Arc::new(tokio::sync::Mutex::new(())),
            wakes: Arc::new(std::sync::Mutex::new(HashMap::new())),
            peers: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Keeps `handle` as the open project of its slug, in place of one opened before.
    pub fn register(&self, handle: &Arc<ProjectHandle>) {
        lock(&self.peers).insert(handle.slug.clone(), Arc::downgrade(handle));
    }

    /// The projects that are still open, in slug order.
    pub fn peers(&self) -> Vec<Arc<ProjectHandle>> {
        let mut peers = lock(&self.peers);
        peers.retain(|_, handle| handle.strong_count() > 0);
        let mut open: Vec<Arc<ProjectHandle>> = peers.values().filter_map(Weak::upgrade).collect();
        open.sort_by(|a, b| a.slug.cmp(&b.slug));
        open
    }

    /// Removes the temporary files of writes that stopped before their rename, which can hold a secret, from the host folder and the project folder.
    pub async fn remove_leftovers(&self) {
        for dir in [self.host_dir(), self.project_dir()] {
            let removed = tokio::task::spawn_blocking(move || remove_temporary(&dir)).await;
            if !matches!(removed, Ok(Ok(()))) {
                tracing::warn!(
                    "the notice settings folder kept a temporary file that could not be removed"
                );
            }
        }
    }

    /// What wakes the notifier of `slug`.
    pub fn waker(&self, slug: &str) -> Arc<Notify> {
        Arc::clone(lock(&self.wakes).entry(slug.to_owned()).or_default())
    }

    /// Wakes the notifier of `slug`, or of every project for `None`.
    pub fn wake(&self, slug: Option<&str>) {
        let wakes = lock(&self.wakes);
        for (project, wake) in wakes.iter() {
            if slug.is_none_or(|slug| slug == project) {
                wake.notify_one();
            }
        }
    }

    fn project_dir(&self) -> PathBuf {
        self.dir.join(PROJECTS_DIR)
    }

    fn host_dir(&self) -> PathBuf {
        self.dir.join(HOST_DIR)
    }

    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T, FoldError> + Send + 'static,
    ) -> Result<T, FoldError> {
        tokio::task::spawn_blocking(work)
            .await
            .map_err(|_| FoldError::new(codes::INTERNAL_ERROR, "a settings read stopped"))?
    }

    async fn host(&self) -> Result<HostFile, FoldError> {
        let path = self.host_dir().join(HOST_FILE);
        self.blocking(move || read_file(&path)).await
    }

    async fn project(&self, slug: &str) -> Result<ProjectFile, FoldError> {
        let path = self.project_dir().join(project_file(slug)?);
        self.blocking(move || read_file(&path)).await
    }

    /// The targets of `scope`, oldest first.
    pub async fn targets(&self, scope: &Scope) -> Result<Vec<Target>, FoldError> {
        Ok(match scope {
            Scope::Host => self.host().await?.targets,
            Scope::Project(slug) => self.project(slug).await?.targets,
        })
    }

    /// The targets that get the notices of `slug`: its own, then the host's while it uses them.
    pub async fn applying(&self, slug: &str) -> Result<Vec<Applying>, FoldError> {
        let file = self.project(slug).await?;
        let mut targets: Vec<Applying> = file
            .targets
            .into_iter()
            .map(|target| Applying {
                target,
                host: false,
            })
            .collect();
        if file.use_host_targets {
            targets.extend(
                self.host()
                    .await?
                    .targets
                    .into_iter()
                    .map(|target| Applying { target, host: true }),
            );
        }
        Ok(targets)
    }

    /// The target `id` of the project `slug` or of the host, with whether it is the host's.
    pub async fn find_for(&self, slug: &str, id: NotifyTargetId) -> Result<Applying, FoldError> {
        if let Some(target) = self
            .project(slug)
            .await?
            .targets
            .into_iter()
            .find(|target| target.id == id)
        {
            return Ok(Applying {
                target,
                host: false,
            });
        }
        self.host()
            .await?
            .targets
            .into_iter()
            .find(|target| target.id == id)
            .map(|target| Applying { target, host: true })
            .ok_or_else(|| not_found(id))
    }

    /// The target `id` among those of `scope`.
    pub async fn find(&self, scope: &Scope, id: NotifyTargetId) -> Result<Target, FoldError> {
        self.targets(scope)
            .await?
            .into_iter()
            .find(|target| target.id == id)
            .ok_or_else(|| not_found(id))
    }

    pub async fn use_host_targets(&self, slug: &str) -> Result<bool, FoldError> {
        Ok(self.project(slug).await?.use_host_targets)
    }

    pub async fn set_use_host_targets(&self, slug: &str, on: bool) -> Result<(), FoldError> {
        let name = project_file(slug)?;
        let _writing = self.writing.lock().await;
        let projects = self.project_dir();
        self.blocking(move || {
            let mut file: ProjectFile = read_file(&projects.join(&name))?;
            file.use_host_targets = on;
            write_file(&projects, &name, &file)
        })
        .await?;
        self.wake(Some(slug));
        Ok(())
    }

    /// The address of the web app that links start with, when a person set one.
    pub async fn web_url(&self) -> Result<Option<String>, FoldError> {
        Ok(self.host().await?.web_url)
    }

    pub async fn set_web_url(&self, url: Option<String>) -> Result<(), FoldError> {
        let _writing = self.writing.lock().await;
        let host_dir = self.host_dir();
        self.blocking(move || {
            let mut host: HostFile = read_file(&host_dir.join(HOST_FILE))?;
            host.web_url = url;
            write_file(&host_dir, HOST_FILE, &host)
        })
        .await
    }

    /// Adds the target `make` builds with the next number to `scope`. The host file keeps the number taken before the target is written, so a number is never handed out twice.
    pub async fn add(
        &self,
        scope: &Scope,
        make: impl FnOnce(NotifyTargetId) -> Result<Target, FoldError> + Send + 'static,
    ) -> Result<Target, FoldError> {
        let _writing = self.writing.lock().await;
        let project = match scope {
            Scope::Host => None,
            Scope::Project(slug) => Some(project_file(slug)?),
        };
        let (projects, host_dir) = (self.project_dir(), self.host_dir());
        let target = self
            .blocking(move || {
                let mut host: HostFile = read_file(&host_dir.join(HOST_FILE))?;
                let number = host.next_number.max(1);
                let id = NotifyTargetId::from_number(number).ok_or_else(|| {
                    FoldError::new(codes::INTERNAL_ERROR, "no target number is left")
                })?;
                let target = make(id)?;
                host.next_number = number.saturating_add(1);
                match project {
                    None => {
                        host.targets.push(target.clone());
                        write_file(&host_dir, HOST_FILE, &host)?;
                    }
                    Some(name) => {
                        write_file(&host_dir, HOST_FILE, &host)?;
                        let mut file: ProjectFile = read_file(&projects.join(&name))?;
                        file.targets.push(target.clone());
                        write_file(&projects, &name, &file)?;
                    }
                }
                Ok(target)
            })
            .await?;
        self.wake_scope(scope);
        Ok(target)
    }

    fn wake_scope(&self, scope: &Scope) {
        match scope {
            Scope::Host => self.wake(None),
            Scope::Project(slug) => self.wake(Some(slug)),
        }
    }

    /// Changes the target `id` of `scope` with `change` and returns it as it is kept; `notify_target_not_found` when the scope has none of that id. The notifiers of the projects the target serves look at it again.
    pub async fn update(
        &self,
        scope: &Scope,
        id: NotifyTargetId,
        change: impl FnOnce(&mut Target) -> Result<(), FoldError> + Send + 'static,
    ) -> Result<Target, FoldError> {
        let target = self.change(scope, id, change).await?;
        self.wake_scope(scope);
        Ok(target)
    }

    /// [`NotifySettings::update`] without a wake: the health a try writes is no news to any notifier.
    async fn change(
        &self,
        scope: &Scope,
        id: NotifyTargetId,
        change: impl FnOnce(&mut Target) -> Result<(), FoldError> + Send + 'static,
    ) -> Result<Target, FoldError> {
        let _writing = self.writing.lock().await;
        let (dir, name) = match scope {
            Scope::Host => (self.host_dir(), HOST_FILE.to_owned()),
            Scope::Project(slug) => (self.project_dir(), project_file(slug)?),
        };
        let host = *scope == Scope::Host;
        let target = self
            .blocking(move || {
                let path = dir.join(&name);
                let changed = |targets: &mut Vec<Target>| -> Result<Target, FoldError> {
                    let target = targets
                        .iter_mut()
                        .find(|target| target.id == id)
                        .ok_or_else(|| not_found(id))?;
                    change(target)?;
                    Ok(target.clone())
                };
                if host {
                    let mut file: HostFile = read_file(&path)?;
                    let target = changed(&mut file.targets)?;
                    write_file(&dir, &name, &file)?;
                    Ok(target)
                } else {
                    let mut file: ProjectFile = read_file(&path)?;
                    let target = changed(&mut file.targets)?;
                    write_file(&dir, &name, &file)?;
                    Ok(target)
                }
            })
            .await?;
        Ok(target)
    }

    /// Counts one try of the target `id` of the host or of `slug`: a success clears its failures and a target fold turned off, a failure counts, and [`OFF_AFTER_FAILURES`] in a row, a send that gave up or `gone` turn it off. Returns the target as it is kept with its state before the try, or `None` when it is gone. A person's switch stays as it is.
    pub async fn record_try(
        &self,
        slug: &str,
        id: NotifyTargetId,
        host: bool,
        outcome: TryOutcome,
        now: Timestamp,
    ) -> Result<Option<Recorded>, FoldError> {
        let scope = if host {
            Scope::Host
        } else {
            Scope::Project(slug.to_owned())
        };
        let before = Arc::new(std::sync::Mutex::new(TargetState::On));
        let seen = Arc::clone(&before);
        let updated = self
            .change(&scope, id, move |target| {
                *lock(&seen) = target.state();
                let health = &mut target.health;
                if outcome == TryOutcome::Delivered {
                    health.failures = 0;
                    health.off = None;
                    health.off_at = None;
                    health.last_ok_at = Some(now);
                    return Ok(());
                }
                health.failures = health.failures.saturating_add(1);
                health.last_failed_at = Some(now);
                let off = match outcome {
                    TryOutcome::Gone => Some(OffReason::Gone),
                    TryOutcome::GaveUp => Some(OffReason::GaveUp),
                    _ if health.failures >= OFF_AFTER_FAILURES => Some(OffReason::Failures),
                    _ => None,
                };
                if let (Some(off), None) = (off, health.off) {
                    health.off = Some(off);
                    health.off_at = Some(now);
                }
                Ok(())
            })
            .await;
        match updated {
            Ok(target) => Ok(Some(Recorded {
                before: *lock(&before),
                target,
            })),
            Err(error) if error.code() == codes::NOTIFY_TARGET_NOT_FOUND => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Removes the target `id` of `scope`; `notify_target_not_found` when the scope has none of that id.
    pub async fn remove(&self, scope: &Scope, id: NotifyTargetId) -> Result<(), FoldError> {
        let _writing = self.writing.lock().await;
        let (dir, name) = match scope {
            Scope::Host => (self.host_dir(), HOST_FILE.to_owned()),
            Scope::Project(slug) => (self.project_dir(), project_file(slug)?),
        };
        let host = *scope == Scope::Host;
        self.blocking(move || {
            let path = dir.join(&name);
            if host {
                let mut file: HostFile = read_file(&path)?;
                take(&mut file.targets, id)?;
                write_file(&dir, &name, &file)
            } else {
                let mut file: ProjectFile = read_file(&path)?;
                take(&mut file.targets, id)?;
                write_file(&dir, &name, &file)
            }
        })
        .await?;
        self.wake_scope(scope);
        Ok(())
    }

    /// Deletes the settings file of the project `slug` with its targets and their secrets, as the project leaves fold: a project that comes back, or a new one that takes the slug, starts with none. A failure is logged with the slug and the error kind and does not stop the removal.
    pub async fn forget_project(&self, slug: &str) {
        let Ok(name) = project_file(slug) else {
            return;
        };
        let _writing = self.writing.lock().await;
        let path = self.project_dir().join(name);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {
                tracing::info!(project = %slug, "the project's notice targets were deleted");
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(project = %slug, kind = %error.kind(), "the project's notice settings could not be deleted");
            }
        }
    }
}

/// What one try of a send meant for its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryOutcome {
    Delivered,
    Failed,
    /// The try failed and was the last of its send's retry schedule.
    GaveUp,
    Gone,
}

/// A target after a try was counted, with its state before it.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub before: TargetState,
    pub target: Target,
}

impl Recorded {
    /// The new state when the try changed it.
    pub fn changed(&self) -> Option<TargetState> {
        let after = self.target.state();
        (after != self.before).then_some(after)
    }
}

pub(crate) fn not_found(id: NotifyTargetId) -> FoldError {
    FoldError::new(
        codes::NOTIFY_TARGET_NOT_FOUND,
        format!("there is no notice target {id} here"),
    )
    .with_hint(
        "list the targets with `fld notify list`, or `fld notify list --host` for the host's",
    )
}

fn take(targets: &mut Vec<Target>, id: NotifyTargetId) -> Result<(), FoldError> {
    let before = targets.len();
    targets.retain(|target| target.id != id);
    if targets.len() == before {
        return Err(not_found(id));
    }
    Ok(())
}

/// The file name of a project's settings. A slug is lowercase letters, digits and hyphens, so it never leaves the folder.
fn project_file(slug: &str) -> Result<String, FoldError> {
    if slug.is_empty()
        || !slug
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(FoldError::new(
            codes::PROJECT_NOT_FOUND,
            "no project has that slug",
        ));
    }
    Ok(format!("{slug}.json"))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn ana() -> HumanName {
        HumanName::parse("ana").unwrap()
    }

    fn fields(url: &str) -> TargetFields {
        TargetFields {
            name: Some("Release bot".to_owned()),
            url: Some(url.to_owned()),
            ..TargetFields::default()
        }
    }

    fn id(number: u32) -> NotifyTargetId {
        NotifyTargetId::from_number(number).unwrap()
    }

    fn make(url: &'static str) -> impl FnOnce(NotifyTargetId) -> Result<Target, FoldError> {
        move |id| {
            fields(url).new_target(
                TargetKind::Webhook,
                id,
                Some("whsec_AAAA".to_owned()),
                &ana(),
                Timestamp::EPOCH,
            )
        }
    }

    #[test]
    fn a_new_target_is_checked_and_takes_the_defaults() {
        let target = fields(" https://hooks.example.com/x ")
            .new_target(TargetKind::Slack, id(1), None, &ana(), Timestamp::EPOCH)
            .unwrap();
        assert_eq!(target.window_seconds, DEFAULT_WINDOW_SECONDS);
        assert_eq!(target.filter, NoticeFilter::default());
        assert_eq!(target.url.expose(), "https://hooks.example.com/x");
        assert!(target.secret.is_none());
        assert!(target.active());
        for (refused, code) in [
            (
                fields("http://hooks.example.com/x"),
                codes::PLAINTEXT_NOTIFY_TARGET,
            ),
            (fields("hooks.example.com"), codes::INVALID_NOTIFY_TARGET),
            (
                TargetFields {
                    window_seconds: Some(4000),
                    ..fields("https://h/x")
                },
                codes::INVALID_NOTIFY_TARGET,
            ),
            (
                TargetFields {
                    types: Some(BTreeSet::new()),
                    ..fields("https://h/x")
                },
                codes::INVALID_NOTIFY_TARGET,
            ),
            (
                TargetFields {
                    url: None,
                    ..fields("https://h/x")
                },
                codes::INVALID_NOTIFY_TARGET,
            ),
        ] {
            let error = refused
                .new_target(TargetKind::Webhook, id(2), None, &ana(), Timestamp::EPOCH)
                .unwrap_err();
            assert_eq!(error.code(), code);
        }
        let mut target = target;
        target.health.failures = 4;
        TargetFields {
            window_seconds: Some(0),
            enabled: Some(false),
            ..TargetFields::default()
        }
        .apply(&mut target)
        .unwrap();
        assert_eq!((target.window_seconds, target.enabled), (0, false));
        assert_eq!(target.health, Health::default());
        assert!(!target.active());
    }

    #[test]
    fn debug_output_names_no_secret_and_no_url() {
        let target = make("https://hooks.example.com/T0/token")(id(1)).unwrap();
        let shown = format!("{target:?}");
        assert!(
            !shown.contains("token") && !shown.contains("whsec"),
            "{shown}"
        );
        assert_eq!(
            format!("{:?}", Secret::new("x".to_owned())),
            "Secret(<redacted>)"
        );
        let fields = format!("{:?}", fields("https://hooks.example.com/T0/token"));
        assert!(
            !fields.contains("token") && fields.contains("<redacted>"),
            "{fields}"
        );
    }

    #[tokio::test]
    async fn a_start_removes_the_temporary_files_a_stop_left() {
        let temp = tempfile::tempdir().unwrap();
        let settings = NotifySettings::new(temp.path().join("notices"));
        settings
            .add(&Scope::Project("demo".to_owned()), make("https://a/x"))
            .await
            .unwrap();
        let host = temp.path().join("notices/host");
        let projects = temp.path().join("notices/projects");
        std::fs::write(host.join(".host.json.7.0.tmp"), "whsec_left").unwrap();
        std::fs::write(projects.join(".demo.json.7.1.tmp"), "whsec_left").unwrap();
        settings.remove_leftovers().await;
        let names = |dir: &Path| -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(dir)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };
        assert_eq!(names(&host), ["host.json"]);
        assert_eq!(names(&projects), ["demo.json"]);
        NotifySettings::new(temp.path().join("none"))
            .remove_leftovers()
            .await;
    }

    #[test]
    fn a_rotation_signs_with_both_secrets_until_it_ends() {
        let mut target = make("https://h/x")(id(1)).unwrap();
        target.previous_secret = Some(Secret::new("whsec_BBBB".to_owned()));
        target.previous_until = Some(Timestamp::from_millis(10).unwrap());
        assert_eq!(
            target.signing_secrets(Timestamp::from_millis(5).unwrap()),
            ["whsec_AAAA", "whsec_BBBB"]
        );
        assert_eq!(
            target.signing_secrets(Timestamp::from_millis(10).unwrap()),
            ["whsec_AAAA"]
        );
    }

    #[tokio::test]
    async fn targets_live_in_private_files_and_numbers_are_never_reused() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let settings = NotifySettings::new(temp.path().join("notices"));
        let project = Scope::Project("demo".to_owned());
        let first = settings.add(&project, make("https://a/x")).await.unwrap();
        let second = settings
            .add(&Scope::Host, make("https://b/x"))
            .await
            .unwrap();
        assert_eq!((first.id, second.id), (id(1), id(2)));
        let applying = settings.applying("demo").await.unwrap();
        assert_eq!(
            applying
                .iter()
                .map(|a| (a.target.id, a.host))
                .collect::<Vec<_>>(),
            [(id(1), false), (id(2), true)]
        );
        assert!(settings.find_for("demo", id(2)).await.unwrap().host);
        assert!(!settings.find_for("demo", id(1)).await.unwrap().host);
        assert_eq!(
            settings.find_for("other", id(1)).await.unwrap_err().code(),
            codes::NOTIFY_TARGET_NOT_FOUND
        );
        settings.remove(&project, first.id).await.unwrap();
        assert_eq!(
            settings
                .remove(&project, first.id)
                .await
                .unwrap_err()
                .code(),
            codes::NOTIFY_TARGET_NOT_FOUND
        );
        let third = settings.add(&project, make("https://c/x")).await.unwrap();
        assert_eq!(third.id, id(3));
        assert_eq!(settings.find(&project, third.id).await.unwrap(), third);
        assert!(settings.find(&Scope::Host, third.id).await.is_err());
        settings
            .set_web_url(Some("https://fold.example.com".to_owned()))
            .await
            .unwrap();
        assert_eq!(
            settings.web_url().await.unwrap().as_deref(),
            Some("https://fold.example.com")
        );
        let mode = |path: PathBuf| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(temp.path().join("notices/host/host.json")), 0o600);
        assert_eq!(mode(temp.path().join("notices/host")), 0o700);
        assert_eq!(mode(temp.path().join("notices")), 0o700);
        assert_eq!(
            mode(temp.path().to_path_buf()),
            0o755,
            "the daemon's data folder keeps its mode"
        );
        assert_eq!(mode(temp.path().join("notices/projects/demo.json")), 0o600);
        assert!(
            settings
                .add(&project, |_| Err(invalid("no")))
                .await
                .is_err()
        );
        assert!(
            settings
                .targets(&Scope::Project("../x".to_owned()))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn failures_turn_a_target_off_and_a_success_turns_it_on_again() {
        let temp = tempfile::tempdir().unwrap();
        let settings = NotifySettings::new(temp.path().join("notices"));
        let host = settings
            .add(&Scope::Host, make("https://h/x"))
            .await
            .unwrap();
        let now = Timestamp::from_millis(9).unwrap();
        let tried = |outcome| {
            let settings = settings.clone();
            async move {
                settings
                    .record_try("demo", host.id, true, outcome, now)
                    .await
                    .unwrap()
                    .unwrap()
            }
        };
        for _ in 1..OFF_AFTER_FAILURES {
            let kept = tried(TryOutcome::Failed).await;
            assert!(kept.target.active());
            assert_eq!(kept.changed(), None);
        }
        let off = tried(TryOutcome::Failed).await;
        assert_eq!(off.target.health.off, Some(OffReason::Failures));
        assert_eq!(off.target.health.off_at, Some(now));
        assert_eq!(
            (off.before, off.changed()),
            (TargetState::On, Some(TargetState::Failing))
        );
        let on = tried(TryOutcome::Delivered).await;
        assert!(on.target.active());
        assert_eq!(on.target.health.failures, 0);
        assert_eq!(on.changed(), Some(TargetState::On));
        let gave_up = tried(TryOutcome::GaveUp).await;
        assert_eq!(gave_up.target.health.off, Some(OffReason::GaveUp));
        assert_eq!(gave_up.changed(), Some(TargetState::Failing));
        tried(TryOutcome::Delivered).await;
        let gone = tried(TryOutcome::Gone).await;
        assert_eq!(gone.target.health.off, Some(OffReason::Gone));
        assert_eq!(gone.changed(), Some(TargetState::Gone));
        assert_eq!(
            tried(TryOutcome::GaveUp).await.changed(),
            None,
            "the first reason stays"
        );
        for reason in [OffReason::Gone, OffReason::Failures, OffReason::GaveUp] {
            assert!(reason.advice().contains("send a test"));
        }
        assert!(
            settings
                .record_try("demo", id(9), false, TryOutcome::Failed, now)
                .await
                .unwrap()
                .is_none()
        );
        assert!(settings.use_host_targets("demo").await.unwrap());
        settings.set_use_host_targets("demo", false).await.unwrap();
        assert!(settings.applying("demo").await.unwrap().is_empty());
        settings.forget_project("demo").await;
        assert!(settings.use_host_targets("demo").await.unwrap());
    }

    #[tokio::test]
    async fn a_broken_file_is_an_error_that_shows_none_of_it() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("notices");
        std::fs::create_dir_all(dir.join("host")).unwrap();
        std::fs::write(dir.join("host/host.json"), "{\"targets\": \"secret-text\"").unwrap();
        let error = NotifySettings::new(dir)
            .targets(&Scope::Host)
            .await
            .unwrap_err();
        assert_eq!(error.code(), codes::NOTIFY_SETTINGS_IO);
        assert!(!error.message().contains("secret-text"));
    }
}
