-- A path of the project folder that changed during a tool call of a supervised worker, so fold does not offer it as the human's change until a human, or the main agent when the human asks, settles it. A path that changed during the calls of two agents has a row for each. `changed_at` is the file's time that placed the change in the call. A row is open until it is `ignored` (the human takes the change as their own), `restored` (the path went into the stash and back to the checkout record's entry), `recorded` (the human recorded it knowing a worker made it) or `settled` (the folder holds the checkout record's entry again by any other way).
CREATE TABLE agent_changes (
    id INTEGER PRIMARY KEY,
    path BLOB NOT NULL CONSTRAINT agent_changes_path_present CHECK (length(path) > 0),
    agent_id INTEGER NOT NULL REFERENCES agents (id),
    changed_at INTEGER NOT NULL,
    seen_at INTEGER NOT NULL,
    cleared_at INTEGER,
    cleared_how TEXT CONSTRAINT agent_changes_cleared_values CHECK (cleared_how IN ('ignored', 'restored', 'recorded', 'settled')),
    CONSTRAINT agent_changes_cleared_together CHECK ((cleared_at IS NULL) = (cleared_how IS NULL))
);

CREATE UNIQUE INDEX agent_changes_one_open ON agent_changes (path, agent_id) WHERE cleared_at IS NULL;
