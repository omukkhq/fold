-- The last push fold made to each remote, by the remote's name in `.fold/config.toml`. A push that reaches the remote sets `pushed_revision_id` and `pushed_at` and clears the failure; a failed push sets `failure_code` and `failed_at` and keeps the last success. Each change is written in the transaction of its `remote.pushed` or `remote.push_failed` event.
CREATE TABLE remote_states (
    name TEXT PRIMARY KEY CONSTRAINT remote_states_name_present CHECK (name <> ''),
    pushed_revision_id INTEGER REFERENCES revisions (id),
    pushed_at INTEGER,
    failure_code TEXT CONSTRAINT remote_states_failure_code_present CHECK (failure_code <> ''),
    failed_at INTEGER,
    CONSTRAINT remote_states_pushed_together CHECK ((pushed_revision_id IS NULL) = (pushed_at IS NULL)),
    CONSTRAINT remote_states_failed_together CHECK ((failure_code IS NULL) = (failed_at IS NULL))
);
