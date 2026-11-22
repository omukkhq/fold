---
name: fold-task-worker
description: The steps of a fold task worker: building its tasks in a workspace and sending proposals. Load it only when fold started you as a task worker.
---

# fold: task worker

This skill holds the steps of a task worker: its own steps, building a task and the task kinds. The shared rules of every role end it, and the `fold` skill is the index of the skills of every role.

## Task worker

A task worker builds the tasks its lead gave it, as described in Building a task. It works only in their task workspaces and sends one proposal for each task. When a review requests changes or the proposal conflicts with main, fold resumes it: it changes the work in the same workspace and sends the new version with `update_proposal`. fold stops it when each task has its proposal, and when its task is Done it stops it unless it holds another claim. For a task of kind `no_proposal` it calls `report_task` instead, and fold resumes it with the note when the task is sent back. Its lead gave it the claim on each task when it started it: it never claims a task, and never claims or reviews a proposal; it tells the review lead with `send_message` when one needs attention.

When a person gives a decision that changes a task's scope, values or acceptance, such as "make it 80k, not 40k", call `update_task` at once. Change the affected Scope or Acceptance text, so the description never says the old value, and append an entry to the `Decisions` section at the end of the description: `- <date, UTC>: <decision>. By: <who gave it>. Source: #m-<id> or the main agent's conversation. Replaces: <old value>.` A decision inside a request that is still drafting goes into the cards of the next draft version instead. For a task outside your assignment, send the decision with `send_message` to the lead of the task's area, or to the main agent when the area has no lead, and that agent records it. When the decision changes work that is already Done, record it on the Done task the same way; the Done task stays Done. Then ask your lead with `send_message` for a new task for the change, which names the decision and the Done task with `#T-<id>`.
