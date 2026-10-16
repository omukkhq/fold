-- A `human` task is work only a person can do, outside every agent: no agent claims it, it has no workspace and no proposal, and a person marks it Done. SQLite cannot change the CHECK of `tasks.kind` in place, and other tables refer to `tasks`, so the column is dropped and added again with the wider CHECK. The triggers that read the column are dropped first and made again after.
DROP TRIGGER tasks_kind_once;
DROP TRIGGER workspaces_task_needs_proposal_kind;
DROP TRIGGER proposals_task_needs_proposal_kind;
DROP TRIGGER task_reports_open_on_no_proposal_task;

ALTER TABLE tasks ADD COLUMN kind_before TEXT;

UPDATE tasks SET kind_before = kind;

ALTER TABLE tasks DROP COLUMN kind;

ALTER TABLE tasks ADD COLUMN kind TEXT NOT NULL DEFAULT 'proposal'
    CONSTRAINT tasks_kind_values CHECK (kind IN ('proposal', 'no_proposal', 'human'));

UPDATE tasks SET kind = kind_before;

ALTER TABLE tasks DROP COLUMN kind_before;

CREATE TRIGGER tasks_kind_once BEFORE UPDATE OF kind ON tasks
WHEN NEW.kind <> OLD.kind
BEGIN
    SELECT RAISE(ABORT, 'tasks_kind_once');
END;

-- Only a task with a proposal has a task workspace or a proposal.
CREATE TRIGGER workspaces_task_needs_proposal_kind BEFORE INSERT ON workspaces
WHEN NEW.kind = 'task' AND (SELECT kind FROM tasks WHERE id = NEW.task_id) IN ('no_proposal', 'human')
BEGIN
    SELECT RAISE(ABORT, 'workspaces_task_needs_proposal_kind');
END;

CREATE TRIGGER proposals_task_needs_proposal_kind BEFORE INSERT ON proposals
WHEN (SELECT kind FROM tasks WHERE id = NEW.task_id) IN ('no_proposal', 'human')
BEGIN
    SELECT RAISE(ABORT, 'proposals_task_needs_proposal_kind');
END;

CREATE TRIGGER task_reports_open_on_no_proposal_task BEFORE INSERT ON task_reports
WHEN NEW.resolution IS NOT NULL
    OR (SELECT kind FROM tasks WHERE id = NEW.task_id) IS NOT 'no_proposal'
BEGIN
    SELECT RAISE(ABORT, 'task_reports_open_on_no_proposal_task');
END;

-- The hold mark: while `hold_reason` is set, no agent claims the task and no lead starts a coder for it. The board labels it Blocker. A mark that is on has its reason, its setter and its time; a mark that is off has none of them.
ALTER TABLE tasks ADD COLUMN hold_reason TEXT
    CONSTRAINT tasks_hold_reason_present CHECK (hold_reason IS NULL OR trim(hold_reason) <> '');

ALTER TABLE tasks ADD COLUMN hold_set_by TEXT;

ALTER TABLE tasks ADD COLUMN hold_set_at INTEGER;

CREATE TRIGGER tasks_hold_shape_insert BEFORE INSERT ON tasks
WHEN (NEW.hold_reason IS NULL) <> (NEW.hold_set_by IS NULL)
    OR (NEW.hold_reason IS NULL) <> (NEW.hold_set_at IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'tasks_hold_shape');
END;

CREATE TRIGGER tasks_hold_shape_update BEFORE UPDATE OF hold_reason, hold_set_by, hold_set_at ON tasks
WHEN (NEW.hold_reason IS NULL) <> (NEW.hold_set_by IS NULL)
    OR (NEW.hold_reason IS NULL) <> (NEW.hold_set_at IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'tasks_hold_shape');
END;

-- No agent claims a human task or a task on hold. A claim that exists when the hold is set stays.
CREATE TRIGGER claims_task_open_to_agents BEFORE INSERT ON claims
WHEN NEW.kind = 'implementation'
    AND EXISTS (
        SELECT 1 FROM tasks t
        WHERE t.id = NEW.task_id AND (t.kind = 'human' OR t.hold_reason IS NOT NULL)
    )
BEGIN
    SELECT RAISE(ABORT, 'claims_task_open_to_agents');
END;

-- `completions` is rebuilt to take the `human` kind, the completion a person writes for a human task. Nothing refers to it, so dropping the old table touches no other row; its append-only triggers go with it and are made again.
CREATE TABLE completions_new (
    task_id INTEGER PRIMARY KEY REFERENCES tasks (id),
    kind TEXT NOT NULL CONSTRAINT completions_kind_values CHECK (kind IN ('integration', 'children', 'check', 'human')),
    integration_id INTEGER REFERENCES integrations (id),
    created_at INTEGER NOT NULL,
    CONSTRAINT completions_integration_matches_kind CHECK (
        (kind = 'integration' AND integration_id IS NOT NULL)
        OR (kind IN ('children', 'check', 'human') AND integration_id IS NULL)
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

-- A human task is done only when a person marks it, and only a human task takes that completion.
CREATE TRIGGER completions_human_kind BEFORE INSERT ON completions
WHEN (NEW.kind = 'human') <> ((SELECT kind FROM tasks WHERE id = NEW.task_id) IS 'human')
BEGIN
    SELECT RAISE(ABORT, 'completions_human_kind');
END;
