use axum::extract::{FromRequestParts, OptionalFromRequestParts};
use axum::http::HeaderMap;
use axum::http::header::COOKIE;
use axum::http::request::Parts;
use fold_core::{Actor, FoldError, HumanName, codes, policy};

use super::Transport;
use super::auth::SessionHuman;
use super::error::ApiError;
use crate::daemon::AppState;
use crate::services::agents::peers;
use crate::services::projects;
use crate::socket::SocketPeer;

/// The header every CLI command sends over the Unix socket.
pub const HUMAN_HEADER: &str = "x-fold-human";

/// The named human behind a request, registered in the host registry on first use. Over the socket the name comes only from `x-fold-human`, and never from a process under a worker that fold started or under a check command that fold runs. Over HTTP it comes only from the live session that `fold_session` names, on loopback too: an agent runs as the host's user and can reach a loopback port, so a name that a request states itself, in a header or a cookie, would let it act as any human.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Human(pub HumanName);

impl Human {
    pub fn actor(&self) -> Actor {
        Actor::human(self.0.clone())
    }
}

impl FromRequestParts<AppState> for Human {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        named(parts, state).await?.ok_or_else(|| {
            FoldError::new(
                codes::HUMAN_NAME_REQUIRED,
                "the request does not name a human",
            )
            .with_hint("set FOLD_HUMAN to your name, then run the command again")
            .into()
        })
    }
}

/// A read that shows more to a named human, such as what waits for them, and the same as before to a request that names none. A name that is malformed, or claimed from a worker's process tree, is still refused.
impl OptionalFromRequestParts<AppState> for Human {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Option<Self>, ApiError> {
        named(parts, state).await
    }
}

async fn named(parts: &Parts, state: &AppState) -> Result<Option<Human>, ApiError> {
    let raw = match parts.extensions.get::<Transport>() {
        Some(Transport::Socket) => {
            let name = header_name(parts)?;
            if name.is_some() {
                refuse_worker_peer(parts, state).await?;
            }
            name
        }
        None => parts
            .extensions
            .get::<SessionHuman>()
            .map(|SessionHuman(name)| name.to_string()),
    };
    let Some(raw) = raw else {
        return Ok(None);
    };
    let name = HumanName::parse(&raw).map_err(FoldError::from)?;
    projects::ensure_human(&state.registry, state.clock.as_ref(), &name).await?;
    Ok(Some(Human(name)))
}

/// A worker's Bash and a check command run as the host's user and can set `FOLD_HUMAN`, so the kernel's peer pid decides instead.
pub(crate) async fn refuse_worker_peer(parts: &Parts, state: &AppState) -> Result<(), ApiError> {
    let pid = parts
        .extensions
        .get::<SocketPeer>()
        .and_then(|peer| peer.pid);
    let worker = peers::peer_worker(&state.projects, pid).await?;
    if let Some(worker) = &worker {
        tracing::warn!(
            project = worker.project,
            worker = worker.agent.as_ref().map(fold_core::AgentName::as_str),
            "refused x-fold-human from a worker's or a check's process tree"
        );
    }
    policy::can_name_human(worker.is_some()).map_err(FoldError::from)?;
    Ok(())
}

fn header_name(parts: &Parts) -> Result<Option<String>, ApiError> {
    let Some(value) = parts.headers.get(HUMAN_HEADER) else {
        return Ok(None);
    };
    let text = value
        .to_str()
        .map_err(|_| FoldError::from(fold_core::NameError::InvalidSegment))?;
    Ok((!text.is_empty()).then(|| text.to_owned()))
}

/// The first non-empty value of the cookie `name` among the `Cookie` headers. A header that is not visible ASCII is skipped like a missing one.
pub(crate) fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|header| header.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, value)| *key == name && !value.is_empty())
        .map(|(_, value)| value.to_owned())
}
