# fold errors

Every error has a stable `code`, a message and a hint that names the next tool. Branch on the code.

| Code | What to do |
| --- | --- |
| `agent_not_registered` | Call `register_agent`. |
| `already_claimed` | Read the holder with `get_task` or `get_proposal`, and tell the lead. Stay within assigned work. For an unassigned session, choose other ready work; for a request, use `list_requests`. |
| `dependency_cycle` | Call `get_task_graph` and remove the edge that closes the cycle. In a request draft, remove the edge between the cards. |
| `not_approved` | Call `get_proposal`. The current version needs an approval. |
| `human_approval_missing` | Call `get_proposal`. A human must approve first. For `complete_task`, the task has the human review mark, so a human completes it. |
| `prerequisites_not_done` | A task that this one needs is not Done. Call `get_proposal` for a proposal, or `get_task` on each prerequisite in the details for `complete_task`. |
| `integrator_required` | Call `get_proposal`. Only the main agent, a reviewer or a human may approve or reject, they and the review lead may integrate, and only the main agent claims a review. Another agent tells the review lead with `send_message`. |
| `stale_version` | Call `get_proposal`, check out and review the newest version, then submit the review or integrate again. For a request, call `get_request` and read the latest version. |
| `changes_requested` | Call `get_proposal`. A review of the current version asks for changes. |
| `invalid_request` | Read `details.reason`. With `proposal_open` from `abandon_workspace`, call `close_proposal`. With `state` from a request tool, call `get_request`: the request is not in the state for that step. With `too_many_attachments` or `attachment_too_large` from `create_request`, attach fewer or smaller files. With `two_areas`, keep one area label on the task, or on each task in `details.tasks` for `define_area`; labels of no area stay free. With `main_approves` from `create_task_plan`, give it only to a task of `main` without `human_review`. |
| `orchestrator_required` | Only the agent with the `main` tag defines areas and starts leads, and only it or an area lead claims a request or a task. Ask the main agent, or ask the lead of the task's area to start a task worker for it. |
| `request_not_found` | Call `list_requests` and use an id from the list. |
| `request_not_open` | The request is approved or closed. Call `get_request` to read it. |
| `requester_required` | Only the requester approves, asks for changes, edits or closes. Call `get_request` to see the requester, and ask with `send_message`. |
| `drafter_required` | Only the agent of the drafter drafts or releases. Call `claim_request` first, or `get_request` to see the drafter. |
| `invalid_draft` | Read `details.reason`, correct the cards or the edges, and submit again. |
| `main_agent_required` | Only the main agent or a human defines areas and starts leads. A lead or a short-lived worker asks the main agent. |
| `main_tag_denied` | Only the session that `fld claude` starts has the `main` tag, and `register_agent` never gives it. One agent at most has the tag, and a worker that fold started never has it. Register without the tag, and send messages to the main agent. |
| `agent_approval_required` | A human approves or integrates a task with the human review mark only after an agent with review rights approved the version. Ask the review lead to review and approve it first. |
| `worker_start_denied` | Only an area lead starts task workers, and only the review lead starts reviewers. Ask that lead. |
| `task_outside_area` | Start task workers or publish plans only for your area's tasks. A plan's existing parents must also belong to your area. Call `get_task` to see the labels. |
| `worker_claim_denied` | A worker that fold started gets its claims from its lead's `start_worker` and never claims a task or a review itself. Work on the tasks or proposals of your first turn, and send other work to your lead with `send_message`. |
| `area_has_lead` | The area's lead runs already. Send it a message instead. |
| `worker_limit_reached` | Call `list_workers`. Wait until a worker stops, or stop one with `stop_worker`. |
| `task_kind_mismatch` | The task's kind does not fit the call. Call `get_task`: a `no_proposal` task ends with `report_task`, any other task with `create_proposal`. |
| `task_kind_denied` | Only the main agent, a human or the lead of the task's area creates a `no_proposal` task. Create it with the default kind, or ask the main agent. |
| `task_check_denied` | Only the lead of the task's area, the main agent or a human completes or sends back a `no_proposal` task. Ask that lead. |
| `task_not_reported` | The task has no report to check. Wait until its task worker calls `report_task`. |
| `task_already_reported` | Your report waits for a lead check. End your turn; fold resumes you if the task is sent back. |
| `handoff_denied` | Only an area lead or the review lead calls `write_handoff`. As the main agent, send yourself the note and ask the user to run `/clear`. |
| `project_paused` | The user paused the agents of the project. Start no work. Only when the user asks, the main agent calls `resume_project`. |
| `worker_peer_refused` | A command in a worker's Bash, or in a check that fold runs for the worker, acts only as that worker's own agent and never as a human; a check that fold runs for no worker acts as no agent. Use the fold tools of your own session. |
| `invalid_hold_reason` | Give the blocker a reason of 1 to 500 characters. |
| `task_on_hold` | The task has a blocker and waits for a person. Call `get_task` to read the reason, and call `get_ready_tasks` for other work. A human or the main agent clears it with `clear_blocker`. |
| `task_needs_human` | The task is a human task: a person does it outside fold. Call `get_ready_tasks` for other work. A human or the main agent marks it Done with `complete_task`. |
| `task_hold_denied` | Only a human, the main agent or the lead of the task's area sets a blocker, and only on a task in Backlog or Todo that no agent has started (detail reason `started`). Ask that lead or the main agent, or let the started work finish. |
| `checklist_denied` | Only the holder's agent, its lead, the main agent or a human changes the checklist of a held task; only the lead of the task's area, the main agent or a human changes it when nobody holds the task. Call `get_task` and ask that agent. |
| `checklist_ambiguous` | The task has two or more checklists and the plain item names none. Name one in `checklist`; `details.checklists` lists them. |
| `checks_file_invalid` | `fold-checks.toml` in the workspace is not valid; the message names the line or the check. Fix the file and call the command again. |
| `checks_running` | fold still runs the checks of the workspace. Wait for `checks.finished` with `wait_for_events`, then call the command again. |
| `check_run_not_found` | Call `get_proposal` to see the check runs of a proposal, or use the run id from the result message. |
| `main_area` | The area `main` belongs to the main agent: it has no lead and no profile, and no review agent reviews its tasks. A human approves a proposal of `main`, or the main agent when the user said so. Review the proposals of the other areas. |
| `review_lead_delegates` | The review lead never reviews a proposal itself. Call `start_worker` with role `reviewer` and the proposal ids; fold gives the reviewer the review claim, or a batch of a group. |
| `review_plan_too_large` | A review plan holds at most 5 tasks, the tasks of its proposals included. Keep the most closely related tasks and review the others alone. |
| `task_not_awaitable` | A plan cannot wait for this task; `details.reason` says why, such as `blocker`, `backlog`, `human_review` or `depends_on_member`, or `proposal_open` when its proposal is open: put that proposal in the plan. |
| `proposal_not_plannable` | Only an open proposal without a verdict, a review claim, a batch or the human review mark waits in a plan. Give the proposal to a reviewer alone with `start_worker`. |
| `already_in_plan` | The proposal or task is in an open review plan. Start the plan with `start_worker` and `plan`, or take it out with `change_review_plan`. |
| `review_plan_not_open` | The plan started or ended, or its wait ended so it takes no more tasks. Read it with `get_review_plan`. |
| `review_plan_not_found` | Call `list_review_plans` to see the plans. |
| `review_plan_denied` | Only the review lead makes and adds to a review plan; the main agent only removes members or closes it, when the user tells it to. Tell the review lead with `send_message` which proposals belong together. |
| `update_checks_running` | The author's update or a reviewer's fix of the proposal runs the checks of its next version (`details.purpose` says which), so its current version is not approved or integrated now. Wait for `checks.finished` with `wait_for_events`, then call `get_proposal` and review the new version. When the checks failed, no new version exists, and the current version can be approved and integrated again. |
| `invalid_intent` | Write the intent again: a `summary` of at most 1200 characters and 1 to 12 `parts`, each with a one-line `name` of at most 80 characters, what it `does` in at most 800 characters and its `files`, so every changed file is in exactly one part; `contracts`, `decisions` and `not_changed` entries are one line of at most 300 characters, at most 20 per list. `details.field` names the part of the intent and `details.reason` says what was wrong: `missing`, `empty`, `too_long`, `control_character`, `no_parts`, `too_many_parts`, `too_many_entries`, `duplicate_part`, `file_in_two_parts`, `part_without_files`, `part_files_uncovered` (add the files in `details.uncovered` to their parts) or `part_file_outside_diff` (remove the files in `details.outside`). Then call the same tool again with `intent`. |
