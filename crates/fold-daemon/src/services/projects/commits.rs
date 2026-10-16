use fold_core::events::types;
use fold_core::reviews::standing_approvers;
use fold_core::{
    Actor, Clock, EventActor, FoldError, NewEvent, ProjectPaths, RevisionId, Timestamp, codes,
};
use fold_store::workspaces::{
    IntegrationKind, RevisionRow, get_by_number, head, insert_revision_commits, list_after,
    uncommitted_revisions,
};
use fold_store::{Store, StoreError, reviews};
use fold_vcs::git::{CommitId, Git};
use fold_vcs::git_export::{ExportBase, ExportRevision, ExportTarget, Signature, write_commits};
use fold_vcs::git_remote::{
    FOLD_HEAD_REF, Followed, branch_ref, follow_main, index_to, is_ancestor, move_branch,
    project_repository, set_ref,
};
use fold_vcs::git_sync_map::SyncMap;
use fold_vcs::objects::ObjectStore;
use serde_json::json;

use super::remote::blocking;
use super::{ProjectError, load_config};
use crate::services::events::EventBuffer;
#[cfg(feature = "failpoints")]
use crate::services::fired;

/// What one run of the commit writer did.
#[derive(Debug, Default)]
pub struct Committed {
    /// The revisions it wrote commits for, oldest first.
    pub added: Vec<(RevisionId, CommitId)>,
    /// The tip of the `[git]` branch when it holds commits fold did not write: the commits are recorded, the branch and the project folder stay. `fld pull` brings them in.
    pub outside: Option<CommitId>,
    /// The project folder has changes that are not committed, or an untracked file in the way, so the branch and the folder stay on the older commit until the next run finds the folder clean.
    pub uncommitted: bool,
    /// One `git.committed` per `revision_commits` row the run added, appended in the transaction that added the rows; the caller publishes them.
    pub events: EventBuffer,
}

/// Writes the commit of every main revision that `.fold/git-sync.map` does not name yet into the project's Git repository at [`ProjectPaths::git_dir`], records each in `revision_commits`, and brings the `[git]` branch and the project folder to the newest. The project's Git sync lock is held throughout, so this, a push and a pull run one after the other, in this process or another. Running it again with nothing new changes nothing.
pub async fn commit_main(
    store: &Store,
    paths: &ProjectPaths,
    git: &Git,
    clock: &dyn Clock,
) -> Result<Committed, ProjectError> {
    let branch = load_config(paths).await?.git.branch;
    let fold_dir = paths.fold_dir();
    let (held, map) = blocking(move || SyncMap::lock(&fold_dir)).await?;
    let (_held, _map, committed) =
        commit_locked(store, paths, git, &branch, held, map, clock.now()).await?;
    Ok(committed)
}

/// [`commit_main`] for a caller that holds the Git sync lock (`held`) and read `map` under it; both come back for the caller's next step.
///
/// The map is the record of which revision is which commit, so [`FOLD_HEAD_REF`] is first put on the map's last commit: a ref a stopped write moved past it is written again, to the same commits, because a commit's id depends only on the revision. `git fast-import` writes the new commits there, and the map gains their lines. The `[git]` branch is checked out in the project folder and moves only by fast-forward, after the map: an unborn branch is created on `r1`'s commit, whose tree the folder holds as `fld init` leaves it, with the index set to it, and then follows as any branch does; a tip that is not an ancestor of fold's newest commit holds commits someone made outside fold, so nothing moves and the tip is returned in [`Committed::outside`]; otherwise the folder follows as [`follow_main`] describes, or stays when it has changes of its own. A stop between the map and the move leaves a branch that the next run moves. Then one transaction adds a `revision_commits` row and a `git.committed` event for every mapped revision that has none, which also covers a map line whose row a stop cut off.
pub(crate) async fn commit_locked<H: Send + 'static>(
    store: &Store,
    paths: &ProjectPaths,
    git: &Git,
    branch: &str,
    held: H,
    map: SyncMap,
    now: Timestamp,
) -> Result<(H, SyncMap, Committed), ProjectError> {
    let last = map.last().cloned();
    let (rows, base) = unsynced(store, last.as_ref()).await?;
    if rows.is_empty() && last.is_none() {
        return Ok((held, map, Committed::default()));
    }
    let revisions = export_revisions(store, &rows).await?;
    let (git, paths_owned, branch_owned) = (git.clone(), paths.clone(), branch.to_owned());
    let (held, map, mut committed) = blocking(move || {
        let (paths, branch) = (paths_owned, branch_owned);
        #[cfg(feature = "failpoints")]
        fail::fail_point!("git_commit_before_write", |_| Err(fired(
            "git_commit_before_write"
        )));
        let repo = project_repository(&paths)?;
        let fold_dir = paths.fold_dir();
        if let Some((number, commit)) = &last
            && !git.has_commit(&repo, commit)?
        {
            return Err(history_missing(*number, commit));
        }
        let wanted = base.as_ref().map(|base| &base.commit);
        if git.ref_commit(&repo, FOLD_HEAD_REF)?.as_ref() != wanted {
            set_ref(&git, &repo, FOLD_HEAD_REF, wanted)?;
        }
        let objects = ObjectStore::open(&fold_dir).map_err(FoldError::from)?;
        let target = ExportTarget {
            repo: &repo,
            refname: FOLD_HEAD_REF,
        };
        let added = write_commits(
            &objects,
            &git,
            &target,
            base.as_ref(),
            &revisions,
            &fold_dir,
        )?;
        let newest = added.last().map(|(_, commit)| commit).or(wanted).cloned();
        #[cfg(feature = "failpoints")]
        fail::fail_point!("git_commit_before_map", |_| Err(fired(
            "git_commit_before_map"
        )));
        let mut map = map;
        map.append(&added)?;
        #[cfg(feature = "failpoints")]
        fail::fail_point!("git_commit_before_branch", |_| Err(fired(
            "git_commit_before_branch"
        )));
        let mut committed = Committed {
            added,
            ..Committed::default()
        };
        let Some(newest) = newest else {
            return Ok((held, map, committed));
        };
        let tip = match git.ref_commit(&repo, &branch_ref(&branch))? {
            Some(tip) => tip,
            None => {
                // The folder's files are r1's when its branch is born, as `fld init` leaves them.
                let first = map
                    .entries()
                    .first()
                    .map_or_else(|| newest.clone(), |(_, commit)| commit.clone());
                move_branch(&git, &repo, &branch, &first, None)?;
                index_to(&git, &paths, &first)?;
                first
            }
        };
        match tip {
            tip if tip == newest => {}
            tip if is_ancestor(&git, &repo, &tip, &newest)? => {
                committed.uncommitted =
                    follow_main(&git, &paths, &branch, &tip, &newest)? == Followed::Dirty;
            }
            tip => committed.outside = Some(tip),
        }
        Ok((held, map, committed))
    })
    .await?;
    #[cfg(feature = "failpoints")]
    fail::fail_point!("git_commit_before_rows", |_| Err(fired(
        "git_commit_before_rows"
    )
    .into()));
    let missing: Vec<(RevisionId, String)> = uncommitted_revisions(store.read())
        .await?
        .into_iter()
        .filter_map(|number| Some((number, map.commit_of(number)?.to_string())))
        .collect();
    if !missing.is_empty() {
        let events = missing
            .iter()
            .map(|(revision, commit)| {
                NewEvent::new(types::GIT_COMMITTED, Actor::System.event_actor())
                    .and_then(|event| event.data(json!({"commit": commit})))
                    .map(|event| event.revision(*revision))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| FoldError::new(codes::INTERNAL_ERROR, error.to_string()))?;
        let branch = branch.to_owned();
        committed.events = store
            .write(async |tx| -> Result<EventBuffer, ProjectError> {
                insert_revision_commits(tx, &branch, &missing, now).await?;
                let mut buffer = EventBuffer::new();
                for event in &events {
                    buffer.append(tx, event, now).await?;
                }
                Ok(buffer)
            })
            .await?;
    }
    Ok((held, map, committed))
}

/// The map names `commit` as the commit of `revision`, and the repository cannot get it, so the commits after it have no parent to continue.
fn history_missing(revision: RevisionId, commit: &CommitId) -> FoldError {
    FoldError::new(
        codes::EXPORT_DIVERGED,
        format!(
            "the project's Git repository does not hold {commit}, the commit of {revision}, so the commits of the revisions after it cannot be written"
        ),
    )
    .with_hint("run fld remote set <name> <url> with a repository that holds that commit, such as one the project was pushed to, then fld pull <name>")
    .with_details([
        ("revision", json!(revision.to_string())),
        ("commit", json!(commit.as_str())),
    ])
}

/// The main revisions after `last`, the newest mapped revision and its commit, oldest first, with the base the first of them continues; every revision and no base when nothing is mapped, and none when main has no revision.
async fn unsynced(
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
            if head(pool).await?.is_none() {
                return Ok((Vec::new(), None));
            }
            let first =
                RevisionId::from_number(1).ok_or(StoreError::Corrupt("revision number 1"))?;
            let mut rows = vec![get_by_number(pool, first).await?];
            rows.extend(list_after(pool, first).await?);
            Ok((rows, None))
        }
    }
}

/// Each revision as the commit it becomes, with the reviewers whose approval of the integrated version stands.
async fn export_revisions(
    store: &Store,
    rows: &[RevisionRow],
) -> Result<Vec<ExportRevision>, ProjectError> {
    let mut revisions = Vec::with_capacity(rows.len());
    for row in rows {
        let reviewers = match row
            .integration
            .as_ref()
            .and_then(|integration| integration.proposal.as_ref())
        {
            Some(proposal) => standing_approvers(
                &reviews::review_facts(store.read(), proposal.number, proposal.version).await?,
            ),
            None => Vec::new(),
        };
        revisions.push(export_revision(row, &reviewers)?);
    }
    Ok(revisions)
}

/// The committer of every commit fold writes, and the author of every commit that no proposal wrote.
fn fold_signature() -> Signature {
    Signature {
        name: "fold".to_owned(),
        email: "fold@localhost".to_owned(),
    }
}

fn export_revision(
    row: &RevisionRow,
    reviewers: &[EventActor],
) -> Result<ExportRevision, StoreError> {
    let proposal = row
        .integration
        .as_ref()
        .and_then(|integration| integration.proposal.as_ref());
    let author = match proposal {
        Some(proposal) => author(&proposal.agent, &proposal.worker),
        None => fold_signature(),
    };
    Ok(ExportRevision {
        number: row.number,
        tree: row.tree_hash,
        created_at: row.created_at,
        message: commit_message(row, reviewers),
        author,
        committer: fold_signature(),
        merge: merge_parent(row)?,
    })
}

/// The worker that wrote a proposal as a Git author: `frontend/ui` is `frontend/ui <frontend.ui@fold.local>` and the main worker `frontend` is `frontend <frontend.frontend@fold.local>`. A deeper worker such as `frontend/ui/forms` gives `frontend.ui.forms@fold.local`.
fn author(agent: &str, worker: &str) -> Signature {
    let local = match worker.split_once('/') {
        Some((_, rest)) => rest.replace('/', "."),
        None => worker.to_owned(),
    };
    Signature {
        name: worker.to_owned(),
        email: format!("{agent}.{local}@fold.local"),
    }
}

/// The second parent of the commit a revision becomes: the remote commit its integration brought in, a pull or the proposal of a sync task.
fn merge_parent(row: &RevisionRow) -> Result<Option<CommitId>, StoreError> {
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

/// The message of the commit a revision becomes, ending in Git trailers that name it by public ids. An integrated proposal gives its title as the subject and its task's title as the body, with `reviewers`, those whose approval of the integrated version stands, as `Reviewed-by`. A pull that merged a remote commit names that commit; an imported `r1` names its source; any other revision is `fold r<n>`.
pub fn commit_message(row: &RevisionRow, reviewers: &[EventActor]) -> String {
    let revision = row.number;
    let Some(integration) = &row.integration else {
        return match &row.import_source {
            Some(source) => format!("Import {}\n\nFold-Revision: {revision}\n", one_line(source)),
            None => format!("fold {revision}\n\nFold-Revision: {revision}\n"),
        };
    };
    let actor = &integration.actor;
    match (&integration.proposal, integration.kind) {
        (Some(proposal), _) => {
            let mut message = format!(
                "{}\n\n{}\n\nFold-Proposal: {}\nFold-Version: {}\nFold-Task: {}\nFold-Revision: {revision}\n",
                one_line(&proposal.title),
                one_line(&proposal.task_title),
                proposal.number,
                proposal.version,
                proposal.task,
            );
            for reviewer in reviewers {
                message.push_str(&format!("Reviewed-by: {reviewer}\n"));
            }
            message.push_str(&format!("Integrated-by: {actor}\n"));
            message
        }
        (None, IntegrationKind::Pull) => {
            let remote = integration.remote_commit.as_deref().unwrap_or_default();
            let short = remote.get(..12).unwrap_or(remote);
            format!(
                "Merge remote commit {short}\n\nFold-Revision: {revision}\nIntegrated-by: {actor}\n"
            )
        }
        (None, IntegrationKind::Proposal) => {
            format!("fold {revision}\n\nFold-Revision: {revision}\nIntegrated-by: {actor}\n")
        }
    }
}

/// A subject or body line is one line whatever the stored text holds.
fn one_line(text: &str) -> String {
    text.split(['\n', '\r'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// An unknown error code stays an internal error in the log line and the event.
pub(crate) fn error_code(error: &ProjectError) -> &'static str {
    error.code().unwrap_or(codes::INTERNAL_ERROR)
}

#[cfg(test)]
mod tests {
    use fold_core::object::ObjectHash;
    use fold_core::{IntegrationId, ProposalId, TaskId};
    use fold_store::workspaces::{IntegratedProposal, IntegrationInfo};

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
            commit: None,
        }
    }

    fn integration(
        kind: IntegrationKind,
        proposal: Option<(&str, u32)>,
        remote_commit: Option<&str>,
    ) -> IntegrationInfo {
        IntegrationInfo {
            number: IntegrationId::from_number(1).unwrap(),
            kind,
            actor: "human:ana".to_owned(),
            remote_commit: remote_commit.map(str::to_owned),
            proposal: proposal.map(|(title, task)| IntegratedProposal {
                number: ProposalId::from_number(4).unwrap(),
                title: title.to_owned(),
                version: 2,
                task: TaskId::from_number(task).unwrap(),
                task_title: "Reports\nfor admins".to_owned(),
                agent: "backend".to_owned(),
                worker: "backend/db".to_owned(),
            }),
        }
    }

    #[test]
    fn messages_follow_where_the_revision_came_from() {
        let reviewers = [
            EventActor::parse("review").unwrap(),
            EventActor::parse("human:bo").unwrap(),
        ];
        let remote = "a".repeat(40);
        let cases = [
            (
                row(
                    7,
                    None,
                    Some(integration(
                        IntegrationKind::Proposal,
                        Some(("Add the reports schema", 12)),
                        None,
                    )),
                ),
                &reviewers[..],
                "Add the reports schema\n\nReports for admins\n\nFold-Proposal: P-4\nFold-Version: 2\nFold-Task: T-12\nFold-Revision: r7\nReviewed-by: review\nReviewed-by: human:bo\nIntegrated-by: human:ana\n",
            ),
            (
                row(
                    8,
                    None,
                    Some(integration(
                        IntegrationKind::Proposal,
                        Some(("Two\r\nlines", 3)),
                        None,
                    )),
                ),
                &[][..],
                "Two lines\n\nReports for admins\n\nFold-Proposal: P-4\nFold-Version: 2\nFold-Task: T-3\nFold-Revision: r8\nIntegrated-by: human:ana\n",
            ),
            (
                row(1, Some("/home/ana/src/app"), None),
                &[][..],
                "Import /home/ana/src/app\n\nFold-Revision: r1\n",
            ),
            (
                row(1, None, None),
                &[][..],
                "fold r1\n\nFold-Revision: r1\n",
            ),
            (
                row(
                    6,
                    None,
                    Some(integration(IntegrationKind::Pull, None, Some(&remote))),
                ),
                &[][..],
                "Merge remote commit aaaaaaaaaaaa\n\nFold-Revision: r6\nIntegrated-by: human:ana\n",
            ),
            (
                row(
                    9,
                    None,
                    Some(integration(IntegrationKind::Proposal, None, None)),
                ),
                &[][..],
                "fold r9\n\nFold-Revision: r9\nIntegrated-by: human:ana\n",
            ),
        ];
        for (row, reviewers, expected) in cases {
            assert_eq!(commit_message(&row, reviewers), expected);
        }
    }

    #[test]
    fn a_proposal_is_authored_by_its_worker_and_committed_by_fold() {
        let integrated = row(
            7,
            None,
            Some(integration(
                IntegrationKind::Proposal,
                Some(("Title", 1)),
                Some(&"b".repeat(40)),
            )),
        );
        let revision = export_revision(&integrated, &[]).unwrap();
        assert_eq!(
            (
                revision.author.name.as_str(),
                revision.author.email.as_str()
            ),
            ("backend/db", "backend.db@fold.local")
        );
        assert_eq!(revision.committer, fold_signature());
        assert_eq!(
            revision.merge.map(|commit| commit.to_string()),
            Some("b".repeat(40))
        );
        let plain = export_revision(&row(1, None, None), &[]).unwrap();
        assert_eq!(plain.author, fold_signature());
        assert_eq!(plain.merge, None);
        let corrupt = row(
            2,
            None,
            Some(integration(IntegrationKind::Pull, None, Some("nope"))),
        );
        assert!(export_revision(&corrupt, &[]).is_err());
    }

    #[test]
    fn the_author_email_joins_the_agent_and_the_worker_below_it() {
        let cases = [
            (
                "frontend",
                "frontend/ui",
                "frontend/ui",
                "frontend.ui@fold.local",
            ),
            (
                "frontend",
                "frontend",
                "frontend",
                "frontend.frontend@fold.local",
            ),
            (
                "frontend",
                "frontend/ui/forms",
                "frontend/ui/forms",
                "frontend.ui.forms@fold.local",
            ),
        ];
        for (agent, worker, name, email) in cases {
            let signature = author(agent, worker);
            assert_eq!(
                (signature.name.as_str(), signature.email.as_str()),
                (name, email)
            );
        }
    }
}
