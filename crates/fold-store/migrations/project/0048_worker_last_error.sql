-- The error of a worker's latest failed turn or failed exit, as Claude Code wrote it, so a human and the agents can see why a worker that fails every turn does nothing. `failed_turns` counts the turns in a row that failed before any token reached the model; a turn that ends without an error sets it back to 0 and clears the error. The text is bounded, and it outlives a restart of the worker, because a restart does not fix what Claude Code reported.
ALTER TABLE worker_processes ADD COLUMN last_error TEXT
    CONSTRAINT worker_processes_last_error_length CHECK (last_error IS NULL OR length(last_error) BETWEEN 1 AND 1000);

ALTER TABLE worker_processes ADD COLUMN last_error_at INTEGER
    CONSTRAINT worker_processes_last_error_at_iff_error CHECK ((last_error IS NULL) = (last_error_at IS NULL));

ALTER TABLE worker_processes ADD COLUMN failed_turns INTEGER NOT NULL DEFAULT 0
    CONSTRAINT worker_processes_failed_turns_count CHECK (failed_turns >= 0);
