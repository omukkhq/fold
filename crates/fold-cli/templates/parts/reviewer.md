---
name: fold-reviewer
description: The steps of a fold reviewer: reviewing, fixing and approving the proposals it was given. Load it only when fold started you as a reviewer.
---

# fold: reviewer

This skill holds the steps of a reviewer: its own steps, reviewing and the review scope. Files next to the `fold` skill hold review batches and integration; the On-demand files section below names each one and when to read it. The shared rules of every role end it, and the `fold` skill is the index of the skills of every role.

## Reviewer

A reviewer reviews the proposals its review lead gave it: one proposal, whose review claim it holds from the start, with `create_review_checkout` and `submit_review`, or a group, which fold made into a batch of the reviewer's at the start, with `create_batch_checkout` and `submit_batch_review` as `.claude/skills/fold/BATCHES.md` describes. It never claims a review or makes a batch (`worker_claim_denied`). When a member is left out for a lost claim or it takes one out with `remove_from_batch`, it tells the review lead which ids still need review. fold stops it when each of its proposals has a verdict, and resumes it in the same session when one of them gets a new version. When it asks the humans about a proposal with `send_message` and that proposal, it ends its turn instead of waiting with `wait_for_events` or `get_thread`: while every proposal it still owes a verdict waits for such an ask, fold parks it, its claim does not go stale and its review lead hears nothing, and the human's answer in the thread resumes it in the same session.

When a person gives a decision that changes a task's scope, values or acceptance, such as "make it 80k, not 40k", call `update_task` at once. Change the affected Scope or Acceptance text, so the description never says the old value, and append an entry to the `Decisions` section at the end of the description: `- <date, UTC>: <decision>. By: <who gave it>. Source: #m-<id> or the main agent's conversation. Replaces: <old value>.` A decision inside a request that is still drafting goes into the cards of the next draft version instead. For a task outside your assignment, send the decision with `send_message` to the lead of the task's area, or to the main agent when the area has no lead, and that agent records it. When the decision changes work that is already Done, record it on the Done task the same way; the Done task stays Done. Then ask the lead of the Done task's area with `send_message`, or the main agent when the area has no lead, for a new task for the change, which names the decision and the Done task with `#T-<id>`.

## On-demand files

These files are next to the `fold` skill, in `.claude/skills/fold/`. Read a file when its work comes up, before the first step of that work, and follow it as part of this skill.

- `BATCHES.md`: review batches. Read it before your first step when fold gave you a batch, and before you call `create_batch_checkout`, `submit_batch_review`, `remove_from_batch`, `list_review_batches` or `get_review_batch`. It covers `batch_conflict`, `left_out` and `unattributed_lines`.
- `INTEGRATION.md`: how fold integrates proposals, batches and serial lanes. Read it before you call `integrate_proposal` or `integrate_review_batch`, and when an integration conflict or a serial lane comes to you.
- `SHARED.md`: the shared rules in full. Read it before you ask a human or acknowledge a message, and when a request, a subagent or a recovery needs more than the shared rules below say.
