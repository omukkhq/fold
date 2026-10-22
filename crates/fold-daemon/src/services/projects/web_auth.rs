use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use fold_core::{Clock, FoldError, HostPaths, HumanName, Timestamp, WebTokenId, codes};
use fold_store::projects::{self, NewWebSession, WebTokenEntry, WebTokenRow};
use fold_store::{Store, StoreError};

use super::ProjectError;

/// How long a use leaves `last_used_at` of a session and its token alone, so checking a session on every request is not a write on every request.
pub const WEB_TOUCH_INTERVAL: Duration = Duration::from_secs(60);

/// The session lifetime when `daemon.toml` sets none.
pub const DEFAULT_SESSION_LIFETIME: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// How long a sign-in link from `fld web` can be opened: long enough to open it in a browser, short enough that a copy found later is useless.
pub const SIGN_IN_LINK_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// 32 bytes from the OS random source as URL-safe base64, which a cookie value takes unquoted.
fn random_secret() -> Result<String, FoldError> {
    let mut secret = [0_u8; 32];
    getrandom::fill(&mut secret).map_err(|error| {
        FoldError::new(
            codes::INTERNAL_ERROR,
            format!("the OS random source failed: {error}"),
        )
    })?;
    Ok(URL_SAFE_NO_PAD.encode(secret))
}

/// The lookup finds a row by its hash; this comparison is the constant-time one `blake3::Hash` provides, so the match does not rest on SQLite's byte comparison.
fn hash_matches(
    stored: &[u8],
    hash: &blake3::Hash,
    column: &'static str,
) -> Result<bool, StoreError> {
    let stored = <[u8; 32]>::try_from(stored).map_err(|_| StoreError::Corrupt(column))?;
    Ok(blake3::Hash::from_bytes(stored) == *hash)
}

/// Makes a web sign-in token for `human`, creating the human when absent, and returns its plaintext. This is the only time the plaintext exists: the registry keeps its BLAKE3 hash.
pub async fn create_web_token(
    registry: &Store,
    human: &HumanName,
    label: Option<&str>,
    now: Timestamp,
) -> Result<String, ProjectError> {
    let token = random_secret()?;
    let hash = blake3::hash(token.as_bytes());
    registry
        .write(async |tx| {
            let row = projects::ensure_human(tx, human, now).await?;
            projects::insert_web_token(tx, hash.as_bytes(), row.id, label, now).await
        })
        .await?;
    Ok(token)
}

/// Every unrevoked token, oldest first. Never the token or its hash.
pub async fn list_web_tokens(registry: &Store) -> Result<Vec<WebTokenEntry>, StoreError> {
    projects::list_web_tokens(registry.read()).await
}

/// [`list_web_tokens`] for `fld token list`, which reads the host registry without a running daemon.
pub async fn list_web_tokens_on_host(host: &HostPaths) -> Result<Vec<WebTokenEntry>, ProjectError> {
    let registry = Store::open_registry(&host.registry_db()).await?;
    let tokens = list_web_tokens(&registry).await;
    registry.close().await?;
    Ok(tokens?)
}

/// Revokes the token `id` and ends every session made from it, in one write. An unknown or already revoked id is `token_not_found`.
pub async fn revoke_web_token(
    registry: &Store,
    id: WebTokenId,
    now: Timestamp,
) -> Result<(), ProjectError> {
    if registry
        .write(async |tx| projects::revoke_web_token(tx, id, now).await)
        .await?
    {
        Ok(())
    } else {
        Err(FoldError::new(
            codes::TOKEN_NOT_FOUND,
            format!("no unrevoked web token is {id}"),
        )
        .with_hint("list the tokens with `fld token list`")
        .into())
    }
}

/// [`revoke_web_token`] for `fld token revoke`. A running daemon reads `web_tokens` and `web_sessions` on every check, so the revocation holds from its next request.
pub async fn revoke_web_token_on_host(
    host: &HostPaths,
    clock: &dyn Clock,
    id: WebTokenId,
) -> Result<(), ProjectError> {
    let registry = Store::open_registry(&host.registry_db()).await?;
    let revoked = revoke_web_token(&registry, id, clock.now()).await;
    registry.close().await?;
    revoked
}

/// `row` when its stored hash really is `hash`.
fn matching_token(
    row: Option<WebTokenRow>,
    hash: &blake3::Hash,
) -> Result<Option<WebTokenRow>, StoreError> {
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(hash_matches(&row.token_hash, hash, "web_tokens.token_hash")?.then_some(row))
}

/// A sign-in that a valid token opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedIn {
    /// The plaintext session id for the cookie. This is the only time it exists: the registry keeps its BLAKE3 hash.
    pub session: String,
    pub human: HumanName,
}

fn later(now: Timestamp, lifetime: Duration) -> Result<Timestamp, FoldError> {
    now.checked_add(lifetime).ok_or_else(|| {
        FoldError::new(
            codes::INTERNAL_ERROR,
            "the lifetime runs past the largest timestamp",
        )
    })
}

/// Opens a session for the human that `token` signs in, ending at `now + lifetime`, or `None` for a token the registry does not hold or has revoked. The write checks the token again, so a revocation that lands between the read and the write still refuses it.
pub async fn create_web_session(
    registry: &Store,
    clock: &dyn Clock,
    lifetime: Duration,
    token: &str,
) -> Result<Option<SignedIn>, ProjectError> {
    let hash = blake3::hash(token.as_bytes());
    if matching_token(
        projects::find_web_token(registry.read(), hash.as_bytes()).await?,
        &hash,
    )?
    .is_none()
    {
        return Ok(None);
    }
    let session = random_secret()?;
    let session_hash = blake3::hash(session.as_bytes());
    let now = clock.now();
    let expires_at = later(now, lifetime)?;
    let human = registry
        .write(async |tx| {
            let found = projects::find_web_token(tx.conn(), hash.as_bytes()).await?;
            let Some(row) = matching_token(found, &hash)? else {
                return Ok(None);
            };
            projects::delete_expired_web_sessions(tx, now).await?;
            projects::insert_web_session(
                tx,
                NewWebSession {
                    session_hash: session_hash.as_bytes(),
                    token_id: Some(row.id),
                    human_id: row.human_id,
                    now,
                    expires_at,
                },
            )
            .await?;
            projects::touch_web_token(tx, row.id, now, now).await?;
            Ok::<_, StoreError>(Some(row.human))
        })
        .await?;
    Ok(human.map(|human| SignedIn { session, human }))
}

/// A one-time sign-in link that [`create_sign_in_link`] made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInLink {
    /// The plaintext secret for the link. This is the only time it exists: the registry keeps its BLAKE3 hash.
    pub secret: String,
    pub expires_at: Timestamp,
}

/// Makes a sign-in link for `human`, creating the human when absent, that opens one session until [`SIGN_IN_LINK_LIFETIME`] has passed. Links that expired go in the same write.
pub async fn create_sign_in_link(
    registry: &Store,
    clock: &dyn Clock,
    human: &HumanName,
) -> Result<SignInLink, ProjectError> {
    let secret = random_secret()?;
    let hash = blake3::hash(secret.as_bytes());
    let now = clock.now();
    let expires_at = later(now, SIGN_IN_LINK_LIFETIME)?;
    registry
        .write(async |tx| {
            projects::delete_expired_sign_in_links(tx, now).await?;
            let row = projects::ensure_human(tx, human, now).await?;
            projects::insert_sign_in_link(tx, hash.as_bytes(), row.id, now, expires_at).await
        })
        .await?;
    Ok(SignInLink { secret, expires_at })
}

/// Opens a session for the human of the sign-in link `secret`, ending at `now + lifetime`, and deletes the link in the same write, so it opens no second session. `None` for a link the registry does not hold, one already used, and one that has expired.
pub async fn open_sign_in_link(
    registry: &Store,
    clock: &dyn Clock,
    lifetime: Duration,
    secret: &str,
) -> Result<Option<SignedIn>, ProjectError> {
    let hash = blake3::hash(secret.as_bytes());
    if projects::find_sign_in_link(registry.read(), hash.as_bytes())
        .await?
        .is_none()
    {
        return Ok(None);
    }
    let session = random_secret()?;
    let session_hash = blake3::hash(session.as_bytes());
    let now = clock.now();
    let expires_at = later(now, lifetime)?;
    let human = registry
        .write(async |tx| {
            let Some(link) = projects::find_sign_in_link(tx.conn(), hash.as_bytes()).await? else {
                return Ok(None);
            };
            if !hash_matches(&link.link_hash, &hash, "sign_in_links.link_hash")? {
                return Ok(None);
            }
            projects::delete_sign_in_link(tx, link.id).await?;
            if link.expires_at <= now {
                return Ok(None);
            }
            projects::delete_expired_web_sessions(tx, now).await?;
            projects::insert_web_session(
                tx,
                NewWebSession {
                    session_hash: session_hash.as_bytes(),
                    token_id: None,
                    human_id: link.human_id,
                    now,
                    expires_at,
                },
            )
            .await?;
            Ok::<_, StoreError>(Some(link.human))
        })
        .await?;
    Ok(human.map(|human| SignedIn { session, human }))
}

/// Every human the host registry knows, ordered by name.
pub async fn list_humans(registry: &Store) -> Result<Vec<HumanName>, StoreError> {
    projects::list_humans(registry.read()).await
}

/// The human that the session `session` signs in, or `None` when the registry holds no such session, its token is revoked, or it has expired. It touches `last_used_at` of the session and its token at most once per [`WEB_TOUCH_INTERVAL`].
pub async fn check_web_session(
    registry: &Store,
    clock: &dyn Clock,
    session: &str,
) -> Result<Option<HumanName>, StoreError> {
    let hash = blake3::hash(session.as_bytes());
    let Some(row) = projects::find_web_session(registry.read(), hash.as_bytes()).await? else {
        return Ok(None);
    };
    if !hash_matches(&row.session_hash, &hash, "web_sessions.session_hash")? {
        return Ok(None);
    }
    let now = clock.now();
    if row.expires_at <= now {
        return Ok(None);
    }
    let stale_before = now
        .checked_sub(WEB_TOUCH_INTERVAL)
        .unwrap_or(Timestamp::EPOCH);
    if row.last_used_at <= stale_before {
        registry
            .write(async |tx| projects::touch_web_session(tx, &row, now, stale_before).await)
            .await?;
    }
    Ok(Some(row.human))
}

/// Deletes every session and every sign-in link that expired by `now`, for the daily job, so one that nobody uses again still leaves the registry. Returns how many went.
pub async fn purge_expired_web_sessions(
    registry: &Store,
    now: Timestamp,
) -> Result<u64, StoreError> {
    registry
        .write(async |tx| {
            let sessions = projects::delete_expired_web_sessions(tx, now).await?;
            let links = projects::delete_expired_sign_in_links(tx, now).await?;
            Ok(sessions + links)
        })
        .await
}

/// Deletes the session `session`. True when it existed; an unknown id costs one read and no write.
pub async fn end_web_session(registry: &Store, session: &str) -> Result<bool, StoreError> {
    let hash = blake3::hash(session.as_bytes());
    if projects::find_web_session(registry.read(), hash.as_bytes())
        .await?
        .is_none()
    {
        return Ok(false);
    }
    registry
        .write(async |tx| projects::delete_web_session(tx, hash.as_bytes()).await)
        .await
}
