use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fold_core::projects::notify::{
    DEFAULT_INTERVAL_MINUTES, MAX_ADDRESS_LEN, MAX_SECRET_LEN, SmtpSecurity, check_email,
    check_interval, check_secret, check_smtp_host, check_smtp_transport, check_webhook_transport,
    check_webhook_url,
};
use fold_core::{FoldError, HumanName, NotifyTargetId, Timestamp, codes};
use serde::{Deserialize, Serialize};

use crate::push::{read_private, write_private};

/// A password or signing secret. It has no `Display` and a redacted `Debug`, so it reaches no log line or error by formatting; only a send reads it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// An email target's SMTP server and addresses.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailChannel {
    pub to: String,
    pub from: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub security: SmtpSecurity,
    pub username: Option<String>,
    pub password: Option<Secret>,
}

/// Where a target sends. The webhook URL can hold a token too, so only a send reads it in full.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Channel {
    Webhook { url: String, secret: Secret },
    Email(EmailChannel),
}

impl Channel {
    pub fn kind(&self) -> &'static str {
        match self {
            Channel::Webhook { .. } => "webhook",
            Channel::Email(_) => "email",
        }
    }
}

/// One notification target as its settings file keeps it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub id: NotifyTargetId,
    /// The shortest time between two sends to the target.
    pub interval_minutes: u32,
    pub created_at: Timestamp,
    /// The human who added the target.
    pub created_by: String,
    pub channel: Channel,
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target")
            .field("id", &self.id)
            .field("kind", &self.channel.kind())
            .finish_non_exhaustive()
    }
}

impl Target {
    pub fn interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.interval_minutes) * 60)
    }
}

/// Whose targets: the host's, which every project without targets of its own uses, or one project's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Host,
    Project(String),
}

/// A target a human asks for, before its checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTarget {
    pub interval_minutes: Option<u32>,
    pub channel: NewChannel,
}

#[derive(Clone, PartialEq, Eq)]
pub enum NewChannel {
    Webhook {
        url: String,
        secret: String,
    },
    Email {
        to: String,
        from: String,
        smtp_host: String,
        smtp_port: Option<u16>,
        security: SmtpSecurity,
        username: Option<String>,
        password: Option<String>,
    },
}

impl std::fmt::Debug for NewChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            NewChannel::Webhook { .. } => "NewChannel::Webhook",
            NewChannel::Email { .. } => "NewChannel::Email",
        })
    }
}

fn invalid(message: &str) -> FoldError {
    FoldError::new(codes::INVALID_NOTIFY_TARGET, message)
}

/// A user name of one line of at most [`MAX_ADDRESS_LEN`] bytes, or none for an empty one.
fn username(text: Option<String>) -> Result<Option<String>, FoldError> {
    let Some(text) = text
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
    else {
        return Ok(None);
    };
    if text.len() > MAX_ADDRESS_LEN || text.chars().any(char::is_control) {
        return Err(invalid(
            "the SMTP user name must be one line of at most 254 bytes",
        ));
    }
    Ok(Some(text))
}

impl NewTarget {
    /// The target as it is kept, once every part passed its check.
    pub fn checked(
        self,
        id: NotifyTargetId,
        by: &HumanName,
        now: Timestamp,
    ) -> Result<Target, FoldError> {
        let interval_minutes = self.interval_minutes.unwrap_or(DEFAULT_INTERVAL_MINUTES);
        check_interval(interval_minutes)?;
        let channel = match self.channel {
            NewChannel::Webhook { url, secret } => {
                let url = url.trim().to_owned();
                check_webhook_url(&url)?;
                check_webhook_transport(&url)?;
                check_secret(&secret)?;
                Channel::Webhook {
                    url,
                    secret: Secret(secret),
                }
            }
            NewChannel::Email {
                to,
                from,
                smtp_host,
                smtp_port,
                security,
                username: user,
                password,
            } => {
                let (to, from, smtp_host) = (
                    to.trim().to_owned(),
                    from.trim().to_owned(),
                    smtp_host.trim().to_owned(),
                );
                check_email(&to)?;
                check_email(&from)?;
                check_smtp_host(&smtp_host)?;
                check_smtp_transport(&smtp_host, security)?;
                let smtp_port = smtp_port.unwrap_or_else(|| security.default_port());
                if smtp_port == 0 {
                    return Err(invalid("the SMTP port must be 1 to 65535"));
                }
                let password = password.filter(|password| !password.is_empty());
                if let Some(password) = &password {
                    if password.len() > MAX_SECRET_LEN || password.chars().any(char::is_control) {
                        return Err(invalid(
                            "the SMTP password must be one line of at most 512 bytes",
                        ));
                    }
                    if security == SmtpSecurity::None {
                        return Err(invalid("a password needs TLS or STARTTLS").with_hint(
                            "choose the security tls or starttls, or send without a password",
                        ));
                    }
                }
                Channel::Email(EmailChannel {
                    to,
                    from,
                    smtp_host,
                    smtp_port,
                    security,
                    username: username(user)?,
                    password: password.map(Secret),
                })
            }
        };
        Ok(Target {
            id,
            interval_minutes,
            created_at: now,
            created_by: by.to_string(),
            channel,
        })
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

#[derive(Default, Serialize, Deserialize)]
struct ProjectFile {
    #[serde(default)]
    targets: Vec<Target>,
}

/// The host file has a folder of its own below the notify folder, as each project file has, because a private write narrows the folder it writes in and that folder's parent to 0700: the parent is then the notify folder, never the daemon's data folder.
const HOST_DIR: &str = "host";
const HOST_FILE: &str = "host.json";
const PROJECTS_DIR: &str = "projects";

fn settings_io(error: &io::Error) -> FoldError {
    FoldError::new(
        codes::NOTIFY_SETTINGS_IO,
        format!("the notification settings could not be read or written: {}", error.kind()),
    )
    .with_hint("check the daemon's data folder notify/: it must belong to this user and be readable only by it")
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

/// The notification settings of the host and of each project, in files of mode 0600 below a folder of mode 0700 in the daemon's data folder, outside every project folder, which agents can read. A file holds its targets with their secrets; the host file also holds the web app's address for links and the next target number. Only the daemon writes them, one change at a time.
#[derive(Debug, Clone)]
pub struct NotifySettings {
    dir: PathBuf,
    writing: Arc<tokio::sync::Mutex<()>>,
}

impl NotifySettings {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            writing: Arc::new(tokio::sync::Mutex::new(())),
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
        work: impl FnOnce(&Path) -> Result<T, FoldError> + Send + 'static,
    ) -> Result<T, FoldError> {
        let dir = self.dir.clone();
        tokio::task::spawn_blocking(move || work(&dir))
            .await
            .map_err(|_| FoldError::new(codes::INTERNAL_ERROR, "a settings read stopped"))?
    }

    async fn host(&self) -> Result<HostFile, FoldError> {
        let host = self.host_dir();
        self.blocking(move |_| read_file(&host.join(HOST_FILE)))
            .await
    }

    async fn project(&self, slug: &str) -> Result<ProjectFile, FoldError> {
        let name = project_file(slug)?;
        self.blocking(move |dir| read_file(&dir.join(PROJECTS_DIR).join(name)))
            .await
    }

    /// The targets of `scope`, oldest first.
    pub async fn targets(&self, scope: &Scope) -> Result<Vec<Target>, FoldError> {
        Ok(match scope {
            Scope::Host => self.host().await?.targets,
            Scope::Project(slug) => self.project(slug).await?.targets,
        })
    }

    /// The targets that get the notices of `slug`: its own, or the host's when it has none, with true for the host's.
    pub async fn applying(&self, slug: &str) -> Result<(Vec<Target>, bool), FoldError> {
        let own = self.project(slug).await?.targets;
        if !own.is_empty() {
            return Ok((own, false));
        }
        Ok((self.host().await?.targets, true))
    }

    /// The address of the web app that links start with, when a human set one.
    pub async fn web_url(&self) -> Result<Option<String>, FoldError> {
        Ok(self.host().await?.web_url)
    }

    pub async fn set_web_url(&self, url: Option<String>) -> Result<(), FoldError> {
        let _writing = self.writing.lock().await;
        let host_dir = self.host_dir();
        self.blocking(move |_| {
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
        let projects = self.project_dir();
        let host_dir = self.host_dir();
        self.blocking(move |_| {
            let mut host: HostFile = read_file(&host_dir.join(HOST_FILE))?;
            let number = host.next_number.max(1);
            let id = NotifyTargetId::from_number(number)
                .ok_or_else(|| FoldError::new(codes::INTERNAL_ERROR, "no target number is left"))?;
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
        .await
    }

    /// Removes the target `id` of `scope`; `notify_target_not_found` when the scope has none of that id.
    pub async fn remove(&self, scope: &Scope, id: NotifyTargetId) -> Result<(), FoldError> {
        let _writing = self.writing.lock().await;
        let project = match scope {
            Scope::Host => None,
            Scope::Project(slug) => Some(project_file(slug)?),
        };
        let projects = self.project_dir();
        let host_dir = self.host_dir();
        self.blocking(move |_| match project {
            None => {
                let mut host: HostFile = read_file(&host_dir.join(HOST_FILE))?;
                take(&mut host.targets, id)?;
                write_file(&host_dir, HOST_FILE, &host)
            }
            Some(name) => {
                let mut file: ProjectFile = read_file(&projects.join(&name))?;
                take(&mut file.targets, id)?;
                write_file(&projects, &name, &file)
            }
        })
        .await
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
                tracing::info!(project = %slug, "the project's notification targets were deleted")
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(project = %slug, kind = %error.kind(), "the project's notification settings could not be deleted")
            }
        }
    }

    /// The target `id` among those of `scope`.
    pub async fn find(&self, scope: &Scope, id: NotifyTargetId) -> Result<Target, FoldError> {
        self.targets(scope)
            .await?
            .into_iter()
            .find(|target| target.id == id)
            .ok_or_else(|| not_found(id))
    }
}

pub(crate) fn not_found(id: NotifyTargetId) -> FoldError {
    FoldError::new(
        codes::NOTIFY_TARGET_NOT_FOUND,
        format!("there is no notification target {id} here"),
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

    fn webhook(url: &str, secret: &str) -> NewTarget {
        NewTarget {
            interval_minutes: None,
            channel: NewChannel::Webhook {
                url: url.to_owned(),
                secret: secret.to_owned(),
            },
        }
    }

    fn email(security: SmtpSecurity, password: Option<&str>) -> NewTarget {
        NewTarget {
            interval_minutes: Some(5),
            channel: NewChannel::Email {
                to: " team@example.com ".to_owned(),
                from: "fold@example.com".to_owned(),
                smtp_host: "smtp.example.com".to_owned(),
                smtp_port: None,
                security,
                username: Some("  ".to_owned()),
                password: password.map(str::to_owned),
            },
        }
    }

    fn id(number: u32) -> NotifyTargetId {
        NotifyTargetId::from_number(number).unwrap()
    }

    #[test]
    fn a_new_target_is_checked_and_takes_the_defaults() {
        let target = webhook(" https://hooks.example.com/x ", "0123456789abcdef")
            .checked(id(1), &ana(), Timestamp::EPOCH)
            .unwrap();
        assert_eq!(target.interval_minutes, DEFAULT_INTERVAL_MINUTES);
        assert_eq!(target.interval().as_secs(), 600);
        assert!(
            matches!(&target.channel, Channel::Webhook { url, .. } if url == "https://hooks.example.com/x")
        );
        assert!(
            webhook("https://h/x", "short")
                .checked(id(1), &ana(), Timestamp::EPOCH)
                .is_err()
        );
        let mail = email(SmtpSecurity::Starttls, Some("pw"))
            .checked(id(2), &ana(), Timestamp::EPOCH)
            .unwrap();
        let Channel::Email(mail) = mail.channel else {
            panic!("an email target");
        };
        assert_eq!(
            (mail.to.as_str(), mail.smtp_port, mail.username),
            ("team@example.com", 587, None)
        );
        let local = |security, password| {
            let mut new = email(security, password);
            if let NewChannel::Email { smtp_host, .. } = &mut new.channel {
                "127.0.0.1".clone_into(smtp_host);
            }
            new
        };
        let error = local(SmtpSecurity::None, Some("pw"))
            .checked(id(3), &ana(), Timestamp::EPOCH)
            .unwrap_err();
        assert_eq!(error.code(), codes::INVALID_NOTIFY_TARGET);
        assert!(
            local(SmtpSecurity::None, None)
                .checked(id(3), &ana(), Timestamp::EPOCH)
                .is_ok()
        );
        for refused in [
            email(SmtpSecurity::None, None),
            email(SmtpSecurity::None, Some("pw")),
            webhook("http://hooks.example.com/x", "0123456789abcdef"),
        ] {
            assert_eq!(
                refused
                    .checked(id(4), &ana(), Timestamp::EPOCH)
                    .unwrap_err()
                    .code(),
                codes::PLAINTEXT_NOTIFY_TARGET
            );
        }
        assert!(
            webhook("http://localhost:9/x", "0123456789abcdef")
                .checked(id(4), &ana(), Timestamp::EPOCH)
                .is_ok()
        );
    }

    #[test]
    fn debug_output_names_no_secret_and_no_url() {
        let target = webhook("https://hooks.example.com/T0/token", "0123456789abcdef")
            .checked(id(1), &ana(), Timestamp::EPOCH)
            .unwrap();
        let shown = format!("{target:?} {:?}", webhook("https://h/t", "s").channel);
        assert!(
            !shown.contains("token") && !shown.contains("0123456789abcdef"),
            "{shown}"
        );
        assert_eq!(
            format!("{:?}", Secret("x".to_owned())),
            "Secret(<redacted>)"
        );
    }

    #[tokio::test]
    async fn targets_live_in_private_files_and_numbers_are_never_reused() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let settings = NotifySettings::new(temp.path().join("notify"));
        let project = Scope::Project("demo".to_owned());
        let make = |url: &'static str| {
            move |id| webhook(url, "0123456789abcdef").checked(id, &ana(), Timestamp::EPOCH)
        };
        let first = settings.add(&project, make("https://a/x")).await.unwrap();
        let second = settings
            .add(&Scope::Host, make("https://b/x"))
            .await
            .unwrap();
        assert_eq!((first.id, second.id), (id(1), id(2)));
        assert!(!settings.applying("demo").await.unwrap().1);
        assert_eq!(
            settings.applying("other").await.unwrap(),
            (vec![second.clone()], true)
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
        assert_eq!(
            settings.applying("demo").await.unwrap(),
            (vec![second.clone()], true)
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
        assert_eq!(mode(temp.path().join("notify/host/host.json")), 0o600);
        assert_eq!(mode(temp.path().join("notify/host")), 0o700);
        assert_eq!(mode(temp.path().join("notify")), 0o700);
        assert_eq!(
            mode(temp.path().to_path_buf()),
            0o755,
            "the daemon's data folder keeps its mode"
        );
        assert_eq!(mode(temp.path().join("notify/projects/demo.json")), 0o600);
        assert_eq!(mode(temp.path().join("notify/projects")), 0o700);
        let refused = settings
            .add(&project, |_| Err(invalid("no")))
            .await
            .unwrap_err();
        assert_eq!(refused.code(), codes::INVALID_NOTIFY_TARGET);
        assert!(
            settings
                .targets(&Scope::Project("../x".to_owned()))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_broken_file_is_an_error_that_shows_none_of_it() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("notify");
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
