use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use fold_core::accounts::{AccountName, AccountStamp, AccountToken, check_token};
use fold_core::{FoldError, HostPaths, ProjectConfig, ProjectPaths, Timestamp, codes};
use fold_store::{Store, projects};
use fold_vcs::atomic::EditLock;
use serde::{Deserialize, Serialize};

use crate::push::{private_folder, read_private, remove_temporary, write_private_here};

/// The index of the accounts: names, dates and token file names, never a token. Its rename is the commit point of every change, so a stop before it leaves the old accounts and a stop after it the new ones.
const INDEX_FILE: &str = "accounts.json";
/// Each token lives in a file of its own named by this prefix and random hex, so a new token never overwrites the file the index names until the index names the new one.
const TOKEN_PREFIX: &str = "token-";

/// One account as the index holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct IndexEntry {
    name: AccountName,
    added_at: Timestamp,
    file: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    accounts: Vec<IndexEntry>,
}

/// An account as fold shows it: the name and when its token was added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountEntry {
    pub name: AccountName,
    pub added_at: Timestamp,
}

fn accounts_io(kind: io::ErrorKind) -> FoldError {
    FoldError::new(
        codes::ACCOUNTS_IO,
        format!("the host's accounts could not be read or written: {kind}"),
    )
    .with_hint("check the accounts/ folder in fold's data folder: it must belong to this user and be readable only by it")
}

fn io_error(error: &io::Error) -> FoldError {
    accounts_io(error.kind())
}

/// The file in which Claude Code keeps the login of `claude login`, in its own folder.
const CREDENTIALS_FILE: &str = ".credentials.json";

/// Whether Claude Code has its own login on this host, which the account `default` stands for: the file of that login in Claude Code's folder, `CLAUDE_CONFIG_DIR` or `.claude` in the home directory as `var` gives them, or on macOS the keychain item Claude Code keeps it in. A host that runs Claude Code only on account tokens has none. The secret is never read. Blocking.
pub fn claude_login(var: impl Fn(&str) -> Option<OsString>) -> bool {
    credentials_file(var) || keychain_login()
}

/// Whether Claude Code's folder holds the file of its login.
fn credentials_file(var: impl Fn(&str) -> Option<OsString>) -> bool {
    let dir = match var("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir)),
        None => var("HOME")
            .filter(|home| !home.is_empty())
            .map(|home| PathBuf::from(home).join(".claude")),
    };
    dir.is_some_and(|dir| dir.join(CREDENTIALS_FILE).is_file())
}

/// Whether the macOS keychain holds Claude Code's login, asked by the item's name only: `security` without `-w` or `-g` prints no secret, and its output is dropped.
#[cfg(target_os = "macos")]
fn keychain_login() -> bool {
    std::process::Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(not(target_os = "macos"))]
fn keychain_login() -> bool {
    false
}

/// The index of the accounts in `dir`, whose every change renames a new file into place.
pub fn index_path(dir: &Path) -> std::path::PathBuf {
    dir.join(INDEX_FILE)
}

/// The refusal for a name that the host has no account of.
pub fn not_found(name: &AccountName) -> FoldError {
    FoldError::new(
        codes::ACCOUNT_NOT_FOUND,
        format!("this host has no account {name}"),
    )
    .with_hint(format!(
        "run `fld account list`, or `claude setup-token` and then `fld account add {name}` with the token on stdin"
    ))
    .with_details([("account", serde_json::json!(name.as_str()))])
}

fn read_index(dir: &Path) -> Result<Index, FoldError> {
    let index: Index =
        match read_private(&dir.join(INDEX_FILE)).map_err(|error| io_error(&error))? {
            None => return Ok(Index::default()),
            Some(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| accounts_io(io::ErrorKind::InvalidData))?,
        };
    if index
        .accounts
        .iter()
        .all(|entry| is_token_file(&entry.file))
    {
        Ok(index)
    } else {
        Err(accounts_io(io::ErrorKind::InvalidData))
    }
}

/// Whether `name` is a token file's name as [`random_file`] makes it: the prefix and 32 hex digits, so an index never makes fold read or remove a file outside the folder.
fn is_token_file(name: &str) -> bool {
    name.strip_prefix(TOKEN_PREFIX).is_some_and(|hex| {
        hex.len() == 32
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// The bytes of the token file at `path`, `None` when it is gone. A file that the group or others can read is refused, because its token may have leaked already.
fn read_token_file(path: &Path, name: &AccountName) -> Result<Option<Vec<u8>>, FoldError> {
    use std::io::Read as _;
    use std::os::unix::fs::PermissionsExt as _;

    use rustix::fs::{Mode, OFlags};
    let opened = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    );
    let mut file = match opened {
        Ok(fd) => std::fs::File::from(fd),
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(error) => return Err(io_error(&error.into())),
    };
    let meta = file.metadata().map_err(|error| io_error(&error))?;
    if !meta.is_file() {
        return Err(accounts_io(io::ErrorKind::InvalidData));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(FoldError::new(
            codes::ACCOUNTS_IO,
            format!("the token file of the account {name} can be read by other users of this host"),
        )
        .with_hint(format!(
            "the token may have leaked: run `claude setup-token` for a new one, then `fld account update {name}` with it on stdin, which writes a file only you can read"
        ))
        .with_details([("account", serde_json::json!(name.as_str()))]));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| io_error(&error))?;
    Ok(Some(bytes))
}

fn write_index(dir: &Path, index: &Index) -> Result<(), FoldError> {
    let mut bytes =
        serde_json::to_vec_pretty(index).map_err(|_| accounts_io(io::ErrorKind::InvalidData))?;
    bytes.push(b'\n');
    write_private_here(dir, INDEX_FILE, &bytes).map_err(|error| io_error(&error))
}

/// Takes the lock that serialises every change of the accounts, the CLI's and the daemon's, after making the folder private. Only the accounts folder is narrowed: its parent is fold's data folder, which the registry's open already narrows and a person may keep behind a link.
fn lock(dir: &Path) -> Result<EditLock, FoldError> {
    private_folder(dir).map_err(|error| io_error(&error))?;
    EditLock::acquire(&dir.join(INDEX_FILE)).map_err(|error| io_error(&error))
}

fn random_file() -> Result<String, FoldError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| {
        FoldError::new(
            codes::INTERNAL_ERROR,
            "the system could not give random bytes",
        )
    })?;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("{TOKEN_PREFIX}{hex}"))
}

fn remove_file(path: &Path) -> Result<(), FoldError> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(io_error(&error)),
        _ => Ok(()),
    }
}

/// Removes, under the lock, the temporary files of writes that stopped before their rename and each token file the index does not name: what a stop between the writes of a change left, which can hold a token.
fn remove_leftovers_locked(dir: &Path, index: &Index) -> Result<(), FoldError> {
    remove_temporary(dir).map_err(|error| io_error(&error))?;
    let named: BTreeSet<&str> = index
        .accounts
        .iter()
        .map(|entry| entry.file.as_str())
        .collect();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error(&error)),
    };
    for entry in entries {
        let entry = entry.map_err(|error| io_error(&error))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(TOKEN_PREFIX) && !named.contains(name.as_ref()) {
            remove_file(&entry.path())?;
        }
    }
    Ok(())
}

/// The accounts in `dir`, in name order. Blocking.
pub fn list(dir: &Path) -> Result<Vec<AccountEntry>, FoldError> {
    let mut accounts: Vec<AccountEntry> = read_index(dir)?
        .accounts
        .into_iter()
        .map(|entry| AccountEntry {
            name: entry.name,
            added_at: entry.added_at,
        })
        .collect();
    accounts.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(accounts)
}

/// The stamp of the account `name`, `None` when the host has none of that name. Blocking.
pub fn stamp(dir: &Path, name: &AccountName) -> Result<Option<AccountStamp>, FoldError> {
    Ok(read_index(dir)?
        .accounts
        .into_iter()
        .find(|entry| &entry.name == name)
        .map(|entry| AccountStamp {
            name: entry.name,
            added_at: entry.added_at,
        }))
}

/// The stamp and the token of the account `name`, `None` when the host has none of that name. A token file the index names but that is gone is an error, after one more read of the index: a change that renamed a new index between the two reads removes the old file. Blocking.
pub fn token(
    dir: &Path,
    name: &AccountName,
) -> Result<Option<(AccountStamp, AccountToken)>, FoldError> {
    let mut tries = 0;
    let (entry, bytes) = loop {
        let Some(entry) = read_index(dir)?
            .accounts
            .into_iter()
            .find(|entry| &entry.name == name)
        else {
            return Ok(None);
        };
        match read_token_file(&dir.join(&entry.file), name)? {
            Some(bytes) => break (entry, bytes),
            None if tries == 0 => tries += 1,
            None => return Err(accounts_io(io::ErrorKind::NotFound)),
        }
    };
    let text = String::from_utf8(bytes).map_err(|_| accounts_io(io::ErrorKind::InvalidData))?;
    let token = check_token(&text).map_err(|_| accounts_io(io::ErrorKind::InvalidData))?;
    Ok(Some((
        AccountStamp {
            name: entry.name,
            added_at: entry.added_at,
        },
        token,
    )))
}

/// The refusal of an add of a name the host has already.
fn exists(name: &AccountName) -> FoldError {
    FoldError::new(
        codes::ACCOUNT_EXISTS,
        format!("this host has an account {name} already"),
    )
    .with_hint(format!(
        "to give it a new token, run `fld account update {name}` with the token on stdin; to keep both, add the new one under another name"
    ))
    .with_details([("account", serde_json::json!(name.as_str()))])
}

/// Adds the account `name` with `token`, refused for a name the host has already ([`codes::ACCOUNT_EXISTS`]) and for `default`. See [`put`] for the order of the writes. Blocking.
pub fn add(
    dir: &Path,
    name: &AccountName,
    token: &AccountToken,
    now: Timestamp,
) -> Result<(), FoldError> {
    name.check_not_reserved()?;
    put(dir, name, token, now, false)
}

/// Gives the account `name` the new `token` and `now` as its date, refused for a name the host does not have and for `default`. See [`put`]. Blocking.
pub fn update(
    dir: &Path,
    name: &AccountName,
    token: &AccountToken,
    now: Timestamp,
) -> Result<(), FoldError> {
    name.check_not_reserved()?;
    put(dir, name, token, now, true)
}

/// Writes the token into a new file first; the index's rename then makes the change, and only after it the old token file goes. A stop before the rename keeps the accounts as they were and leaves a token file that the next change removes; a stop after it leaves the old token file, which the next change removes too. `replace` says whether the account must exist or must not.
fn put(
    dir: &Path,
    name: &AccountName,
    token: &AccountToken,
    now: Timestamp,
    replace: bool,
) -> Result<(), FoldError> {
    let _lock = lock(dir)?;
    let mut index = read_index(dir)?;
    let known = index.accounts.iter().any(|found| &found.name == name);
    match (replace, known) {
        (true, false) => return Err(not_found(name)),
        (false, true) => return Err(exists(name)),
        _ => {}
    }
    remove_leftovers_locked(dir, &index)?;
    let file = random_file()?;
    write_private_here(dir, &file, token.expose().as_bytes()).map_err(|error| io_error(&error))?;
    #[cfg(feature = "failpoints")]
    fail::fail_point!("accounts_after_token_file", |_| Err(accounts_io(
        io::ErrorKind::Interrupted
    )));
    let entry = IndexEntry {
        name: name.clone(),
        added_at: now,
        file,
    };
    let old = match index.accounts.iter_mut().find(|found| &found.name == name) {
        Some(found) => Some(std::mem::replace(found, entry)),
        None => {
            index.accounts.push(entry);
            None
        }
    };
    if let Err(error) = write_index(dir, &index) {
        let _ = remove_leftovers_locked(dir, &read_index(dir).unwrap_or_default());
        return Err(error);
    }
    #[cfg(feature = "failpoints")]
    fail::fail_point!("accounts_after_index", |_| Err(accounts_io(
        io::ErrorKind::Interrupted
    )));
    if let Some(old) = old {
        remove_file(&dir.join(old.file))?;
    }
    Ok(())
}

/// Removes the account `name`: the index first, then its token file. A stop between them leaves a token file that the next change removes. Blocking.
pub fn remove(dir: &Path, name: &AccountName) -> Result<(), FoldError> {
    name.check_not_reserved()?;
    let _lock = lock(dir)?;
    let mut index = read_index(dir)?;
    let Some(at) = index.accounts.iter().position(|entry| &entry.name == name) else {
        return Err(not_found(name));
    };
    let gone = index.accounts.remove(at);
    write_index(dir, &index)?;
    #[cfg(feature = "failpoints")]
    fail::fail_point!("accounts_after_index", |_| Err(accounts_io(
        io::ErrorKind::Interrupted
    )));
    remove_file(&dir.join(gone.file))
}

/// The slugs of the registered projects whose `.fold/config.toml` names each account, `default` for a config that names none, read without a running daemon. A project whose config cannot be read is left out; a host without a registry has no project.
pub async fn usage_on_host(
    host: &HostPaths,
) -> Result<BTreeMap<AccountName, Vec<String>>, FoldError> {
    if !tokio::fs::try_exists(host.registry_db())
        .await
        .unwrap_or(false)
    {
        return Ok(BTreeMap::new());
    }
    let registry = Store::open_registry(&host.registry_db())
        .await
        .map_err(|error| {
            FoldError::new(
                codes::INTERNAL_ERROR,
                format!("cannot open the host registry: {}", error.log_code()),
            )
        })?;
    let rows = projects::list_projects(registry.read()).await;
    let _ = registry.close().await;
    let rows = rows.map_err(|error| {
        FoldError::new(
            codes::INTERNAL_ERROR,
            format!("cannot read the host registry: {}", error.log_code()),
        )
    })?;
    tokio::task::spawn_blocking(move || {
        let mut usage: BTreeMap<AccountName, Vec<String>> = BTreeMap::new();
        for row in rows {
            let path = ProjectPaths::new(&row.root).config();
            let config = match std::fs::read_to_string(&path) {
                Ok(text) => ProjectConfig::from_toml(&text).ok(),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    Some(ProjectConfig::default())
                }
                Err(_) => None,
            };
            if let Some(config) = config {
                let name = config.account.unwrap_or_else(AccountName::default_account);
                usage.entry(name).or_default().push(row.slug);
            }
        }
        usage
    })
    .await
    .map_err(|_| FoldError::new(codes::INTERNAL_ERROR, "the read of the projects stopped"))
}

/// [`remove`] for `fld account remove`, refused for `default` and while a registered project names the account: its workers could not start without it.
pub async fn remove_on_host(host: &HostPaths, name: &AccountName) -> Result<(), FoldError> {
    name.check_not_reserved()?;
    if let Some(projects) = usage_on_host(host).await?.remove(name) {
        return Err(FoldError::new(
            codes::ACCOUNT_IN_USE,
            format!("the account {name} is the account of the project {}", projects.join(", ")),
        )
        .with_hint("choose another account with `fld account use <name>` in that project's folder, or Claude Code's own login in the project's Settings, then remove it")
        .with_details([("projects", serde_json::json!(projects))]));
    }
    let dir = host.accounts_dir();
    let name = name.clone();
    tokio::task::spawn_blocking(move || remove(&dir, &name))
        .await
        .map_err(|_| FoldError::new(codes::INTERNAL_ERROR, "the removal stopped"))?
}

/// Removes what a stopped change left in `dir`, for the daemon's start. A host without accounts has no folder, and none is made. Blocking.
pub fn remove_leftovers(dir: &Path) -> Result<(), FoldError> {
    if !dir.is_dir() {
        return Ok(());
    }
    let _lock = lock(dir)?;
    let index = read_index(dir)?;
    remove_leftovers_locked(dir, &index)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    const TOKEN_A: &str = "sk-ant-oat01-aaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const TOKEN_B: &str = "sk-ant-oat01-bbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn name(text: &str) -> AccountName {
        AccountName::parse(text).unwrap()
    }

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis).unwrap()
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn tokens_live_in_private_files_and_the_index_holds_none() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let dir = temp.path().join("accounts");
        assert_eq!(list(&dir).unwrap(), Vec::new());
        let given = check_token(TOKEN_A).unwrap();
        add(&dir, &name("work-a"), &given, at(10)).unwrap();
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(temp.path()), 0o755, "the data folder keeps its mode");
        let index = std::fs::read_to_string(dir.join(INDEX_FILE)).unwrap();
        assert!(!index.contains(TOKEN_A));
        for file in files(&dir) {
            assert_eq!(mode(&dir.join(&file)), 0o600, "{file}");
        }
        let (stamp, read) = token(&dir, &name("work-a")).unwrap().unwrap();
        assert!(read.expose() == TOKEN_A);
        assert_eq!(stamp.added_at, at(10));
        assert_eq!(super::stamp(&dir, &name("work-a")).unwrap(), Some(stamp));
        assert_eq!(token(&dir, &name("other")).unwrap(), None);
        assert_eq!(
            list(&dir).unwrap(),
            vec![AccountEntry {
                name: name("work-a"),
                added_at: at(10)
            }]
        );
    }

    #[test]
    fn an_update_replaces_a_token_an_add_never_does_and_a_remove_leaves_no_file() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("accounts");
        add(&dir, &name("work-b"), &check_token(TOKEN_A).unwrap(), at(1)).unwrap();
        add(&dir, &name("work-a"), &check_token(TOKEN_A).unwrap(), at(2)).unwrap();
        let twice = add(&dir, &name("work-b"), &check_token(TOKEN_B).unwrap(), at(3)).unwrap_err();
        assert_eq!(twice.code(), codes::ACCOUNT_EXISTS);
        assert!(twice.hint().unwrap().contains("fld account update work-b"));
        let unknown =
            update(&dir, &name("work-c"), &check_token(TOKEN_B).unwrap(), at(3)).unwrap_err();
        assert_eq!(unknown.code(), codes::ACCOUNT_NOT_FOUND);
        update(&dir, &name("work-b"), &check_token(TOKEN_B).unwrap(), at(3)).unwrap();
        for refused in [
            add(
                &dir,
                &AccountName::default_account(),
                &check_token(TOKEN_B).unwrap(),
                at(4),
            ),
            update(
                &dir,
                &AccountName::default_account(),
                &check_token(TOKEN_B).unwrap(),
                at(4),
            ),
            remove(&dir, &AccountName::default_account()),
        ] {
            assert_eq!(refused.unwrap_err().code(), codes::ACCOUNT_RESERVED);
        }
        let (stamp, read) = token(&dir, &name("work-b")).unwrap().unwrap();
        assert!(read.expose() == TOKEN_B);
        assert_eq!(stamp.added_at, at(3));
        let names: Vec<String> = list(&dir)
            .unwrap()
            .iter()
            .map(|a| a.name.to_string())
            .collect();
        assert_eq!(names, ["work-a", "work-b"]);
        let tokens = files(&dir)
            .iter()
            .filter(|f| f.starts_with(TOKEN_PREFIX))
            .count();
        assert_eq!(tokens, 2, "the replaced token's file is gone");
        remove(&dir, &name("work-b")).unwrap();
        remove(&dir, &name("work-a")).unwrap();
        assert_eq!(
            files(&dir),
            [INDEX_FILE.to_owned(), format!("{INDEX_FILE}.lock")]
        );
        let missing = remove(&dir, &name("work-a")).unwrap_err();
        assert_eq!(missing.code(), codes::ACCOUNT_NOT_FOUND);
        assert!(missing.hint().unwrap().contains("fld account list"));
    }

    #[test]
    fn a_token_file_the_index_does_not_name_goes_at_the_next_change_or_start() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("accounts");
        add(&dir, &name("work-a"), &check_token(TOKEN_A).unwrap(), at(1)).unwrap();
        std::fs::write(dir.join("token-0123"), TOKEN_B).unwrap();
        std::fs::write(dir.join(".token-9.1.2.tmp"), TOKEN_B).unwrap();
        remove_leftovers(&dir).unwrap();
        assert!(!dir.join("token-0123").exists());
        assert!(!dir.join(".token-9.1.2.tmp").exists());
        assert!(token(&dir, &name("work-a")).unwrap().is_some());
        remove_leftovers(&temp.path().join("none")).unwrap();
        assert!(!temp.path().join("none").exists());
    }

    #[test]
    fn a_broken_index_is_an_error_that_names_no_token() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("accounts");
        add(&dir, &name("work-a"), &check_token(TOKEN_A).unwrap(), at(1)).unwrap();
        std::fs::write(dir.join(INDEX_FILE), "{").unwrap();
        let error = list(&dir).unwrap_err();
        assert_eq!(error.code(), codes::ACCOUNTS_IO);
        assert!(error.hint().unwrap().contains("accounts/"));
        assert!(!format!("{error:?}").contains(TOKEN_A));
    }

    #[test]
    fn claude_codes_login_is_the_file_in_its_folder() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config");
        let home = temp.path().join("home");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        let env = |pairs: Vec<(&'static str, PathBuf)>| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| OsString::from(value))
            }
        };
        assert!(!credentials_file(env(vec![("HOME", home.clone())])));
        std::fs::write(home.join(".claude").join(CREDENTIALS_FILE), "{}").unwrap();
        assert!(credentials_file(env(vec![("HOME", home.clone())])));
        assert!(
            !credentials_file(env(vec![
                ("HOME", home.clone()),
                ("CLAUDE_CONFIG_DIR", config.clone())
            ])),
            "CLAUDE_CONFIG_DIR names the folder Claude Code reads"
        );
        std::fs::write(config.join(CREDENTIALS_FILE), "{}").unwrap();
        assert!(credentials_file(env(vec![("CLAUDE_CONFIG_DIR", config)])));
        assert!(!credentials_file(env(Vec::new())));
    }

    #[test]
    fn a_token_file_others_can_read_is_refused_and_an_index_names_only_token_files() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("accounts");
        add(&dir, &name("work-a"), &check_token(TOKEN_A).unwrap(), at(1)).unwrap();
        let file = files(&dir)
            .into_iter()
            .find(|file| file.starts_with(TOKEN_PREFIX))
            .unwrap();
        std::fs::set_permissions(dir.join(&file), std::fs::Permissions::from_mode(0o640)).unwrap();
        let refused = token(&dir, &name("work-a")).unwrap_err();
        assert_eq!(refused.code(), codes::ACCOUNTS_IO);
        assert!(
            refused
                .hint()
                .unwrap()
                .contains("fld account update work-a")
        );
        assert!(!format!("{refused:?}").contains(TOKEN_A));
        update(&dir, &name("work-a"), &check_token(TOKEN_B).unwrap(), at(2)).unwrap();
        assert!(token(&dir, &name("work-a")).unwrap().unwrap().1.expose() == TOKEN_B);

        let index = std::fs::read_to_string(dir.join(INDEX_FILE)).unwrap();
        let named = index
            .split('"')
            .find(|part| part.starts_with(TOKEN_PREFIX))
            .unwrap()
            .to_owned();
        std::fs::write(dir.join(INDEX_FILE), index.replace(&named, "../outside")).unwrap();
        assert_eq!(list(&dir).unwrap_err().code(), codes::ACCOUNTS_IO);
        assert!(!is_token_file("token-0123"));
        assert!(!is_token_file(&format!("{TOKEN_PREFIX}{}", "A".repeat(32))));
        assert!(is_token_file(&random_file().unwrap()));
    }
}
