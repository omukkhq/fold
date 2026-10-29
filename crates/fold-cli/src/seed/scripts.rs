use std::collections::BTreeMap;

use serde_json::{Value, json};

use fold_core::EventActor;

use super::HUMAN;
use super::stub::script::{Call, Script, Step, Turn};

/// The ids of the tasks the scripts work on.
pub struct Tasks {
    pub schema: String,
    pub routes: String,
    pub local_dates: String,
    pub relative_dates: String,
    pub counts: String,
    pub login: String,
    pub audit: String,
    pub limit: String,
}

pub const REVIEW_LEAD: &str = "review-lead";
pub const API_LEAD: &str = "api-lead";
pub const WEB_LEAD: &str = "web-lead";
pub const ROUTES_WORKER: &str = "api-routes";
pub const LIMIT_WORKER: &str = "api-limit";

/// The script of every agent the demo starts, by agent name. The leads wait for each other's work through the tools, as agents poll: the area leads start the work on the user routes and the review round once the schema is on main, so every proposal of the round has the same base, and the review lead integrates the local dates before it tries the relative dates, which changed the same line.
pub fn scripts(tasks: &Tasks) -> Vec<(&'static str, Script)> {
    vec![
        (REVIEW_LEAD, review_lead(tasks)),
        (API_LEAD, api_lead(tasks)),
        (WEB_LEAD, web_lead(tasks)),
        ("api-schema", schema_worker(tasks)),
        (ROUTES_WORKER, routes_worker(tasks)),
        ("api-audit", audit_worker(tasks)),
        (LIMIT_WORKER, limit_worker(tasks)),
        ("web-local-dates", local_worker(tasks)),
        ("web-relative-dates", relative_worker(tasks)),
        ("web-counts", counts_worker(tasks)),
        ("web-login", login_worker(tasks)),
        (
            "review-schema",
            reviewer(
                &tasks.schema,
                approve("The table and the helper look right."),
            ),
        ),
        (
            "review-local-dates",
            reviewer(&tasks.local_dates, approve("Reads well.")),
        ),
        (
            "review-relative-dates",
            reviewer(&tasks.relative_dates, approve("Reads well.")),
        ),
        ("review-counts", reviewer(&tasks.counts, counts_review())),
        ("review-login", reviewer(&tasks.login, login_review())),
        (
            "review-audit",
            reviewer(
                &tasks.audit,
                approve("Each write adds one entry, as the evidence says."),
            ),
        ),
    ]
}

fn one_turn(steps: Vec<Step>, reply: &str) -> Script {
    Script {
        turns: vec![Turn {
            when: None,
            steps,
            reply: Some(reply.to_owned()),
        }],
    }
}

fn arguments(value: Value) -> serde_json::Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    }
}

fn call(tool: &str, value: Value) -> Step {
    Step::Call(Call {
        tool: tool.to_owned(),
        arguments: arguments(value),
        ..Call::default()
    })
}

fn call_saving(tool: &str, value: Value, save: &[(&str, &str)]) -> Step {
    Step::Call(Call {
        tool: tool.to_owned(),
        arguments: arguments(value),
        save: save
            .iter()
            .map(|(name, pointer)| ((*name).to_owned(), (*pointer).to_owned()))
            .collect(),
        ..Call::default()
    })
}

/// Calls `tool` until its result has a value at `pointer`, then saves it as `name`.
fn poll(tool: &str, value: Value, pointer: &str, name: &str) -> Step {
    Step::Call(Call {
        tool: tool.to_owned(),
        arguments: arguments(value),
        save: BTreeMap::from([(name.to_owned(), pointer.to_owned())]),
        until: Some(pointer.to_owned()),
        ..Call::default()
    })
}

/// Waits until `task` has a proposal and saves its id as `name`.
fn proposal_of(task: &str, name: &str) -> Step {
    poll("get_task", json!({"id": task}), "/proposal/id", name)
}

/// Waits until `task` is integrated.
fn integrated(task: &str) -> Step {
    poll(
        "get_task",
        json!({"id": task}),
        "/integration",
        "integrated",
    )
}

/// Waits until the proposal `${name}` has a review.
fn reviewed(name: &str) -> Step {
    poll(
        "get_proposal",
        json!({"id": format!("${{{name}}}")}),
        "/reviews/0",
        "review",
    )
}

fn write(path: &str, text: &str) -> Step {
    Step::Write {
        path: format!("${{dir}}/{path}"),
        text: text.to_owned(),
    }
}

fn insert(path: &str, after: usize, text: &str) -> Step {
    Step::Insert {
        path: format!("${{dir}}/{path}"),
        after,
        text: text.to_owned(),
    }
}

fn replace(path: &str, line: usize, text: &str) -> Step {
    Step::Replace {
        path: format!("${{dir}}/{path}"),
        line,
        text: text.to_owned(),
    }
}

fn start(role: &str, name: &str, work: (&str, &str)) -> Step {
    let (key, id) = work;
    call(
        "start_worker",
        json!({"role": role, "name": name, (key): [id]}),
    )
}

fn task_worker(name: &str, task: &str) -> Step {
    start("task_worker", name, ("tasks", task))
}

fn review_by(name: &str, proposal: &str) -> Step {
    start("reviewer", name, ("proposals", &format!("${{{proposal}}}")))
}

fn integrate(proposal: &str) -> Step {
    call(
        "integrate_proposal",
        json!({"id": format!("${{{proposal}}}"), "expected_version": 1}),
    )
}

/// The review lead starts one reviewer per proposal and integrates what they approve. The schema goes first, because both area leads wait for it; the human review mark goes on the audit log before its reviewer starts.
fn review_lead(tasks: &Tasks) -> Script {
    let mut steps = vec![
        proposal_of(&tasks.schema, "schema"),
        review_by("review-schema", "schema"),
        reviewed("schema"),
        integrate("schema"),
    ];
    let round = [
        (&tasks.local_dates, "local", "review-local-dates"),
        (&tasks.relative_dates, "relative", "review-relative-dates"),
        (&tasks.counts, "counts", "review-counts"),
        (&tasks.login, "login", "review-login"),
        (&tasks.audit, "audit", "review-audit"),
    ];
    steps.extend(round.iter().map(|(task, name, _)| proposal_of(task, name)));
    steps.push(call(
        "require_human_review",
        json!({
            "task": tasks.audit,
            "reviewer": HUMAN,
            "reason": "The audit log records personal data.",
        }),
    ));
    steps.extend(
        round
            .iter()
            .map(|(_, proposal, reviewer)| review_by(reviewer, proposal)),
    );
    steps.extend([reviewed("local"), integrate("local"), reviewed("relative")]);
    // Both proposals changed the same line of one file, so the second gets its conflict report.
    steps.push(Step::Call(Call {
        tool: "integrate_proposal".to_owned(),
        arguments: arguments(json!({"id": "${relative}", "expected_version": 1})),
        refused: Some("integration_conflicted".to_owned()),
        ..Call::default()
    }));
    one_turn(
        steps,
        "Integrated the schema and the local dates; the relative dates conflict.",
    )
}

fn api_lead(tasks: &Tasks) -> Script {
    one_turn(
        vec![
            task_worker("api-schema", &tasks.schema),
            integrated(&tasks.schema),
            task_worker(ROUTES_WORKER, &tasks.routes),
            task_worker("api-audit", &tasks.audit),
            task_worker(LIMIT_WORKER, &tasks.limit),
            call(
                "send_message",
                json!({
                    "to": [WEB_LEAD],
                    "body": "The user routes are in progress; the form can post to /users.",
                    "task": tasks.routes,
                }),
            ),
        ],
        "The api tasks have their workers.",
    )
}

fn web_lead(tasks: &Tasks) -> Script {
    one_turn(
        vec![
            integrated(&tasks.schema),
            task_worker("web-local-dates", &tasks.local_dates),
            task_worker("web-relative-dates", &tasks.relative_dates),
            task_worker("web-counts", &tasks.counts),
            task_worker("web-login", &tasks.login),
            call(
                "send_message",
                json!({
                    "to": [API_LEAD],
                    "body": "Thanks. I will connect the login form once the first change lands.",
                    "task": tasks.login,
                }),
            ),
        ],
        "The web tasks have their workers.",
    )
}

/// The first steps of every task worker: the workspace of its task, whose id and folder later steps use as `${ws}` and `${dir}`.
fn workspace(task: &str) -> Step {
    call_saving(
        "create_workspace",
        json!({"task": task}),
        &[("ws", "/workspace"), ("dir", "/path")],
    )
}

fn propose(task: &str, title: &str, intent: &str, evidence: &str) -> [Step; 2] {
    [
        call("record_changes", json!({"ws": "${ws}", "message": title})),
        call_saving(
            "create_proposal",
            json!({"task": task, "title": title, "intent": intent, "evidence": evidence}),
            &[("proposal", "/id")],
        ),
    ]
}

fn proposing_worker(
    task: &str,
    edits: Vec<Step>,
    title: &str,
    intent: &str,
    evidence: &str,
) -> Script {
    let mut steps = vec![workspace(task)];
    steps.extend(edits);
    steps.extend(propose(task, title, intent, evidence));
    one_turn(steps, "Proposed ${proposal}.")
}

fn schema_worker(tasks: &Tasks) -> Script {
    proposing_worker(
        &tasks.schema,
        vec![insert("src/db.js", 6, "  tables.set(\"users\", []);\n")],
        "Create the users table",
        "Adds an empty users table when the service starts, so sign-up has somewhere to write. It does not change the other tables. Risk: none; the table starts empty.",
        "The service starts and the users table exists.",
    )
}

/// The routes task stays In Progress with recorded and unrecorded changes in its workspace.
fn routes_worker(tasks: &Tasks) -> Script {
    one_turn(
        vec![
            workspace(&tasks.routes),
            write(
                "src/users.js",
                "export function listUsers(db) {\n  return db.all(\"users\");\n}\n\nexport function createUser(db, user) {\n  return db.insert(\"users\", user);\n}\n",
            ),
            insert(
                "src/routes.js",
                4,
                "    \"GET /users\": async () => ({ status: 200, body: await db.all(\"users\") }),\n",
            ),
            Step::Remove {
                path: "${dir}/docs/changelog.md".to_owned(),
            },
            call(
                "record_checkpoint",
                json!({"ws": "${ws}", "label": "Handlers listed"}),
            ),
            insert(
                "src/users.js",
                usize::MAX,
                "\nexport function removeUser(db, id) {\n  return db.remove(\"users\", id);\n}\n",
            ),
        ],
        "The handlers are listed; removing a user is next.",
    )
}

fn audit_worker(tasks: &Tasks) -> Script {
    proposing_worker(
        &tasks.audit,
        vec![write(
            "src/audit.js",
            "export function record(log, who, what) {\n  log.push({ who, what });\n}\n",
        )],
        "Record changes in an audit log",
        "Adds record(), which appends who changed what to the audit log, so a change can be traced. It does not read or trim the log. Risk: the log grows without a limit.",
        "Each write adds one entry.",
    )
}

/// The limit task keeps its workspace without a proposal, and asks the human a question.
fn limit_worker(tasks: &Tasks) -> Script {
    one_turn(
        vec![
            workspace(&tasks.limit),
            insert(
                "src/routes.js",
                usize::MAX,
                "\nexport function limited(counts, client, max) {\n  return (counts.get(client) ?? 0) >= max;\n}\n",
            ),
            call(
                "send_message",
                json!({
                    "to": [format!("{}{HUMAN}", EventActor::HUMAN_PREFIX)],
                    "body": "Should the limit apply per client address or per token?",
                    "task": tasks.limit,
                    "asks_human": true,
                }),
            ),
        ],
        "Waiting for an answer about the limit.",
    )
}

fn local_worker(tasks: &Tasks) -> Script {
    proposing_worker(
        &tasks.local_dates,
        vec![replace(
            "src/format.js",
            4,
            "  return new Date(value).toLocaleDateString(\"en-GB\");",
        )],
        "Show dates in the local time zone",
        "Shows each date in the reader's local time zone instead of UTC, so a date matches the reader's day. It does not change how dates are stored. Risk: a date near midnight moves to the next or previous day.",
        "Checked in the list view with two time zones.",
    )
}

fn relative_worker(tasks: &Tasks) -> Script {
    proposing_worker(
        &tasks.relative_dates,
        vec![replace(
            "src/format.js",
            4,
            "  return relativeTime(Date.now() - new Date(value));",
        )],
        "Show dates as relative text",
        "Shows each date as relative text such as 3 hours ago, so recent items are easy to read. It does not change how dates are stored. Risk: an old date loses its exact day.",
        "Checked with items from the last day.",
    )
}

/// The counts proposal gets a second version for the change request its review makes, which fold hands to the worker as its next turn.
fn counts_worker(tasks: &Tasks) -> Script {
    let mut script = proposing_worker(
        &tasks.counts,
        vec![replace(
            "src/format.js",
            8,
            "  return count.toLocaleString(\"en-US\");",
        )],
        "Format counts with separators",
        "Formats counts with thousands separators, so large counts are easy to read. It does not change small counts. Risk: none.",
        "Checked with counts above one thousand.",
    );
    script.turns.push(Turn {
        when: Some("changes requested".to_owned()),
        steps: vec![
            insert(
                "src/format.js",
                6,
                "// Counts use a separator at every size.\n",
            ),
            call(
                "record_changes",
                json!({"ws": "${ws}", "message": "Use the separator at every size"}),
            ),
            call(
                "update_proposal",
                json!({"id": "${proposal}", "intent": "Formats counts with thousands separators at every size, so large counts are easy to read. It does not change how counts are stored. Risk: none.", "evidence": "Checked with counts from 1 to 10000000."}),
            ),
        ],
        reply: Some("Updated ${proposal}.".to_owned()),
    });
    script
}

/// The login form stays with its author after its review.
fn login_worker(tasks: &Tasks) -> Script {
    proposing_worker(
        &tasks.login,
        vec![replace(
            "src/app.js",
            6,
            "  const handler = routes(db);\n  const form = {\n    fields: [\"name\", \"password\"],\n    submit: (values) => db.insert(\"sessions\", values),\n  };\n  handler.login = form;",
        )],
        "Add the login form",
        "Adds a login form with name and password that starts a session, so a user can sign in. It does not check the password yet. Risk: any password signs in until the check exists.",
        "The form renders and submits.",
    )
}

/// A reviewer of the one proposal of `task`, which its review lead gave it.
fn reviewer(task: &str, review: Value) -> Script {
    let mut review = review;
    review["id"] = json!("${proposal}");
    review["version"] = json!(1);
    one_turn(
        vec![proposal_of(task, "proposal"), call("submit_review", review)],
        "Reviewed ${proposal}.",
    )
}

fn approve(body: &str) -> Value {
    json!({"verdict": "approve", "body": body})
}

fn counts_review() -> Value {
    json!({
        "verdict": "request_changes",
        "body": "Keep the separator for small counts too.",
        "comments": [{
            "path": "src/format.js",
            "start_line": 8,
            "end_line": 8,
            "side": "new",
            "body": "This is right for large counts. What about counts below one thousand?"
        }],
    })
}

fn login_review() -> Value {
    json!({
        "verdict": "request_changes",
        "body": "The form needs a failure path and a clearer name for the handler.",
        "comments": [
            {
                "path": "src/app.js",
                "start_line": 6,
                "end_line": 10,
                "side": "new",
                "body": "The submit function writes the password into the session row. Store a hash instead."
            },
            {
                "path": "src/app.js",
                "start_line": 12,
                "end_line": 12,
                "side": "new",
                "body": "Name the server after what it serves."
            },
            {
                "path": "src/app.js",
                "start_line": 7,
                "end_line": 7,
                "side": "old",
                "body": "Keep creating the server after the handler is built."
            }
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tasks() -> Tasks {
        let id = |n: u32| format!("T-{n}");
        Tasks {
            schema: id(1),
            routes: id(2),
            local_dates: id(3),
            relative_dates: id(4),
            counts: id(5),
            login: id(6),
            audit: id(7),
            limit: id(8),
        }
    }

    /// Every worker a script starts has a script of its own, and every agent with a script is started by one, except the leads, which the main agent starts.
    #[test]
    fn every_started_worker_has_a_script() {
        let scripts = scripts(&tasks());
        let named: Vec<&str> = scripts.iter().map(|(name, _)| *name).collect();
        let mut started = Vec::new();
        for (_, script) in &scripts {
            for turn in &script.turns {
                for step in &turn.steps {
                    if let Step::Call(call) = step
                        && call.tool == "start_worker"
                    {
                        started.push(call.arguments["name"].as_str().unwrap().to_owned());
                    }
                }
            }
        }
        for name in &started {
            assert!(named.contains(&name.as_str()), "{name} has no script");
        }
        for name in named {
            let lead = [REVIEW_LEAD, API_LEAD, WEB_LEAD].contains(&name);
            assert_eq!(
                started.iter().any(|started| started == name),
                !lead,
                "{name}"
            );
        }
        let text = serde_json::to_string(&scripts[0].1).unwrap();
        assert_eq!(serde_json::from_str::<Script>(&text).unwrap(), scripts[0].1);
    }
}
