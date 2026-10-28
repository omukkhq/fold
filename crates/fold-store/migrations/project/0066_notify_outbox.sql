-- The notifier's place in the event stream and the next number of a send. One row: the cursor moves in the same transaction as the outbox rows of the events it passed, so a restart neither loses a notice nor queues one twice.
CREATE TABLE notify_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_seq INTEGER NOT NULL CHECK (last_seq >= 0),
    next_send INTEGER NOT NULL DEFAULT 1 CHECK (next_send >= 1)
);

-- The Needs you items the notifier found open, by the key that names each one across reads, such as `question:m-31`, with what a `needs_you.closed` notice names. An item that leaves Needs you loses its row, so one that comes back is news again.
CREATE TABLE notify_needs_you (
    key TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    item TEXT,
    path TEXT NOT NULL,
    seen_at INTEGER NOT NULL
);

-- One notice that waits for its target's next send, in the order of the events: `notice` is the notice as fold-core builds it, ids, titles and links only.
CREATE TABLE notify_outbox (
    id INTEGER PRIMARY KEY,
    target INTEGER NOT NULL CHECK (target BETWEEN 1 AND 4294967295),
    seq INTEGER NOT NULL CHECK (seq >= 0),
    notice TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX notify_outbox_target ON notify_outbox (target, id);

-- One send to a target: its body, fixed when the window closed, and the webhook id that every try of it carries, so a receiver drops a repeat. `sending` marks a try in flight; a restart sends it again with the same id. The send log is the newest rows of each target.
CREATE TABLE notify_sends (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL UNIQUE CHECK (number >= 1),
    target INTEGER NOT NULL CHECK (target BETWEEN 1 AND 4294967295),
    webhook_id TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL CONSTRAINT notify_sends_state_values CHECK (state IN ('pending', 'sending', 'delivered', 'given_up')),
    tries INTEGER NOT NULL DEFAULT 0 CHECK (tries >= 0),
    next_try_at INTEGER,
    body BLOB NOT NULL,
    summary TEXT NOT NULL,
    notices INTEGER NOT NULL CHECK (notices >= 0),
    test INTEGER NOT NULL DEFAULT 0 CHECK (test IN (0, 1)),
    created_at INTEGER NOT NULL,
    last_try_at INTEGER,
    status INTEGER,
    error TEXT,
    duration_ms INTEGER
);

CREATE INDEX notify_sends_target ON notify_sends (target, id);
