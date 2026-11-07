#![cfg(test)]

use fold_core::{AgentName, Timestamp};
use fold_store::agents::insert_agent;
use fold_store::workspaces::{
    AgentChangeEnd, clear_agent_change, dismiss_shared_changes, insert_agent_change,
    note_shared_change, open_agent_changes, open_shared_changes,
};

use crate::common::{exec, open};

fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).unwrap()
}

#[tokio::test]
async fn a_path_waits_once_per_agent_and_a_settled_change_never_opens_again() {
    let (_dir, _paths, store) = open().await;
    let lead = AgentName::parse("web-lead").unwrap();
    let other = AgentName::parse("backend").unwrap();
    let ghost = AgentName::parse("ghost").unwrap();
    let added = store
        .write(async |tx| {
            insert_agent(tx, &lead, at(1)).await?;
            insert_agent(tx, &other, at(1)).await?;
            let mut added = Vec::new();
            for (path, agent, changed) in [
                (b"plan.md".as_slice(), &lead, 10),
                (b"plan.md".as_slice(), &lead, 11),
                (b"plan.md".as_slice(), &other, 10),
                (b"junk".as_slice(), &lead, 12),
                (b"junk".as_slice(), &ghost, 12),
            ] {
                added.push(insert_agent_change(tx, path, agent, at(changed), at(20)).await?);
            }
            Ok::<_, fold_store::StoreError>(added)
        })
        .await
        .unwrap();
    assert_eq!(added, [true, false, true, true, false]);
    let open_rows = open_agent_changes(store.read()).await.unwrap();
    let listed: Vec<(&[u8], &str)> = open_rows
        .iter()
        .map(|row| (row.path.as_slice(), row.agent.as_str()))
        .collect();
    assert_eq!(
        listed,
        [
            (b"junk".as_slice(), "web-lead"),
            (b"plan.md".as_slice(), "backend"),
            (b"plan.md".as_slice(), "web-lead"),
        ]
    );
    assert_eq!(open_rows[0].changed_at, at(12));
    assert_eq!(open_rows[0].seen_at, at(20));

    let (ended, again, later) = store
        .write(async |tx| {
            let ended = clear_agent_change(tx, b"plan.md", AgentChangeEnd::Ignored, at(30)).await?;
            let again = insert_agent_change(tx, b"plan.md", &lead, at(10), at(31)).await?;
            let later = insert_agent_change(tx, b"plan.md", &lead, at(40), at(41)).await?;
            Ok::<_, fold_store::StoreError>((ended, again, later))
        })
        .await
        .unwrap();
    assert_eq!(ended, 2);
    assert!(!again, "the change a human ignored stays settled");
    assert!(later, "a newer change of the same file waits again");
    assert!(
        exec(
            &store,
            "UPDATE agent_changes SET cleared_at = 50 WHERE cleared_at IS NULL"
        )
        .await
        .is_err(),
        "an ended change names how it ended"
    );
    assert!(
        exec(
            &store,
            "UPDATE agent_changes SET cleared_at = 50, cleared_how = 'lost' WHERE cleared_at IS NULL"
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn an_agent_has_one_open_shared_change_that_gains_each_new_path() {
    let (_dir, _paths, store) = open().await;
    let lead = AgentName::parse("web-lead").unwrap();
    let ghost = AgentName::parse("ghost").unwrap();
    let paths = |list: &[&str]| {
        list.iter()
            .map(|path| (*path).to_owned())
            .collect::<Vec<_>>()
    };
    let noted = store
        .write(async |tx| {
            insert_agent(tx, &lead, at(1)).await?;
            let mut noted = Vec::new();
            for (list, now) in [
                (&["design/b.pen"][..], 10),
                (&["design/a.pen", "design/b.pen"][..], 11),
                (&["design/b.pen"][..], 12),
            ] {
                noted.push(note_shared_change(tx, &lead, &paths(list), at(now)).await?);
            }
            noted.push(note_shared_change(tx, &ghost, &paths(&["x"]), at(13)).await?);
            Ok::<_, fold_store::StoreError>(noted)
        })
        .await
        .unwrap();
    assert_eq!(
        noted,
        [
            Some(paths(&["design/b.pen"])),
            Some(paths(&["design/a.pen"])),
            None,
            None
        ]
    );
    let open_rows = open_shared_changes(store.read()).await.unwrap();
    assert_eq!(open_rows.len(), 1);
    assert_eq!(open_rows[0].paths, paths(&["design/a.pen", "design/b.pen"]));
    assert_eq!(open_rows[0].created_at, at(10));
    assert!(
        exec(
            &store,
            "INSERT INTO shared_changes (agent_id, paths, created_at) SELECT id, '[]', 20 FROM agents WHERE name = 'web-lead'"
        )
        .await
        .is_err(),
        "one open change per agent"
    );
    let again = store
        .write(async |tx| {
            dismiss_shared_changes(tx, "human:ana", at(30)).await?;
            note_shared_change(tx, &lead, &paths(&["design/b.pen"]), at(31)).await
        })
        .await
        .unwrap();
    assert_eq!(
        again,
        Some(paths(&["design/b.pen"])),
        "a dismissed change never takes a new path"
    );
}
