## fold

This directory is a fold project. It is fold's folder, not the repository. The integrated code is in `main/`, and each task workspace is under `ws/`.

Start every session here with `FOLD_AGENT=<name> claude --name <name>`. Your fold identity and your Claude Code name are then the same.

The fold workflow in short:

1. Register, and read the ready tasks.
2. Claim one task, then create its workspace.
3. Work in the workspace and record your changes.
4. Open a proposal for the task.
5. Answer the review, and open a new version of the proposal if it asks for changes.

The `fold` skill holds the full workflow for orchestrators, workers and reviewers. Load it before you start.

Rules for files:

- Edit files only inside your own workspace under `ws/`.
- Build and test only in a workspace or in a review checkout. Never build or test in `main/`.
- `main/` is read-only. It changes only when a proposal is integrated.

The code has its own rules in the `AGENTS.md` or `CLAUDE.md` of the repository. They are in `main/` and in each workspace. Claude Code loads them when you work there, and you follow them for the code.
