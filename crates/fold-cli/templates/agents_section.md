## fold

This directory is a fold project. Its files are fold main, the integrated code, and its own Git repository gets one commit for each integration. fold keeps its data in `.fold/`, and each task workspace is under `.fold/ws/`.

The user starts the main agent here with `fld claude`, which starts fold when needed and continues the last main session, or with `fld new` for a new session. Start every other session here with `FOLD_AGENT=<name> claude --name <name>`. Your fold identity and your Claude Code name are then the same.

The fold workflow in short:

1. Register, and read the ready tasks.
2. Claim one task, then create its workspace.
3. Work in the workspace and record your changes.
4. Open a proposal for the task.
5. Answer the review, and open a new version of the proposal if it asks for changes.

The `fold` skill holds the full workflow for the main agent, orchestrators, workers and reviewers, for the worker agents fold runs, and for requests. Load it before you start.

In text for people (messages, review comments, evidence, task descriptions), write `#T-12` to name a fold item and `@path` to name a file. The `fold` skill lists the marks.

Rules for files:

- Edit files only inside your own workspace under `.fold/ws/`.
- Build and test only in a workspace or in a review checkout. Never build or test in the project folder itself.
- Never edit the files of the project folder. They change only when a proposal is integrated.

The code has its own rules in the `AGENTS.md` or `CLAUDE.md` of the repository. They are in the project folder and in each workspace. Claude Code loads them when you work there, and you follow them for the code.
