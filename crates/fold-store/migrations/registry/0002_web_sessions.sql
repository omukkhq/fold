CREATE TABLE web_tokens_numbered (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL UNIQUE CHECK (number BETWEEN 1 AND 4294967295),
    token_hash BLOB NOT NULL UNIQUE,
    human_id INTEGER NOT NULL REFERENCES humans (id),
    label TEXT,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER,
    revoked_at INTEGER
);

INSERT INTO web_tokens_numbered (id, number, token_hash, human_id, label, created_at, last_used_at)
SELECT id, id, token_hash, human_id, label, created_at, last_used_at FROM web_tokens;

DROP TABLE web_tokens;

ALTER TABLE web_tokens_numbered RENAME TO web_tokens;

CREATE INDEX web_tokens_human ON web_tokens (human_id);

CREATE TABLE web_sessions (
    id INTEGER PRIMARY KEY,
    session_hash BLOB NOT NULL UNIQUE,
    token_id INTEGER NOT NULL REFERENCES web_tokens (id) ON DELETE CASCADE,
    human_id INTEGER NOT NULL REFERENCES humans (id),
    created_at INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    CHECK (expires_at > created_at)
);

CREATE INDEX web_sessions_token ON web_sessions (token_id);
