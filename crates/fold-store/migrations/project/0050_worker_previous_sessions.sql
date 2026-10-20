-- Each Claude Code session a worker left for a new one, so the session id that names its transcript stays known. `reason` is `handoff` (a fresh context after a hand-off note), `lost` (Claude Code no longer had the session) or `requested` (a human or the main agent asked for a new session).
CREATE TABLE worker_previous_sessions (
    id INTEGER PRIMARY KEY,
    worker_process_id INTEGER NOT NULL REFERENCES worker_processes (id),
    session_id TEXT NOT NULL UNIQUE CONSTRAINT worker_previous_sessions_session_present CHECK (session_id <> ''),
    reason TEXT NOT NULL CONSTRAINT worker_previous_sessions_reason_values CHECK (reason IN ('handoff', 'lost', 'requested')),
    ended_at INTEGER NOT NULL
);

CREATE INDEX worker_previous_sessions_worker ON worker_previous_sessions (worker_process_id, id);

CREATE TRIGGER worker_previous_sessions_no_update BEFORE UPDATE ON worker_previous_sessions
BEGIN
    SELECT RAISE(ABORT, 'worker_previous_sessions is append-only');
END;

CREATE TRIGGER worker_previous_sessions_no_delete BEFORE DELETE ON worker_previous_sessions
BEGIN
    SELECT RAISE(ABORT, 'worker_previous_sessions is append-only');
END;
