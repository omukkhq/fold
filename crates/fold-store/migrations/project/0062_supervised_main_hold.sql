-- When a human stopped the supervised main. fold starts a stopped main for its work only after a stop that fold made itself, so a row with it stays stopped until a human starts it again; every start of the row clears it.
ALTER TABLE worker_processes ADD COLUMN held_at INTEGER
    CONSTRAINT worker_processes_held_only_stopped_main CHECK (held_at IS NULL OR (role = 'main' AND state IN ('stopping', 'stopped', 'failed')));
