---
name: fold
description: Work in a fold project. Use it for tasks, workspaces, proposals, reviews, requests and messages: register an agent, find and claim tasks, edit files in a task workspace, open and review proposals, integrate them, turn requests into tasks, and read and send messages through the fold MCP tools. A session opened by hand loads it; an agent with a fold role loads the skill of its role instead.
---

# fold

fold plans, builds, reviews and integrates the work of this project. Every action goes through the tools of the `fold` MCP server, and the daemon decides what each tool may do. A task has a status that fold calculates from its dependencies, claims, workspaces and proposals. You never set a status.

This skill is the index of fold's workflow and the skill of a session opened by hand. Each role has a skill of its own, so that each agent loads only what its role needs: load the skill of your role before your first fold step. This skill and each role's skill end with the shared rules of every role: rights, messaging, noticing news, mentions, requests, subagents, recovery and the Never list. A role skill holds only the steps of that role; the rarer procedures are in files next to this one.

## Skills

- `fold-main`: the main agent. Orchestrating, the task graph and the task template.
- `fold-area-lead`: an area lead. Its own steps, the task graph and the task template.
- `fold-review-lead`: the review lead. Its own steps and the review scope.
- `fold-task-worker`: a task worker. Its own steps and building a task.
- `fold-reviewer`: a reviewer. Its own steps, reviewing and the review scope.
- `ERRORS.md`, next to this file: every agent, when the hint of a refusal does not settle the next step. Every error code with what to do.
- `REFERENCE.md`, next to this file: the main agent and the leads, and every agent whose work needs a format or a command.
- `WORKERS.md`, next to this file: the worker agents, stalls and stale claims, for the main agent and the leads.
- `DRAFTING.md`, next to this file: drafting requests, for the main agent and an area lead.
- `KINDS.md`, next to this file: the task kinds `no_proposal` and `human`.
- `BUILDING.md`, next to this file: building a task, for the main agent's tasks of `main`.
- `REVIEWING.md`, next to this file: reviewing and the review scope, for the main agent's own reviews.
- `BATCHES.md`, next to this file: review batches, for the main agent and a reviewer.
- `INTEGRATION.md`, next to this file: integration of proposals, batches and serial lanes.
- `PROJECT_FOLDER.md`, next to this file: the human's changes in the project folder, excluded paths and shared paths, for the main agent.
- `SHARED.md`, next to this file: the shared rules in full, for every agent.

Each role skill names the files next to this one that its role reads, with when to read each.

## Sessions opened by hand

A session opened by hand has no role and no skill of its own but this one. It calls `register_agent` first, claims no task and no review, and does none of the main agent's work: fold refuses that with `orchestrator_required`. It may comment on a review and request changes with `submit_review`, whose arguments the Reviewing section of `.claude/skills/fold-reviewer/SKILL.md` gives, send and read messages, and create requests. The shared rules below apply to it.
