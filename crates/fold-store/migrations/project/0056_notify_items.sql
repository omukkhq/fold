-- The Needs you items the notifier found open, by the key that names each one across reads, such as `question:m-31`. An item that leaves Needs you loses its row, so one that comes back is news again.
CREATE TABLE notify_items (
    key TEXT PRIMARY KEY,
    seen_at INTEGER NOT NULL
);

-- Each notification target that applies to the project, with its last send. The items open when a target starts to apply are not news to it, so they get rows in `notify_sent` without a send.
CREATE TABLE notify_targets_seen (
    target INTEGER PRIMARY KEY CHECK (target BETWEEN 1 AND 4294967295),
    last_sent_at INTEGER
);

-- The items each target got, or had no need of.
CREATE TABLE notify_sent (
    target INTEGER NOT NULL REFERENCES notify_targets_seen (target) ON DELETE CASCADE,
    key TEXT NOT NULL REFERENCES notify_items (key) ON DELETE CASCADE,
    PRIMARY KEY (target, key)
);
