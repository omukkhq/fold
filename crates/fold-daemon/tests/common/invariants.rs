use std::collections::BTreeSet;

use fold_core::events::types;
use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::revision::Revision;
use fold_core::status::calculate;
use fold_core::tree::{EntryKind, Tree, flatten};
use fold_core::{FoldError, ProjectPaths, TaskId};
use fold_store::testing::TestProject;
use fold_store::{Store, tasks};
use fold_vcs::objects::ObjectStore;

/// Fails with every broken rule of the project and the ids that break it. A crash at any point of a command must leave a project that passes: all of the change or none of it.
pub async fn assert_consistent(project: &TestProject) {
    assert_store_consistent(project.store(), project.paths()).await;
}

/// [`assert_consistent`] on any open project database and its directory, for a test that runs the real daemon.
pub async fn assert_store_consistent(store: &Store, paths: &ProjectPaths) {
    let mut problems = Vec::new();
    revisions(store, &mut problems).await;
    revision_objects(store, paths, &mut problems).await;
    completions(store, &mut problems).await;
    statuses(store, &mut problems).await;
    objects(store, paths, &mut problems).await;
    events(store, &mut problems).await;
    append_only(store, &mut problems).await;
    assert!(
        problems.is_empty(),
        "the project is inconsistent:\n{}",
        problems.join("\n")
    );
}

/// Runs `sql`, which selects one text column naming each offender, and reports a non-empty result under `rule`.
async fn offenders(store: &Store, problems: &mut Vec<String>, rule: &str, sql: &'static str) {
    let found: Vec<String> = sqlx::query_scalar(sql)
        .fetch_all(store.read())
        .await
        .unwrap();
    if !found.is_empty() {
        problems.push(format!("{rule}: {}", found.join(", ")));
    }
}

/// `main` names the highest revision, and every revision after the genesis, the first one, which has no parent, is the one revision of a succeeded integration. Integration numbers may leave gaps: a number is allocated before the transaction that would use it.
async fn revisions(store: &Store, problems: &mut Vec<String>) {
    offenders(
        store,
        problems,
        "main is not the highest revision",
        "SELECT 'r' || r.number FROM refs f JOIN revisions r ON r.id = f.revision_id
         WHERE f.name = 'main' AND r.number <> (SELECT MAX(number) FROM revisions)
         UNION ALL
         SELECT 'no main' WHERE NOT EXISTS (SELECT 1 FROM refs WHERE name = 'main')",
    )
    .await;
    offenders(
        store,
        problems,
        "a revision after the genesis without a succeeded integration",
        "SELECT 'r' || r.number FROM revisions r LEFT JOIN integrations i ON i.id = r.integration_id
         WHERE r.parent_id IS NOT NULL
           AND (i.id IS NULL OR i.result <> 'succeeded' OR i.kind NOT IN ('proposal', 'pull', 'direct', 'record')
                OR i.revision_id IS NOT r.id)",
    )
    .await;
    offenders(
        store,
        problems,
        "a succeeded integration without its revision",
        "SELECT 'i-' || i.number FROM integrations i LEFT JOIN revisions r ON r.id = i.revision_id
         WHERE i.result = 'succeeded' AND (r.id IS NULL OR r.integration_id IS NOT i.id)",
    )
    .await;
}

/// Each revision object holds what its row says, its tree, its parent's hash, its number and its integration, and its parent is the revision numbered just before it, so `main` is one chain of whole revisions from the genesis, the first revision.
async fn revision_objects(store: &Store, paths: &ProjectPaths, problems: &mut Vec<String>) {
    type Row = (
        i64,
        String,
        String,
        Option<String>,
        Option<i64>,
        Option<i64>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT r.number, r.hash, r.tree_hash, p.hash, p.number, i.number FROM revisions r
         LEFT JOIN revisions p ON p.id = r.parent_id
         LEFT JOIN integrations i ON i.id = r.integration_id
         ORDER BY r.number",
    )
    .fetch_all(store.read())
    .await
    .unwrap();
    let objects = ObjectStore::open(&paths.fold_dir()).unwrap();
    let first = rows.first().map(|row| row.0);
    for (number, hash, tree, parent_hash, parent_number, integration) in rows {
        if parent_number != (Some(number) != first).then_some(number - 1) {
            problems.push(format!(
                "a revision whose parent is not the one before it: r{number}"
            ));
        }
        // A missing object is reported by `objects`.
        let Ok(object) = objects.read_expecting(&hash.parse().unwrap(), ObjectKind::Revision)
        else {
            continue;
        };
        let Ok(decoded) = Revision::decode(&object.payload) else {
            problems.push(format!("an unreadable revision object: r{number}"));
            continue;
        };
        let parent: Option<ObjectHash> = parent_hash.map(|parent| parent.parse().unwrap());
        let tree: ObjectHash = tree.parse().unwrap();
        if decoded.tree != tree
            || decoded.parent != parent
            || i64::from(decoded.number.number()) != number
            || decoded.integration.map(|id| i64::from(id.number())) != integration
        {
            problems.push(format!(
                "a revision object that differs from its row: r{number}"
            ));
        }
    }
}

/// Done means a completion: an `integration` completion belongs to the task of a succeeded proposal integration, a succeeded proposal integration completes exactly one task, and a pull or a direct change completes none.
async fn completions(store: &Store, problems: &mut Vec<String>) {
    offenders(
        store,
        problems,
        "an integration completion without its succeeded proposal integration",
        "SELECT 'T-' || t.number FROM completions c JOIN tasks t ON t.id = c.task_id
         LEFT JOIN integrations i ON i.id = c.integration_id
         LEFT JOIN proposals p ON p.id = i.proposal_id
         WHERE c.kind = 'integration'
           AND (i.id IS NULL OR i.result <> 'succeeded' OR i.kind <> 'proposal' OR p.task_id IS NOT c.task_id)",
    )
    .await;
    offenders(
        store,
        problems,
        "a Done task without a completion",
        "SELECT 'T-' || number FROM tasks
         WHERE status = 'done' AND id NOT IN (SELECT task_id FROM completions)",
    )
    .await;
    offenders(
        store,
        problems,
        "a completion of a task that is not Done",
        "SELECT 'T-' || t.number FROM completions c JOIN tasks t ON t.id = c.task_id
         WHERE t.status <> 'done' AND t.archived_at IS NULL",
    )
    .await;
    offenders(
        store,
        problems,
        "a succeeded integration with the wrong number of completions",
        "SELECT 'i-' || i.number FROM integrations i
         WHERE i.result = 'succeeded'
           AND (SELECT COUNT(*) FROM completions c WHERE c.integration_id = i.id)
               <> CASE i.kind WHEN 'proposal' THEN 1 ELSE 0 END",
    )
    .await;
}

/// Every cached status and blocked flag equals a fresh calculation from the stored facts. Archived tasks keep the cache they had when they were archived.
async fn statuses(store: &Store, problems: &mut Vec<String>) {
    let numbers: Vec<i64> = sqlx::query_scalar("SELECT number FROM tasks ORDER BY number")
        .fetch_all(store.read())
        .await
        .unwrap();
    let ids: Vec<TaskId> = numbers
        .into_iter()
        .map(|number| TaskId::from_number(u32::try_from(number).unwrap()).unwrap())
        .collect();
    // The facts are read on the writer's connection; the transaction writes nothing.
    let loaded = store
        .write(async |tx| tasks::load_facts(tx.conn(), &ids).await)
        .await
        .unwrap();
    for task in loaded.into_iter().filter(|task| !task.archived) {
        let calculated = calculate(&task.facts);
        if (calculated.status, calculated.blocked()) != (task.status, task.blocked) {
            problems.push(format!(
                "a stale status cache: {} stores {} blocked={} but calculates {} blocked={}",
                task.task,
                task.status.as_str(),
                task.blocked,
                calculated.status.as_str(),
                calculated.blocked()
            ));
        }
    }
}

/// Every object a revision, changeset, unpruned checkpoint or proposal version names is in the store, down to the last blob of its tree.
async fn objects(store: &Store, paths: &ProjectPaths, problems: &mut Vec<String>) {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT 'r' || number, hash, tree_hash FROM revisions
         UNION ALL SELECT 'C-' || number, '', tree_hash FROM changesets
         UNION ALL SELECT 'ck-' || number, '', tree_hash FROM checkpoints WHERE pruned_at IS NULL
         UNION ALL SELECT 'P-' || p.number || ' v' || v.version, '', v.tree_hash
             FROM proposal_versions v JOIN proposals p ON p.id = v.proposal_id",
    )
    .fetch_all(store.read())
    .await
    .unwrap();
    let objects = ObjectStore::open(&paths.fold_dir()).unwrap();
    let mut checked = BTreeSet::new();
    for (owner, revision, tree) in rows {
        if !revision.is_empty() {
            let hash: ObjectHash = revision.parse().unwrap();
            if objects.read_expecting(&hash, ObjectKind::Revision).is_err() {
                problems.push(format!("a missing revision object: {owner}"));
            }
        }
        if !checked.insert(tree.clone()) {
            continue;
        }
        let root: ObjectHash = tree.parse().unwrap();
        let load = |hash: &ObjectHash| -> Result<Tree, FoldError> {
            let object = objects.read_expecting(hash, ObjectKind::Tree)?;
            Ok(Tree::decode(&object.payload)?)
        };
        match flatten(&root, &load) {
            Ok(flat) => {
                let missing = flat
                    .values()
                    .filter(|entry| entry.kind != EntryKind::Directory)
                    .any(|entry| !objects.contains(&entry.hash).unwrap());
                if missing {
                    problems.push(format!("a missing blob in the tree of {owner}"));
                }
            }
            Err(_) => problems.push(format!("a missing tree object: {owner}")),
        }
    }
}

/// Event sequence numbers have no gap, and no stored event describes a change the database does not hold.
async fn events(store: &Store, problems: &mut Vec<String>) {
    let seqs: Vec<i64> = sqlx::query_scalar("SELECT seq FROM events ORDER BY seq")
        .fetch_all(store.read())
        .await
        .unwrap();
    if let Some((index, seq)) = seqs
        .iter()
        .enumerate()
        .find(|(index, seq)| i64::try_from(*index).unwrap() + 1 != **seq)
    {
        problems.push(format!(
            "a gap in event sequence numbers before seq {seq} at position {}",
            index + 1
        ));
    }
    let rules: [(&str, Option<&str>, &str); 7] = [
        (
            "an event names a revision that does not exist",
            None,
            "SELECT e.seq FROM events e
             WHERE e.revision_id IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM revisions r WHERE r.number = e.revision_id)",
        ),
        (
            "an event names a workspace that does not exist",
            None,
            "SELECT e.seq FROM events e
             WHERE e.workspace_id IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM workspaces w WHERE w.number = e.workspace_id)",
        ),
        (
            types::INTEGRATION_SUCCEEDED.as_str(),
            Some(types::INTEGRATION_SUCCEEDED.as_str()),
            "SELECT e.seq FROM events e WHERE e.type = ?1
               AND NOT EXISTS (SELECT 1 FROM integrations i
                               WHERE 'i-' || i.number = json_extract(e.data, '$.integration')
                                 AND i.result = 'succeeded')",
        ),
        (
            types::PROPOSAL_INTEGRATED.as_str(),
            Some(types::PROPOSAL_INTEGRATED.as_str()),
            "SELECT e.seq FROM events e WHERE e.type = ?1
               AND NOT EXISTS (SELECT 1 FROM proposals p
                               WHERE p.number = e.proposal_id AND p.state = 'integrated')",
        ),
        (
            types::TASK_COMPLETED.as_str(),
            Some(types::TASK_COMPLETED.as_str()),
            "SELECT e.seq FROM events e WHERE e.type = ?1
               AND NOT EXISTS (SELECT 1 FROM completions c JOIN tasks t ON t.id = c.task_id
                               WHERE t.number = e.task_id)",
        ),
        (
            types::CHECKPOINT_RECORDED.as_str(),
            Some(types::CHECKPOINT_RECORDED.as_str()),
            "SELECT e.seq FROM events e WHERE e.type = ?1
               AND NOT EXISTS (SELECT 1 FROM checkpoints c
                               WHERE 'ck-' || c.number = json_extract(e.data, '$.checkpoint'))",
        ),
        (
            types::MESSAGE_SENT.as_str(),
            Some(types::MESSAGE_SENT.as_str()),
            "SELECT e.seq FROM events e WHERE e.type = ?1
               AND NOT EXISTS (SELECT 1 FROM messages m JOIN message_recipients r ON r.message_id = m.id
                               WHERE 'm-' || m.number = json_extract(e.data, '$.message'))",
        ),
    ];
    for (rule, event_type, sql) in rules {
        let query = sqlx::query_scalar(sql);
        let query = match event_type {
            Some(event_type) => query.bind(event_type),
            None => query,
        };
        let found: Vec<i64> = query.fetch_all(store.read()).await.unwrap();
        if !found.is_empty() {
            let seqs: Vec<String> = found.iter().map(i64::to_string).collect();
            problems.push(format!(
                "{rule} without its change: seq {}",
                seqs.join(", ")
            ));
        }
    }
}

/// The tables whose rows never change once written; a crash may only add whole rows to them.
const APPEND_ONLY: [&str; 9] = [
    "events",
    "revisions",
    "changesets",
    "proposal_versions",
    "reviews",
    "review_comments",
    "integrations",
    "completions",
    "messages",
];

/// Every append-only table still has the triggers that refuse any `UPDATE` and `DELETE` of its rows.
async fn append_only(store: &Store, problems: &mut Vec<String>) {
    let triggers: Vec<(String, String)> =
        sqlx::query_as("SELECT tbl_name, sql FROM sqlite_master WHERE type = 'trigger'")
            .fetch_all(store.read())
            .await
            .unwrap();
    for table in APPEND_ONLY {
        for statement in ["UPDATE", "DELETE"] {
            let refuses = triggers.iter().any(|(on, sql)| {
                on == table
                    && sql.contains(&format!("BEFORE {statement} ON {table}"))
                    && sql.contains("RAISE(ABORT")
            });
            if !refuses {
                problems.push(format!(
                    "an append-only table without its {statement} refusal: {table}"
                ));
            }
        }
    }
}
