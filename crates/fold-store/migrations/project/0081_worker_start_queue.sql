-- A worker start that waited for a place under the worker limits. Only the roles with a limit queue. `name`, `model` and `instructions` are what the starter gave `start_worker`, kept so the start can run later without the starter; `queued_by` is the starter's actor label. The row ends once: `started` with the worker it started, or `dropped` with the reason.
CREATE TABLE worker_start_queue (
    id INTEGER PRIMARY KEY,
    role TEXT NOT NULL CONSTRAINT worker_start_queue_role_values CHECK (role IN ('task_worker', 'reviewer')),
    area_id INTEGER REFERENCES areas (id),
    name TEXT CONSTRAINT worker_start_queue_name_present CHECK (name IS NULL OR name <> ''),
    review_plan_id INTEGER REFERENCES review_plans (id),
    model TEXT CONSTRAINT worker_start_queue_model_present CHECK (model IS NULL OR model <> ''),
    instructions TEXT CONSTRAINT worker_start_queue_instructions_present CHECK (instructions IS NULL OR instructions <> ''),
    queued_by TEXT NOT NULL CONSTRAINT worker_start_queue_queued_by_present CHECK (queued_by <> ''),
    queued_at INTEGER NOT NULL,
    ended_at INTEGER,
    outcome TEXT CONSTRAINT worker_start_queue_outcome_values CHECK (outcome IN ('started', 'dropped')),
    drop_reason TEXT CONSTRAINT worker_start_queue_drop_reason_values CHECK (drop_reason IN ('task_claimed', 'task_done', 'task_archived', 'proposal_ended', 'cancelled')),
    worker_process_id INTEGER REFERENCES worker_processes (id),
    CONSTRAINT worker_start_queue_area_of_role CHECK ((role = 'task_worker') = (area_id IS NOT NULL)),
    CONSTRAINT worker_start_queue_plan_of_reviewer CHECK (review_plan_id IS NULL OR role = 'reviewer'),
    CONSTRAINT worker_start_queue_ended_iff_outcome CHECK ((ended_at IS NULL) = (outcome IS NULL)),
    CONSTRAINT worker_start_queue_reason_iff_dropped CHECK ((outcome IS 'dropped') = (drop_reason IS NOT NULL)),
    CONSTRAINT worker_start_queue_worker_iff_started CHECK ((outcome IS 'started') = (worker_process_id IS NOT NULL))
);

CREATE INDEX worker_start_queue_waiting ON worker_start_queue (queued_at) WHERE ended_at IS NULL;

-- A queued start ends once and keeps what it asked for.
CREATE TRIGGER worker_start_queue_ends_once BEFORE UPDATE ON worker_start_queue
WHEN OLD.ended_at IS NOT NULL
    OR NEW.id IS NOT OLD.id
    OR NEW.role IS NOT OLD.role
    OR NEW.area_id IS NOT OLD.area_id
    OR NEW.name IS NOT OLD.name
    OR NEW.review_plan_id IS NOT OLD.review_plan_id
    OR NEW.model IS NOT OLD.model
    OR NEW.instructions IS NOT OLD.instructions
    OR NEW.queued_by IS NOT OLD.queued_by
    OR NEW.queued_at IS NOT OLD.queued_at
BEGIN
    SELECT RAISE(ABORT, 'a queued worker start only ends, once');
END;

CREATE TRIGGER worker_start_queue_no_delete BEFORE DELETE ON worker_start_queue
BEGIN
    SELECT RAISE(ABORT, 'worker_start_queue keeps every queued start');
END;

-- The tasks a queued task worker gets, in the order the starter gave them.
CREATE TABLE worker_start_queue_tasks (
    queued_start_id INTEGER NOT NULL REFERENCES worker_start_queue (id),
    position INTEGER NOT NULL CONSTRAINT worker_start_queue_tasks_position_count CHECK (position >= 0),
    task_id INTEGER NOT NULL REFERENCES tasks (id),
    PRIMARY KEY (queued_start_id, position),
    CONSTRAINT worker_start_queue_tasks_once UNIQUE (queued_start_id, task_id)
);

CREATE INDEX worker_start_queue_tasks_task ON worker_start_queue_tasks (task_id);

CREATE TRIGGER worker_start_queue_tasks_no_update BEFORE UPDATE ON worker_start_queue_tasks
BEGIN
    SELECT RAISE(ABORT, 'worker_start_queue_tasks is append-only');
END;

CREATE TRIGGER worker_start_queue_tasks_no_delete BEFORE DELETE ON worker_start_queue_tasks
BEGIN
    SELECT RAISE(ABORT, 'worker_start_queue_tasks is append-only');
END;

-- The proposals a queued reviewer gets, in the order the starter gave them.
CREATE TABLE worker_start_queue_proposals (
    queued_start_id INTEGER NOT NULL REFERENCES worker_start_queue (id),
    position INTEGER NOT NULL CONSTRAINT worker_start_queue_proposals_position_count CHECK (position >= 0),
    proposal_id INTEGER NOT NULL REFERENCES proposals (id),
    PRIMARY KEY (queued_start_id, position),
    CONSTRAINT worker_start_queue_proposals_once UNIQUE (queued_start_id, proposal_id)
);

CREATE INDEX worker_start_queue_proposals_proposal ON worker_start_queue_proposals (proposal_id);

CREATE TRIGGER worker_start_queue_proposals_no_update BEFORE UPDATE ON worker_start_queue_proposals
BEGIN
    SELECT RAISE(ABORT, 'worker_start_queue_proposals is append-only');
END;

CREATE TRIGGER worker_start_queue_proposals_no_delete BEFORE DELETE ON worker_start_queue_proposals
BEGIN
    SELECT RAISE(ABORT, 'worker_start_queue_proposals is append-only');
END;
