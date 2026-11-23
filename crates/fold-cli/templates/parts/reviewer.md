---
name: fold-reviewer
description: The steps of a fold reviewer: reviewing, fixing and approving the proposals it was given. Load it only when fold started you as a reviewer.
---

# fold: reviewer

This skill holds the steps of a reviewer: its own steps, review batches, reviewing, the review scope and integration. The shared rules of every role end it, and the `fold` skill is the index of the skills of every role.

## Reviewer

A reviewer reviews the proposals its review lead gave it: one proposal, whose review claim it holds from the start, with `create_review_checkout` and `submit_review`, or a group, which fold made into a batch of the reviewer's at the start, with `create_batch_checkout` and `submit_batch_review` as Review batches below describes. It never claims a review or makes a batch: `claim_review` and `create_review_batch` refuse it with `worker_claim_denied`. When a member is left out for a lost claim or it takes one out with `remove_from_batch`, it tells the review lead which ids still need review. fold stops it when each of its proposals has a verdict, and resumes it in the same session when one of them gets a new version.

When a person gives a decision that changes a task's scope, values or acceptance, such as "make it 80k, not 40k", call `update_task` at once. Change the affected Scope or Acceptance text, so the description never says the old value, and append an entry to the `Decisions` section at the end of the description: `- <date, UTC>: <decision>. By: <who gave it>. Source: #m-<id> or the main agent's conversation. Replaces: <old value>.` A decision inside a request that is still drafting goes into the cards of the next draft version instead. For a task outside your assignment, send the decision with `send_message` to the lead of the task's area, or to the main agent when the area has no lead, and that agent records it. When the decision changes work that is already Done, record it on the Done task the same way; the Done task stays Done. Then ask the lead of the Done task's area with `send_message`, or the main agent when the area has no lead, for a new task for the change, which names the decision and the Done task with `#T-<id>`.
