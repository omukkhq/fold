#![cfg(test)]

use fold_core::{AgentName, WorkerState};
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, WAIT, Work};
use crate::workers_support::FakeClaude;

/// About the size of the evidence, review bodies and task descriptions that agents write.
fn prose(topic: &str, chars: usize) -> String {
    let sentence = format!("The {topic} keeps its old behaviour and the tests cover each case. ");
    sentence.repeat(chars / sentence.len() + 1)[..chars].to_owned()
}

fn size(value: &Value) -> usize {
    value.to_string().len()
}

/// A source file of `lines` lines, with line `changed` (from 1) given another text.
fn source(lines: usize, changed: Option<usize>) -> String {
    (1..=lines)
        .map(|line| match changed {
            Some(at) if at == line => format!("    let value_{line} = compute({line}) + 1;\n"),
            _ => format!("    let value_{line} = compute({line});\n"),
        })
        .collect()
}

/// `backend` builds `task` in its own workspace and sends `files` for review; the review agent approves and integrates it, so the task is Done.
async fn integrate(harness: &Harness, task: &str, files: &[(&str, &str)]) {
    let work = harness.start_work("backend", task).await;
    work.write(files);
    let proposal = harness.propose("backend", &work, task).await;
    harness.give_review("review", &proposal).await;
    harness.approve(&proposal).await;
    let integrated = harness
        .call("review", "integrate_proposal", json!({"id": proposal}))
        .await;
    assert_eq!(integrated["result"], "succeeded", "{integrated}");
}

/// `review` asks for changes on the current version of `proposal`, and `backend` sends the next version of `work`.
async fn revise(harness: &Harness, proposal: &str, work: &Work, file: &str, round: usize) {
    let version = harness.version_of("review", proposal).await;
    harness
        .call(
            "review",
            "submit_review",
            json!({"id": proposal, "version": version, "verdict": "request_changes", "body": prose("change request", 900)}),
        )
        .await;
    work.write(&[("src/report.rs", file)]);
    harness
        .call(
            "backend",
            "record_changes",
            json!({"ws": work.workspace, "message": format!("round {round}")}),
        )
        .await;
    harness
        .call(
            "backend",
            "update_proposal",
            json!({"id": proposal, "evidence": prose("revised evidence", 1500)}),
        )
        .await;
}

/// The numbers of the lines of one compact hunk, counted from its header as a reviewer counts them: `(mark, old, new, text)` for each line.
fn numbered(hunk: &Value) -> Vec<(char, Option<u32>, Option<u32>, String)> {
    let header = hunk["header"].as_str().unwrap();
    let numbers: Vec<u32> = header
        .trim_start_matches("@@ -")
        .trim_end_matches(" @@")
        .split([',', ' ', '+'])
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().unwrap())
        .collect();
    let (mut old, mut new) = (numbers[0].max(1), numbers[2].max(1));
    let mut lines = Vec::new();
    for line in hunk["lines"].as_array().unwrap() {
        let line = line.as_str().unwrap();
        let mark = line.chars().next().unwrap();
        let text = line[1..].to_owned();
        let numbers = match mark {
            ' ' => (Some(old), Some(new)),
            '-' => (Some(old), None),
            '+' => (None, Some(new)),
            _ => continue,
        };
        old += u32::from(numbers.0.is_some());
        new += u32::from(numbers.1.is_some());
        lines.push((mark, numbers.0, numbers.1, text));
    }
    lines
}

/// The size of the same hunks in the shape of one object per line, as the tool gave them before.
fn objects_size(hunks: &Value) -> usize {
    let hunks: Vec<Value> = hunks
        .as_array()
        .unwrap()
        .iter()
        .map(|hunk| {
            let lines: Vec<Value> = numbered(hunk)
                .into_iter()
                .map(|(mark, old, new, text)| {
                    let kind = match mark {
                        '+' => "added",
                        '-' => "removed",
                        _ => "context",
                    };
                    json!({"kind": kind, "old_no": old, "new_no": new, "text": text})
                })
                .collect();
            json!({"old_start": 0, "old_lines": 0, "new_start": 0, "new_lines": 0, "lines": lines})
        })
        .collect();
    size(&json!(hunks))
}

#[tokio::test]
async fn the_default_reads_of_a_reviewed_proposal_are_smaller_and_keep_what_an_agent_acts_on() {
    let harness = Harness::start().await;
    let labels = Harness::work_labels_of("backend");
    for title in ["Schema", "Storage"] {
        harness
            .call(
                "backend",
                "create_task",
                json!({"title": title, "labels": labels, "description": prose(title, 600)}),
            )
            .await;
    }
    integrate(
        &harness,
        "T-1",
        &[("src/schema.rs", "pub struct Schema;\n")],
    )
    .await;
    integrate(
        &harness,
        "T-2",
        &[("src/storage.rs", "pub struct Storage;\n")],
    )
    .await;
    let report = harness
        .call(
            "backend",
            "create_task",
            json!({"title": "Report", "labels": labels, "needs": ["T-1", "T-2"], "description": prose("report task", 1500)}),
        )
        .await;
    assert_eq!(report["id"], "T-3");
    for number in 0..4 {
        harness
            .call(
                "backend",
                "create_task",
                json!({"title": format!("Follow-up {number}"), "labels": labels, "needs": ["T-3"], "description": prose("follow-up", 400)}),
            )
            .await;
    }

    let work = harness.start_work("backend", "T-3").await;
    work.write(&[("src/report.rs", &source(80, None))]);
    harness
        .call(
            "backend",
            "record_changes",
            json!({"ws": work.workspace, "message": "report"}),
        )
        .await;
    harness
        .call(
            "backend",
            "create_proposal",
            json!({"task": "T-3", "title": "Report", "evidence": prose("evidence", 1500)}),
        )
        .await;
    harness.give_review("review", "P-3").await;
    revise(&harness, "P-3", &work, &source(80, Some(40)), 1).await;
    revise(&harness, "P-3", &work, &source(80, Some(41)), 2).await;
    harness
        .call(
            "review",
            "submit_review",
            json!({"id": "P-3", "version": 3, "verdict": "comment", "body": prose("current review", 300)}),
        )
        .await;

    let short = harness
        .call("review", "get_proposal", json!({"id": "P-3"}))
        .await;
    let full = harness
        .call(
            "review",
            "get_proposal",
            json!({"id": "P-3", "detail": "full"}),
        )
        .await;
    eprintln!(
        "get_proposal: {} bytes by default, {} with detail full",
        size(&short),
        size(&full)
    );
    assert!(size(&short) * 2 < size(&full), "{short}");
    for field in [
        "id",
        "state",
        "turn",
        "version",
        "current_version",
        "versions",
        "base",
        "changeset",
        "needs",
        "claim",
        "comments",
        "last_reviewed",
        "review_scope",
        "gates",
        "human_review",
        "checks",
        "files",
        "totals",
    ] {
        assert_eq!(short[field], full[field], "{field}");
    }
    assert_eq!(short["last_reviewed"], "v2");
    assert_eq!(short.get("evidence"), None);
    assert_eq!(short["evidence_chars"], 1500);
    assert_eq!(full["evidence"], prose("revised evidence", 1500));
    assert_eq!(
        short["task"],
        json!({"id": "T-3", "title": "Report", "status": "review"})
    );
    assert_eq!(full["task"]["description"], prose("report task", 1500));
    let reviews = short["reviews"].as_array().unwrap();
    assert_eq!(reviews.len(), 3);
    for (index, review) in reviews.iter().enumerate() {
        let whole = &full["reviews"][index];
        for field in ["version", "verdict", "reviewer", "comments", "scope"] {
            assert_eq!(review.get(field), whole.get(field), "{field}");
        }
        if review["version"] == "v3" {
            assert_eq!(review, whole, "the selected version's review stays whole");
        } else {
            assert_eq!(review["body_chars"], 900);
            assert_eq!(review["body"].as_str().unwrap().chars().count(), 200);
            assert!(review["body"].as_str().unwrap().ends_with('…'));
            assert_eq!(whole["body"], prose("change request", 900));
        }
    }

    let task = harness
        .call("backend", "get_task", json!({"id": "T-3"}))
        .await;
    let all = harness
        .call(
            "backend",
            "get_task",
            json!({"id": "T-3", "history": "all"}),
        )
        .await;
    eprintln!(
        "get_task: {} bytes by default, {} with history all",
        size(&task),
        size(&all)
    );
    assert!(size(&task) < size(&all));
    assert_eq!(task["description"], prose("report task", 1500));
    let history = task["history"].as_array().unwrap();
    let count = all["history"].as_array().unwrap().len();
    assert_eq!(history.len(), 5);
    assert!(count > 5, "{all}");
    assert_eq!(task["history_count"], count);
    assert_eq!(all["history_count"], count);
    assert_eq!(history[..], all["history"].as_array().unwrap()[..5]);
    let error = harness
        .refused(
            "backend",
            "get_task",
            json!({"id": "T-3", "history": "some"}),
        )
        .await;
    assert_eq!(error["code"], "invalid_request");

    let graph = harness.call("backend", "get_task_graph", json!({})).await;
    let whole = harness
        .call("backend", "get_task_graph", json!({"include_done": true}))
        .await;
    eprintln!(
        "get_task_graph: {} bytes by default, {} with include_done",
        size(&graph),
        size(&whole)
    );
    let ids = |graph: &Value| -> Vec<String> {
        graph["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|task| task["id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(ids(&graph), ["T-3", "T-4", "T-5", "T-6", "T-7"]);
    assert_eq!(
        ids(&whole),
        ["T-1", "T-2", "T-3", "T-4", "T-5", "T-6", "T-7"]
    );
    assert_eq!(graph["done_count"], 2);
    assert_eq!(whole["done_count"], 2);
    assert_eq!(graph["tasks"][0]["needs"], json!(["T-1", "T-2"]));
    assert_eq!(graph["tasks"][0], whole["tasks"][2]);

    let diff = harness
        .call(
            "review",
            "get_proposal_diff",
            json!({"id": "P-3", "path": "src/report.rs"}),
        )
        .await;
    let since = harness
        .call(
            "review",
            "get_proposal_diff",
            json!({"id": "P-3", "since": 2, "path": "src/report.rs"}),
        )
        .await;
    eprintln!(
        "get_proposal_diff with a path: {} bytes of hunks against {} as one object per line; since v2 {} against {}",
        size(&diff["hunks"]),
        objects_size(&diff["hunks"]),
        size(&since["hunks"]),
        objects_size(&since["hunks"])
    );
    assert!(size(&diff["hunks"]) * 2 < objects_size(&diff["hunks"]));
    let lines = numbered(&diff["hunks"][0]);
    assert_eq!(lines.len(), 80, "the whole new file");
    assert!(lines.iter().all(|line| line.0 == '+'));
    assert_eq!(lines[40].2, Some(41));
    assert_eq!(lines[40].3, "    let value_41 = compute(41) + 1;\n");

    // A reviewer places a comment on the added line of the since change by counting from the hunk header.
    let since_lines = numbered(&since["hunks"][0]);
    let line_41 = |mark: char| {
        since_lines
            .iter()
            .find(|line| line.0 == mark && line.3.contains("value_41 "))
            .unwrap()
    };
    let (added, removed) = (line_41('+'), line_41('-'));
    assert_eq!(added.3, "    let value_41 = compute(41) + 1;\n");
    assert_eq!((added.1, added.2), (None, Some(41)));
    assert_eq!(removed.3, "    let value_41 = compute(41);\n");
    assert_eq!((removed.1, removed.2), (Some(41), None));
    harness
        .call(
            "review",
            "submit_review",
            json!({"id": "P-3", "version": 3, "verdict": "comment", "body": "", "comments": [
                {"path": "src/report.rs", "start_line": added.2, "end_line": added.2, "side": "new", "body": "name it"},
            ]}),
        )
        .await;
    let comments = harness
        .call("review", "get_review_comments", json!({"id": "P-3"}))
        .await;
    let placed = comments["comments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|comment| comment["body"] == "name it")
        .unwrap();
    assert_eq!(
        (&placed["side"], &placed["start_line"], &placed["outdated"]),
        (&json!("new"), &json!(41), &json!(false))
    );
    let checkout = harness
        .call("review", "create_review_checkout", json!({"id": "P-3"}))
        .await;
    let file = std::path::Path::new(harness.root())
        .join(checkout["path"].as_str().unwrap())
        .join("src/report.rs");
    let text = std::fs::read_to_string(file).unwrap();
    assert_eq!(
        text.lines().nth(40),
        Some("    let value_41 = compute(41) + 1;"),
        "the new number is the line of the version's file"
    );
}

const MAIN: &str = "frontend";

async fn until_state(harness: &Harness, agent: &str, state: WorkerState) {
    let name = AgentName::parse(agent).unwrap();
    eventually(WAIT, async || {
        let row = harness.handle.agents().get_worker(&name).await.unwrap();
        if row.state == state {
            Ok(())
        } else {
            Err(row.state)
        }
    })
    .await;
}

#[tokio::test]
async fn list_workers_leaves_out_stopped_workers_and_area_instructions_unless_asked() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    for (area, label) in [("web", "ui"), ("api", "server")] {
        harness
            .call(
                MAIN,
                "create_label",
                json!({"name": label, "color": "sage"}),
            )
            .await;
        harness
            .call(
                MAIN,
                "define_area",
                json!({"name": area, "label": label, "instructions": prose(area, 2400)}),
            )
            .await;
    }
    for number in 1..=4 {
        harness
            .call(
                MAIN,
                "create_task",
                json!({"title": format!("Page {number}"), "labels": ["ui"]}),
            )
            .await;
    }
    harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    harness.connect("web-lead").await;
    let mut workers = Vec::new();
    for number in 1..=4 {
        let started = harness
            .call(
                "web-lead",
                "start_worker",
                json!({"role": "task_worker", "tasks": [format!("T-{number}")]}),
            )
            .await;
        workers.push(started["worker"]["name"].as_str().unwrap().to_owned());
    }
    for worker in &workers {
        until_state(&harness, worker, WorkerState::Idle).await;
    }
    for worker in &workers[1..] {
        harness
            .call("web-lead", "stop_worker", json!({"name": worker}))
            .await;
        until_state(&harness, worker, WorkerState::Stopped).await;
    }

    let short = harness.call(MAIN, "list_workers", json!({})).await;
    let full = harness
        .call(
            MAIN,
            "list_workers",
            json!({"include_stopped": true, "include_instructions": true}),
        )
        .await;
    eprintln!(
        "list_workers: {} bytes by default, {} with include_stopped and include_instructions",
        size(&short),
        size(&full)
    );
    assert!(size(&short) * 2 < size(&full), "{short}");
    let names = |listed: &Value| -> Vec<String> {
        listed["workers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|worker| worker["name"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(names(&short), ["web-lead", workers[0].as_str()]);
    let mut every = vec!["web-lead".to_owned()];
    every.extend(workers.iter().cloned());
    assert_eq!(names(&full), every);
    assert_eq!(short["stopped_count"], 3);
    assert_eq!(full["stopped_count"], 3);
    assert_eq!(short["workers"][1], full["workers"][1]);
    for (index, area) in short["areas"].as_array().unwrap().iter().enumerate() {
        let whole = &full["areas"][index];
        assert_eq!(area.get("instructions"), None);
        assert_eq!(area["instructions_chars"], 2400);
        assert_eq!(whole["instructions"].as_str().unwrap().len(), 2400);
        for field in ["name", "label", "lead", "model", "mcp"] {
            assert_eq!(area[field], whole[field], "{field}");
        }
    }
    assert_eq!(short["limits"], full["limits"]);
    assert_eq!(short["pause"], full["pause"]);
    let stopped = harness
        .call(MAIN, "list_workers", json!({"include_stopped": true}))
        .await;
    assert_eq!(names(&stopped), every);
    assert_eq!(stopped["areas"][0].get("instructions"), None);
}
