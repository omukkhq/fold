---
name: fold
description: Work in a fold project. Use it for tasks, workspaces, proposals, reviews, requests and messages: register an agent, find and claim tasks, edit files in a task workspace, open and review proposals, integrate them, turn requests into tasks, and read and send messages through the fold MCP tools. After loading it, read the part of your role next to it.
---

# fold

fold plans, builds, reviews and integrates the work of this project. Every action goes through the tools of the `fold` MCP server, and the daemon decides what each tool may do. A task has a status that fold calculates from its dependencies, claims, workspaces and proposals. You never set a status.

The skill is split into parts, so that each agent reads only what its role needs. After you load this skill, read the part of your role next to this file in full before your first fold step. Each part ends with the shared rules of every role: rights, messaging, noticing news, mentions, requests, subagents, recovery and the Never list.

## Parts

- `MAIN.md`: the main agent. Orchestrating, the task graph and the task template, drafting requests, task kinds, the worker agents, building the tasks of `main`, reviewing, the review scope and integration.
- `AREA_LEAD.md`: an area lead. Its own steps, the task graph and the task template, drafting requests, task kinds and the worker agents.
- `REVIEW_LEAD.md`: the review lead. Its own steps, the review scope, integration and the worker agents.
- `TASK_WORKER.md`: a task worker. Its own steps, building a task and the task kinds.
- `REVIEWER.md`: a reviewer. Its own steps, reviewing, the review scope and integration.
- `ERRORS.md`: every agent, when the hint of a refusal does not settle the next step. Every error code with what to do.
- `REFERENCE.md`: the main agent and the leads, and every agent whose work needs a format or a command.

`REFERENCE.md` next to this file holds what surrounds this workflow, with exact formats and examples: `fold-checks.toml`, `.fold/config.toml` and role prompts, area profiles and the area `main`, ignore files, the human's `fld` commands, the web app's screens and an index of the tools by purpose. A check that must run in both stages takes `stage = ["proposal", "integration"]` and keeps one name. Read its section before you write or review checks, plan config or an area profile, or tell the user what to run.

## Identity

The user starts the main agent in the project folder with `fld claude`. It starts the daemon when needed, then the user's own Claude Code session as the agent `main` with the `main` tag. `fld claude` continues the last main session, also after `/clear` or `/resume` inside it, and `fld new` starts a new one. `fld claude --name <name>` picks another name, and the arguments after `--` go to `claude`. `fld claude` appends the main agent prompt to the session's system prompt on every start and resume: the prompt that ships with fold, or `.fold/roles/main.md` when the project has one; fold adds its shared rules and its default engineering practices after either. A system prompt flag of the user's own after `--` replaces it. fold starts every other agent itself: the leads, the task workers and the reviewers, each with its agent name and the prompt of its role (see the Worker agents section of `MAIN.md`). A session opened in the project by hand has no agent name; it calls `register_agent(name, tags?)` first. It has the fold tools when fold is in the user's own Claude Code config. `fld setup` and the first `fld claude` offer to add fold there, and `fld setup --claude` adds it. `fld claude` and the workers of fold get the tools from fold in all cases.

Every other tool refuses with `agent_not_registered` until the agent is registered. Use `list_agents` to see the agents, their presence, their workers and their claims.

The main agent (the agent with the `main` tag) and the reviewers that the review lead starts may approve and reject a proposal, and they and the review lead may integrate one; the review lead itself never reviews, and the author's agent never may. A session opened without a role and without the `main` tag claims no task and no review, and only comments and requests changes; ask a human or the review lead to approve. A named human may also approve, reject and integrate.

## Sessions opened by hand

A session opened by hand has no role and no part of its own. It calls `register_agent` first, claims no task and no review, and does none of the main agent's work: fold refuses that with `orchestrator_required`. It may comment on a review and request changes with `submit_review` (`REVIEWER.md` gives the arguments), send and read messages, and create requests. The shared rules at the end of every part apply to it. Never edit the files of the project folder: they are main, and they change only through integration.
