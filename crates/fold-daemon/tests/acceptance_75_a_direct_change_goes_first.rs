#![cfg(test)]

use std::path::{Path, PathBuf};

use fold_core::Actor;
use fold_daemon::services::reviews::DirectChange;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::auto_integrate::Running;
use crate::common::acceptance::{Harness, WAIT};

const MAIN: &str = "main";

/// `agent` builds a task titled `title` that writes `path` and sends it for review; returns the proposal.
async fn proposed(h: &Harness, agent: &str, title: &str, path: &str) -> String {
    let task = h.create_task(agent, title).await;
    let work = h.start_work(agent, &task).await;
    work.write(&[(path, "content\n")]);
    h.propose(agent, &work, title).await
}

async fn approved(h: &Harness, id: &str) {
    h.give_review("review", id).await;
    h.approve(id).await;
}

/// The main agent writes `files` in its main workspace, records them and submits the changeset; returns the result.
async fn direct_change(h: &Harness, files: &[(&str, &str)], message: &str) -> Value {
    let opened = h.call(MAIN, "open_main_workspace", json!({})).await;
    let dir = Path::new(h.root()).join(opened["path"].as_str().unwrap());
    for (path, text) in files {
        let at = dir.join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, text).unwrap();
    }
    let recorded = h
        .call(
            MAIN,
            "record_changes",
            json!({"ws": opened["workspace"], "message": message}),
        )
        .await;
    h.call(
        MAIN,
        "submit_main_change",
        json!({"changeset": recorded["changeset"], "message": message}),
    )
    .await
}

/// The subjects of the successful integrations in the order main took them: a proposal id, or the changeset of a direct change.
async fn integrated(h: &Harness) -> Vec<String> {
    h.events()
        .await
        .into_iter()
        .filter(|event| event.kind == "integration.succeeded")
        .map(|event| {
            event
                .proposal
                .unwrap_or_else(|| event.data["changeset"].as_str().unwrap().to_owned())
        })
        .collect()
}

/// The queue the scenarios meet: main's integration check waits while the file `hold` exists; proposal A integrates through the automatic job and holds the integration mutex in that check, proposal B is queued by hand with `integrate_proposal`, and proposal C, approved last but numbered before B, waits for the automatic integration.
struct Queue {
    _gate: tempfile::TempDir,
    hold: PathBuf,
    a: String,
    b: String,
    c: String,
    integrate: Running,
}

async fn queue(h: &Harness) -> Queue {
    let gate = tempfile::tempdir().unwrap();
    let hold = gate.path().join("hold");
    let checks = format!(
        "[[check]]\nname = \"e2e\"\nrun = \"while [ -e '{}' ]; do sleep 0.05; done\"\nstage = \"integration\"\n",
        hold.display()
    );
    direct_change(h, &[("fold-checks.toml", &checks)], "Add the e2e check").await;
    let a = proposed(h, "backend", "Add the login endpoint", "src/a.txt").await;
    let c = proposed(h, "frontend", "Add the page footer", "src/c.txt").await;
    let b = proposed(h, "backend", "Add the logout endpoint", "src/b.txt").await;
    std::fs::write(&hold, "").unwrap();
    let integrate = Running::start(&h.state);
    approved(h, &a).await;
    eventually(WAIT, async || {
        let started =
            h.events().await.into_iter().any(|event| {
                event.kind == "checks.started" && event.proposal.as_deref() == Some(&a)
            });
        if started {
            Ok(())
        } else {
            Err("A's integration run has not started")
        }
    })
    .await;
    approved(h, &b).await;
    let queued = h
        .call(
            "review",
            "integrate_proposal",
            json!({"id": b, "expected_version": 1}),
        )
        .await;
    assert_eq!(queued["result"], "queued", "{queued}");
    approved(h, &c).await;
    Queue {
        _gate: gate,
        hold,
        a,
        b,
        c,
        integrate,
    }
}

/// Waits until `count` direct changes wait for their turn.
async fn waiting_first(h: &Harness, count: usize) {
    eventually(WAIT, async || match h.handle.integration.waiting_first() {
        now if now == count => Ok(()),
        other => Err(other),
    })
    .await;
}

/// Waits until main took `count` integrations and returns their order.
async fn integrated_count(h: &Harness, count: usize) -> Vec<String> {
    eventually(WAIT, async || {
        let order = integrated(h).await;
        if order.len() >= count {
            Ok(order)
        } else {
            Err(order)
        }
    })
    .await
}

/// Acceptance 75: a direct change takes first place in the integration queue. While proposal A's integration check holds the integration mutex, proposal B waits queued by hand and proposal C waits for the automatic integration, the main agent submits a direct change. When A ends, the direct change integrates next, before B and C, which then follow in their usual order: B, queued first, then C.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_75_a_direct_change_integrates_before_the_waiting_integrations() {
    let mut h = Harness::start().await;
    h.connect_tagged(MAIN, &["main"]).await;
    let queue = queue(&h).await;
    let submit = direct_change(&h, &[("docs/notes.md", "notes\n")], "Add the notes");
    let release = async {
        waiting_first(&h, 1).await;
        assert_eq!(
            integrated(&h).await.len(),
            1,
            "only the checks file is on main yet"
        );
        std::fs::remove_file(&queue.hold).unwrap();
    };
    let (submitted, ()) = tokio::join!(submit, release);
    assert_eq!(submitted["result"], "succeeded", "{submitted}");
    let direct = submitted["changeset"].as_str().unwrap().to_owned();
    let order = integrated_count(&h, 5).await;
    assert_eq!(
        &order[1..],
        [
            queue.a.as_str(),
            direct.as_str(),
            queue.b.as_str(),
            queue.c.as_str()
        ]
    );
    queue.integrate.stop().await;
}

/// Acceptance 75: a direct change whose call ends while it waits for its turn gives up its place, so the queue goes on as if it never came: B and then C integrate after A.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_75_a_cancelled_direct_change_does_not_block_the_queue() {
    let mut h = Harness::start().await;
    h.connect_tagged(MAIN, &["main"]).await;
    let queue = queue(&h).await;
    let reviews = h.handle.reviews(&h.state.registry);
    let change = DirectChange {
        changeset: "C-999".parse().unwrap(),
        message: "Never integrates".to_owned(),
    };
    tokio::select! {
        ended = reviews.integrate_direct(&Actor::System, change) => {
            panic!("the direct change took a turn while A held the mutex: {ended:?}")
        }
        () = waiting_first(&h, 1) => {}
    }
    assert_eq!(h.handle.integration.waiting_first(), 0);
    std::fs::remove_file(&queue.hold).unwrap();
    let order = integrated_count(&h, 4).await;
    assert_eq!(
        &order[1..],
        [queue.a.as_str(), queue.b.as_str(), queue.c.as_str()]
    );
    queue.integrate.stop().await;
}
