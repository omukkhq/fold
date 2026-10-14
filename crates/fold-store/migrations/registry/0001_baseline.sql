CREATE TABLE projects (
    id INTEGER PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    root TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
);

CREATE TABLE humans (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
);

CREATE TABLE web_tokens (
    id INTEGER PRIMARY KEY,
    token_hash BLOB NOT NULL UNIQUE,
    human_id INTEGER NOT NULL REFERENCES humans (id),
    label TEXT,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER
);

CREATE INDEX web_tokens_human ON web_tokens (human_id);
