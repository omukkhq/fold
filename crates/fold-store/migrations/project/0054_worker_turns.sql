-- One finished turn of a supervised worker, with what Claude Code reported for it: the cost the turn added to the process's running total and the turn's own token counts, at the time the turn ended. A turn counts for the one task or proposal it worked on, or for none. The worker's role and area are copied from its row, so a sum by role or area needs no join; the model is the one the row named when the turn ended.
CREATE TABLE worker_turns (
    id INTEGER PRIMARY KEY,
    worker_process_id INTEGER NOT NULL REFERENCES worker_processes (id),
    role TEXT NOT NULL,
    area_id INTEGER REFERENCES areas (id),
    task_id INTEGER REFERENCES tasks (id),
    proposal_id INTEGER REFERENCES proposals (id),
    model TEXT,
    cost_micro_usd INTEGER NOT NULL CONSTRAINT worker_turns_cost_count CHECK (cost_micro_usd >= 0),
    input_tokens INTEGER NOT NULL CONSTRAINT worker_turns_input_count CHECK (input_tokens >= 0),
    output_tokens INTEGER NOT NULL CONSTRAINT worker_turns_output_count CHECK (output_tokens >= 0),
    cache_read_tokens INTEGER NOT NULL CONSTRAINT worker_turns_cache_read_count CHECK (cache_read_tokens >= 0),
    cache_write_tokens INTEGER NOT NULL CONSTRAINT worker_turns_cache_write_count CHECK (cache_write_tokens >= 0),
    failed INTEGER NOT NULL CONSTRAINT worker_turns_failed_flag CHECK (failed IN (0, 1)),
    created_at INTEGER NOT NULL,
    CONSTRAINT worker_turns_one_subject CHECK (task_id IS NULL OR proposal_id IS NULL)
);

CREATE INDEX worker_turns_created ON worker_turns (created_at);
CREATE INDEX worker_turns_worker ON worker_turns (worker_process_id);
CREATE INDEX worker_turns_task ON worker_turns (task_id) WHERE task_id IS NOT NULL;
CREATE INDEX worker_turns_proposal ON worker_turns (proposal_id) WHERE proposal_id IS NOT NULL;

CREATE TRIGGER worker_turns_of_worker BEFORE INSERT ON worker_turns
WHEN NEW.role IS NOT (SELECT role FROM worker_processes WHERE id = NEW.worker_process_id)
    OR NEW.area_id IS NOT (SELECT area_id FROM worker_processes WHERE id = NEW.worker_process_id)
BEGIN
    SELECT RAISE(ABORT, 'worker_turns_of_worker');
END;

CREATE TRIGGER worker_turns_no_update BEFORE UPDATE ON worker_turns
BEGIN
    SELECT RAISE(ABORT, 'worker_turns is append-only');
END;

CREATE TRIGGER worker_turns_no_delete BEFORE DELETE ON worker_turns
BEGIN
    SELECT RAISE(ABORT, 'worker_turns is append-only');
END;
