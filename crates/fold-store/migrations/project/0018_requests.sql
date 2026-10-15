-- A report, feature wish or question about the product, which an orchestrator turns into card drafts and the requester approves into tasks. `requester` is an agent name or `human:<name>`: every worker of that agent acts as the requester. The requester writes only the message: `kind` and `title` stay empty until the claiming orchestrator sets them.
CREATE TABLE requests (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL UNIQUE,
    kind TEXT CONSTRAINT requests_kind_values CHECK (kind IS NULL OR kind IN ('bug', 'feature', 'question')),
    title TEXT CONSTRAINT requests_title_single_line CHECK (
        title IS NULL OR (trim(title) <> '' AND instr(title, char(10)) = 0 AND instr(title, char(13)) = 0)
    ),
    body TEXT NOT NULL CONSTRAINT requests_body_present CHECK (trim(body) <> ''),
    state TEXT NOT NULL DEFAULT 'open'
        CONSTRAINT requests_state_values CHECK (state IN ('open', 'drafting', 'draft_ready', 'changes_requested', 'approved', 'closed')),
    requester TEXT NOT NULL CONSTRAINT requests_requester_present CHECK (requester <> ''),
    drafter_worker_id INTEGER REFERENCES workers (id),
    drafter_since INTEGER,
    close_reason TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CONSTRAINT requests_drafter_since_with_drafter CHECK ((drafter_worker_id IS NULL) = (drafter_since IS NULL)),
    -- An open request waits for a drafter, and a drafting one has one.
    CONSTRAINT requests_drafter_matches_state CHECK (
        (state <> 'open' OR drafter_worker_id IS NULL) AND (state <> 'drafting' OR drafter_worker_id IS NOT NULL)
    ),
    -- A claim names and sorts the request, so only a request nobody has claimed yet, still open or closed unclaimed, lacks a kind or a title.
    CONSTRAINT requests_sorted_once_claimed CHECK ((kind IS NOT NULL AND title IS NOT NULL) OR state IN ('open', 'closed')),
    CONSTRAINT requests_close_reason_iff_closed CHECK ((state = 'closed') = (close_reason IS NOT NULL AND trim(close_reason) <> ''))
);

CREATE INDEX requests_state ON requests (state);
CREATE INDEX requests_requester ON requests (requester);
CREATE INDEX requests_drafter ON requests (drafter_worker_id) WHERE drafter_worker_id IS NOT NULL;

-- A file dropped on the request. `object` is the hash of its blob in the project object store, written before this row.
CREATE TABLE request_attachments (
    id INTEGER PRIMARY KEY,
    request_id INTEGER NOT NULL REFERENCES requests (id),
    position INTEGER NOT NULL CONSTRAINT request_attachments_position_range CHECK (position BETWEEN 1 AND 10),
    name TEXT NOT NULL CONSTRAINT request_attachments_plain_name CHECK (
        length(CAST(name AS BLOB)) BETWEEN 1 AND 255 AND instr(name, '/') = 0 AND instr(name, '..') = 0
    ),
    media_type TEXT NOT NULL CONSTRAINT request_attachments_media_type_present CHECK (instr(media_type, '/') > 1),
    size INTEGER NOT NULL CONSTRAINT request_attachments_size_range CHECK (size BETWEEN 0 AND 10485760),
    object TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (request_id, position)
);

-- One version of a request's card draft, written by the drafter or by the requester. `author` is the actor label.
CREATE TABLE request_drafts (
    id INTEGER PRIMARY KEY,
    request_id INTEGER NOT NULL REFERENCES requests (id),
    version INTEGER NOT NULL CONSTRAINT request_drafts_version_positive CHECK (version >= 1),
    author TEXT NOT NULL CONSTRAINT request_drafts_author_present CHECK (author <> ''),
    note TEXT,
    created_at INTEGER NOT NULL,
    UNIQUE (request_id, version)
);

-- `key` is local to its draft, so an edge names its cards by key and two versions may reuse a key.
CREATE TABLE request_draft_cards (
    id INTEGER PRIMARY KEY,
    draft_id INTEGER NOT NULL REFERENCES request_drafts (id),
    key TEXT NOT NULL CONSTRAINT request_draft_cards_key_present CHECK (key <> ''),
    position INTEGER NOT NULL CONSTRAINT request_draft_cards_position_positive CHECK (position >= 1),
    title TEXT NOT NULL CONSTRAINT request_draft_cards_title_single_line CHECK (
        trim(title) <> '' AND instr(title, char(10)) = 0 AND instr(title, char(13)) = 0
    ),
    description TEXT NOT NULL,
    UNIQUE (draft_id, key),
    UNIQUE (draft_id, position)
);

-- The dependent card needs the prerequisite card, as a dependency edge between the tasks an approval creates.
CREATE TABLE request_draft_edges (
    id INTEGER PRIMARY KEY,
    draft_id INTEGER NOT NULL REFERENCES request_drafts (id),
    prerequisite_key TEXT NOT NULL,
    dependent_key TEXT NOT NULL,
    CONSTRAINT request_draft_edges_no_self_edge CHECK (prerequisite_key <> dependent_key),
    UNIQUE (draft_id, prerequisite_key, dependent_key),
    FOREIGN KEY (draft_id, prerequisite_key) REFERENCES request_draft_cards (draft_id, key),
    FOREIGN KEY (draft_id, dependent_key) REFERENCES request_draft_cards (draft_id, key)
);

-- A requester's comment on one draft version, written when the requester asks for changes.
CREATE TABLE request_comments (
    id INTEGER PRIMARY KEY,
    request_id INTEGER NOT NULL,
    version INTEGER NOT NULL,
    author TEXT NOT NULL CONSTRAINT request_comments_author_present CHECK (author <> ''),
    body TEXT NOT NULL CONSTRAINT request_comments_body_present CHECK (trim(body) <> ''),
    created_at INTEGER NOT NULL,
    FOREIGN KEY (request_id, version) REFERENCES request_drafts (request_id, version)
);

CREATE INDEX request_comments_request ON request_comments (request_id);

-- The request whose approval created the task, set in the transaction that creates it.
ALTER TABLE tasks ADD COLUMN request_id INTEGER REFERENCES requests (id);

CREATE INDEX tasks_request ON tasks (request_id) WHERE request_id IS NOT NULL;

CREATE TRIGGER tasks_request_fixed
BEFORE UPDATE OF request_id ON tasks
WHEN OLD.request_id IS NOT NULL AND NEW.request_id IS NOT OLD.request_id
BEGIN
    SELECT RAISE(ABORT, 'tasks.request_id never changes once set');
END;

-- The request a message clarifies; its thread belongs to that request.
ALTER TABLE messages ADD COLUMN request_id INTEGER REFERENCES requests (id);

CREATE INDEX messages_request ON messages (request_id, number) WHERE request_id IS NOT NULL;

CREATE TRIGGER request_attachments_no_update BEFORE UPDATE ON request_attachments
BEGIN
    SELECT RAISE(ABORT, 'request_attachments is append-only');
END;

CREATE TRIGGER request_attachments_no_delete BEFORE DELETE ON request_attachments
BEGIN
    SELECT RAISE(ABORT, 'request_attachments is append-only');
END;

CREATE TRIGGER request_drafts_no_update BEFORE UPDATE ON request_drafts
BEGIN
    SELECT RAISE(ABORT, 'request_drafts is append-only');
END;

CREATE TRIGGER request_drafts_no_delete BEFORE DELETE ON request_drafts
BEGIN
    SELECT RAISE(ABORT, 'request_drafts is append-only');
END;

CREATE TRIGGER request_draft_cards_no_update BEFORE UPDATE ON request_draft_cards
BEGIN
    SELECT RAISE(ABORT, 'request_draft_cards is append-only');
END;

CREATE TRIGGER request_draft_cards_no_delete BEFORE DELETE ON request_draft_cards
BEGIN
    SELECT RAISE(ABORT, 'request_draft_cards is append-only');
END;

CREATE TRIGGER request_draft_edges_no_update BEFORE UPDATE ON request_draft_edges
BEGIN
    SELECT RAISE(ABORT, 'request_draft_edges is append-only');
END;

CREATE TRIGGER request_draft_edges_no_delete BEFORE DELETE ON request_draft_edges
BEGIN
    SELECT RAISE(ABORT, 'request_draft_edges is append-only');
END;

CREATE TRIGGER request_comments_no_update BEFORE UPDATE ON request_comments
BEGIN
    SELECT RAISE(ABORT, 'request_comments is append-only');
END;

CREATE TRIGGER request_comments_no_delete BEFORE DELETE ON request_comments
BEGIN
    SELECT RAISE(ABORT, 'request_comments is append-only');
END;
