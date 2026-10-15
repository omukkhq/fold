---
name: fold
description: Work in a fold project. Use it for tasks, workspaces, proposals, reviews and messages: register an agent, find and claim tasks, edit files in a task workspace, open and review proposals, integrate them, and read and send messages through the fold MCP tools.
---

# fold

fold plans, builds, reviews and integrates the work of this project. Every action goes through the tools of the `fold` MCP server, and the daemon decides what each tool may do. A task has a status that fold calculates from its dependencies, claims, workspaces and proposals. You never set a status.

Each section below is for one role. An orchestrator plans the task graph, a worker builds one task, and a reviewer reviews proposals and integrates them. One agent can act in more than one role.

## Identity

Start the session with `FOLD_AGENT=<name> claude --name <name>`. The environment variable gives your agent name to the tools. Without it, call `register_agent(name, tags?)` first.

Every other tool refuses with `agent_not_registered` until the agent is registered. Use `list_agents` to see the agents, their presence, their workers and their claims.

Only a human gives the `integrator` tag. An agent with that tag may approve, reject and integrate a proposal. A named human may also approve, reject and integrate without the tag.

## Messaging

To message another orchestrator, use the native `SendMessage` of Claude Code. Use `send_message`, `get_inbox`, `get_thread` and `acknowledge_message` only where native messaging is not available, and to write to humans.

Addresses for `send_message(to[], body, task?, proposal?, reply_to?)`:

- `frontend` is the agent `frontend` and its main worker.
- `frontend/ui` is the worker `ui` of the agent `frontend`.
- `tag:<name>` is every agent that has the tag.
- `human:<name>` is one human.
- `humans` is every human.
- `all` is every agent.

To answer a message, pass its id as `reply_to`: the reply joins its thread. `get_inbox` lists your unread messages, and `get_thread(thread)` shows one conversation. Call `acknowledge_message(id, up_to?)` when you have read a message. With `up_to` true it acknowledges every message up to `id`.

A message does not change a task. It is not a claim and not a dependency. When a conversation decides something about the work, put the result into the task graph: create the task, add the dependency or update the description.

## Noticing news

You get news in four ways:

- The session push puts a new message into your session.
- The footer of every tool result can show `fold: <u> unread · <e> new events · <c> claims near stale`. A part with a count of zero is left out.
- `wait_for_events(types?, task?, proposal?, timeout_s?)` blocks until a matching event arrives or the time runs out. The default is 300 s and the most is 1500 s. Use it to wait for a proposal, a review or a dependency, and not to poll.
- The Stop hook continues your turn once when messages are unread.

`get_events(after?, types?, task?, proposal?, limit?)` reads events. Without `after` it resumes from your cursor and moves the cursor. With an explicit `after` it leaves the cursor as it is. A session that was closed reads the events it missed through its cursor.

## Orchestrator

An orchestrator keeps the task graph correct and the workers busy.

1. Read the graph with `get_task_graph` and one task with `get_task(id)`. Read what each worker does with `get_activity(agent?)`: claims, workspaces with changed files, latest checkpoints and open proposals.
2. Create a task with `create_task(title, description?, labels?, needs?, parent?)`. Write the description with the task template below. Change it with `update_task`, and remove it with `delete_task` or `archive_task`.
3. Put every dependency in the graph with `add_dependency(task, needs)`. Remove one with `remove_dependency`. A dependency that would close a cycle fails with `dependency_cycle`.
4. Split a large task with a parent and child tasks. Add each child to the parent's checklist with `add_checklist_item(task, text? / linked_task?, checklist?)`. With `linked_task` the item follows the child task, and fold ticks it when the child is Done. Change a plain item with `update_checklist_item(id, text?, done?)`. Move an item with `move_checklist_item(id, position, checklist?)`: the position counts from 0, a position past the end puts the item last, and `checklist` moves it into another checklist of the same task. Remove an item with `remove_checklist_item`.
5. Sort tasks with labels. Make one with `create_label(name, color)`.
6. Call `require_human_review(task, reviewer?, reason)` on a task when the human's prompt asks for a human to review it. The mark stays until a human clears it.
7. Give each subagent its own worker with `register_worker(name, parent?)`, and tell it to pass `as` (see Subagents).

A task is ready when its dependencies are Done and nobody holds it. Stalled and stale are two different states. A claim is stalled when its workspace had no file change for 5 minutes (`stall_minutes`). This only shows on the board. A claim is stale when its 30-minute lease (`lease_minutes`) ran out. A stale claim allows `takeover_claim`.

## Worker

A worker builds one task at a time.

1. Call `get_ready_tasks` and choose a task. Read it with `get_task(id)`.
2. Call `claim_task(id)`. If another agent holds the task, you get `already_claimed`: call `get_ready_tasks` again.
3. Call `create_workspace(task)`. Do all the work inside the workspace under `ws/`. Read it with `get_workspace(ws or task)`. See your changed files with `get_workspace_changes(ws)`.
4. Build and test only in the workspace. Read the repository's own `AGENTS.md` or `CLAUDE.md` there.
5. Save your progress with `record_checkpoint(ws, label?)`. Read the checkpoints with `list_checkpoints(ws)`, and go back with `restore_checkpoint(ck)`. If `restore_checkpoint` fails with `unrecorded_changes`, call it again: the new call records your latest edits in its safety checkpoint. fold also records checkpoints by itself after a quiet time.
6. When the task is finished, call `record_changes(ws, message)`. The message says what changed and why.
7. If `main/` moved, call `sync_workspace(ws)`. Resolve each conflict in the workspace files: remove the conflict markers, and remove each `<path>.fold-main` file after you merge it. Then record again. `record_changes` refuses with `unresolved_conflicts` while a conflict stays. Read `get_repository_state`, `get_revision(r)` and `compare_revisions(from, to, path?)` to see what changed on main.
8. Open the proposal with `create_proposal(task, title, evidence?)`. The evidence says how you tested the change. The first version is `v1`.
9. Read the review with `get_review_comments(id, version?)`. Fix the points in the workspace, record again, and call `update_proposal(id, evidence?)`. This opens `v2`, which starts without approvals.
10. Call `close_proposal(id, reason)` to withdraw a proposal. `abandon_workspace(ws, reason)` refuses with `invalid_request` and the reason `proposal_open` while a proposal for the task is open: call `close_proposal` first.
11. Call `release_task(id)` when you cannot finish the task. Call `takeover_claim(id)` to take a task whose claim is stale. It fails with `claim_not_stale` while the lease is valid.

## Reviewer

A reviewer reads proposals, asks for changes and integrates the proposals that are ready.

1. Call `list_proposals(state?, unclaimed?)` or wait with `wait_for_events` for `proposal.opened`.
2. Call `claim_review(id)`. It also takes over a review claim that is stale. Read the proposal with `get_proposal(id, version?)`.
3. Call `create_review_checkout(id)`. It gives you a checkout of the version. Build and test there, never in `main/`.
4. Call `submit_review(id, verdict, body, comments[]?)`. The verdict is one of `comment`, `request_changes`, `approve` or `reject`. A comment has `path`, `start_line`, `end_line`, `side` and `body`. A review with no content fails with `empty_review`.
5. An approval belongs to the version you reviewed. A new version starts without approvals, so review it again.
6. If the proposal is ready, call `integrate_proposal(id, expected_version?)`. Pass the version you reviewed as `expected_version`: if a newer version exists, the call fails with `stale_version`. fold checks the approvals, the human review mark and the prerequisite tasks before it moves `main/`.
7. Call `release_review(id)` when you stop the review.

## Task template

Write the description of every task in this form:

- A summary paragraph that says what the task delivers.
- **Refs**: the documents, tasks and code the worker must read.
- **Scope**: what to build, with the files or modules.
- **Out of scope**: what belongs to other tasks.
- **Acceptance**: the checks that show the task is done, each one testable.

Write dependencies as edges with `add_dependency`, never as text in the description.

## Subagents

A subagent works as a worker of your agent. The orchestrator calls `register_worker(name, parent?)` for each subagent and tells the subagent to pass `as` with that name in every tool call. A call without `as` acts as the main worker of the agent. Each worker holds its own claims and workspaces and gets its own checkpoints.

## Errors

Every error has a stable `code`, a message and a hint that names the next tool. Branch on the code.

| Code | What to do |
| --- | --- |
| `agent_not_registered` | Call `register_agent`. |
| `already_claimed` | Call `get_ready_tasks` and choose another task. |
| `dependency_cycle` | Call `get_task_graph` and remove the edge that closes the cycle. |
| `not_approved` | Call `get_proposal`. The current version needs an approval. |
| `human_approval_missing` | Call `get_proposal`. A human must approve first. |
| `prerequisites_not_done` | Call `get_proposal`. A task that this one needs is not Done. |
| `integrator_required` | Call `get_proposal`. An agent with the `integrator` tag or a human must integrate. |
| `stale_version` | Call `get_proposal`, review the newest version and integrate again. |
| `changes_requested` | Call `get_proposal`. A review of the current version asks for changes. |
| `invalid_request` | Read `details.reason`. With `proposal_open` from `abandon_workspace`, call `close_proposal`. |

## Never

- Never edit `main/`. It changes only through integration.
- Never edit the workspace of another worker.
- Never tick a linked checklist item. fold ticks it when the child task is Done.
- Never clear the human review mark.
- Never approve your own proposal.
- Never treat a message as a claim or as a dependency.
- Never move a card or set a status. fold calculates the status.
