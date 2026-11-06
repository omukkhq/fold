#![cfg(test)]

use fold_core::{AgentName, Timestamp};
use fold_store::agents::insert_agent;
use fold_store::workspaces::{
    AgentChangeEnd, clear_agent_change, insert_agent_change, open_agent_changes,
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
