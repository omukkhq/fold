-- Times are UTC milliseconds. `id` is the SQLite row id and never leaves the daemon; public ids are built from `number`.
-- Named CHECK constraints let a service map a violation to its domain code through the constraint name in the error.

CREATE TABLE counters (
    kind TEXT PRIMARY KEY,
    next INTEGER NOT NULL CONSTRAINT counters_next_positive CHECK (next >= 1)
);

CREATE TABLE agents (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
);

CREATE TABLE agent_tags (
    agent_id INTEGER NOT NULL REFERENCES agents (id),
    tag TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (agent_id, tag)
);

CREATE TABLE sessions (
    id INTEGER PRIMARY KEY,
    agent_id INTEGER REFERENCES agents (id),
    pid INTEGER NOT NULL,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    last_call_at INTEGER
);

CREATE INDEX sessions_agent ON sessions (agent_id);
CREATE INDEX sessions_open ON sessions (ended_at) WHERE ended_at IS NULL;

CREATE TABLE workers (
    id INTEGER PRIMARY KEY,
    agent_id INTEGER NOT NULL REFERENCES agents (id),
    name TEXT NOT NULL,
    parent_worker_id INTEGER REFERENCES workers (id),
    last_call_at INTEGER,
    activity TEXT,
    created_at INTEGER NOT NULL,
    UNIQUE (agent_id, name)
);

CREATE TABLE labels (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    color TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE tasks (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL UNIQUE,
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    archived_at INTEGER,
    human_review INTEGER NOT NULL DEFAULT 0 CONSTRAINT tasks_human_review_flag CHECK (human_review IN (0, 1)),
    human_reviewer TEXT,
    human_review_reason TEXT,
    human_review_set_by TEXT,
    status TEXT NOT NULL DEFAULT 'backlog'
        CONSTRAINT tasks_status_values CHECK (status IN ('backlog', 'todo', 'in_progress', 'review', 'done')),
    blocked INTEGER NOT NULL DEFAULT 0 CONSTRAINT tasks_blocked_flag CHECK (blocked IN (0, 1))
);

CREATE INDEX tasks_status ON tasks (status);

CREATE TABLE task_labels (
    task_id INTEGER NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    label_id INTEGER NOT NULL REFERENCES labels (id),
    PRIMARY KEY (task_id, label_id)
);

CREATE INDEX task_labels_label ON task_labels (label_id);

CREATE TABLE checklists (
    id INTEGER PRIMARY KEY,
    task_id INTEGER NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    position INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX checklists_task ON checklists (task_id);

CREATE TABLE checklist_items (
    id INTEGER PRIMARY KEY,
    checklist_id INTEGER NOT NULL REFERENCES checklists (id) ON DELETE CASCADE,
    text TEXT,
    linked_task_id INTEGER REFERENCES tasks (id),
    position INTEGER NOT NULL,
    done_at INTEGER,
    done_by TEXT,
    created_at INTEGER NOT NULL,
    CONSTRAINT checklist_items_text_or_link CHECK ((text IS NULL) <> (linked_task_id IS NULL))
);

CREATE INDEX checklist_items_checklist ON checklist_items (checklist_id);
CREATE INDEX checklist_items_linked ON checklist_items (linked_task_id) WHERE linked_task_id IS NOT NULL;

CREATE TABLE dependencies (
    task_id INTEGER NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    needs_task_id INTEGER NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    origin TEXT NOT NULL CONSTRAINT dependencies_origin_values CHECK (origin IN ('direct', 'checklist')),
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (task_id, needs_task_id),
    CONSTRAINT dependencies_no_self_edge CHECK (task_id <> needs_task_id)
);

CREATE INDEX dependencies_needs ON dependencies (needs_task_id);

CREATE TABLE revisions (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL UNIQUE,
    hash TEXT NOT NULL UNIQUE,
    tree_hash TEXT NOT NULL,
    parent_id INTEGER REFERENCES revisions (id),
    integration_id INTEGER REFERENCES integrations (id) DEFERRABLE INITIALLY DEFERRED,
    import_source TEXT,
    created_at INTEGER NOT NULL,
    CONSTRAINT revisions_import_source_first_only CHECK (import_source IS NULL OR number = 1)
);

CREATE TABLE refs (
    name TEXT PRIMARY KEY,
    revision_id INTEGER NOT NULL REFERENCES revisions (id)
);

CREATE TABLE workspaces (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL UNIQUE,
    task_id INTEGER NOT NULL REFERENCES tasks (id),
    worker_id INTEGER REFERENCES workers (id),
    kind TEXT NOT NULL CONSTRAINT workspaces_kind_values CHECK (kind IN ('task', 'review')),
    path TEXT NOT NULL,
    base_revision_id INTEGER NOT NULL REFERENCES revisions (id),
    proposal_id INTEGER REFERENCES proposals (id),
    state TEXT NOT NULL CONSTRAINT workspaces_state_values CHECK (state IN ('active', 'missing', 'archived', 'abandoned')),
    conflicted_paths INTEGER NOT NULL DEFAULT 0 CONSTRAINT workspaces_conflicted_paths_count CHECK (conflicted_paths >= 0),
    changed_files INTEGER NOT NULL DEFAULT 0 CONSTRAINT workspaces_changed_files_count CHECK (changed_files >= 0),
    last_activity_at INTEGER,
    created_at INTEGER NOT NULL,
    closed_at INTEGER
);

CREATE UNIQUE INDEX workspaces_one_live_task_workspace ON workspaces (task_id)
    WHERE kind = 'task' AND state IN ('active', 'missing');
CREATE INDEX workspaces_worker ON workspaces (worker_id);
CREATE INDEX workspaces_task ON workspaces (task_id);

CREATE TABLE checkpoints (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL UNIQUE,
    workspace_id INTEGER NOT NULL REFERENCES workspaces (id),
    tree_hash TEXT NOT NULL,
    kind TEXT NOT NULL CONSTRAINT checkpoints_kind_values CHECK (kind IN ('auto', 'explicit')),
    reason TEXT,
    label TEXT,
    created_at INTEGER NOT NULL,
    pruned_at INTEGER
);

CREATE INDEX checkpoints_workspace ON checkpoints (workspace_id);

CREATE TABLE changesets (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL UNIQUE,
    workspace_id INTEGER NOT NULL REFERENCES workspaces (id),
    task_id INTEGER NOT NULL REFERENCES tasks (id),
    parent_changeset_id INTEGER REFERENCES changesets (id),
    base_revision_id INTEGER NOT NULL REFERENCES revisions (id),
    tree_hash TEXT NOT NULL,
    message TEXT NOT NULL,
    stats TEXT NOT NULL DEFAULT '{}',
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX changesets_workspace ON changesets (workspace_id);
CREATE INDEX changesets_task ON changesets (task_id);

CREATE TABLE proposals (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL UNIQUE,
    task_id INTEGER NOT NULL REFERENCES tasks (id),
    author_worker_id INTEGER NOT NULL REFERENCES workers (id),
    title TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'open' CONSTRAINT proposals_state_values CHECK (state IN ('open', 'integrated', 'closed')),
    conflicted INTEGER NOT NULL DEFAULT 0 CONSTRAINT proposals_conflicted_flag CHECK (conflicted IN (0, 1)),
    close_reason TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX proposals_one_open_per_task ON proposals (task_id) WHERE state = 'open';
CREATE INDEX proposals_task ON proposals (task_id);

CREATE TABLE proposal_versions (
    id INTEGER PRIMARY KEY,
    proposal_id INTEGER NOT NULL REFERENCES proposals (id),
    version INTEGER NOT NULL CONSTRAINT proposal_versions_version_positive CHECK (version >= 1),
    base_revision_id INTEGER NOT NULL REFERENCES revisions (id),
    tree_hash TEXT NOT NULL,
    head_changeset_id INTEGER NOT NULL REFERENCES changesets (id),
    evidence TEXT,
    created_at INTEGER NOT NULL,
    UNIQUE (proposal_id, version)
);

CREATE TABLE claims (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL CONSTRAINT claims_kind_values CHECK (kind IN ('implementation', 'review')),
    task_id INTEGER REFERENCES tasks (id),
    proposal_id INTEGER REFERENCES proposals (id),
    worker_id INTEGER NOT NULL REFERENCES workers (id),
    acquired_at INTEGER NOT NULL,
    renewed_at INTEGER NOT NULL,
    stale_at INTEGER,
    released_at INTEGER,
    release_reason TEXT,
    CONSTRAINT claims_target_matches_kind CHECK (
        (kind = 'implementation' AND task_id IS NOT NULL AND proposal_id IS NULL)
        OR (kind = 'review' AND proposal_id IS NOT NULL AND task_id IS NULL)
    )
);

CREATE UNIQUE INDEX claims_one_unreleased_implementation ON claims (task_id)
    WHERE kind = 'implementation' AND released_at IS NULL;
CREATE UNIQUE INDEX claims_one_unreleased_review ON claims (proposal_id)
    WHERE kind = 'review' AND released_at IS NULL;
CREATE INDEX claims_worker ON claims (worker_id);

CREATE TABLE integrations (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL CONSTRAINT integrations_kind_values CHECK (kind IN ('proposal', 'pull')),
    proposal_id INTEGER REFERENCES proposals (id),
    version_id INTEGER REFERENCES proposal_versions (id),
    remote_commit TEXT,
    result TEXT NOT NULL CONSTRAINT integrations_result_values CHECK (result IN ('succeeded', 'conflicted', 'refused')),
    revision_id INTEGER REFERENCES revisions (id) DEFERRABLE INITIALLY DEFERRED,
    report TEXT NOT NULL DEFAULT '{}',
    actor TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    CONSTRAINT integrations_columns_match_kind CHECK (
        (kind = 'proposal' AND proposal_id IS NOT NULL AND version_id IS NOT NULL)
        OR (kind = 'pull' AND proposal_id IS NULL AND version_id IS NULL AND remote_commit IS NOT NULL)
    )
);

CREATE INDEX integrations_proposal ON integrations (proposal_id);

CREATE TABLE completions (
    task_id INTEGER PRIMARY KEY REFERENCES tasks (id),
    kind TEXT NOT NULL CONSTRAINT completions_kind_values CHECK (kind IN ('integration', 'children')),
    integration_id INTEGER REFERENCES integrations (id),
    created_at INTEGER NOT NULL,
    CONSTRAINT completions_integration_matches_kind CHECK (
        (kind = 'integration' AND integration_id IS NOT NULL)
        OR (kind = 'children' AND integration_id IS NULL)
    )
);

CREATE TABLE reviews (
    id INTEGER PRIMARY KEY,
    version_id INTEGER NOT NULL REFERENCES proposal_versions (id),
    reviewer_worker_id INTEGER REFERENCES workers (id),
    reviewer_human TEXT,
    verdict TEXT NOT NULL CONSTRAINT reviews_verdict_values CHECK (verdict IN ('approve', 'request_changes', 'comment')),
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    CONSTRAINT reviews_exactly_one_reviewer CHECK ((reviewer_worker_id IS NULL) <> (reviewer_human IS NULL))
);

CREATE INDEX reviews_version ON reviews (version_id);

CREATE TABLE review_comments (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL UNIQUE,
    version_id INTEGER NOT NULL REFERENCES proposal_versions (id),
    review_id INTEGER NOT NULL REFERENCES reviews (id),
    author_worker_id INTEGER REFERENCES workers (id),
    author_human TEXT,
    path TEXT,
    start_line INTEGER,
    end_line INTEGER,
    side TEXT CONSTRAINT review_comments_side_values CHECK (side IN ('old', 'new')),
    reply_to INTEGER REFERENCES review_comments (number),
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    CONSTRAINT review_comments_exactly_one_author CHECK ((author_worker_id IS NULL) <> (author_human IS NULL)),
    CONSTRAINT review_comments_lines_both_or_neither CHECK ((start_line IS NULL) = (end_line IS NULL)),
    CONSTRAINT review_comments_range_order CHECK (start_line IS NULL OR (start_line >= 1 AND start_line <= end_line))
);

CREATE INDEX review_comments_version ON review_comments (version_id);
CREATE INDEX review_comments_review ON review_comments (review_id);

CREATE TABLE messages (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL UNIQUE,
    thread_id INTEGER NOT NULL REFERENCES messages (number),
    reply_to INTEGER REFERENCES messages (number),
    sender_kind TEXT NOT NULL CONSTRAINT messages_sender_kind_values CHECK (sender_kind IN ('worker', 'human', 'fold')),
    sender_worker_id INTEGER REFERENCES workers (id),
    sender_human TEXT,
    task_id INTEGER REFERENCES tasks (id),
    proposal_id INTEGER REFERENCES proposals (id),
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    CONSTRAINT messages_sender_matches_kind CHECK (
        (sender_kind = 'worker' AND sender_worker_id IS NOT NULL AND sender_human IS NULL)
        OR (sender_kind = 'human' AND sender_human IS NOT NULL AND sender_worker_id IS NULL)
        OR (sender_kind = 'fold' AND sender_worker_id IS NULL AND sender_human IS NULL)
    )
);

CREATE INDEX messages_thread ON messages (thread_id);

CREATE TABLE message_recipients (
    id INTEGER PRIMARY KEY,
    message_id INTEGER NOT NULL REFERENCES messages (id),
    worker_id INTEGER REFERENCES workers (id),
    human TEXT,
    addressed_as TEXT NOT NULL
        CONSTRAINT message_recipients_addressed_as_values CHECK (addressed_as IN ('agent', 'worker', 'tag', 'all', 'human', 'humans')),
    delivered_at INTEGER,
    acked_at INTEGER,
    CONSTRAINT message_recipients_exactly_one_target CHECK ((worker_id IS NULL) <> (human IS NULL)),
    UNIQUE (message_id, worker_id),
    UNIQUE (message_id, human)
);

CREATE INDEX message_recipients_worker ON message_recipients (worker_id) WHERE worker_id IS NOT NULL;
CREATE INDEX message_recipients_human ON message_recipients (human) WHERE human IS NOT NULL;

-- The four reference columns hold public numbers and have no foreign key: history outlives a deleted task.
CREATE TABLE events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    type TEXT NOT NULL,
    actor TEXT NOT NULL,
    task_id INTEGER,
    proposal_id INTEGER,
    workspace_id INTEGER,
    revision_id INTEGER,
    data TEXT NOT NULL DEFAULT '{}' CONSTRAINT events_data_is_object CHECK (json_valid(data) AND json_type(data) = 'object'),
    created_at INTEGER NOT NULL
);

CREATE INDEX events_type_seq ON events (type, seq);
CREATE INDEX events_task_seq ON events (task_id, seq);
CREATE INDEX events_proposal_seq ON events (proposal_id, seq);

CREATE TABLE agent_cursors (
    agent_id INTEGER PRIMARY KEY REFERENCES agents (id),
    last_seq INTEGER NOT NULL DEFAULT 0
);

CREATE TRIGGER completions_no_update BEFORE UPDATE ON completions
BEGIN
    SELECT RAISE(ABORT, 'completions is append-only');
END;

CREATE TRIGGER completions_no_delete BEFORE DELETE ON completions
BEGIN
    SELECT RAISE(ABORT, 'completions is append-only');
END;

CREATE TRIGGER revisions_no_update BEFORE UPDATE ON revisions
BEGIN
    SELECT RAISE(ABORT, 'revisions is append-only');
END;

CREATE TRIGGER revisions_no_delete BEFORE DELETE ON revisions
BEGIN
    SELECT RAISE(ABORT, 'revisions is append-only');
END;

CREATE TRIGGER changesets_no_update BEFORE UPDATE ON changesets
BEGIN
    SELECT RAISE(ABORT, 'changesets is append-only');
END;

CREATE TRIGGER changesets_no_delete BEFORE DELETE ON changesets
BEGIN
    SELECT RAISE(ABORT, 'changesets is append-only');
END;

CREATE TRIGGER proposal_versions_no_update BEFORE UPDATE ON proposal_versions
BEGIN
    SELECT RAISE(ABORT, 'proposal_versions is append-only');
END;

CREATE TRIGGER proposal_versions_no_delete BEFORE DELETE ON proposal_versions
BEGIN
    SELECT RAISE(ABORT, 'proposal_versions is append-only');
END;

CREATE TRIGGER reviews_no_update BEFORE UPDATE ON reviews
BEGIN
    SELECT RAISE(ABORT, 'reviews is append-only');
END;

CREATE TRIGGER reviews_no_delete BEFORE DELETE ON reviews
BEGIN
    SELECT RAISE(ABORT, 'reviews is append-only');
END;

CREATE TRIGGER review_comments_no_update BEFORE UPDATE ON review_comments
BEGIN
    SELECT RAISE(ABORT, 'review_comments is append-only');
END;

CREATE TRIGGER review_comments_no_delete BEFORE DELETE ON review_comments
BEGIN
    SELECT RAISE(ABORT, 'review_comments is append-only');
END;

CREATE TRIGGER integrations_no_update BEFORE UPDATE ON integrations
BEGIN
    SELECT RAISE(ABORT, 'integrations is append-only');
END;

CREATE TRIGGER integrations_no_delete BEFORE DELETE ON integrations
BEGIN
    SELECT RAISE(ABORT, 'integrations is append-only');
END;

CREATE TRIGGER messages_no_update BEFORE UPDATE ON messages
BEGIN
    SELECT RAISE(ABORT, 'messages is append-only');
END;

CREATE TRIGGER messages_no_delete BEFORE DELETE ON messages
BEGIN
    SELECT RAISE(ABORT, 'messages is append-only');
END;

CREATE TRIGGER events_no_update BEFORE UPDATE ON events
BEGIN
    SELECT RAISE(ABORT, 'events is append-only');
END;

CREATE TRIGGER events_no_delete BEFORE DELETE ON events
BEGIN
    SELECT RAISE(ABORT, 'events is append-only');
END;
