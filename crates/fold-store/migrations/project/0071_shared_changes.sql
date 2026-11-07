-- A change of shared reference files that fold saw during a supervised worker's tool call: the worker reads the files through links to the project folder, where a write has no history to undo it. `paths` is the JSON list of the changed files relative to the project folder, shown in Needs you only. The item waits until a human, or the main agent when the human asks, dismisses it.
CREATE TABLE shared_changes (
    id INTEGER PRIMARY KEY,
    agent_id INTEGER NOT NULL REFERENCES agents (id),
    paths TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    dismissed_at INTEGER,
    dismissed_by TEXT,
    CONSTRAINT shared_changes_dismissed_together CHECK ((dismissed_at IS NULL) = (dismissed_by IS NULL))
);

CREATE INDEX shared_changes_open ON shared_changes (dismissed_at, id);
