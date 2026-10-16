---
name: fold
description: Work in a fold project. Use it for tasks, workspaces, proposals, reviews, requests and messages: register an agent, find and claim tasks, edit files in a task workspace, open and review proposals, integrate them, turn requests into tasks, and read and send messages through the fold MCP tools.
---

# fold

fold plans, builds, reviews and integrates the work of this project. Every action goes through the tools of the `fold` MCP server, and the daemon decides what each tool may do. A task has a status that fold calculates from its dependencies, claims, workspaces and proposals. You never set a status.

Each section below is for one role. The main agent runs the team, an orchestrator plans the task graph, a worker builds one task, and a reviewer reviews proposals and integrates them. One agent can act in more than one role. The worker agents that fold runs have their own roles: see Worker agents.

## Identity

The user starts the main agent in the project folder with `fld claude`. It starts the daemon when needed, then the user's own Claude Code session as the agent `main` with the `main` tag. `fld claude --name <name>` picks another name, and the arguments after `--` go to `claude`. Start any other session with `FOLD_AGENT=<name> claude --name <name>`. The environment variable gives your agent name to the tools, and the Claude Code name gives the session its messaging socket. Without it, call `register_agent(name, tags?)` first.

Every other tool refuses with `agent_not_registered` until the agent is registered. Use `list_agents` to see the agents, their presence, their workers and their claims.

Only a human gives the `integrator` tag. An agent with that tag may approve, reject and integrate a proposal. A named human may also approve, reject and integrate without the tag.

## Messaging

To message another orchestrator, use the native `SendMessage` of Claude Code. Use `send_message`, `get_inbox`, `get_thread` and `acknowledge_message` only where native messaging is not available, and to write to humans.

Addresses for `send_message(to[], body, task?, proposal?, request?, reply_to?, choices?)`:

- `frontend` is the agent `frontend` and its main worker.
- `frontend/ui` is the worker `ui` of the agent `frontend`.
- `tag:<name>` is every agent that has the tag.
- `human:<name>` is one human.
- `humans` is every human.
- `all` is every agent.

To ask a human to choose, send the question to `humans` or `human:<name>` with 2 to 6 options in `choices`. Each option is a short label, or `{label, description}` with one line that says what the option means; the human sees the description under the label. The human answers with a reply that names one label, or with free text.

To answer a message, pass its id as `reply_to`: the reply joins its thread. `get_inbox` lists your unread messages, `get_thread(thread)` shows one conversation, and `get_thread(request)` shows the clarification thread of a request. Call `acknowledge_message(id, up_to?)` when you have read a message. With `up_to` true it acknowledges every message up to `id`.

A message does not change a task. It is not a claim and not a dependency. When a conversation decides something about the work, put the result into the task graph: create the task, add the dependency or update the description.

## Noticing news

You get news in four ways:

- The session push puts a new message into your session.
- The footer of every tool result can show `fold: <u> unread · <e> new events · <c> claims near stale`. A part with a count of zero is left out.
- `wait_for_events(types?, task?, proposal?, timeout_s?)` blocks until a matching event arrives or the time runs out. The default is 300 s and the most is 1500 s. Use it to wait for a proposal, a review or a dependency, and not to poll.
- The Stop hook continues your turn once when messages are unread.

`get_events(after?, types?, task?, proposal?, limit?)` reads events. Without `after` it resumes from your cursor and moves the cursor. With an explicit `after` it leaves the cursor as it is. A session that was closed reads the events it missed through its cursor.

## Orchestrating

The main agent is the session the user talks to. It runs the team through fold: it handles requests, decides the areas, plans the tasks and starts the leads. The leads, the coders and the reviewers do the rest.

1. Handle every request as described in Requests: claim it, ask in its thread when the message is not clear, draft the cards with the label of the area that must build each one, and revise the draft until the requester approves it. When the user asks you for work in the conversation, plan the tasks yourself with `create_task` and `add_dependency`.
2. Decide the areas. An area is a part of the project that one lead can own, such as `backend` or `web`. Make its label with `create_label`, then call `define_area(name, label)`. Give every task the label of its area.
3. Plan the tasks as described in Orchestrator. Put every dependency in the graph, and split a large task into child tasks.
4. Call `require_human_review(task, reviewer?, reason)` when the user wants a human to review a task.
5. Start one `area_lead` for each area and one `review_lead` with `start_worker`. Tell each lead in `instructions` what is important for its area. The area leads start the task coders, and the review lead starts the batch reviewers. You do not start them.
6. Approvals integrate only when an agent with the `integrator` tag gives them. Ask the user to run `fld agent tag <lead> integrator` for the review lead.
7. Watch the work with `list_workers`, `get_task_graph` and `wait_for_events`, and answer the messages of the leads. Stop a worker with `stop_worker` only when its work is no longer necessary.
8. While worker agents run, never edit code and never build a task yourself. Give the work to the lead of its area.

While one agent has the `main` tag, only that agent defines areas, starts leads and claims requests. Every other agent that fold does not supervise gets `orchestrator_required`. While no agent has the tag, the main worker of any agent that fold does not supervise can do this work.

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
3. Call `create_workspace(task)`. Do all the work inside the workspace under `.fold/ws/`. Read it with `get_workspace(ws or task)`. See your changed files with `get_workspace_changes(ws)`.
4. Build and test only in the workspace. Read the repository's own `AGENTS.md` or `CLAUDE.md` there.
5. Save your progress with `record_checkpoint(ws, label?)`. Read the checkpoints with `list_checkpoints(ws)`, and go back with `restore_checkpoint(ck)`. If `restore_checkpoint` fails with `unrecorded_changes`, call it again: the new call records your latest edits in its safety checkpoint. fold also records checkpoints by itself after a quiet time.
6. When the task is finished, call `record_changes(ws, message)`. The message says what changed and why.
7. If main moved, call `sync_workspace(ws)`. Resolve each conflict in the workspace files: remove the conflict markers, and remove each `<path>.fold-main` file after you merge it. Then record again. `record_changes` refuses with `unresolved_conflicts` while a conflict stays. Read `get_repository_state`, `get_revision(r)` and `compare_revisions(from, to, path?)` to see what changed on main.
8. Open the proposal with `create_proposal(task, title, evidence?)`. The title becomes the subject of the Git commit that fold writes when the proposal is integrated, so write it as a short imperative subject of at most 72 characters, such as "Add the login endpoint". The evidence says how you tested the change. The first version is `v1`.
9. Read the review with `get_review_comments(id, version?)`. Fix the points in the workspace, record again, and call `update_proposal(id, evidence?)`. This opens `v2`, which starts without approvals.
10. Call `close_proposal(id, reason)` to withdraw a proposal. `abandon_workspace(ws, reason)` refuses with `invalid_request` and the reason `proposal_open` while a proposal for the task is open: call `close_proposal` first.
11. Call `release_task(id)` when you cannot finish the task. Call `takeover_claim(id)` to take a task whose claim is stale. It fails with `claim_not_stale` while the lease is valid.

## Reviewer

A reviewer reads proposals, asks for changes and integrates the proposals that are ready.

1. Call `list_proposals(state?, unclaimed?)` or wait with `wait_for_events` for `proposal.opened`.
2. Call `claim_review(id)`. It also takes over a review claim that is stale. Read the proposal with `get_proposal(id, version?)`.
3. Call `create_review_checkout(id)`. It gives you a checkout of the version. Build and test there, never in the project folder itself.
4. Call `submit_review(id, verdict, body, comments[]?)`. The verdict is one of `comment`, `request_changes`, `approve` or `reject`. A comment has `path`, `start_line`, `end_line`, `side` and `body`. A review with no content fails with `empty_review`.
5. An approval belongs to the version you reviewed. A new version starts without approvals, so review it again.
6. fold integrates a ready proposal by itself when the project integrates automatically, which is the default: an integrator approved it, nothing asks for changes, a marked task has a human approval after the agent approval, and every prerequisite task is Done. In a project that does not integrate automatically, call `integrate_proposal(id, expected_version?)` when the proposal is ready. Pass the version you reviewed as `expected_version`: if a newer version exists, the call fails with `stale_version`. fold checks the approvals, the human review mark and the prerequisite tasks before it moves main.
7. Call `release_review(id)` when you stop the review.

## Mentions

In all text you write for people, `#` names a fold item, `@name` names a person and `@path` names a file. The text is in messages, review comments, proposal evidence, task descriptions and requests. `#T-12` is a task, `#P-3` a proposal, `#RQ-2` a request, `#r17` a revision, `#ws-9` a workspace, `#ck-88` a checkpoint and `#m-210` a message. `@tanim` is a human and `@frontend` or `@frontend/ui` is an agent or a worker: fold reads `@x` as a person when `x` is the name of a known human or agent. `@web/src/board.ts` is a file of main, in the project folder, and `@ws-9:web/src/board.ts` a file in a workspace. `@web/src/` with a `/` at the end is a folder. If a file has the same name as a person, write `@./name` for the file. The web interface shows each mention as a link or a name. fold keeps the text as it is written and checks no mention. Write a mention only for an item, person or file that exists, and write it with no space after the mark.

## Requests

A request asks for work in plain words. Anyone can create one: a human in the web interface, or any worker with `create_request(body, attachments?, ws?)`. A worker that finds a bug or missing work outside its own tasks or proposals creates a request for it. The request goes to the main agent like the request of a human. Give only the message. Do not give a title or a kind, because the orchestrator writes them. To attach files such as a log or a screenshot, give their paths inside your own workspace as `attachments` and that workspace as `ws`. A request holds at most 10 files of at most 10 MiB each.

In the text of a request, a draft note and a comment, write mentions as described in Mentions. Read each item with its tool, such as `get_task` or `get_proposal`, and read each file in the project folder or in its workspace.

The main agent turns a request into cards. The main agent is the agent with the `main` tag: `fld claude` gives it to the user's own interactive session, which can also take it with `register_agent(name, tags)`. One agent at most has the tag, and a worker that fold started never has it; another agent gets `main_tag_denied`. When no agent has the `main` tag, an orchestrator (the main worker of any agent) does this work.

1. When a request is created, the main agent gets a push. When no agent has the `main` tag, every agent except the requester's agent gets it. Call `list_requests(state?, kind?, mine?)` to see the requests, and `get_request(id, version?)` to read one: the message, the attachments, the latest draft (or the draft `version`), the comments of the requester, the id of the clarification thread and the tasks of an approval.
2. Call `claim_request(id, kind, title)`. When a main agent exists, only the main agent claims; another agent gets `orchestrator_required`. The kind is `bug`, `feature` or `question`, as you read the message. Write the title from the message: one line, at most 200 characters.
3. When the message is not clear, ask with `send_message(to[], body, request)`. The message goes into the clarification thread of the request, and `get_thread(request)` reads that thread. Anyone can write there. A message does not change the request.
4. Write the draft with `submit_request_draft(id, cards[], edges[]?, note?, kind?, title?)`. A draft has 1 to 20 cards. Each card has a `key`, a `title` and a `description` in the task template below. Each edge names the `prerequisite` card and the `dependent` card. Write dependencies as edges, never as text in a description. Give a card the `label` of the area that must build it. Approval gives the task this label. The label must exist, so call `create_label` first; an unknown label fails with `label_not_found`.
5. When the requester asks for changes, you get a push. Read the comment with `get_request`, then submit the next version.
6. Call `release_request(id)` when you cannot finish the draft. Another orchestrator can then claim the request.

When a task becomes ready and nobody holds its claim, fold sends a push to the lead of its area, the area whose label the task carries, with `start_worker` as the next tool. The cards of an approved request become ready this way, and so do the dependents of an integrated task. When the area has no lead, the main agent gets the push and starts the lead. When the task has no area, the main agent gets the push and gives the task a label with `update_task`.

Only the requester decides. The requester approves the latest version with `approve_request_draft(id, expected_version)`, asks for changes with `request_draft_changes(id, version, comment)`, writes the next version with `edit_request_draft(id, cards[], edges[]?, note?)`, or closes the request with `close_request(id, reason)`. When an agent is the requester, it gets a push for each new version from the drafter. When a task coder or a batch reviewer is the requester, the lead that started it also decides, because fold stops a short-lived worker when its work is done. After the worker stops, the lead gets its pushes for the request.

The approval creates one task for each card and one dependency for each edge, in one step. fold calculates the column of each new task: a task whose prerequisites are not Done is in Backlog, and the other tasks are in Todo. The drafter gets a push with the new task ids.

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

## Worker agents

fold can run agents for you. Each worker agent is a headless Claude Code session that fold starts, feeds with notices, watches and stops. It has its own agent name and works through the same tools as you.

1. The main agent, which is the user's own interactive session, or a human defines each area with `define_area(name, label)`. The tasks with that label belong to the area. Create the label with `create_label` first.
2. The main agent or a human starts the leads with `start_worker(role, name?, area?, model?, instructions?)`: one `area_lead` for each area and one `review_lead`. A start for an area whose lead is stopped or failed resumes that lead. When the lead's session is gone, fold starts it in a new session whose first turn holds its latest hand-off note.
3. An area lead starts a `task_coder` with `start_worker(role, tasks[], model?, instructions?)` for tasks of its own area. A task without the area's label fails with `task_outside_area`.
4. The review lead starts a `batch_reviewer` with `start_worker(role, proposals[], model?, instructions?)` for related proposals.
5. Nobody else starts a worker. A subagent worker, a task coder, a batch reviewer or a lead of another kind gets `worker_start_denied` or `main_agent_required`, and another agent that fold does not supervise gets `orchestrator_required` while the main agent exists.
6. fold stops a task coder when each of its tasks is Done, is integrated or has an open proposal with no changes requested and no conflict, a batch reviewer when each of its proposals has a verdict, and either one after `idle_minutes` without a turn. Call `stop_worker(name)` to stop a worker. A stopped worker keeps its session, so fold can resume it.
7. `list_workers` shows the areas with their leads and every worker with its role, state, tasks or proposals, turns, cost and context size.
8. A start over the limits in `.fold/config.toml` fails with `worker_limit_reached`, and its hint names the role and its limit. By default 4 task coders and 3 batch reviewers run at a time. The leads are outside the limits, so a start of a lead is never refused for them.
9. When a review requests changes on a stopped task coder's proposal, or the proposal conflicts with main at integration, fold resumes that coder in its own session with the comments or the conflicting paths and the `sync_workspace` step. It stops again when it posts the new version with `update_proposal`. When the worker limits are full, fold resumes it as soon as a worker stops. A coder whose session is gone starts in a new session with the same name, so it stays the author. After a daemon restart, each worker whose session is gone starts in a new session the same way.
10. A lead never loses its work to a full context. When a lead's context passes `handoff_at` of its role (60 percent of the model's context window by default) and its turn has ended, fold asks it once for a hand-off note. The lead calls `write_handoff(text)` with what it is doing, its open decisions and what comes next, then ends its turn. fold starts the lead again in a new session whose first turn holds the note and a summary of its open tasks, open proposals and unread messages. A lead that writes no note within a few turns starts fresh with the summary alone. `list_workers` shows each worker's newest note.
11. fold does not refresh the main agent, because it is the user's own session. When your context as the main agent grows large, send yourself a hand-off note with `send_message`, run `/clear`, and read the note with `get_inbox` in the new session.

Each role has its own part:

### Area lead

The area lead owns the tasks with its area's label. It plans and starts coders, and writes no code.

1. Read the tasks of the area with `get_task_graph` and `get_ready_tasks`. Split a large task with `create_task` and `add_dependency`, and give each new task the area's label.
2. Start a `task_coder` for ready work with `start_worker(role, tasks[], model?, instructions?)`. Choose the model and write the instructions for the tasks. fold sends you a notice when a task of the area becomes ready and nobody holds its claim.
3. Watch the coders with `list_workers`. fold stops each one when its work is done, and resumes it when its proposal needs changes.
4. When your context grows large, call `write_handoff(text)` when fold asks for it, then end your turn.

### Review lead

The review lead keeps the open proposals moving.

1. Read the open proposals with `list_proposals` or wait for `proposal.opened` with `wait_for_events`.
2. Put related proposals, such as the proposals of dependent tasks, into one review batch with `create_review_batch(proposals[])`. fold takes the review claim of each one and orders them by their task dependencies. Review a single proposal as described in Reviewer.
3. Review a batch yourself, or start a `batch_reviewer` for it with `start_worker(role, proposals[], model?, instructions?)` when its tests take long.
4. To review a batch, call `create_batch_checkout(id)`: a checkout of main with every member merged in order. A member that conflicts with another gives `batch_conflict`. Take it out with `remove_from_batch(id, proposal)` and review it alone. Build and test the checkout once.
5. Call `submit_batch_review(id, verdicts[], comments[]?)` with one verdict for each member. A line comment on the batch checkout goes to the member that brought those lines. A line that no member brought gives `unattributed_lines`. `list_review_batches(open?)` and `get_review_batch(id)` show the batches.
6. fold integrates each approved proposal by itself in the order of its dependencies, with one Git commit each, and resumes the coder of each proposal that needs changes.

### Task coder

A task coder builds the tasks its lead gave it, as described in Worker. It works only in their task workspaces and sends one proposal for each task. When a review requests changes or the proposal conflicts with main, fold resumes it: it changes the work in the same workspace and sends the new version with `update_proposal`. fold stops it when each task has its proposal.

### Batch reviewer

A batch reviewer reviews the proposals its review lead gave it, with `create_review_batch`, `create_batch_checkout` and `submit_batch_review` as described for the review lead, or with `submit_review` for a single proposal. fold stops it when each of its proposals has a verdict.

When Claude Code's auto mode is not sure about a tool call of a worker, fold asks a human and the worker waits for the answer. A worker that uses `AskUserQuestion` asks the humans a question with choices, and the human's reply is its answer.

A worker reads and writes project files only inside its own workspace under `.fold/ws/`. It never edits the files of the project folder, which are main, or another workspace, and it writes no files outside the project. fold checks every path that its own tools receive and every file edit of Claude Code's edit tools, but not shell commands, so each worker keeps this rule itself.

## Errors

Every error has a stable `code`, a message and a hint that names the next tool. Branch on the code.

| Code | What to do |
| --- | --- |
| `agent_not_registered` | Call `register_agent`. |
| `already_claimed` | Call `get_ready_tasks` and choose another task. For a request, call `list_requests` and choose another request. |
| `dependency_cycle` | Call `get_task_graph` and remove the edge that closes the cycle. In a request draft, remove the edge between the cards. |
| `not_approved` | Call `get_proposal`. The current version needs an approval. |
| `human_approval_missing` | Call `get_proposal`. A human must approve first. |
| `prerequisites_not_done` | Call `get_proposal`. A task that this one needs is not Done. |
| `integrator_required` | Call `get_proposal`. An agent with the `integrator` tag or a human must integrate. |
| `stale_version` | Call `get_proposal`, review the newest version and integrate again. For a request, call `get_request` and read the latest version. |
| `changes_requested` | Call `get_proposal`. A review of the current version asks for changes. |
| `invalid_request` | Read `details.reason`. With `proposal_open` from `abandon_workspace`, call `close_proposal`. With `state` from a request tool, call `get_request`: the request is not in the state for that step. With `too_many_attachments` or `attachment_too_large` from `create_request`, attach fewer or smaller files. |
| `orchestrator_required` | Only the main worker of an agent claims a request, and while one agent has the `main` tag, only that agent defines areas, starts leads and claims requests. Ask the main agent. |
| `request_not_found` | Call `list_requests` and use an id from the list. |
| `request_not_open` | The request is approved or closed. Call `get_request` to read it. |
| `requester_required` | Only the requester approves, asks for changes, edits or closes. Call `get_request` to see the requester, and ask with `send_message`. |
| `drafter_required` | Only the agent of the drafter drafts or releases. Call `claim_request` first, or `get_request` to see the drafter. |
| `invalid_draft` | Read `details.reason`, correct the cards or the edges, and submit again. |
| `main_agent_required` | Only the main agent or a human defines areas and starts leads. A lead or a short-lived worker asks the main agent. |
| `main_tag_denied` | One agent at most has the `main` tag, and a worker that fold started never has it. Register without the tag, and send messages to the main agent. |
| `worker_start_denied` | Only an area lead starts task coders, and only the review lead starts batch reviewers. Ask that lead. |
| `task_outside_area` | Start a task coder only for tasks with the label of your own area. Call `get_task` to see the labels. |
| `area_has_lead` | The area's lead runs already. Send it a message instead. |
| `worker_limit_reached` | Call `list_workers`. Wait until a worker stops, or stop one with `stop_worker`. |
| `handoff_denied` | Only an area lead or the review lead writes a hand-off note. As the main agent, send yourself the note and run `/clear`. |

## Never

- Never edit the files of the project folder. They are main, and they change only through integration.
- Never edit the workspace of another worker.
- Never tick a linked checklist item. fold ticks it when the child task is Done.
- Never clear the human review mark.
- Never approve your own proposal.
- Never approve a request you did not create.
- Never treat a message as a claim or as a dependency.
- Never move a card or set a status. fold calculates the status.
