-- The hash of each MCP tool's schema as a daemon binary served it, keyed by the fingerprint of that binary (`worker_processes.fold_fingerprint`). After an upgrade the daemon knows only its own tools, so the hashes of the old binary are read from here to name the tools whose schema changed. A fingerprint's rows are written once.
CREATE TABLE tool_schema_hashes (
    fingerprint TEXT NOT NULL,
    tool TEXT NOT NULL,
    hash TEXT NOT NULL,
    PRIMARY KEY (fingerprint, tool)
) WITHOUT ROWID;

CREATE TRIGGER tool_schema_hashes_no_update BEFORE UPDATE ON tool_schema_hashes
BEGIN
    SELECT RAISE(ABORT, 'tool_schema_hashes is append-only');
END;

CREATE TRIGGER tool_schema_hashes_no_delete BEFORE DELETE ON tool_schema_hashes
BEGIN
    SELECT RAISE(ABORT, 'tool_schema_hashes is append-only');
END;
