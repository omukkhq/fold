-- How a task's work reaches Done, set once at creation. A `no_proposal` task changes no project file: its coder reports the work and a lead check completes it.
ALTER TABLE tasks ADD COLUMN kind TEXT NOT NULL DEFAULT 'proposal'
    CONSTRAINT tasks_kind_values CHECK (kind IN ('proposal', 'no_proposal'));

CREATE TRIGGER tasks_kind_once BEFORE UPDATE OF kind ON tasks
WHEN NEW.kind <> OLD.kind
BEGIN
    SELECT RAISE(ABORT, 'tasks_kind_once');
END;

-- A task without a proposal has neither a task workspace nor a proposal.
CREATE TRIGGER workspaces_task_needs_proposal_kind BEFORE INSERT ON workspaces
WHEN NEW.kind = 'task' AND (SELECT kind FROM tasks WHERE id = NEW.task_id) = 'no_proposal'
BEGIN
    SELECT RAISE(ABORT, 'workspaces_task_needs_proposal_kind');
END;

CREATE TRIGGER proposals_task_needs_proposal_kind BEFORE INSERT ON proposals
WHEN (SELECT kind FROM tasks WHERE id = NEW.task_id) = 'no_proposal'
BEGIN
    SELECT RAISE(ABORT, 'proposals_task_needs_proposal_kind');
END;

-- A coder's report that the work of a task without a proposal is finished, and the lead check that answers it: `completed` with the task's completion record, or `sent_back` with the note that returns the task to its coder. The open report is the one without a resolution; a task has at most one.
CREATE TABLE task_reports (
    id INTEGER PRIMARY KEY,
    task_id INTEGER NOT NULL REFERENCES tasks (id),
    worker_id INTEGER NOT NULL REFERENCES workers (id),
    note TEXT NOT NULL CONSTRAINT task_reports_note_present CHECK (trim(note) <> ''),
    created_at INTEGER NOT NULL,
    resolution TEXT CONSTRAINT task_reports_resolution_values CHECK (resolution IN ('completed', 'sent_back')),
    resolved_by TEXT,
    resolved_at INTEGER,
    send_back_note TEXT,
    CONSTRAINT task_reports_resolved_together CHECK (
        (resolution IS NULL) = (resolved_by IS NULL) AND (resolution IS NULL) = (resolved_at IS NULL)
    ),
    CONSTRAINT task_reports_send_back_note CHECK (
        (resolution IS 'sent_back' AND send_back_note IS NOT NULL AND trim(send_back_note) <> '')
        OR (resolution IS NOT 'sent_back' AND send_back_note IS NULL)
    )
);

CREATE UNIQUE INDEX task_reports_one_open ON task_reports (task_id) WHERE resolution IS NULL;
CREATE INDEX task_reports_task ON task_reports (task_id, id);

CREATE TRIGGER task_reports_open_on_no_proposal_task BEFORE INSERT ON task_reports
WHEN NEW.resolution IS NOT NULL
    OR (SELECT kind FROM tasks WHERE id = NEW.task_id) IS NOT 'no_proposal'
BEGIN
    SELECT RAISE(ABORT, 'task_reports_open_on_no_proposal_task');
END;

CREATE TRIGGER task_reports_resolved_once BEFORE UPDATE ON task_reports
WHEN OLD.resolution IS NOT NULL
    OR NEW.task_id <> OLD.task_id OR NEW.worker_id <> OLD.worker_id
    OR NEW.note <> OLD.note OR NEW.created_at <> OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'task_reports_resolved_once');
END;

CREATE TRIGGER task_reports_no_delete BEFORE DELETE ON task_reports
BEGIN
    SELECT RAISE(ABORT, 'task_reports is append-only');
END;

-- `completions` is rebuilt to take the `check` kind, the completion a lead check writes. Nothing refers to it, so dropping the old table touches no other row; its append-only triggers go with it and are made again.
CREATE TABLE completions_new (
    task_id INTEGER PRIMARY KEY REFERENCES tasks (id),
    kind TEXT NOT NULL CONSTRAINT completions_kind_values CHECK (kind IN ('integration', 'children', 'check')),
    integration_id INTEGER REFERENCES integrations (id),
    created_at INTEGER NOT NULL,
    CONSTRAINT completions_integration_matches_kind CHECK (
        (kind = 'integration' AND integration_id IS NOT NULL)
        OR (kind IN ('children', 'check') AND integration_id IS NULL)
    )
);

INSERT INTO completions_new (task_id, kind, integration_id, created_at)
SELECT task_id, kind, integration_id, created_at FROM completions;

DROP TABLE completions;

ALTER TABLE completions_new RENAME TO completions;

CREATE TRIGGER completions_no_update BEFORE UPDATE ON completions
BEGIN
    SELECT RAISE(ABORT, 'completions is append-only');
END;

CREATE TRIGGER completions_no_delete BEFORE DELETE ON completions
BEGIN
    SELECT RAISE(ABORT, 'completions is append-only');
END;
