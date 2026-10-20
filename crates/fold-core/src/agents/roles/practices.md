## Engineering practices

These are fold's default practices for the code you write, review or plan. The project's own AGENTS.md (or CLAUDE.md) and the user's word come first: where they set a different rule, follow theirs. Your role, its limits and the approved scope come first too. Where they say nothing, follow these. When you write or extend a project's AGENTS.md, carry over the practices that fit its stack.

### Work and checks
- **Never hand back work on a red check.** If a formatter, linter, type checker or test of your own work fails, fix it, even when your change did not cause it. When a check fails outside your own work, such as a web test for a backend agent, ask for a fix with `create_request` instead of fixing it.
- **In a fold project you record and propose changes with fold's tools; never run `git commit` yourself.** Never skip the project's hooks or checks.
- **Prefer the project's task runner** (Makefile or its equivalent) over calling the tools directly.
- **Hold the code to 85% line and 85% branch coverage, each on its own.** Put logic in modules that tests reach, not in UI components. Raise coverage with tests of real behaviour, never by excluding reachable code.
- **One package manager per language,** never a mix. Add, remove or bump a dependency only with its commands, never by editing a manifest by hand.
- **Generate every file a tool can generate:** migrations, API clients, lockfiles, schema metadata. Edit a generated file only for a change it genuinely needs.

### Writing
- **Ask instead of leaving questions in deliverables.** No TBDs, open questions or "this doesn't match X" in code, docs or commit messages.
- **When the code and a settled product decision disagree, stop and ask.** Never resolve the conflict silently in either direction.
- **Comment only what the code can't say:** a non-obvious, project-specific fact, never a narration of the code. Describe what is, not what was; what changed and why belongs in the commit message.
- **Never refer to planning documents, their sections or design frames in code or commits;** state the rule or the reason itself.
- **Don't hard-wrap prose.** One paragraph is one line in Markdown and commit messages.
- **Write commit messages for the person reading the history:** what changed and why it matters. No trailing signature or attribution footer. The descriptions of your recorded changes and proposals follow the same rules.
- **Write everything that faces the user in ASD-STE100 Simplified Technical English:** messages, questions, reports and documents you prepare for the user; approved words, short sentences, active voice. Code, commands, paths, identifiers and commit messages keep their normal language.

### Architecture
- **Split each layer by domain, never by entity.**
- **Keep the core pure:** no I/O, no clock, no randomness, no environment. Time and randomness are inputs, so every decision can be tested and replayed without infrastructure.
- **Dependencies flow toward the core and the leaves.** Hand adapters plain values, never database rows or sessions.
- **Keep transports thin.** A route handler or tool parses input, calls one service function and maps the result. No rule lives in a transport.
- **Only commands change state.** Every mutating service takes the acting user or agent and checks it in one policy module, inside one transaction.
- **Errors are typed and carry a stable code.** Services never raise transport errors; clients branch on the code, never on the message.
- **Every route declares its response and its error responses,** so clients are generated from the schema. The schema is the only contract between a backend and its clients; keep the projects apart.
- **No trailing slash on endpoint paths,** and handler names say what the route does.

### Data
- **Let the database hold the invariants it can:** constraints, uniqueness, foreign keys, and append-only tables that refuse updates and deletes.
- **One convention per project for ids, timestamps and enums.** Timestamps are UTC.
- **A value used to filter, join, constrain or decide is a real column,** never a key inside a JSON column.
- **Migrations are forward-only.** Never edit an applied migration. The app never connects with the migrator's credentials.
- **Keep the wire schema separate from the storage model.**

### Security
- **Decide access in one module and nothing else.** Answer a record the caller can't see exactly like a missing one.
- **Log ids, never content:** no file content, message bodies, tokens or personal data in a log line or an error detail.
- **Secrets use the language's secret type, never a plain string.** Keep the settings and the example environment file in sync, and give a secret only to the service that needs it.
- **Read role and account status from the database on every request,** never from the session.
- **Treat every upload and every model input as hostile.** Never let a storage URL reach a client.

### Background jobs
- **Pass jobs ids, never content.**
- **Make every job idempotent or keyed.**
- **A service never enqueues.** Put each job on the queue of the process that waits for it.

### Types and lints
- **Use each type checker in strict mode and never relax it.**
- **No type escape hatches:** no ignore comments, no `any`, no casts through an unknown type. Annotate every signature.
- **Suppress a lint only at the line, with a reason,** never for a whole file or project.

### Web clients
- **Keep the web app static** (no server-side rendering) unless the user chooses otherwise; a static build is easier to deploy.
- **Use semantic design tokens, never raw colours.** Type, spacing and radii come from the tokens too.
- **Build on one primitive library** for menus, dialogs, popovers, tabs and tooltips; never hand-roll focus management. Use one icon set.
- **Call the API only through the generated client.**
- **Show a pending state, never optimistic success,** for every write.
- **Load every font and asset from the bundle,** never from a CDN.
- **Hide what a role can't use, but never rely on hiding.** The server decides.
- **Match the design for the screen you build** when the project has one.
- **No mock or hard-coded demo data in app code.** An empty project renders empty states; demo data comes from a seed command that runs the real commands.

### Tests
- **Use a real database and real files in tests,** never mocks of them.
- **Call routes in-process, not through a live port.**
- **Mock only external HTTP.** Never call a real AI, SMS, push or email provider in ordinary tests.
- **Test the core without infrastructure.**
- **Wait for asynchronous effects with a timeout, never a fixed sleep.**
- **Each end-to-end product scenario is one numbered acceptance test.**

### Containers
- **Pin every image to an exact version.**
- **Keep the development and production compose files in sync.**
- **Give a route out of the network only to the services that need one.**
