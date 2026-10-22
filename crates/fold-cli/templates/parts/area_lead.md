# fold: area lead

This part of the fold skill holds the steps of an area lead: its own steps, the task graph and its task template, requests, task kinds and the worker agents. The shared rules of every role end it, and `SKILL.md` next to this file is the index of the parts.

## Area lead

The area lead owns the tasks with its area's label. It plans and starts task workers, writes no code, and never claims or reviews a proposal: it tells the review lead with `send_message` when a proposal needs attention. It gives a task to a task worker with `start_worker`, which claims the task for the worker, and does not call `claim_task` itself to give work: the start moves a claim the lead holds to the new worker.

1. Read the tasks of the area with `get_task_graph` and `get_ready_tasks`. Split a large task with `create_task` and `add_dependency`, and give each new task the area's label and no other area label.
2. Start a `task_worker` for ready work with `start_worker(role, tasks[], model?, instructions?)`; fold gives it the claim on each task in the same step. Choose the model and write the instructions for the tasks. Leave `model` out to use the model of your area's profile. fold sends you a notice when a task of the area becomes ready and nobody holds its claim.
3. Watch the task workers with `list_workers`. fold stops each one when its work is done, and resumes it when its proposal needs changes. When its task is Done, fold stops it unless it holds another claim.
4. For work that changes no project file, create the task with kind `no_proposal` and start a task worker for it. When the task worker calls `report_task`, fold sends you a notice. Read the report with `get_task`, check the work, and call `complete_task(id)` or `send_back_task(id, note)`. A send-back resumes the same task worker with your note.
5. When a task of the area needs work that only a person can do, create a task of kind `human` with the area's label and make the waiting tasks need it. When a task must wait for a person's word, give it a `blocker` in `create_task_plan`, or call `set_blocker(id, reason)` before you start a task worker for it; fold refuses a blocker on a task an agent has started. Never start a task worker for a human task or a task with a blocker. You cannot clear a blocker or mark a human task Done: send the reason to a human with `send_message`. fold sends you a notice when a blocker is cleared and the task is ready.
6. When your context grows large, call `write_handoff(text)` when fold asks for it, then end your turn.
7. When fold tells you that a human change in main touches files that a task of your area also changes, tell its task worker with `send_message` to call `sync_workspace` before it records again.
8. When the review lead asks you to start a task next because a review plan waits for its proposal, start a task worker for it before your other ready tasks, within the worker limits, or tell the review lead why you cannot.
