-- Whether the agents of the project are paused. One row at most: the first pause writes it, and a resume clears `paused_at` and `paused_by` but keeps `resumed_at`, from which stall time counts again.
CREATE TABLE project_pause (
    id INTEGER PRIMARY KEY CONSTRAINT project_pause_single_row CHECK (id = 1),
    paused_at INTEGER,
    paused_by TEXT CONSTRAINT project_pause_by_present CHECK (paused_by <> ''),
    resumed_at INTEGER,
    CONSTRAINT project_pause_who_and_when CHECK ((paused_at IS NULL) = (paused_by IS NULL))
);
