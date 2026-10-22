---
name: fold
description: Work in a fold project. Use it for tasks, workspaces, proposals, reviews, requests and messages: register an agent, find and claim tasks, edit files in a task workspace, open and review proposals, integrate them, turn requests into tasks, and read and send messages through the fold MCP tools. A session opened by hand loads it; an agent with a fold role loads the skill of its role instead.
---

# fold

fold plans, builds, reviews and integrates the work of this project. Every action goes through the tools of the `fold` MCP server, and the daemon decides what each tool may do. A task has a status that fold calculates from its dependencies, claims, workspaces and proposals. You never set a status.

This skill is the index of fold's workflow and the skill of a session opened by hand. Each role has a skill of its own, so that each agent loads only what its role needs: load the skill of your role before your first fold step. This skill and each role's skill end with the shared rules of every role: rights, messaging, noticing news, mentions, requests, subagents, recovery and the Never list.

## Skills

- `fold-main`: the main agent. Orchestrating, the task graph and the task template, drafting requests, task kinds, the worker agents, building the tasks of `main`, reviewing, review batches, the review scope and integration.
- `fold-area-lead`: an area lead. Its own steps, the task graph and the task template, drafting requests, task kinds and the worker agents.
- `fold-review-lead`: the review lead. Its own steps, the review scope, integration and the worker agents.
- `fold-task-worker`: a task worker. Its own steps, building a task and the task kinds.
- `fold-reviewer`: a reviewer. Its own steps, review batches, reviewing, the review scope and integration.
- `ERRORS.md`, next to this file: every agent, when the hint of a refusal does not settle the next step. Every error code with what to do.
- `REFERENCE.md`, next to this file: the main agent and the leads, and every agent whose work needs a format or a command.

## Sessions opened by hand

A session opened by hand has no role and no skill of its own but this one. It calls `register_agent` first, claims no task and no review, and does none of the main agent's work: fold refuses that with `orchestrator_required`. It may comment on a review and request changes with `submit_review`, whose arguments the Reviewing section of `.claude/skills/fold-reviewer/SKILL.md` gives, send and read messages, and create requests. The shared rules below apply to it.
