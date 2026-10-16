-- A part of the project that one area lead owns: every task with the area's label. The lead is set when the first lead of the area starts and never changes, so an area has one lead and a lead leads one area.
CREATE TABLE areas (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE CONSTRAINT areas_name_present CHECK (name <> ''),
    label_id INTEGER NOT NULL UNIQUE REFERENCES labels (id),
    lead_agent_id INTEGER UNIQUE REFERENCES agents (id),
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TRIGGER areas_no_delete BEFORE DELETE ON areas
BEGIN
    SELECT RAISE(ABORT, 'areas keeps every area');
END;

CREATE TRIGGER areas_lead_once BEFORE UPDATE ON areas
WHEN OLD.lead_agent_id IS NOT NULL OR NEW.lead_agent_id IS NULL
    OR NEW.name <> OLD.name OR NEW.label_id <> OLD.label_id
    OR NEW.created_by <> OLD.created_by OR NEW.created_at <> OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'areas_lead_once');
END;

-- The Claude Code process fold runs for one worker agent. The row stays when the process ends, so the worker keeps its session and can be resumed; a resume runs a new process for the same row. `turns`, `cost_micro_usd` and the token counts add up over every process of the row.
CREATE TABLE worker_processes (
    id INTEGER PRIMARY KEY,
    agent_id INTEGER NOT NULL UNIQUE REFERENCES agents (id),
    role TEXT NOT NULL CONSTRAINT worker_processes_role_values CHECK (role IN ('area_lead', 'review_lead', 'task_coder', 'batch_reviewer')),
    area_id INTEGER REFERENCES areas (id),
    model TEXT,
    session_id TEXT NOT NULL UNIQUE CONSTRAINT worker_processes_session_present CHECK (session_id <> ''),
    pid INTEGER,
    state TEXT NOT NULL CONSTRAINT worker_processes_state_values CHECK (state IN ('starting', 'working', 'idle', 'stopping', 'stopped', 'failed')),
    started_by TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    stopped_at INTEGER,
    idle_since INTEGER,
    turns INTEGER NOT NULL DEFAULT 0 CONSTRAINT worker_processes_turns_count CHECK (turns >= 0),
    cost_micro_usd INTEGER NOT NULL DEFAULT 0 CONSTRAINT worker_processes_cost_count CHECK (cost_micro_usd >= 0),
    cache_read_tokens INTEGER NOT NULL DEFAULT 0 CONSTRAINT worker_processes_cache_read_count CHECK (cache_read_tokens >= 0),
    cache_write_tokens INTEGER NOT NULL DEFAULT 0 CONSTRAINT worker_processes_cache_write_count CHECK (cache_write_tokens >= 0),
    context_tokens INTEGER NOT NULL DEFAULT 0 CONSTRAINT worker_processes_context_count CHECK (context_tokens >= 0),
    exit_code INTEGER,
    CONSTRAINT worker_processes_area_of_role CHECK ((role IN ('area_lead', 'task_coder')) = (area_id IS NOT NULL)),
    CONSTRAINT worker_processes_stopped_at_iff_ended CHECK ((state IN ('stopped', 'failed')) = (stopped_at IS NOT NULL)),
    CONSTRAINT worker_processes_idle_since_iff_idle CHECK ((state = 'idle') = (idle_since IS NOT NULL))
);

CREATE INDEX worker_processes_state ON worker_processes (state);

-- A project has one review lead. Its row stays when it stops, and a later start resumes it.
CREATE UNIQUE INDEX worker_processes_one_review_lead ON worker_processes (role) WHERE role = 'review_lead';

CREATE TRIGGER worker_processes_no_delete BEFORE DELETE ON worker_processes
BEGIN
    SELECT RAISE(ABORT, 'worker_processes keeps every worker');
END;

CREATE TRIGGER worker_processes_fixed BEFORE UPDATE ON worker_processes
WHEN NEW.agent_id <> OLD.agent_id OR NEW.role <> OLD.role OR NEW.area_id IS NOT OLD.area_id
    OR NEW.session_id <> OLD.session_id OR NEW.started_by <> OLD.started_by
BEGIN
    SELECT RAISE(ABORT, 'worker_processes_fixed');
END;

-- The tasks a task coder is bound to. A deleted task leaves the binding with it.
CREATE TABLE worker_tasks (
    worker_process_id INTEGER NOT NULL REFERENCES worker_processes (id),
    task_id INTEGER NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    PRIMARY KEY (worker_process_id, task_id)
);

CREATE INDEX worker_tasks_task ON worker_tasks (task_id);

CREATE TRIGGER worker_tasks_coder_only BEFORE INSERT ON worker_tasks
WHEN (SELECT role FROM worker_processes WHERE id = NEW.worker_process_id) IS NOT 'task_coder'
BEGIN
    SELECT RAISE(ABORT, 'worker_tasks_coder_only');
END;

-- The proposals a batch reviewer is bound to.
CREATE TABLE worker_proposals (
    worker_process_id INTEGER NOT NULL REFERENCES worker_processes (id),
    proposal_id INTEGER NOT NULL REFERENCES proposals (id),
    PRIMARY KEY (worker_process_id, proposal_id)
);

CREATE INDEX worker_proposals_proposal ON worker_proposals (proposal_id);

CREATE TRIGGER worker_proposals_reviewer_only BEFORE INSERT ON worker_proposals
WHEN (SELECT role FROM worker_processes WHERE id = NEW.worker_process_id) IS NOT 'batch_reviewer'
BEGIN
    SELECT RAISE(ABORT, 'worker_proposals_reviewer_only');
END;
