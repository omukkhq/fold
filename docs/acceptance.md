# Acceptance procedures

Each acceptance scenario has an automated test in `crates/fold-daemon/tests/acceptance_<n>_*.rs` and a manual procedure here that runs the same scenario with real Claude Code sessions. Run the manual procedure before a release and after any change to the MCP tools, the hooks or the generated Claude Code files. Fill in the result line of each section for every run.

`make acceptance` runs every automated acceptance test. To run one scenario, select its module, for example `cargo nextest run -p fold-daemon --test integration -E 'test(/^acceptance_3_parallel_workspaces::/)'`.

## Setup

Every procedure starts from this setup. You need the `fold` binary on `PATH` and a Claude Code version that sets `CLAUDE_CODE_MESSAGING_SOCKET` and `CLAUDE_CODE_MESSAGING_TOKEN` for a session started with `--name`.

1. Run `fold init ~/fold-acceptance`. **Expected:** the command prints the new project, and `~/fold-acceptance` holds `.fold/`, `main/`, `.mcp.json`, `.claude/settings.json`, `AGENTS.md` and `.claude/skills/fold/SKILL.md`.
2. In a fourth terminal, run `fold serve` and leave it running. If a fold daemon already runs, for example as a user service, stop it first. **Expected:** the daemon logs that it started; `http://127.0.0.1:7420` opens the web interface with the project `fold-acceptance`.
3. In three terminals, `cd ~/fold-acceptance` and start one session in each: `FOLD_AGENT=frontend claude --name frontend`, `FOLD_AGENT=backend claude --name backend` and `FOLD_AGENT=review claude --name review`. Approve the `fold` MCP server when Claude Code asks. **Expected:** each session starts with the fold session context, which names its agent and main worker, and `/mcp` lists the `fold` server as connected.
4. In the project root, run `fold agent tag review integrator`. It acts as the human named by `FOLD_HUMAN`, or `USER` when that is not set. **Expected:** the command prints the tags of `review`, which include `integrator`. The tag needs the agent to exist, so it comes after `review` has connected once.

## Acceptance 1: three sessions see one project

1. In each of the three sessions, ask: "Call register_agent with your agent name." **Expected:** each result names the session's agent and main worker with `created` false, because the adapter already registered the agent from `FOLD_AGENT`.
2. In each of the three sessions, ask: "Call list_agents and show the result." Do this within two minutes of step 1: an agent is active for two minutes after its last call. **Expected:** every session lists `frontend`, `backend` and `review` in that order, each with presence `active`, and `review` with the tag `integrator`.
3. In `frontend`, ask: "Create the task 'Add the login endpoint', then the task 'Build the login screen', then make the login screen need the login endpoint." **Expected:** `create_task` returns `T-1` and `T-2`, and `add_dependency` succeeds.
4. In each of the three sessions, ask: "Call get_task_graph and show the result." **Expected:** every session shows the same two tasks: `T-1` with no needs and `T-2` needing `T-1`. Only the footer line that starts with `fold:` may differ between sessions.
5. In each of the three sessions, ask: "Call get_repository_state and show the result." **Expected:** every session shows head `r1`, the same `main_path` and the same list of recent revisions.
6. Open the board of the project in the web interface. **Expected:** it shows the cards `T-1` and `T-2` with the titles from step 3.

Result: date, fold version, Claude Code version, pass or fail, notes.

## Acceptance 2: messaging

### Native messaging

Claude Code sessions can talk without fold. fold stores nothing for these messages, so they never appear in a fold inbox.

1. In `frontend`, ask: "Use SendMessage to send the session named backend: 'Native hello from frontend.'" **Expected:** the `backend` session receives the text as a cross-session message from `frontend` and starts a turn.
2. In `backend`, ask: "Use SendMessage to answer the session named frontend: 'Native reply from backend.'" **Expected:** the `frontend` session receives the reply.
3. In `backend`, ask: "Call get_inbox." **Expected:** the result has no messages.
4. Open the messages screen of the project in the web interface. **Expected:** neither native message appears.

Result: date, fold version, Claude Code version, pass or fail, notes.

### fold messages with a reconnect

1. In `frontend`, ask: "Send a fold message to backend: 'Is the login endpoint ready?'" **Expected:** `send_message` returns a message id, here `m-1`, with recipient `backend`.
2. In `backend`, ask: "Call get_inbox." **Expected:** the inbox holds `m-1` from `frontend` with the text from step 1.
3. In `backend`, ask: "Reply to m-1 with a fold message: 'Yes, POST /login is up.', then acknowledge m-1." **Expected:** `send_message` returns `m-2` with `reply_to` `m-1`, and `acknowledge_message` returns `m-1`.
4. In `frontend`, ask: "Call get_inbox, then get_thread for m-1." **Expected:** the inbox holds `m-2` with thread `m-1`, and the thread lists `m-1` then `m-2`.
5. In `backend`, type `/exit`. **Expected:** the session ends, and `list_agents` in `frontend` shows `backend` with presence `offline`.
6. In `frontend`, ask: "Send a fold message to backend: 'Sent while you were away.'" **Expected:** `send_message` returns `m-3`.
7. Start `backend` again with `FOLD_AGENT=backend claude --name backend` and ask: "Call get_inbox." **Expected:** the session context counts one unread message, and the inbox holds `m-3` only; the acknowledged `m-1` does not come back.
8. In `backend`, ask: "Acknowledge m-3." Then in `frontend`, ask: "Send a fold message to backend: 'Kept through a restart.'" **Expected:** `acknowledge_message` returns `m-3`, and `send_message` returns `m-4`.
9. Stop `fold serve` with Ctrl-C and run it again. Then end and start all three sessions again with the commands from setup step 3. **Expected:** the daemon starts again, and each session reconnects with the fold session context.
10. In `backend`, ask: "Call get_inbox." **Expected:** the inbox holds `m-4` only.

Result: date, fold version, Claude Code version, pass or fail, notes.

### Waiting for a message

1. In `review`, ask: "Call get_events until it returns no events, then call wait_for_events for the type message.sent with timeout_s 300." **Expected:** the `wait_for_events` call stays open.
2. In `frontend`, ask: "Send a fold message to review: 'P-1 is ready for review.'" **Expected:** the wait in `review` returns at once with one `message.sent` event whose `recipients` are `review` and whose `message` is the new message id.

Result: date, fold version, Claude Code version, pass or fail, notes.

### The push wakes an idle session

The session-start hook of each session registered its inbox socket with the daemon. After a daemon restart, end and start a session again so its hook registers the socket with the new daemon.

1. Let `review` finish its turn and leave it idle at the prompt; do not type in it. **Expected:** `review` shows its prompt with no tool call running, so the next message can reach it only through the push.
2. In `frontend`, ask: "Send a fold message to review: 'Please look at T-2.'" **Expected:** within a few seconds `review` receives a cross-session message from `fold` that reads "fold: message m-N arrived. Call get_inbox.", where `m-N` is the new message id, and it starts a turn on its own.
3. Let `review` act on the notice. **Expected:** it calls `get_inbox` and finds the message from step 2.
4. In `backend`, type `/exit`; ending an agent's last session drops its inbox socket. Then in `frontend`, ask: "Send a fold message to backend: 'No push for this one.'" **Expected:** `send_message` succeeds, and no session receives a cross-session message from `fold`.
5. Start `backend` again with `FOLD_AGENT=backend claude --name backend` and ask: "Call get_inbox." **Expected:** the inbox holds the message from step 4.

Result: date, fold version, Claude Code version, pass or fail, notes.

## Acceptance 3: independent parallel work

The automated test is `acceptance_3_parallel_workspaces_do_not_change_each_other` in `crates/fold-daemon/tests/acceptance_3_parallel_workspaces.rs`. `make acceptance` runs it.

1. In `review`, ask: "Create the tasks 'Build the login screen' and 'Add the login endpoint'. They need no edge between them." In `frontend` and `backend`, ask each to claim one of them with `claim_task`. **Expected:** both claims succeed, each with a different holder.
2. In `frontend` and `backend` at the same time, ask: "Call create_workspace for your task, then write `shared/config.txt` and one file of your own in the workspace." Give the two sessions different content for `shared/config.txt`. **Expected:** each result names its own workspace, for example `ws-1` and `ws-2`.
3. In each of the two sessions, ask: "Call get_workspace_changes for your workspace." **Expected:** each result lists its own two files as `added` and no file of the other workspace. The two `shared/config.txt` files in the two workspace directories hold different content.
4. In `review`, ask: "Call get_task for both tasks and get_repository_state." **Expected:** both tasks are `in_progress`, and the head is `r1`.
5. Leave both workspaces untouched for the quiet period (`auto_checkpoint_quiet_seconds` in `.fold/config.toml`, 120 by default). Then ask each session: "Call list_checkpoints for your workspace." **Expected:** each result lists one automatic checkpoint with the reason `quiet`, and the two checkpoint ids differ. The head is still `r1`.

Result: date, fold version, Claude Code version, pass or fail, notes.

## Acceptance 4: a change is live and main is untouched

The automated tests are `acceptance_4_change_is_live_and_main_is_untouched` and `acceptance_4_revert_clears_changes` in `crates/fold-daemon/tests/acceptance_4_live_diff.rs`. `make acceptance` runs both.

1. In `frontend`, ask: "Create a task, claim it and create its workspace." Open the live activity screen of the project in the web interface and leave it open. **Expected:** the screen lists the workspace with no changed files.
2. In `frontend`, edit `README.md` in the workspace: add one line. Do not reload the screen. **Expected:** within a few seconds the screen lists `README.md` as `modified` with `+1 -0`.
3. In the web interface, open the diff of the workspace against `r1`, then the diff of `README.md`. **Expected:** the file list holds `README.md` only, and the file diff shows the new line as added.
4. In `review`, ask: "Call get_repository_state, then compare_revisions from r1 to the workspace." **Expected:** the head is still `r1` and the comparison lists `README.md` as `modified`.
5. In `frontend`, put the original line back in `README.md`. **Expected:** the screen shows the workspace with no changed files, and the head is still `r1`.

Result: date, fold version, Claude Code version, pass or fail, notes.

## Acceptance 5, 6 and 7: review, dependencies and conflicts

These three scenarios run automated only, with three MCP clients named `frontend`, `backend` and `review` that act through MCP tools alone. `make acceptance` runs them with the other acceptance tests. They have no manual procedure.

### Acceptance 5: review and integration

The automated tests are `acceptance_5_backend_proposal_is_reviewed_and_integrated_with_no_card_moved_by_hand` and `acceptance_5_named_human_integrates_without_a_tag_over_an_earlier_request_changes` in `crates/fold-daemon/tests/acceptance_5_review_and_integrate.rs`.

The first test proves that a task goes from claim to Done through the proposal flow alone. `backend` claims a task, writes a file in its workspace, records it and opens a proposal, and the task moves to Review. `review`, which holds `integrator`, lists the proposal, claims the review, reads a review checkout whose files equal the version, approves and integrates. The task is Done with a completion row, main is `r2` with the file, the `main/` directory shows the file within a timeout, the workspace is archived, every status change has the actor `fold`, and the events `proposal.opened`, `review.submitted`, `integration.succeeded`, `main.advanced`, `proposal.integrated` and `task.status_changed` appear in that order.

The second test proves that a named human integrates without a tag. No agent holds `integrator` in this project. `review` requests changes, so the task is In Progress and nobody approved. The human `ana` integrates through `POST /api/v1/projects/{slug}/proposals/{id}/integrate` with the `fold_human` cookie and the current version. Main moves, the task is Done, and the human's `approve` is the latest verdict on the version. `review` calling `integrate_proposal` on another proposal gets `integrator_required`.

### Acceptance 6: a dependency unblocks

The automated tests are `acceptance_6_frontend_task_moves_from_backlog_to_todo_when_backend_reaches_done` and `acceptance_6_integration_is_refused_while_a_prerequisite_is_open` in `crates/fold-daemon/tests/acceptance_6_dependency_unblocks.rs`.

The first test proves that a frontend task that needs a backend task stays in Backlog until the backend task is Done, then moves to Todo in the integration's commit with no agent acting on it. Its only event after setup is `task.status_changed` from `backlog` to `todo` with the actor `fold`, after `proposal.integrated`, and `get_ready_tasks` then lists it. Nobody holds a claim on it, so the integration message from `fold` goes to `backend`, the author, and names the frontend task as ready; `frontend` gets no message.

The second test proves that an approved proposal on a task whose prerequisite is not Done does not integrate. `integrate_proposal` returns `prerequisites_not_done` with the unfinished task and its status, main stays at `r1`, the proposal stays open and `integration.refused` is recorded.

### Acceptance 7: conflicting proposals

The automated tests are `acceptance_7_second_proposal_is_refused_with_a_conflict_report_and_main_is_intact` and `acceptance_7_conflicted_proposal_integrates_after_sync_and_a_new_version` in `crates/fold-daemon/tests/acceptance_7_conflicting_proposals.rs`.

The first test proves that the second of two proposals that change the same line from `r1` is refused and leaves everything intact. `frontend` and `backend` each change the line of `README.md`, `review` approves both, and the first integrates as `r2`. The second `integrate_proposal` returns `integration_conflicted` naming `README.md` with the kind `text`. Main is still `r2` with the same revision and tree, and `main/` still shows the first change. The second proposal is open, with the turn on its author and a conflict report that names the file, the kind, both conflicting lines and the base, main and version trees. Its task is In Progress with no completion row, and its author has a message from `fold` that names the file.

The second test proves that the conflicted proposal integrates after the author resolves it. `backend` runs `sync_workspace`, which moves the base to `r2` and writes conflict markers into `README.md`. It writes a line that keeps both changes, records the changes and sends `v2` with `update_proposal`. `review` approves `v2` and integrates it with `expected_version` 2, main is `r3` and holds both changes, `main/` shows the resolved line, and the task is Done.

## Acceptance 8: a restarted daemon recovers

The automated tests are `acceptance_8_restart_recovery_graceful` and `acceptance_8_restart_recovery_dropped` in `crates/fold-daemon/tests/acceptance_8_restart_recovery.rs`. `make acceptance` runs both. The first stops the daemon gracefully; the second drops it without a shutdown, as a killed daemon ends.

1. In `review`, ask: "Create the tasks 'Build the login screen', 'Add the login endpoint' and 'Ship the login', and make 'Ship the login' need 'Build the login screen'." In `frontend` and `backend`, ask each to claim one of the first two tasks, create its workspace and write one new file in it. **Expected:** both claims and both workspaces succeed.
2. Leave both workspaces untouched for the quiet period, then in `frontend` ask: "Call record_checkpoint for your workspace with the label 'screen drafted', then create_proposal for your task." In `backend`, ask: "Call record_changes for your workspace with the message 'Add the login endpoint'." In `review`, ask: "Send a fold message to frontend and another to backend." **Expected:** each workspace has an automatic checkpoint, `frontend` has an explicit one, `backend` has a changeset, the proposal `P-1` is open, and nobody acknowledges the messages.
3. Open the live activity screen in the web interface. In `review`, ask: "Call get_events until it returns no events." Close the browser tab, then in `review` ask: "Create the task 'Write the release notes' and add the checklist item 'List the login changes' to it."
4. Stop `fold serve` with Ctrl-C. Delete the workspace directory of `backend` and write a new file into the workspace of `frontend`. Start `fold serve` again, then end and start all three sessions with the commands from setup step 3. **Expected:** the daemon starts, and each session reconnects with the fold session context.
5. In `review`, ask: "Call get_task for the four tasks, get_workspace for both workspaces, get_proposal for P-1 and get_events." In `frontend` and `backend`, ask: "Call get_inbox." **Expected:** the tasks, the proposal and the checkpoints are as before; the workspace of `backend` is `missing` and the other is `active`; each inbox still holds its message; `get_events` starts at the event after the last one `review` read in step 3, with no gap.
6. Open the live activity screen again. **Expected:** it lists the workspace of `frontend` with the new file from step 4, and the activity feed shows `workspace.missing` for the deleted workspace and `session.ended` for each agent.
7. Compare `main/` with `get_repository_state`. **Expected:** `main/` holds the files of the head revision.
8. Repeat steps 4 to 7, but stop the daemon with `kill -9` instead of Ctrl-C. **Expected:** the same results, and `fold serve` starts at once although the socket file is still there.
9. Leave the sessions idle for 31 minutes. **Expected:** the board marks both claims stale, with one `claim.stale` event each.

Result: date, fold version, Claude Code version, pass or fail, notes.

## Acceptance 9: fold works with no network

This scenario runs automated only. The automated tests are `acceptance_9_offline` and `acceptance_9_offline_inner` in `crates/fold-daemon/tests/acceptance_9_offline.rs`. `make acceptance` runs them with the other acceptance tests.

`acceptance_9_offline` runs its own test binary as `unshare --user --map-root-user --net <test binary> --exact acceptance_9_offline::acceptance_9_offline_inner --ignored --nocapture`. The only interface of the new user and network namespace is a loopback that is down, so a connect to any address fails at once. The inner test first proves this: `/proc/net/dev` lists only `lo`, a TCP connect to the literal address `1.1.1.1:443` fails as an unreachable network, and the test prints a line that says so. The proof does not look up a host name, because a lookup through a local resolver socket can succeed with no network. Then it runs the flow with the three MCP clients `frontend`, `backend` and `review`: `fold init` from a directory makes `r1`, tasks with a dependency move from Backlog to Todo, `backend` claims, edits, records a checkpoint and a changeset, the agents exchange and acknowledge fold messages, a human grants `integrator` to `review`, which approves and integrates to `r2`, a second pair of workers gets a conflict report, the open event stream delivers every stored event, and the project passes the consistency check of the crash tests. The outer test passes only when the child passes and printed the proof.

On Linux the test needs `unshare` from the util-linux package and unprivileged user namespaces (`kernel.unprivileged_userns_clone=1`, and `kernel.apparmor_restrict_unprivileged_userns=0` where AppArmor restricts them). Without them the test fails with a message that names both; it is never skipped. On other platforms `acceptance_9_offline` runs the flow directly. `acceptance_9_offline_inner` is ignored in a plain run; run alone it skips the proof and still passes, because the flow needs no network.

The offline guarantee covers every fold operation except `fold push` and `fold pull`, which need a remote. The flow runs with no remote and with `push_on_integrate` at its default `false`, and its event log holds no `remote.*` event.
