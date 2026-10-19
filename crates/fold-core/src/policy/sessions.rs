use crate::agents::AgentName;
use crate::error::codes;

use super::PolicyDenied;

/// Where the process behind a socket session runs, read from the kernel's peer pid and its parent processes when the session starts. A worker's Bash runs as the host's user, so only its process tree tells it apart from the user's own terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SessionPeer {
    /// No process of a worker that fold started is an ancestor: the user's terminal, the main agent, or a peer whose pid is unknown.
    #[default]
    Unsupervised,
    /// The named worker agent of this project, or one of its descendants, runs the session.
    Worker(AgentName),
    /// A worker of another project runs the session; it is no agent of this one.
    OtherProjectWorker,
    /// A check command that fold runs for no worker, such as an integration check, runs the session; it is no agent.
    Check,
}

impl SessionPeer {
    /// The worker agent of this project whose process tree runs the session.
    pub fn worker(&self) -> Option<&AgentName> {
        match self {
            Self::Worker(agent) => Some(agent),
            Self::Unsupervised | Self::OtherProjectWorker | Self::Check => None,
        }
    }
}

/// A process under a worker, or under a check that fold runs for a worker, acts only as that worker's own agent, so a worker cannot become `main` or another agent by setting `FOLD_AGENT` in its Bash or in a check command. A check that fold runs for no worker acts as no agent.
pub fn can_bind_agent(peer: &SessionPeer, agent: &AgentName) -> Result<(), PolicyDenied> {
    match peer {
        SessionPeer::Unsupervised => Ok(()),
        SessionPeer::Worker(own) if own == agent => Ok(()),
        SessionPeer::Worker(_) | SessionPeer::OtherProjectWorker => Err(PolicyDenied {
            code: codes::WORKER_PEER_REFUSED,
            message: "a process that a fold worker started can act only as that worker's own agent",
            hint: "use the fold tools of your own session; do not set FOLD_AGENT",
        }),
        SessionPeer::Check => Err(PolicyDenied {
            code: codes::WORKER_PEER_REFUSED,
            message: "a check command that fold runs for no worker cannot act as an agent",
            hint: "a check runs the project's own commands; it does not call fold",
        }),
    }
}

/// A worker never acts as a human, so `x-fold-human` from any process under a worker is refused. `under_worker` is true when a worker of any project, or a check command that fold runs, is an ancestor of the peer: a check runs commands from a file that a worker can change.
pub fn can_name_human(under_worker: bool) -> Result<(), PolicyDenied> {
    if !under_worker {
        return Ok(());
    }
    Err(PolicyDenied {
        code: codes::WORKER_PEER_REFUSED,
        message: "a process that a fold worker or a fold check started cannot act as a human",
        hint: "use the fold tools of your own session, or ask a human",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> AgentName {
        AgentName::parse(text).unwrap()
    }

    #[test]
    fn a_worker_binds_only_to_its_own_agent() {
        let worker = SessionPeer::Worker(name("web-worker"));
        assert_eq!(can_bind_agent(&worker, &name("web-worker")), Ok(()));
        let refused = can_bind_agent(&worker, &name("main")).unwrap_err();
        assert_eq!(refused.code(), codes::WORKER_PEER_REFUSED);
        assert!(can_bind_agent(&SessionPeer::OtherProjectWorker, &name("web-worker")).is_err());
        assert_eq!(
            can_bind_agent(&SessionPeer::Unsupervised, &name("main")),
            Ok(())
        );
        assert_eq!(worker.worker(), Some(&name("web-worker")));
        assert_eq!(SessionPeer::OtherProjectWorker.worker(), None);
        let check = can_bind_agent(&SessionPeer::Check, &name("main")).unwrap_err();
        assert_eq!(check.code(), codes::WORKER_PEER_REFUSED);
        assert_eq!(SessionPeer::Check.worker(), None);
        assert_eq!(SessionPeer::default(), SessionPeer::Unsupervised);
    }

    #[test]
    fn only_a_peer_outside_every_worker_names_a_human() {
        assert_eq!(can_name_human(false), Ok(()));
        assert_eq!(
            can_name_human(true).unwrap_err().code(),
            codes::WORKER_PEER_REFUSED
        );
    }
}
