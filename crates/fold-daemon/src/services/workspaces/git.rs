use std::path::Path;

use fold_core::{FoldError, RevisionId, codes};
use fold_store::workspaces::{RevisionRow, get_by_number, list_after};
use fold_store::{Store, StoreError};
use fold_vcs::git::CommitId;
use fold_vcs::git_export::{
    ExportBase, ExportRevision, ExportTarget, Signature, TargetState, check_target, create_target,
    recover_stopped, write_commits,
};
use fold_vcs::git_remote::{MIRROR_DIR, fetch_remote_refs};
use fold_vcs::git_sync_map::SyncMap;
use fold_vcs::objects::ObjectStore;

use super::{WorkspaceError, WorkspaceService, blocking};

/// The branch every export writes, in the target repository.
pub const MAIN_REF: &str = "refs/heads/main";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitExport {
    /// Every main revision is exported already; `last` is the newest mapped one.
    UpToDate { last: Option<RevisionId> },
    Exported {
        first: RevisionId,
        last: RevisionId,
        commits: usize,
        /// The commit `main` points to in the target.
        head: CommitId,
    },
}

/// What an export does with the target `check_target` found.
enum Plan {
    Write(TargetState),
    /// The branch is not where the map says; the error is what a branch fold did not write gets.
    Recover(FoldError),
}

impl WorkspaceService {
    /// Appends the main revisions after the last one in `.fold/git-sync.map` to `main` in the Git repository `repo`, one commit each, creating a bare repository when the path is missing or an empty directory. A revision that brought a remote commit in gets that commit as its second parent; the mirror's fetched refs are copied into `repo` first, so the commit is there. The target must continue the mapped chain, or the export is refused with `export_diverged` and its branch does not move; with nothing new it reports up to date. The map gains the new commits only after `git fast-import` succeeds; a branch that fast-import moved before the map was appended to, on the first export too, is recognised as fold's own by its commits and finished on the next export. Telling the two apart writes the expected commits into the target, where a refused export leaves them unreferenced. The project's Git sync lock is held throughout, so concurrent exports, in this process or another, run one after the other.
    pub async fn export_git(&self, repo: &Path) -> Result<GitExport, WorkspaceError> {
        let fold_dir = self.paths.fold_dir();
        // Held until the map is appended to, so a second export waits and then finds the first one's commits.
        let (_lock, map) = {
            let fold_dir = fold_dir.clone();
            blocking(move || SyncMap::lock(&fold_dir)).await?
        };
        let last = map.last().cloned();
        let checked = {
            let (git, repo, last) = (self.git.clone(), repo.to_owned(), last.clone());
            blocking(move || {
                let target = ExportTarget {
                    repo: &repo,
                    refname: MAIN_REF,
                };
                Ok(check_target(&git, &target, last.as_ref()))
            })
            .await?
        };
        let (rows, base) = unsynced(&self.store, last.as_ref()).await?;
        let up_to_date = GitExport::UpToDate {
            last: last.as_ref().map(|(number, _)| *number),
        };
        // A branch that moved on from the last mapped commit may be an export that stopped before the map was appended to; `recover_stopped` tells that from a commit fold did not write.
        let plan = match checked {
            Ok(_) if rows.is_empty() => return Ok(up_to_date),
            Ok(state) => Plan::Write(state),
            Err(error) if error.code() == codes::EXPORT_DIVERGED && !rows.is_empty() => {
                Plan::Recover(error)
            }
            Err(error) => return Err(error.into()),
        };
        let fold = Signature {
            name: "fold".to_owned(),
            email: "fold@localhost".to_owned(),
        };
        let revisions = rows
            .iter()
            .map(|row| {
                Ok(ExportRevision {
                    number: row.number,
                    tree: row.tree_hash,
                    created_at: row.created_at,
                    message: commit_message(row),
                    author: fold.clone(),
                    committer: fold.clone(),
                    merge: merge_parent(row)?,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        let merges = revisions.iter().any(|revision| revision.merge.is_some());
        let (git, repo) = (self.git.clone(), repo.to_owned());
        let added = blocking(move || {
            let target = ExportTarget {
                repo: &repo,
                refname: MAIN_REF,
            };
            let objects = ObjectStore::open(&fold_dir)?;
            let added = match plan {
                Plan::Write(state) => {
                    if state == TargetState::Missing {
                        create_target(&git, &target)?;
                    }
                    if merges {
                        fetch_remote_refs(&git, &repo, &fold_dir.join(MIRROR_DIR))?;
                    }
                    write_commits(
                        &objects,
                        &git,
                        &target,
                        base.as_ref(),
                        &revisions,
                        &fold_dir,
                    )?
                }
                Plan::Recover(diverged) => {
                    if merges {
                        fetch_remote_refs(&git, &repo, &fold_dir.join(MIRROR_DIR))?;
                    }
                    recover_stopped(
                        &objects,
                        &git,
                        &target,
                        base.as_ref(),
                        &revisions,
                        &fold_dir,
                    )?
                    .ok_or(diverged)?
                }
            };
            #[cfg(feature = "failpoints")]
            fail::fail_point!("git_export_before_map", |_| Err(FoldError::new(
                codes::FAILPOINT,
                "git_export_before_map fired"
            )));
            let mut map = map;
            map.append(&added)?;
            Ok(added)
        })
        .await?;
        Ok(match (added.first(), added.last()) {
            (Some((first, _)), Some((newest, head))) => GitExport::Exported {
                first: *first,
                last: *newest,
                commits: added.len(),
                head: head.clone(),
            },
            _ => up_to_date,
        })
    }
}

/// The main revisions after `last`, the newest mapped revision and its commit, oldest first, with the base the first of them continues; every revision and no base when nothing is mapped.
pub(crate) async fn unsynced(
    store: &Store,
    last: Option<&(RevisionId, CommitId)>,
) -> Result<(Vec<RevisionRow>, Option<ExportBase>), StoreError> {
    let pool = store.read();
    match last {
        Some((number, commit)) => {
            let base = ExportBase {
                commit: commit.clone(),
                tree: get_by_number(pool, *number).await?.tree_hash,
            };
            Ok((list_after(pool, *number).await?, Some(base)))
        }
        None => {
            let first =
                RevisionId::from_number(1).ok_or(StoreError::Corrupt("revision number 1"))?;
            let mut rows = vec![get_by_number(pool, first).await?];
            rows.extend(list_after(pool, first).await?);
            Ok((rows, None))
        }
    }
}

/// The second parent of the commit a revision becomes: the remote commit its integration brought in, a pull or the proposal of a sync task.
pub(crate) fn merge_parent(row: &RevisionRow) -> Result<Option<CommitId>, StoreError> {
    row.integration
        .as_ref()
        .and_then(|integration| integration.remote_commit.as_deref())
        .map(|commit| {
            commit
                .parse()
                .map_err(|_| StoreError::Corrupt("integrations.remote_commit"))
        })
        .transpose()
}

/// The message of the commit a revision becomes. An integrated proposal gives its id and title as the subject and names the revision, the task and the integrating actor; an imported `r1` names its source; any other revision, a plain-init `r1` or a pull among them, is `fold r<n>`.
pub fn commit_message(row: &RevisionRow) -> String {
    let proposal = row
        .integration
        .as_ref()
        .and_then(|integration| Some((integration.proposal.as_ref()?, &integration.actor)));
    match (proposal, &row.import_source) {
        (Some((proposal, actor)), _) => format!(
            "{}: {}\n\nRevision: {}\nTask: {}\nIntegrated by: {actor}\n",
            proposal.number,
            one_line(&proposal.title),
            row.number,
            proposal.task
        ),
        (None, Some(source)) => {
            format!("Import {}\n\nRevision: {}\n", one_line(source), row.number)
        }
        (None, None) => format!("fold {}\n\nRevision: {}\n", row.number, row.number),
    }
}

/// A subject is one line whatever the stored text holds.
fn one_line(text: &str) -> String {
    text.split(['\n', '\r'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use fold_core::object::ObjectHash;
    use fold_core::{IntegrationId, ProposalId, TaskId, Timestamp};
    use fold_store::workspaces::{IntegratedProposal, IntegrationInfo, IntegrationKind};

    use super::*;

    fn row(
        number: u32,
        import_source: Option<&str>,
        integration: Option<IntegrationInfo>,
    ) -> RevisionRow {
        let hash: ObjectHash = "0".repeat(64).parse().unwrap();
        RevisionRow {
            number: RevisionId::from_number(number).unwrap(),
            hash,
            tree_hash: hash,
            parent: None,
            created_at: Timestamp::from_millis(0).unwrap(),
            import_source: import_source.map(str::to_owned),
            integration,
        }
    }

    fn integration(kind: IntegrationKind, proposal: Option<(&str, u32)>) -> IntegrationInfo {
        IntegrationInfo {
            number: IntegrationId::from_number(1).unwrap(),
            kind,
            actor: "human:ana".to_owned(),
            remote_commit: None,
            proposal: proposal.map(|(title, task)| IntegratedProposal {
                number: ProposalId::from_number(4).unwrap(),
                title: title.to_owned(),
                task: TaskId::from_number(task).unwrap(),
                agent: "backend".to_owned(),
                worker: "w1".to_owned(),
            }),
        }
    }

    #[test]
    fn messages_follow_where_the_revision_came_from() {
        let cases = [
            (
                row(
                    7,
                    None,
                    Some(integration(
                        IntegrationKind::Proposal,
                        Some(("Database schema for reports", 12)),
                    )),
                ),
                "P-4: Database schema for reports\n\nRevision: r7\nTask: T-12\nIntegrated by: human:ana\n",
            ),
            (
                row(1, Some("/home/ana/src/app"), None),
                "Import /home/ana/src/app\n\nRevision: r1\n",
            ),
            (row(1, None, None), "fold r1\n\nRevision: r1\n"),
            (row(5, None, None), "fold r5\n\nRevision: r5\n"),
            (
                row(6, None, Some(integration(IntegrationKind::Pull, None))),
                "fold r6\n\nRevision: r6\n",
            ),
            (
                row(
                    8,
                    None,
                    Some(integration(
                        IntegrationKind::Proposal,
                        Some(("Two\r\nlines", 3)),
                    )),
                ),
                "P-4: Two lines\n\nRevision: r8\nTask: T-3\nIntegrated by: human:ana\n",
            ),
        ];
        for (row, expected) in cases {
            assert_eq!(commit_message(&row), expected);
        }
    }
}
