/// Why fold keeps the folder of a task worker or reviewer that stopped before the `closed_workspace_days` window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderKeep {
    /// A worker row of the agent holds an unreleased claim.
    Claim,
    /// The agent has an open session, or an open session works in the folder.
    Session,
    /// An agent stopped the worker to wait for its go.
    Parked,
    /// Its last turn ended for a cause outside it, and fold owes it a re-run.
    Waiting,
    /// fold owes the worker a resume: requested changes, a conflict, failed checks, a send-back or a new version to review.
    OwedResume,
    /// A message waits for the worker, which fold starts again to answer it.
    OwedMessage,
    /// The folder is no directory, or a symlink.
    InvalidPath,
    /// The removal failed; the next run tries again.
    Failed,
}

/// What fold does, or would do, with the folder of one worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderDecision {
    Remove,
    Keep(FolderKeep),
}

/// What still ties a task worker or reviewer that stopped before the window to work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FolderTies {
    pub claim: bool,
    /// The agent has an open session, or an open session works in its folder.
    pub session: bool,
    pub parked: bool,
    pub waiting: bool,
    pub owed_resume: bool,
    pub owed_message: bool,
}

/// Whether the folder goes: only once nothing ties the worker to work, so it can never run again unless someone sends it new work, which writes the folder again at its start.
pub fn decide_folder(ties: FolderTies) -> FolderDecision {
    let keep = if ties.claim {
        FolderKeep::Claim
    } else if ties.session {
        FolderKeep::Session
    } else if ties.parked {
        FolderKeep::Parked
    } else if ties.waiting {
        FolderKeep::Waiting
    } else if ties.owed_resume {
        FolderKeep::OwedResume
    } else if ties.owed_message {
        FolderKeep::OwedMessage
    } else {
        return FolderDecision::Remove;
    };
    FolderDecision::Keep(keep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_goes_only_once_nothing_ties_the_worker() {
        assert_eq!(decide_folder(FolderTies::default()), FolderDecision::Remove);
        let cases = [
            (
                FolderTies {
                    claim: true,
                    ..FolderTies::default()
                },
                FolderKeep::Claim,
            ),
            (
                FolderTies {
                    session: true,
                    ..FolderTies::default()
                },
                FolderKeep::Session,
            ),
            (
                FolderTies {
                    parked: true,
                    ..FolderTies::default()
                },
                FolderKeep::Parked,
            ),
            (
                FolderTies {
                    waiting: true,
                    ..FolderTies::default()
                },
                FolderKeep::Waiting,
            ),
            (
                FolderTies {
                    owed_resume: true,
                    ..FolderTies::default()
                },
                FolderKeep::OwedResume,
            ),
            (
                FolderTies {
                    owed_message: true,
                    ..FolderTies::default()
                },
                FolderKeep::OwedMessage,
            ),
        ];
        for (ties, keep) in cases {
            assert_eq!(decide_folder(ties), FolderDecision::Keep(keep));
        }
    }

    #[test]
    fn a_claim_names_the_reason_before_the_others() {
        let all = FolderTies {
            claim: true,
            session: true,
            parked: true,
            waiting: true,
            owed_resume: true,
            owed_message: true,
        };
        assert_eq!(decide_folder(all), FolderDecision::Keep(FolderKeep::Claim));
    }
}
