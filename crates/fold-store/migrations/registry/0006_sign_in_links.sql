-- A one-time sign-in link opens one session for its human, before it expires. The registry holds only the hash of the link's secret.
CREATE TABLE sign_in_links (
    id INTEGER PRIMARY KEY,
    link_hash BLOB NOT NULL UNIQUE,
    human_id INTEGER NOT NULL REFERENCES humans (id),
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    CHECK (expires_at > created_at)
);

CREATE INDEX sign_in_links_expiry ON sign_in_links (expires_at);

-- A session that a sign-in link opened has no token, so `token_id` takes NULL.
CREATE TABLE web_sessions_any_source (
    id INTEGER PRIMARY KEY,
    session_hash BLOB NOT NULL UNIQUE,
    token_id INTEGER REFERENCES web_tokens (id) ON DELETE CASCADE,
    human_id INTEGER NOT NULL REFERENCES humans (id),
    created_at INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    CHECK (expires_at > created_at)
);

INSERT INTO web_sessions_any_source (id, session_hash, token_id, human_id, created_at, last_used_at, expires_at)
SELECT id, session_hash, token_id, human_id, created_at, last_used_at, expires_at FROM web_sessions;

DROP TABLE web_sessions;

ALTER TABLE web_sessions_any_source RENAME TO web_sessions;

CREATE INDEX web_sessions_token ON web_sessions (token_id);

-- A session made from a token acts for the human of that token.
CREATE TRIGGER web_sessions_human_of_token_insert BEFORE INSERT ON web_sessions
WHEN NEW.token_id IS NOT NULL
    AND NEW.human_id IS NOT (SELECT human_id FROM web_tokens WHERE id = NEW.token_id)
BEGIN
    SELECT RAISE(ABORT, 'web_sessions_human_of_token');
END;

CREATE TRIGGER web_sessions_human_of_token_update BEFORE UPDATE OF human_id, token_id ON web_sessions
WHEN NEW.token_id IS NOT NULL
    AND NEW.human_id IS NOT (SELECT human_id FROM web_tokens WHERE id = NEW.token_id)
BEGIN
    SELECT RAISE(ABORT, 'web_sessions_human_of_token');
END;
