use fold_core::events::types;
use fold_core::{Actor, Clock, FoldError, NewEvent, ProjectPaths, RevisionId, Timestamp, codes};
use fold_store::workspaces::{
    IntegrationKind, RevisionRow, get_by_number, head, insert_revision_commits, list_after,
    newest_human_change, uncommitted_revisions,
};
use fold_store::{Store, StoreError};
use fold_vcs::git::{CommitId, Git};
use fold_vcs::git_export::{
    ExportBase, ExportRevision, ExportTarget, Signature, repository_identity, write_commits,
};
use fold_vcs::git_remote::{
    FOLD_HEAD_REF, Followed, branch_ref, follow_main, follow_main_via, index_to, is_ancestor,
    move_branch, project_repository, set_ref,
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
/// The map is the record of which revision is which commit, so [`FOLD_HEAD_REF`] is first put on the map's last commit: a ref a stopped write moved past it is written again. A commit's id depends on its revision and on the Git identity the repository's config names when the commit is written, which is its author and committer, so the same identity writes the same commits again; once the map names a commit, it is that revision's commit for good, whatever the config says later. `git fast-import` writes the new commits there, and the map gains their lines. The `[git]` branch is checked out in the project folder and moves only by fast-forward, after the map: an unborn branch is created on `r1`'s commit, whose tree the folder holds as `fld init` leaves it, with the index set to it, and then follows as any branch does; a tip that is not an ancestor of fold's newest commit holds commits someone made outside fold, so nothing moves and the tip is returned in [`Committed::outside`]; otherwise the folder follows as [`follow_main`] describes, or stays when it has changes of its own. A stop between the map and the move leaves a branch that the next run moves. Then one transaction adds a `revision_commits` row and a `git.committed` event for every mapped revision that has none, which also covers a map line whose row a stop cut off.
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
    let merges = rows
        .iter()
        .map(merge_parents)
        .collect::<Result<Vec<_>, _>>()?;
    let via = {
        let mut conn = store.read().acquire().await.map_err(StoreError::from)?;
        newest_human_change(&mut conn, true).await?
    }
    .map(|(commit, _)| commit.parse::<CommitId>())
    .transpose()?;
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
        let identity = if rows.is_empty() {
            Signature::fold()
        } else {
            repository_identity(&git, &repo)?
        };
        let revisions: Vec<ExportRevision> = rows
            .iter()
            .zip(merges)
            .map(|(row, merge)| export_revision(row, merge, &identity))
            .collect();
        let wanted = base.as_ref().map(|base| &base.commit);
        if git.ref_commit(&repo, FOLD_HEAD_REF)?.as_ref() != wanted {
            set_ref(&git, &repo, FOLD_HEAD_REF, wanted)?;
        }
        let objects = ObjectStore::open_existing(&fold_dir).map_err(FoldError::from)?;
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
                let mut followed = follow_main(&git, &paths, &branch, &tip, &newest)?;
                // A folder that still holds the newest recorded human change follows from that change's commit.
                if followed == Followed::Dirty
                    && let Some(via) = via.as_ref().filter(|via| **via != tip)
                    && is_ancestor(&git, &repo, &tip, via)?
                    && (*via == newest || is_ancestor(&git, &repo, via, &newest)?)
                {
                    followed = follow_main_via(&git, &paths, &branch, &tip, via, &newest)?;
                }
                committed.uncommitted = followed == Followed::Dirty;
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

/// A revision as the commit it becomes, with `merges`, from [`merge_parents`], as its parents after the first and `identity` as its author and committer.
fn export_revision(
    row: &RevisionRow,
    merges: Vec<CommitId>,
    identity: &Signature,
) -> ExportRevision {
    ExportRevision {
        number: row.number,
        tree: row.tree_hash,
        created_at: row.created_at,
        message: commit_message(row),
        author: identity.clone(),
        committer: identity.clone(),
        merges,
    }
}

/// The parents after the first of the commit a revision becomes: the remote commit its integration brought in, a pull or the proposal of a sync task, and then the human's commit of a human change it brought onto main when that is another commit, as when a pull resolves a waiting human change. The remote commit comes first, as in the merge of every pull, and the human's commit is there so the project folder that holds the human's files can follow main through it.
fn merge_parents(row: &RevisionRow) -> Result<Vec<CommitId>, StoreError> {
    let Some(integration) = &row.integration else {
        return Ok(Vec::new());
    };
    let mut parents: Vec<CommitId> = Vec::new();
    let commits = [
        (&integration.remote_commit, "integrations.remote_commit"),
        (&integration.human_commit, "human_changes.commit_id"),
    ];
    for (commit, column) in commits {
        let Some(commit) = commit else {
            continue;
        };
        let commit: CommitId = commit.parse().map_err(|_| StoreError::Corrupt(column))?;
        if !parents.contains(&commit) {
            parents.push(commit);
        }
    }
    Ok(parents)
}

/// The message of the commit a revision becomes, ending in Git trailers that name it by public ids and never name an agent or a human: fold's names stay in fold. An integrated proposal gives its title as the subject and its task's title as the body. A direct change gives the message the main agent wrote, as it wrote it. A pull that merged a remote commit names that commit; an imported `r1` names its source; any other revision is `fold r<n>`.
pub fn commit_message(row: &RevisionRow) -> String {
    let revision = row.number;
    let Some(integration) = &row.integration else {
        return match &row.import_source {
            Some(source) => format!("Import {}\n\nFold-Revision: {revision}\n", one_line(source)),
            None => format!("fold {revision}\n\nFold-Revision: {revision}\n"),
        };
    };
    match (&integration.proposal, integration.kind) {
        (Some(proposal), _) => format!(
            "{}\n\n{}\n\nFold-Proposal: {}\nFold-Version: {}\nFold-Task: {}\nFold-Revision: {revision}\n",
            one_line(&proposal.title),
            one_line(&proposal.task_title),
            proposal.number,
            proposal.version,
            proposal.task,
        ),
        (None, IntegrationKind::Direct) => {
            let mut message = integration
                .message
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map_or_else(|| format!("fold {revision}"), str::to_owned);
            message.push_str("\n\n");
            if let Some(changeset) = integration.changeset {
                message.push_str(&format!("Fold-Changeset: {changeset}\n"));
            }
            message.push_str(&format!("Fold-Revision: {revision}\n"));
            message
        }
        (None, IntegrationKind::Pull) => {
            let remote = integration.remote_commit.as_deref().unwrap_or_default();
            let short = remote.get(..12).unwrap_or(remote);
            format!("Merge remote commit {short}\n\nFold-Revision: {revision}\n")
        }
        (None, IntegrationKind::Proposal) => {
            format!("fold {revision}\n\nFold-Revision: {revision}\n")
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
            human_commit: None,
            proposal: proposal.map(|(title, task)| IntegratedProposal {
                number: ProposalId::from_number(4).unwrap(),
                title: title.to_owned(),
                version: 2,
                task: TaskId::from_number(task).unwrap(),
                task_title: "Reports\nfor admins".to_owned(),
                agent: "backend".to_owned(),
                worker: "backend/db".to_owned(),
            }),
            changeset: None,
            message: None,
        }
    }

    fn direct(message: &str) -> IntegrationInfo {
        IntegrationInfo {
            actor: "main".to_owned(),
            changeset: Some(fold_core::ChangesetId::from_number(40).unwrap()),
            message: Some(message.to_owned()),
            ..integration(IntegrationKind::Direct, None, None)
        }
    }

    #[test]
    fn messages_follow_where_the_revision_came_from() {
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
                "Add the reports schema\n\nReports for admins\n\nFold-Proposal: P-4\nFold-Version: 2\nFold-Task: T-12\nFold-Revision: r7\n",
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
                "Two lines\n\nReports for admins\n\nFold-Proposal: P-4\nFold-Version: 2\nFold-Task: T-3\nFold-Revision: r8\n",
            ),
            (
                row(1, Some("/home/ana/src/app"), None),
                "Import /home/ana/src/app\n\nFold-Revision: r1\n",
            ),
            (row(1, None, None), "fold r1\n\nFold-Revision: r1\n"),
            (
                row(
                    6,
                    None,
                    Some(integration(IntegrationKind::Pull, None, Some(&remote))),
                ),
                "Merge remote commit aaaaaaaaaaaa\n\nFold-Revision: r6\n",
            ),
            (
                row(
                    9,
                    None,
                    Some(integration(IntegrationKind::Proposal, None, None)),
                ),
                "fold r9\n\nFold-Revision: r9\n",
            ),
            (
                row(
                    10,
                    None,
                    Some(direct(
                        "Add the AGENTS.md rules\n\nTaken from the parent folder.\n",
                    )),
                ),
                "Add the AGENTS.md rules\n\nTaken from the parent folder.\n\nFold-Changeset: C-40\nFold-Revision: r10\n",
            ),
            (
                row(11, None, Some(direct("  "))),
                "fold r11\n\nFold-Changeset: C-40\nFold-Revision: r11\n",
            ),
        ];
        for (row, expected) in cases {
            assert_eq!(commit_message(&row), expected);
        }
    }

    #[test]
    fn every_commit_is_authored_and_committed_by_the_given_identity() {
        let ana = Signature {
            name: "Ana Lima".to_owned(),
            email: "ana@example.com".to_owned(),
        };
        let integrated = row(
            7,
            None,
            Some(integration(
                IntegrationKind::Proposal,
                Some(("Title", 1)),
                Some(&"b".repeat(40)),
            )),
        );
        let merges = merge_parents(&integrated).unwrap();
        let revision = export_revision(&integrated, merges, &ana);
        assert_eq!((&revision.author, &revision.committer), (&ana, &ana));
        assert!(!revision.message.contains("backend") && !revision.message.contains("ana"));
        assert_eq!(
            revision
                .merges
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["b".repeat(40)]
        );
        let plain = row(1, None, None);
        let plain = export_revision(&plain, merge_parents(&plain).unwrap(), &Signature::fold());
        assert_eq!(
            (&plain.author, &plain.committer),
            (&Signature::fold(), &Signature::fold())
        );
        assert!(plain.merges.is_empty());
        let corrupt = row(
            2,
            None,
            Some(integration(IntegrationKind::Pull, None, Some("nope"))),
        );
        assert!(merge_parents(&corrupt).is_err());
        let changed = export_revision(
            &row(3, None, Some(direct("Initial setup"))),
            Vec::new(),
            &ana,
        );
        assert_eq!((&changed.author, &changed.committer), (&ana, &ana));
        assert!(!changed.message.contains("main"));
    }

    #[test]
    fn a_pull_that_resolves_a_human_change_merges_the_pulled_commit_then_the_humans() {
        let (pulled, human) = ("c".repeat(40), "d".repeat(40));
        let resolving = IntegrationInfo {
            human_commit: Some(human.clone()),
            ..integration(IntegrationKind::Pull, None, Some(&pulled))
        };
        let merges = merge_parents(&row(5, None, Some(resolving))).unwrap();
        assert_eq!(
            merges.iter().map(ToString::to_string).collect::<Vec<_>>(),
            [pulled.clone(), human.clone()]
        );
        // A record brings in the human's own commit, which is the pulled commit too.
        let recorded = IntegrationInfo {
            human_commit: Some(pulled.clone()),
            ..integration(IntegrationKind::Pull, None, Some(&pulled))
        };
        assert_eq!(
            merge_parents(&row(6, None, Some(recorded))).unwrap().len(),
            1
        );
        let corrupt = IntegrationInfo {
            human_commit: Some("nope".to_owned()),
            ..integration(IntegrationKind::Pull, None, Some(&pulled))
        };
        assert!(merge_parents(&row(7, None, Some(corrupt))).is_err());
    }
}
