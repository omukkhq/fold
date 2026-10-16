use std::path::Path;
use std::time::Instant;

use serde_json::json;

use super::fixture::{Sizes, append_line, path_of, prepend_line};
use super::session::Session;
use super::{BenchResult, fail};

/// The three `fld mcp` sessions of the integration target. `review` is the main agent, so it approves and integrates.
pub struct Agents<'a> {
    pub frontend: &'a Session,
    pub backend: &'a Session,
    pub review: &'a Session,
}

/// Times the `integrate_proposal` tool call for a proposal whose base is one revision behind main.
///
/// Per sample, two task workspaces start from the same head and change the same files at different ends (a line at the top, a line at the bottom). The first proposal integrates on the fast path and is not timed; the second needs the merge.
pub async fn measure(
    agents: &Agents<'_>,
    root: &Path,
    sizes: Sizes,
    samples: usize,
) -> BenchResult<Vec<f64>> {
    let mut timings = Vec::new();
    for sample in 0..samples {
        let first_task = agents
            .frontend
            .create_task(&format!("bench integration {sample} a"))
            .await?;
        let second_task = agents
            .backend
            .create_task(&format!("bench integration {sample} b"))
            .await?;
        let (_, first_path) = agents.frontend.create_workspace(&first_task).await?;
        let (_, second_path) = agents.backend.create_workspace(&second_task).await?;
        let first_directory = root.join(first_path);
        let second_directory = root.join(second_path);
        for index in 0..sizes.proposal_files {
            let file = path_of(index, sizes.directories);
            prepend_line(&first_directory.join(&file), "first proposal")?;
            append_line(&second_directory.join(&file), "second proposal")?;
        }
        let first = propose(agents.frontend, &first_task, "first").await?;
        let second = propose(agents.backend, &second_task, "second").await?;
        for proposal in [&first, &second] {
            agents
                .review
                .call(
                    "submit_review",
                    json!({ "id": proposal, "version": 1, "verdict": "approve", "body": "approved" }),
                )
                .await?;
        }
        integrate(agents.review, &first).await?;
        let start = Instant::now();
        integrate(agents.review, &second).await?;
        timings.push(start.elapsed().as_secs_f64());
        for directory in [first_directory, second_directory] {
            std::fs::remove_dir_all(directory)?;
        }
    }
    Ok(timings)
}

/// Creates the proposal of `task` and returns its public id, such as `P-3`.
async fn propose(session: &Session, task: &str, title: &str) -> BenchResult<String> {
    let created = session
        .call("create_proposal", json!({ "task": task, "title": title }))
        .await?;
    match created["id"].as_str() {
        Some(id) => Ok(id.to_owned()),
        None => fail("create_proposal returned no id"),
    }
}

async fn integrate(session: &Session, proposal: &str) -> BenchResult<()> {
    let outcome = session
        .call("integrate_proposal", json!({ "id": proposal }))
        .await?;
    if outcome["result"] != "succeeded" {
        return fail(format!("{proposal} did not integrate: {outcome}"));
    }
    Ok(())
}
