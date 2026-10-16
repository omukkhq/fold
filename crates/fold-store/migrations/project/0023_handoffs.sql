-- A hand-off note a long-lived worker wrote for the fresh session that follows it: what it is doing, its open decisions and what comes next. `session_id` is the session that wrote it. The body is content and never reaches a log or an event.
CREATE TABLE handoffs (
    id INTEGER PRIMARY KEY,
    agent_id INTEGER NOT NULL REFERENCES agents (id),
    session_id TEXT NOT NULL CONSTRAINT handoffs_session_present CHECK (session_id <> ''),
    body TEXT NOT NULL CONSTRAINT handoffs_body_present CHECK (body <> ''),
    created_at INTEGER NOT NULL
);

CREATE INDEX handoffs_agent ON handoffs (agent_id, id);

CREATE INDEX handoffs_session ON handoffs (session_id);

CREATE TRIGGER handoffs_no_update BEFORE UPDATE ON handoffs
BEGIN
    SELECT RAISE(ABORT, 'handoffs is append-only');
END;

CREATE TRIGGER handoffs_no_delete BEFORE DELETE ON handoffs
BEGIN
    SELECT RAISE(ABORT, 'handoffs is append-only');
END;

-- When fold asked the worker's current session for a hand-off note, and the row's `turns` at that time, so a restarted daemon neither asks twice nor waits forever for a note.
ALTER TABLE worker_processes ADD COLUMN handoff_asked_at INTEGER;

ALTER TABLE worker_processes ADD COLUMN handoff_asked_turns INTEGER
    CONSTRAINT worker_processes_handoff_asked_turns_count CHECK (handoff_asked_turns >= 0);

-- A fresh context gives the worker a new session, so the session id changes, but only in the update that starts the row again; the rest stays fixed.
DROP TRIGGER worker_processes_fixed;

CREATE TRIGGER worker_processes_fixed BEFORE UPDATE ON worker_processes
WHEN NEW.agent_id <> OLD.agent_id OR NEW.role <> OLD.role OR NEW.area_id IS NOT OLD.area_id
    OR NEW.started_by <> OLD.started_by
    OR (NEW.session_id <> OLD.session_id AND NEW.state <> 'starting')
BEGIN
    SELECT RAISE(ABORT, 'worker_processes_fixed');
END;
