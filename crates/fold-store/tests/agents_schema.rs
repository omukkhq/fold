#[cfg(test)]
mod common;

use common::{count, exec, open};

#[cfg(test)]
async fn agent(store: &fold_store::Store, id: u32, name: &str) {
    exec(
        store,
        &format!("INSERT INTO agents (id, name, created_at) VALUES ({id}, '{name}', 0)"),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn a_second_main_worker_violates_the_partial_unique_index() {
    let (_dir, _paths, store) = open().await;
    agent(&store, 1, "frontend").await;
    agent(&store, 2, "backend").await;
    exec(
        &store,
        "INSERT INTO workers (agent_id, name, created_at) VALUES (1, 'frontend', 0)",
    )
    .await
    .unwrap();
    let error = exec(
        &store,
        "INSERT INTO workers (agent_id, name, created_at) VALUES (1, 'other', 0)",
    )
    .await
    .unwrap_err();
    assert!(error.unique_violation());
    assert_eq!(error.constraint().as_deref(), Some("workers.agent_id"));
    exec(
        &store,
        "INSERT INTO workers (agent_id, name, created_at) VALUES (2, 'backend', 0)",
    )
    .await
    .unwrap();
    exec(
        &store,
        "INSERT INTO workers (agent_id, name, parent_worker_id, created_at) VALUES (1, 'frontend/ui', 1, 0)",
    )
    .await
    .unwrap();
    exec(
        &store,
        "INSERT INTO workers (agent_id, name, parent_worker_id, created_at) VALUES (1, 'frontend/api', 1, 0)",
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn a_worker_name_is_unique_per_agent() {
    let (_dir, _paths, store) = open().await;
    agent(&store, 1, "frontend").await;
    exec(
        &store,
        "INSERT INTO workers (agent_id, name, created_at) VALUES (1, 'frontend', 0)",
    )
    .await
    .unwrap();
    exec(
        &store,
        "INSERT INTO workers (agent_id, name, parent_worker_id, created_at) VALUES (1, 'frontend/ui', 1, 0)",
    )
    .await
    .unwrap();
    let error = exec(
        &store,
        "INSERT INTO workers (agent_id, name, parent_worker_id, created_at) VALUES (1, 'frontend/ui', 1, 0)",
    )
    .await
    .unwrap_err();
    assert!(error.unique_violation());
    assert_eq!(
        error.constraint().as_deref(),
        Some("workers.agent_id, workers.name")
    );
}

#[tokio::test]
async fn a_session_without_an_agent_is_accepted() {
    let (_dir, _paths, store) = open().await;
    exec(
        &store,
        "INSERT INTO sessions (pid, started_at) VALUES (4242, 0)",
    )
    .await
    .unwrap();
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM sessions WHERE agent_id IS NULL"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn agent_names_must_match_the_pattern() {
    let (_dir, _paths, store) = open().await;
    for good in ["a", "0", "front-end_2", &"a".repeat(32)] {
        agent(&store, 1, good).await;
        exec(&store, "DELETE FROM agents").await.unwrap();
    }
    for bad in ["", "-a", "_a", "Front", "a b", "a/b", &"a".repeat(33)] {
        let error = exec(
            &store,
            &format!("INSERT INTO agents (name, created_at) VALUES ('{bad}', 0)"),
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("agents_name_pattern"),
            "{bad}: {error}"
        );
    }
    agent(&store, 1, "ok").await;
    let error = exec(&store, "UPDATE agents SET name = 'Bad' WHERE id = 1")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("agents_name_pattern"));
}

#[tokio::test]
async fn a_child_worker_hangs_under_a_parent_of_its_own_agent() {
    let (_dir, _paths, store) = open().await;
    agent(&store, 1, "frontend").await;
    agent(&store, 2, "backend").await;
    for sql in [
        "INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'frontend', 0)",
        "INSERT INTO workers (id, agent_id, name, created_at) VALUES (2, 2, 'backend', 0)",
        "INSERT INTO workers (id, agent_id, name, parent_worker_id, created_at) VALUES (3, 1, 'frontend/ui', 1, 0)",
        "INSERT INTO workers (agent_id, name, parent_worker_id, created_at) VALUES (1, 'frontend/ui/btn', 3, 0)",
    ] {
        exec(&store, sql).await.unwrap();
    }
    for (agent_id, name, parent) in [
        (2, "backend/ui", 1),
        (1, "backend/ui", 1),
        (1, "frontend/a/b", 1),
        (1, "frontend/", 1),
        (1, "frontendx/a", 1),
        (1, "frontend/ui", 3),
        (1, "frontend/x", 99),
    ] {
        let error = exec(
            &store,
            &format!(
                "INSERT INTO workers (agent_id, name, parent_worker_id, created_at) VALUES ({agent_id}, '{name}', {parent}, 0)"
            ),
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("workers_child_of_parent"),
            "{name}: {error}"
        );
    }
    for sql in [
        "UPDATE workers SET name = 'frontend/web' WHERE id = 3",
        "UPDATE workers SET agent_id = 2 WHERE id = 3",
        "UPDATE workers SET parent_worker_id = 2 WHERE id = 3",
    ] {
        let error = exec(&store, sql).await.unwrap_err();
        assert!(
            error.to_string().contains("workers_identity_fixed"),
            "{sql}"
        );
    }
    exec(
        &store,
        "UPDATE workers SET activity = 'claim_task T-1' WHERE id = 3",
    )
    .await
    .unwrap();
    assert_eq!(count(&store, "SELECT COUNT(*) FROM workers").await, 4);
}
