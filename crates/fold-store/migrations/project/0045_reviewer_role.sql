-- The short-lived worker that the review lead starts for one proposal or a group of related proposals has the role `reviewer`, and a stored `batch_reviewer` row takes that name. The role checks of `worker_processes` live in its table definition, so the table is made again with its rows, ids and triggers. The bindings and questions that refer to its rows lose their parent for a moment between the drop and the copy back, so their foreign key checks wait for the commit.
PRAGMA defer_foreign_keys = ON;

DROP TRIGGER worker_proposals_reviewer_only;

CREATE TABLE worker_processes_copy AS SELECT * FROM worker_processes;

DROP TABLE worker_processes;

CREATE TABLE worker_processes (
    id INTEGER PRIMARY KEY,
    agent_id INTEGER NOT NULL UNIQUE REFERENCES agents (id),
    role TEXT NOT NULL CONSTRAINT worker_processes_role_values CHECK (role IN ('area_lead', 'review_lead', 'task_worker', 'reviewer')),
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
    handoff_asked_at INTEGER,
    handoff_asked_turns INTEGER CONSTRAINT worker_processes_handoff_asked_turns_count CHECK (handoff_asked_turns >= 0),
    CONSTRAINT worker_processes_area_of_role CHECK ((role IN ('area_lead', 'task_worker')) = (area_id IS NOT NULL)),
    CONSTRAINT worker_processes_stopped_at_iff_ended CHECK ((state IN ('stopped', 'failed')) = (stopped_at IS NOT NULL)),
    CONSTRAINT worker_processes_idle_since_iff_idle CHECK ((state = 'idle') = (idle_since IS NOT NULL))
);

INSERT INTO worker_processes (
    id, agent_id, role, area_id, model, session_id, pid, state, started_by, started_at, stopped_at, idle_since,
    turns, cost_micro_usd, cache_read_tokens, cache_write_tokens, context_tokens, exit_code, handoff_asked_at, handoff_asked_turns
)
SELECT
    id, agent_id, CASE role WHEN 'batch_reviewer' THEN 'reviewer' ELSE role END, area_id, model, session_id, pid, state, started_by, started_at, stopped_at, idle_since,
    turns, cost_micro_usd, cache_read_tokens, cache_write_tokens, context_tokens, exit_code, handoff_asked_at, handoff_asked_turns
FROM worker_processes_copy;

DROP TABLE worker_processes_copy;

CREATE INDEX worker_processes_state ON worker_processes (state);

-- A project has one review lead. Its row stays when it stops, and a later start resumes it.
CREATE UNIQUE INDEX worker_processes_one_review_lead ON worker_processes (role) WHERE role = 'review_lead';

CREATE TRIGGER worker_processes_no_delete BEFORE DELETE ON worker_processes
BEGIN
    SELECT RAISE(ABORT, 'worker_processes keeps every worker');
END;

CREATE TRIGGER worker_processes_fixed BEFORE UPDATE ON worker_processes
WHEN NEW.agent_id <> OLD.agent_id OR NEW.role <> OLD.role OR NEW.area_id IS NOT OLD.area_id
    OR NEW.started_by <> OLD.started_by
    OR (NEW.session_id <> OLD.session_id AND NEW.state <> 'starting')
BEGIN
    SELECT RAISE(ABORT, 'worker_processes_fixed');
END;

-- Only a reviewer is bound to proposals.
CREATE TRIGGER worker_proposals_reviewer_only BEFORE INSERT ON worker_proposals
WHEN (SELECT role FROM worker_processes WHERE id = NEW.worker_process_id) IS NOT 'reviewer'
BEGIN
    SELECT RAISE(ABORT, 'worker_proposals_reviewer_only');
END;
