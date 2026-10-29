# fold reference

This file sits next to the `fold` skill and holds the formats and commands around the workflow: the checks file, the project config, area profiles, ignore files, the human's `fld` commands, the web app and an index of the fold tools. Read the section you need before you plan checks, change config, define areas or tell the user what to run. `fld init --refresh` writes this file again when it is missing.

## Checks

`fold-checks.toml` at the root of the project's tree names the commands that must pass before work reaches main. Each tree's own copy decides, so a proposal changes its checks together with its code. A project without the file has no fold checks.

```toml
# Runs before the checks of every run, in the directory where they run.
[setup]
run = "bun install --frozen-lockfile"

[[check]]
name = "rust-tests"
run = "cargo test --workspace"
files = '\.rs$|(^|/)Cargo\.(toml|lock)$'

[[check]]
name = "web"
run = "cd web && bun run test"
files = '^web/'

[[check]]
name = "lint"
run = "cargo clippy --workspace -- -D warnings"
required = false
timeout = "10m"

[[check]]
name = "acceptance"
run = "make acceptance"
stage = "integration"
timeout = "2h"

[[check]]
name = "build"
run = "cargo build --workspace"
stage = ["proposal", "integration"]
```

Keys. Every key is known, and a misspelt key refuses the whole file with `checks_file_invalid`, whose message names the line or the check:

- `[setup]` with `run`: one shell command that runs first in every run, such as installing dependencies. A failed setup fails the run with `setup_failed`, and no check runs (`not_run`).
- `[[check]]`, at most 64, each with:
  - `name`: 1 to 64 letters, digits, `_`, `-` or `.`, unique in the file.
  - `run`: the shell command, at most 4000 characters. It runs as `sh -c` in the check directory, with stdin closed and without fold's agent and human variables. A fold call from inside a check acts only as the agent of the worker that asked for the run, never as a human.
  - `files`: optional. A regular expression, not a glob, searched anywhere in each changed path; `'^web/'` matches the folder `web` at the root and `'\.rs$'` every Rust file. A check whose pattern matches no changed path is `skipped`. Without it the check always runs.
  - `required`: optional, `true` by default. A check with `required = false` shows its result and never blocks.
  - `stage`: optional, `proposal` (the default), `integration`, or a list such as `["proposal", "integration"]` to run the check in both stages under one name. A string is a list of one. An empty list, an unknown name or a repeated name refuses the file.
  - `timeout`: optional, a whole number with `s`, `m` or `h`, such as `"90s"`, `"10m"` or `"2h"`, from 1s to 24h. The default is 30 minutes. fold stops a command's whole process group at its timeout (`timed_out`), and at its end, so a server that a test started ends with it.

The file is at most 256 KiB. Prefer one check per tool or part of the repository with a `files` pattern, so a change of the docs runs no test suite.

Stages:

- `proposal` checks run on the files of a version before fold makes it. A task worker asks with `run_checks(task)`; `create_proposal` and `update_proposal` run them when no passed run of the same files on the same base stands, and answer with `checks` `running` and a run id; a reviewer's `submit_review_fix` runs them in its review checkout. They run in the author's task workspace or the reviewer's checkout, and the changed paths are those of the change against the workspace's base. A version comes into being only when the required checks pass. A human's edit in the web app, `fld record` and `submit_main_change` run no proposal checks.
- `integration` checks run on the merged tree, after the approvals and before fold publishes the revision, under the project's integration lock, so main cannot move meanwhile. They run in `.fold/checks/main`, fold's own copy of main that keeps what the setup installs between runs and follows main after every revision. Only the integration checks whose pattern matches a path the merge changes on main run. A failure publishes nothing and sends the proposal back to its author, whose task worker fold resumes. `fld record` and `submit_main_change` run no integration checks.

Runs and results. A run is `running`, then `passed`, `failed` or `stopped`; each command is `passed`, `failed`, `timed_out`, `skipped` or `not_run`. A failed or stopped run names its reason: `check_failed`, `setup_failed`, `checks_file_invalid`, `files_changed` (a check or an edit changed the recorded files during the run, so put what a check writes in `.gitignore`), `workspace_changed`, `refused` (the command the run waited for is refused now), `daemon_stopped`, `superseded` (an integration started again on a newer state) or `fold_failed` (fold itself could not run the checks; the run says nothing of the change, and the proposal keeps its version and approvals). fold sends the result as a message from fold to the worker that asked, with the failed checks and the end of the first failed log; `get_check_run(run)` returns the whole run with each log's last 64 KiB, and `get_proposal` lists the runs of a version in `checks`. The events are `checks.started` and `checks.finished`.

Queue and load. One run at a time goes in a workspace; a second gets `checks_running`. Integrations of a project wait in a queue in the order they were asked for. All projects of the daemon share at most half the processors' worth of check runs at once.

Changing the checks. A proposal version that adds, changes or removes `fold-checks.toml` gives its task the human review mark when it has none, so a human approves every change of the checks and no agent can weaken one without the user. Write the first file as a task of the area `main` or as a main workspace change with the user's OK (`submit_main_change`, where that OK is the approval). When main's own file is not valid, fold holds every integration, sends `integration.held` and lists the problem in Needs you under Project; a revision of main that fixes it, from `submit_main_change`, `fld record` or a proposal that fixes the file, lets the held integrations go on.

## Project config

`.fold/config.toml` holds the project's settings. `fld init` writes it with every value commented out at its default. It is outside the project's tree, so agents never edit it: tell the user the exact lines to change. `set_worker_limits` rewrites only `[workers]`. fold reads the file when it uses a value, so most edits apply at once; the watcher reads `debounce_ms`, `rescan_seconds` and `max_file_bytes` when the daemon opens the project, so an edit of them reaches it after `fld restart`. An invalid file fails the commands that read it with `invalid_config`, which names the key.

```toml
lease_minutes = 30                   # a claim expires after this long without a heartbeat
debounce_ms = 500                    # the watcher waits this long after a file change before it scans
rescan_seconds = 60                  # time between full rescans of a workspace
auto_checkpoint_quiet_seconds = 120  # quiet time before an automatic checkpoint
auto_checkpoints_kept = 20           # automatic checkpoints each workspace keeps
max_file_bytes = 52428800            # a larger file keeps its recorded version and gives a warning
stall_minutes = 5                    # a claimed task without a file change for this long counts as stalled; never while its workspace runs checks, and counted from the end of its last run
closed_workspace_days = 14           # how long `fld gc` keeps the checkpoints of a closed workspace, a record only

[git]
branch = "main"                      # the branch of the project folder's .git that every integration commits to

[remotes.origin]                     # one section per remote; `fld remote add` writes it
url = "git@example.com:team/app.git"
branch = "main"
push_on_integrate = false            # true pushes main to this remote after every integration

[integration]
automatic = true                     # false: an agent that decides reviews, or a human, integrates each ready proposal

[workers]
task_worker = 4                      # task workers running at once
reviewer = 3                         # reviewers running at once
max_per_area = 2                     # optional: task workers of one area at once

[main]
supervised = false                   # true: fold runs the main agent as a supervised worker of role main, for a server where nobody runs `fld claude`

[roles.task_worker]                  # also area_lead, review_lead, reviewer and main
model = "sonnet"                     # passed to Claude Code as --model
cache_ttl = "5m"                     # "1h" or "5m": "1h" for leads and "5m" for reviewers and task workers by default
idle_minutes = 15                    # task_worker and reviewer only: an idle worker stops after this long
mcp = ["fold", "pencil"]             # the servers the role may load: fold and servers of the project's .mcp.json

[roles.review_lead]
handoff_at = 50                      # area_lead, review_lead and main only: the percent of the context window past which fold refreshes the lead through a hand-off note; without it, 85% of the window below 220k tokens and 60% from 220k
```

- The top-level numbers must be whole numbers above 0. A worker limit has no upper bound, and 0 lets no worker of its role start; the leads are outside the limits, and a lower limit never stops a running worker.
- `automatic = true` integrates a proposal once a reviewer, the main agent or a human approved it, nothing asks for changes, a human approved it when the task is marked, and every prerequisite task is Done.
- A role without `mcp` loads every server the user's own Claude Code loads. With `mcp`, the worker gets the fold server and each named server of the project's `.mcp.json`; servers of the user's own configuration are never copied.
- An older file with `remote` and `push_on_integrate` in `[git]` is read as `[remotes.origin]` on the `[git]` branch.
- `supervised = true` under `[main]` makes fold run the main agent itself: it starts it at the daemon's start, keeps its session, and resumes it for a message to it, a request, a ready task of `main` or a review it must act on, never while the project is paused. The humans talk to it in the web app, and `fld claude` refuses with `main_supervised`. While a `fld claude` session is the main agent, fold starts no supervised one (`main_interactive`), and once that session ends fold starts it. A human's Stop on the Agents screen holds the supervised main: fold starts it for no work until a human starts it in a new session.

Role prompts. `.fold/roles/<role>.md` (`area_lead`, `review_lead`, `task_worker` or `reviewer`) replaces the prompt that ships with fold for that role, and `.fold/roles/main.md` replaces the main agent's; fold adds its shared authority and recovery rules after either. For the supervised main, fold first adds its rules of a main agent without a terminal after `.fold/roles/main.md`. A worker gets a changed prompt at its next start.

## Areas

An area is a part of the project that one area lead owns, with one label that every task of the area carries. Make the label with `create_label`, then `define_area(name, label, instructions?, model?, mcp?)`; change it with `update_area(name, instructions?, model?, mcp?, clear_instructions?, clear_model?, clear_mcp?)`. The profile:

- `instructions`: at most 8000 characters, added to the system prompt of the area's lead and task workers under `## Area <name>`. Name the folders the area owns and what matters for its work. They specialize work inside the role and never widen its rights or the approved scope.
- `model`: a Claude Code model name or alias without spaces, such as `opus` or `sonnet`.
- `mcp`: a list of distinct server names, never empty. The fold server is always there, and `["fold"]` gives the workers no other server.

A field left out falls back to `[roles.<role>]` of `.fold/config.toml`, and the `model` and `instructions` of a `start_worker` call come before the profile. Reviewers take no area profile. fold writes the prompt at each start, so a changed profile reaches the next start and never a running worker.

```text
create_label(name: "design")
define_area(name: "design", label: "design",
  instructions: "You own the screens in design/. Draw each screen in Pencil; change no code.",
  model: "opus", mcp: ["fold", "pencil"])
```

The built-in area `main` with the label `main` holds the tasks only the main agent builds, such as `fold-checks.toml`, the root `.gitignore` or the notes in `AGENTS.md` before code lands. It has no lead and no profile: `define_area` refuses its name and label with `area_exists`, and `start_worker` and `update_area` refuse it with `main_area`. Each of its tasks gets the human review mark unless the approved plan gives it `main_approves`. When a task leaves `main`, fold takes off the mark it set there, and a mark a person set stays.

## Ignore files

- `.gitignore` and `.foldignore` are files of the tree, in any folder, in the format of Git's ignore files. As in Git, they apply only to paths the tree does not track yet: fold never starts to track, record or propose a path they ignore, but a tracked file stays tracked when a rule matches it, so its changes and its deletion are recorded. A tracked file leaves main only when a change deletes it, or when the user runs `fld untrack <path>`, which removes it from main and leaves it on disk. Each applies below its own folder; a `.foldignore` rule wins over a `.gitignore` rule, so a `.foldignore` can ignore more for fold alone or bring a path back with `!`. `fld init` writes an empty `.foldignore` when the folder has none.
- `.fold/exclude` holds paths that stay out of fold and Git on this machine only, such as local notes or tool output; it is never part of the tree. `.git/info/exclude` is a link to it, so Git and fold leave out the same paths, and it always lists `/.fold/`. The user edits it, or `add_exclude_patterns(patterns)` adds `.gitignore` lines when the user asks. A `!` line of `.gitignore` or `.foldignore` brings an excluded path back.
- fold keeps its own lines in `.fold/exclude`: the comment `# fold writes this file. To keep a path, add a ! line to .foldignore.` at the top, and after the user's lines `/.claude/skills/fold/`, `/.claude/skills/fold-area-lead/`, `/.claude/skills/fold-review-lead/`, `/.claude/skills/fold-task-worker/`, `/.claude/skills/fold-reviewer/`, `/.claude/skills/fold-main/` and `/.claude/settings.local.json`. fold writes its skills into the project folder and writes them again after each update, and every session reads them from the project root, so they are never part of main; `.claude/settings.local.json` holds one person's Claude Code settings. The rest of `.claude`, such as `settings.json` and the project's own skills, agents and commands, is tracked as usual, also a skill of the project whose name starts with `fold-`. Each start of the daemon adds back a line of fold's that the file does not hold, so removing a line does not keep a path: a `!` line of `.foldignore` for its folder or file, such as `!/.claude/skills/fold/` or `!/.claude/settings.local.json`, keeps it tracked. When main still tracks files below fold's skill folders that the rules leave out, the daemon's next start of the project makes one revision of `fold` titled "Stop tracking fold's skill files" that removes them from main and leaves them on disk. The revision waits for a later start of the daemon while the project folder is not ready: main holds paths the user changed, the folder has not followed main, `HEAD` or the Git branch shows a change fold did not make (`head_moved`), commits of the user wait on the branch, a stash conflict or an unstash that stopped part-way waits, a change of the user waits for the main agent, or main or the folder's record changes the skill files while fold makes the revision. Meanwhile `fld status` names the skill folders main still tracks and the `fld untrack` command that stops tracking them at once. While Git sync is on, the revision is one commit on the `[git]` branch that removes the files from the branch and the index and leaves them in the work tree; a teammate's pull of that commit removes them from the teammate's folder until fold writes them there again. A tracked `.claude/settings.local.json` stays tracked until the user runs `fld untrack .claude/settings.local.json`.
- `.git` and `.fold` are never part of the tree. With Git sync on, `.gitignore` holds a `.fold/` line.
- A check's output, build folders and dependency folders belong in `.gitignore`, or the run fails with `files_changed`.

## Commands

The user runs these in a terminal; agents never do. Commands that act on a project take the project from the current folder, or from `--project DIR` where it is offered, and most take `--socket` for a daemon on another socket. `fld <command> --help` shows every flag.

Start and set up:

- `fld` with no command: print fold's own state (not the project folder, which `fld status` shows): the fld version, whether the daemon runs, its web address and socket, whether a service runs it, and the project of the current folder. It never starts the daemon, and it names `fld restart` when the daemon runs another version.
- `fld init [DIR] [--name NAME] [--with-git] [--git-name NAME --git-email EMAIL]`: make the folder a fold project and register it. Without `DIR` the current folder; a new `DIR` is created. `--with-git` turns Git sync on: in a Git repository its `HEAD` commit becomes the genesis, else fold creates `.git`, and every revision becomes a commit on the `[git]` branch. `fld init --refresh [DIR]` rewrites fold's Claude Code files (`AGENTS.md` or `CLAUDE.md` section, the skill and this file) in an existing project.
- `fld setup [--no-start] [--claude] [--remove]`: install the service that starts the daemon at login (a systemd user unit on Linux, a launchd agent on macOS) and start it, then offer to add fold to the user's Claude Code config. `--claude` only adds fold to Claude Code; `--remove` takes fold's entries out of it.
- `fld claude [--name NAME] [-- ARGS]`: start the daemon when needed and continue the main agent's last session, or start one. `fld new` (also `fld new claude`) starts a new main session. A project with `supervised = true` under `[main]` refuses it with `main_supervised`.
- `fld serve [--listen ADDR] [--socket PATH] [--web-dir DIR]`: run the daemon in the foreground; the service runs it the same way.
- `fld mcp`: the stdio connection of a Claude Code session to the daemon, which Claude Code starts itself.
- `fld restart`: restart the daemon, so a newly installed fld takes over; supervised workers stop and continue in their sessions. A session keeps the skill it loaded, so the next turn of a worker whose session loaded another fld's skill text starts with a note to load its role skill again; a new fld with the same skill text sends no note.
- `fld doctor [--project DIR]`: check the host, the daemon and each project, and name the fix for each problem.

Work in the project folder:

- `fld record`: record the user's changes in the project folder on main as one change of the user's. A conflict with the agents' work waits for the main agent in Needs you.
- `fld untrack <path>...`: stop tracking files of the project folder, like `git rm --cached` with its commit: one revision of the user removes every tracked path at or below each named path from main and leaves the files on disk. Other unrecorded changes stay unrecorded. Older revisions keep the files, so a leaked secret still needs to be changed. A file that no ignore rule matches is recorded again by the next `fld record`. A path main does not track is refused with `path_not_tracked`.
- `fld stash`: put the user's unrecorded changes aside, so the folder follows main again. `fld stash list` shows the entries, newest first.
- `fld unstash`: merge the newest stash entry back into the folder; a conflict writes conflict markers and keeps the entry until `fld record`. fold notes the planned writes before the first one, so an unstash that stopped part-way is finished by the next `fld stash`, `fld unstash` or `fld record`, which names it in its answer; a path the user changed after the stop stays as it is.
- `fld status [--json]`: show the project folder against main and change nothing: main's head and the revision the folder holds, each file `fld record` would record now (added, modified, deleted or a type change between a file and a symlink) with those main changed too, what keeps the folder from following main (a Git branch fold did not move, Git commits fold has not recorded, an unstash that stopped part-way, an unstash conflict, a conflicting change that waits for the main agent, files held by main) with the next step for each, the number of tracked files that an ignore rule matches, listed only when one of them has a change (`--json` always lists them), and the stash with the entry `fld unstash` takes next. `--json` prints the daemon's answer.
- `fld pause` and `fld resume`: stop every worker of the project, keeping its session and claims, and run them again.
- `fld worker restart NAME [--instructions TEXT]`: start the worker `NAME` of any role again in a new Claude Code session, for example when its session is broken; it keeps its name, claims and workers, and its old session id stays in its previous sessions.
- `fld log [-n N] [--oneline]`: list main's revisions, newest first, as the web app's repository screen shows them: a line with the revision id and its title, then lines that start with `↳`: main's head marked, the proposal with its task and author or what the revision is (a pull, a direct change, a record or the genesis), and the time in the user's local time zone with the Git commit; a blank line separates revisions. `-n` sets how many (20 by default, at most 1000); `--oneline` prints only the id and the title.
- `fld cost [--since WHEN] [--by SPLIT] [--task T-N] [--json]`: print what the project's supervised workers cost, from the values Claude Code reported: the total of all time, today and the last 7 days (local days, from midnight in the terminal's time zone), the tokens (cache read, cache write, output and input), then by role, by area and the top tasks with their build and review cost and their tokens. `--since` takes today, 12h, 7d, a date (from its local midnight) or an RFC 3339 time; `--by` prints one split in full (role, area, model, worker or task); `--task` prints one task's cost per worker; `--json` prints the daemon's answer. The main agent's own session is not supervised, so its cost is not counted.
- `fld transcript AGENT [--tail N]`: print the path of the agent's Claude Code transcript, the file of its current session: the main agent's recorded session, or the session fold runs for a worker. `--tail N` prints the file's last N lines instead; fld reads them itself, and the daemon never opens a transcript.

Maintenance and access:

- `fld gc [--dry-run]`: delete unreferenced objects, prune automatic checkpoints beyond `auto_checkpoints_kept`, remove the closed workspace directories that are still there, and prune the checkpoints of workspaces closed longer than `closed_workspace_days`. fold removes a workspace's directory as soon as the workspace closes and its owning agent has no running worker process and no open session, after a final checkpoint of the tracked files it had not recorded (a review checkout takes none). That checkpoint is a record, not a restore point: a closed workspace is never restored. `fld gc` retries a removal that failed. `--dry-run` only counts.
- `fld web`: print a one-time link that signs the user in to the web app in a browser on this machine. The link works once, for a few minutes; fld prints when it expires and starts the daemon when needed.
- `fld token create USERNAME [--label TEXT]`: create a sign-in token for the web app for the person with the username `USERNAME`, for a browser that cannot open a link from `fld web`, such as one on another machine, and print it once. The daemon makes the token and starts when needed. `fld token list` shows the tokens without their secrets, and `fld token revoke ID` (such as `wt-3`) ends every session made from one.
- `fld web` and `fld token create` refuse a process that a fold worker or a fold check started, as fold refuses such a process that names a human. A `fld claude` session, such as the main agent in the user's own terminal, is the user's trusted terminal: it and the commands it runs can sign in as the user, while the workers that fold starts cannot. A process that left a worker's process tree, and any process of the user, can still read and write the user's files, fold's own files too, because agents run as the user's OS user. A browser sends the sign-in cookie to every port of the same host, so a local development server opened in the same browser, such as one that an agent started, can receive it. For full isolation, run the agents as a separate OS user or in a sandbox.
- `fld subscribe [EMAIL]`: subscribe an address to fold release news.

Notices (webhooks, Slack and Discord):

- `fld notify` (or `fld notify list`): list the notice targets that get the project's notices: its own and the host's while `fld notify host-targets` is on (the default), each with its id (such as `nt-3`), name, kind, URL without its path, events, card-move columns, window and health, then the host switch and the web address of links.
- `fld notify add webhook|slack|discord NAME [URL] [--host] [--events EVENTS] [--moves-to COLUMNS] [--window SECONDS]`: add a target. Without `URL`, fld reads it from the first line of standard input, so a Slack or Discord URL, which is the channel's secret, stays out of the shell history. A generic `webhook` gets a signed JSON body (Standard Webhooks: `webhook-id`, `webhook-timestamp`, `webhook-signature`); fld prints its `whsec_` signing secret once. `slack` and `discord` get one message with a line per notice. `--events` is `recommended` (the default), `all`, `needs-you-only` or event names separated by commas; `--moves-to` names the columns whose card moves the target gets (`review,done` by default); `--window` is the seconds after the first notice whose notices go in one send (15 by default, 0 sends each alone, at most 3600). `--host` adds a target of the host, which every project that uses the host targets sends to. A URL is `https`, or `http` only to localhost or a loopback address (`plaintext_notify_target`); fold never follows a redirect.
- `fld notify edit TARGET [--name NAME] [--url-stdin] [--events EVENTS] [--moves-to COLUMNS] [--window SECONDS] [--switch on|off]`, `fld notify remove TARGET`, `fld notify test TARGET` and `fld notify rotate TARGET`: change, remove, test or give a new signing secret to a target named by its id or its name. A test waits for the receiver's answer and goes into the send log; one that gets through turns on a target that fold turned off. After a rotation the old secret signs too for a day.
- `fld notify log TARGET`: the target's newest sends (fold keeps 50): the notices each carried, the status or error of its newest try, the try count and its `webhook-id`, which every try of a send keeps.
- `fld notify web-url [URL | --clear]`: show or set the address people open fold at; links in notices start with it, and without it notices carry no links. `fld notify host-targets [on|off]`: show or set whether the host's targets get the project's notices too.
- The events are `needs_you.opened`, `needs_you.closed`, `task.created`, `task.moved`, `task.blocked`, `task.sent_back`, `proposal.opened`, `review.approved`, `review.changes_requested`, `proposal.integrated`, `proposal.closed`, `integration.failed`, `checks.failed`, `worker.failed`, `git.failed`, `message.to_human`, `request.approved` and `project.paused`; a test sends `webhook.test`. A notice holds ids, titles, columns, names and links, never a message body, review text, diff or file content. A failed send tries again after 10 s, 1 min, 5 min, 30 min, 2 h and 6 h with the same `webhook-id`, honouring `Retry-After`. fold turns the target off when a send fails its last try (about 8.6 hours after its first), after 20 failed tries in a row, or at once on a 410 answer, and puts it into Needs you with the reason; a test send is one try. The Settings tab's Send again resends a finished send only while its target gets the project's notices (`notify_target_inactive` otherwise). The event `notify.target_changed` (`target`, `name`, `host`, `state`: `on`, `off`, `gone`, `failing` or `removed`) records each change of a target by a person and each time fold turns one off or on again. Only a person changes targets, never an agent; the daemon keeps them and their secrets in its own data folder, readable only by the user, and deletes a project's targets when the project leaves fold.

Git remotes (with Git sync on):

- `fld remote add NAME URL [BRANCH]`, `fld remote remove NAME` and `fld remote list` (also plain `fld remote`) manage `[remotes.<name>]`.
- `fld push [NAME]`: push main's history, one commit per revision, without force. The name can be left out when the project has one remote.
- `fld pull [NAME]`: bring the remote's commits into main; a conflict becomes a sync task.

Host config. `$XDG_CONFIG_HOME/fold/daemon.toml` (else `~/.config/fold/daemon.toml`) holds `listen` (the web address, `127.0.0.1:7420` by default), `socket` and `session_days` (how long a web sign-in lasts). The daemon reads it at its start, so an edit needs `fld restart`.

## Web app

The daemon serves the web app at `http://127.0.0.1:7420` by default. Every browser signs in, on loopback too, so a request that only names a person acts as nobody; this does not keep agents away from the user's files, as the access commands above state. A person opens the link that `fld web` prints, or enters a token from `fld token create` on the sign-in screen. Each project has these tabs, and every write a person makes there goes through the same commands as an agent's:

- Board: the five columns of the tasks, Backlog, To do, In progress, Review and Done, with search and the Attention, Labels, Agent and Blocked only filters; a phone shows one column at a time with a column switcher. The board is read only; cards move when fold's state changes. Each card shows one line about its work and at most one chip, and its details on hover or focus. A card with a question for you has Mark answered in its details, for an answer you gave elsewhere. While a check run goes in the holder's workspace, the card's line says Running checks, or Running checks for v1 before a new proposal, or Running checks for v2 when the run makes the proposal's next version, and its chip says Running checks in the neutral tone: no person waits for the run, and the worker is not stalled. A question or a review for you, a blocker, an unfinished prerequisite or a stale claim still shows over it, because a worker that waits for a long run makes no calls and its lease can run out. New task opens a form (title, description, area and labels, who does the work), which suggests a request first.
- Task (a card, in a dialog whose main column and side panel scroll on their own): edit the title, description, area and labels; edit the checklist; add and remove dependencies; set, change and clear a blocker; require or turn off human review; Mark Done on a human task; Complete task or Send back on a reported `no_proposal` task; send feedback to the task's agents; archive the task. It also shows the workspace, checkpoints, changesets, the proposal, the working changes and the task's cost, and its live strip says that the worker runs checks while a check run goes in its workspace.
- Activity: live workspaces, review checkouts and the event stream; a workspace in which a check run goes shows running checks instead of a stall.
- Proposals: the list, with the proposals that wait for your review marked Waits for you, counted on the tab and shown alone by a filter, and each proposal's page with four tabs and one decision bar, held at the top of every tab and at the bottom of a phone's screen, with the review buttons, Integrate and Ask the author for a question about the whole proposal: Overview (the integration state, conflicts and the history of each version with its check runs and reviews), Changes (the diff, full or since the last review, beside a folder tree of the changed files; a file opened there is marked viewed in this browser; between hunks a row counts the hidden unchanged lines and shows 20 more up or down or all of them, a file's head switches between Show whole file and Show changes only, and the lines shown take comments like any context line), Checks (every check run, with each step's full log) and Conversation (the proposal's threads). A person comments on lines, asks the author about selected lines, approves the version, requests changes, integrates, edits a file into a new version and sends that version for review again. Approve and Integrate are off, with the reason under the button, while a check run of the current version goes or the author's update or a reviewer's fix runs the checks of the next version. A batch page shows a review batch's members and their change merged onto main, with the same unchanged lines to show, stops its reviewer and integrates its approved members with Integrate the batch. A proposal that waits in a review plan for related tasks shows the awaited tasks and the time the wait ends, on its page and its board card.
- Requests: create a request with attachments; on a request, approve a draft, request changes, edit the draft into a new version, close it and answer its clarification thread.
- Agents: the main agent, the leads and their workers with state, model, turns, cost and context, with running checks in the step of a live worker whose workspace runs them, and `older fold` on a worker whose session still holds an older fold's skill until its next turn. Start an area lead or the review lead, stop a worker, start a worker in a new session, and allow or deny a worker's tool permission. The Cost tab shows what the supervised workers cost today, in the last 7 days and of all time, with their tokens, by role, area, model, task and worker.
- Messages: one chat per agent, which holds the agent's threads that concern you in time order, each message with its task, proposal or request, with a search and one Filter menu that shows All chats, Waits for you, Questions or Mine and narrows the list by agent, task or proposal; an agent asks everything it needs at one time as one question set (`send_message` with `questions`: 1 to 4 questions, each with optional choices, and an own answer is always possible). Each chat in the list counts in gold its question sets that wait for you, one per set, and the tab's count, the Waits for you choice and Needs you count and list the same sets. A chat shows one question of its oldest set at a time above the reply box, with the agent, the task and a step bar such as 1 of 3; a choice (click or its number key) or Enter on an own answer shows the next question, and Back changes an earlier answer. After the last answer fold sends every answer as one reply in the thread, with each question and its answer, and stores the answers, which `get_thread` shows as `answers`; then the next set shows, and a line says how many more sets wait. The timeline sits at the bottom, with the newest message directly above the question or the reply box, and About sends a reply to another thread of the chat. A link to a thread or a message opens the chat of its agent at it. Enter sends a reply and Shift+Enter starts a new line. Each message of a person says per recipient whom it was delivered to or who read it, and a thread that a person opens is read for that person only. Reply, answer a question set with choices or free text, mark the set on screen answered, and send feedback. Agents' messages to each other are on each task and in Activity. The leads, the reviewers and the task workers ask the humans directly with `asks_human`, so their questions wait in Needs you under Agents ask you, and a reply goes back to the asker in the same thread, never through the main agent.
- Repository: revisions, the tree, files and a compare of two revisions, which names the older one first, as r1 → r4.
- Settings: where the project's notices go. Turn the host targets on or off, list the project's targets and the host's with their health and next try, add or edit one (name, type Generic JSON, Slack or Discord, URL, an events preset or single events, card-move columns, grouping window), send a test, see a target's recent sends and send one again, give a generic target a new signing secret, which shows once, and set the web address that links in notices start with.

The top bar holds the project switcher, the tabs, the Agents control, the Inbox, New task and the avatar menu. The Proposals and Messages tabs count in gold the proposals that wait for your review and the threads that wait for your answer. The Agents control shows the live state and how many agents work, and opens a panel with every agent, the stream status and Pause or Resume; an agent's row adds running checks, or running checks in N tasks, while a check run goes in the workspaces of its workers. The avatar menu sets the Light, Dark or System theme and has Sign out. Below 1440px one menu holds the tabs, the agents' state with a link to the Agents screen and Pause or Resume, the theme and Sign out. A toast names the card of a single move and counts more moves per column, as 3 cards moved to Review, 1 moved to Done. The Inbox has two tabs. Updates lists the last ten card moves, one row per task with a link to it, and Clear all. Needs you lists what waits for a human, each with a link to the screen that handles it, in three groups: Agents ask you (questions and tool permissions), Waiting for you (reviews, request drafts, human tasks and blockers) and Project (your conflicting change from the project folder, the Git branch, unrecorded changes in project folder files main changed, an unstash conflict, the checks file on main, a worker that cannot reach fold and needs `fld restart`, a worker that cannot reach its model or finds no claude, and a notice target fold turned off after its sends failed). A task, proposal, request or review batch that does not exist shows a not-found panel inside the project's screens with a link back to its list. Config, areas, worker limits and tokens change through `.fold/config.toml`, the fold tools and `fld`; the Settings tab holds only the notice targets.

## Tools

Every fold tool by purpose; the skill holds their arguments and workflows. The heavy reads answer in a short form by default and name the full form in their descriptions: `get_proposal` with `detail` `full` adds the evidence text, the task description and the whole earlier review bodies; `get_task` with `history` `all` gives every history entry the read holds instead of the newest 5; `get_task_graph` with `include_done` adds the Done tasks, which `done_count` counts, and with `detail` `full` the empty `labels`, `needs` and `holder` that it leaves out; `list_workers` with `include_stopped` lists the stopped workers and with `include_instructions` each area's instructions. `get_proposal_diff` and `compare_revisions` give each hunk as a unified `@@` header with the first line and the line count of each side, and one string per line: a mark (space, `-` or `+`), the line's number, a space and the text, such as `+14 let x = 1;`. A removed line has its old number and a context or added line its new number, so a comment uses the number on the line.

- Identity and agents: `register_agent`, `list_agents`, `get_activity`.
- Areas and workers: `define_area`, `update_area`, `start_worker`, `stop_worker`, `list_workers`, `set_worker_limits`, `write_handoff`, `restart_worker`, `pause_project`, `resume_project`.
- Messages and events: `send_message`, `get_inbox`, `get_thread`, `acknowledge_message`, `mark_answered`, `list_open_asks` (main agent: every agent's open ask with id, agent, thread and first line, to answer for the user what the user says is done), `get_events`, `wait_for_events`.
- Tasks: `get_task_graph`, `get_ready_tasks`, `get_task`, `create_task`, `create_task_plan`, `update_task`, `delete_task`, `archive_task`, `add_dependency`, `remove_dependency`, `create_label`, `set_blocker`, `clear_blocker`, `require_human_review`.
- Checklists: `add_checklist_item`, `update_checklist_item`, `move_checklist_item`, `remove_checklist_item`.
- Claims and completion: `claim_task`, `release_task`, `takeover_claim`, `report_task`, `complete_task`, `send_back_task`.
- Repository and workspaces: `get_repository_state`, `get_revision`, `compare_revisions`, `create_workspace`, `get_workspace`, `get_workspace_changes`, `record_checkpoint`, `list_checkpoints`, `restore_checkpoint`, `record_changes`, `sync_workspace`, `abandon_workspace`, `open_main_workspace`, `submit_main_change`, `add_exclude_patterns`.
- Checks: `run_checks`, `get_check_run`.
- Proposals: `create_proposal`, `list_proposals`, `get_proposal`, `get_proposal_diff`, `update_proposal`, `close_proposal`, `integrate_proposal`.
- Reviews: `claim_review`, `release_review`, `create_review_checkout`, `submit_review`, `submit_review_fix`, `get_review_comments`, `set_review_scope`, `create_review_batch`, `list_review_batches`, `get_review_batch`, `remove_from_batch`, `create_batch_checkout`, `submit_batch_review`, `integrate_review_batch`, `create_review_plan`, `change_review_plan`, `get_review_plan`, `list_review_plans`.
- Requests: `create_request`, `list_requests`, `get_request`, `claim_request`, `release_request`, `submit_request_draft`, `request_draft_changes`, `edit_request_draft`, `approve_request_draft`, `close_request`.
