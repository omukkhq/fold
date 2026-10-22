---
name: fold-reviewer
description: The steps of a fold reviewer: reviewing, fixing and approving the proposals it was given. Load it only when fold started you as a reviewer.
---

# fold: reviewer

This skill holds the steps of a reviewer: its own steps, reviewing, the review scope and integration. The shared rules of every role end it, and the `fold` skill is the index of the skills of every role.

## Reviewer

A reviewer reviews the proposals its review lead gave it: one proposal, whose review claim it holds from the start, with `create_review_checkout` and `submit_review`, or a group, which fold made into a batch of the reviewer's at the start, with `create_batch_checkout` and `submit_batch_review` as below. It never claims a review or makes a batch: `claim_review` and `create_review_batch` refuse it with `worker_claim_denied`. When a member is left out for a lost claim or it takes one out with `remove_from_batch`, it tells the review lead which ids still need review. fold stops it when each of its proposals has a verdict, and resumes it in the same session when one of them gets a new version.

1. To review a batch, call `create_batch_checkout(id)`: a checkout of main with every member merged in order. A member that conflicts with another gives `batch_conflict`. Take it out with `remove_from_batch(id, proposal)`, which gives up its review claim, and tell the review lead its id. Build and test the checkout once.
2. Call `submit_batch_review(id, verdicts[], comments[]?)` with one verdict for each member. Inspect `reviewed` and `left_out`: new versions, closed proposals and lost claims can leave verdicts unwritten. Read omitted proposals again: review the current version of a member that still has your claim with `submit_review`, and return the ids of the others to the review lead. A successful call does not mean every verdict was stored. A member you fixed with `submit_review_fix` has a new version, so the batch review leaves it out; review that version with `submit_review`. Batch line comments go to the member that brought the lines; other lines give `unattributed_lines`. Use `list_review_batches(open?)` and `get_review_batch(id)` to recover existing batches.
