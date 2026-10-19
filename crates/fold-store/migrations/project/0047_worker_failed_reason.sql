-- Why a worker's latest process failed, such as `exited` or `mcp_unavailable`, so the human's list of what waits for them and the agent views can tell a failure that a person must fix from one that a new start fixes. A row that failed before the column existed has none.
ALTER TABLE worker_processes ADD COLUMN failed_reason TEXT
    CONSTRAINT worker_processes_failed_reason_only_failed CHECK (failed_reason IS NULL OR state = 'failed');
