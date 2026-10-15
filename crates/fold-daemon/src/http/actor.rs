use axum::extract::FromRequestParts;
use axum::http::header::COOKIE;
use axum::http::request::Parts;
use fold_core::{Actor, FoldError, HumanName, codes};

use super::Transport;
use super::error::ApiError;
use crate::daemon::AppState;
use crate::services::projects;

/// The header every CLI command sends over the Unix socket.
pub const HUMAN_HEADER: &str = "x-fold-human";
/// The loopback cookie that `POST /api/v1/session` sets. The web application never reads it in script.
pub const HUMAN_COOKIE: &str = "fold_human";

/// The named human behind a request, registered in the host registry on first use. Over the socket the name comes only from `x-fold-human`; over HTTP only from the `fold_human` cookie, so a browser page cannot claim a name with a header. The cookie is trusted only because the daemon refuses to listen off loopback; off loopback the name must come from the session token.
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
        let raw = match parts.extensions.get::<Transport>() {
            Some(Transport::Socket) => header_name(parts)?,
            None => cookie_name(parts),
        };
        let raw = raw.ok_or_else(|| {
            FoldError::new(
                codes::HUMAN_NAME_REQUIRED,
                "the request does not name a human",
            )
            .with_hint("set FOLD_HUMAN for the CLI, or sign in to the web interface")
        })?;
        let name = HumanName::parse(&raw).map_err(FoldError::from)?;
        projects::ensure_human(&state.registry, state.clock.as_ref(), &name).await?;
        Ok(Self(name))
    }
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

/// The first non-empty `fold_human` value among the `Cookie` headers. A header that is not visible ASCII is skipped like a missing one.
fn cookie_name(parts: &Parts) -> Option<String> {
    parts
        .headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|header| header.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, value)| *name == HUMAN_COOKIE && !value.is_empty())
        .map(|(_, value)| value.to_owned())
}
