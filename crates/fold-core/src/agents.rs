use std::fmt;

use thiserror::Error;

use crate::error::{FoldError, codes};

const MAX_NAME_LEN: usize = 32;
const MAX_WORKER_NAME_LEN: usize = 128;

/// The tag that makes an agent the integrator. Only a human can grant it.
pub const INTEGRATOR_TAG: &str = "integrator";

/// The messages never repeat the rejected text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum NameError {
    #[error(
        "an agent name or tag is 1 to 32 characters of a-z, 0-9, - and _, starting with a letter or digit"
    )]
    InvalidSegment,
    #[error("a worker name is an agent name followed by /<segment> parts, at most 128 characters")]
    InvalidWorker,
}

impl From<NameError> for FoldError {
    fn from(error: NameError) -> Self {
        FoldError::new(codes::INVALID_NAME, error.to_string())
    }
}

fn valid_segment(text: &str) -> bool {
    let mut bytes = text.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    text.len() <= MAX_NAME_LEN
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
}

macro_rules! segment_newtype {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn parse(text: &str) -> Result<Self, NameError> {
                if valid_segment(text) {
                    Ok(Self(text.to_owned()))
                } else {
                    Err(NameError::InvalidSegment)
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

segment_newtype!(
    /// 1 to 32 characters of `a-z`, `0-9`, `-` and `_`, starting with a letter or digit.
    AgentName
);
segment_newtype!(
    /// A role or capability label on an agent. It follows the agent name pattern.
    Tag
);

impl Tag {
    pub fn is_integrator(&self) -> bool {
        self.0 == INTEGRATOR_TAG
    }
}

/// `frontend` for the main worker of agent `frontend`, `frontend/ui` for its child `ui`. Every segment follows the agent name pattern.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkerName(String);

impl WorkerName {
    pub fn parse(text: &str) -> Result<Self, NameError> {
        if text.len() <= MAX_WORKER_NAME_LEN && text.split('/').all(valid_segment) {
            Ok(Self(text.to_owned()))
        } else {
            Err(NameError::InvalidWorker)
        }
    }

    pub fn main(agent: &AgentName) -> Self {
        Self(agent.as_str().to_owned())
    }

    /// The worker `<self>/<segment>`; it fails when the result is too long.
    pub fn child(&self, segment: &str) -> Result<Self, NameError> {
        if !valid_segment(segment) {
            return Err(NameError::InvalidSegment);
        }
        Self::parse(&format!("{}/{segment}", self.0))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_main(&self) -> bool {
        !self.0.contains('/')
    }

    /// The agent this worker belongs to, which is its first segment.
    pub fn agent(&self) -> AgentName {
        let first = self.0.split('/').next().unwrap_or_default();
        AgentName(first.to_owned())
    }

    /// The worker one level up, or `None` for a main worker.
    pub fn parent(&self) -> Option<WorkerName> {
        self.0
            .rsplit_once('/')
            .map(|(parent, _)| WorkerName(parent.to_owned()))
    }
}

impl fmt::Display for WorkerName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
