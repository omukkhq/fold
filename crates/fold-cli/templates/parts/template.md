## Task template

Write the description of every task in this form:

- A summary paragraph that says what the task delivers.
- **Refs**: the documents, tasks and code the worker must read.
- **Scope**: what to build, with the files or modules.
- **Out of scope**: what belongs to other tasks.
- **Acceptance**: the checks that show the task is done, each one testable.
- **How to do it** (every task of kind `human`, and only those): numbered steps that a person can follow without reading the code. Each step names one action, where to do it and what the person sees when it works. When the steps need a script or a tool, an agent task builds it first, and the human task needs that task.
- **Decisions** (optional, always last): one entry for each decision a person gave after the task was written, in the form that the `fold` skill's `REFERENCE.md` gives. The Scope and Acceptance text already says the decided value.

Write dependencies as edges with `add_dependency`, never as text in the description.
