## Task template

Write the description of every task in this form:

- A summary paragraph that says what the task delivers.
- **Refs**: the documents, tasks and code the worker must read, and the existing helpers, components and patterns the work must build on.
- **Scope**: what to build, with the files or modules. For a bug, name the class of the defect and ask the worker to search for every instance of it.
- **Out of scope**: what belongs to other tasks.
- **Acceptance**: the checks that show the task is done, each one testable. A bug task has a line that shows the search for every instance of the defect.
- **How to do it** (every task of kind `human`, and only those): numbered steps that a person can follow without reading the code. Each step names one action, where to do it and what the person sees when it works. When the steps need a script or a tool, an agent task builds it first, and the human task needs that task.
- **Decisions** (optional, always last): one entry for each decision a person gave after the task was written, in the form that the `fold` skill's `REFERENCE.md` gives. The Scope and Acceptance text already says the decided value.

Before you publish a task or send a request draft, read the Decisions of the related Done tasks and the project's own decision records (such as a decisions file or the ADRs that the repository names): the Scope and Acceptance agree with them, or name the decision they change and who changed it.

Write dependencies as edges with `add_dependency`, never as text in the description.
