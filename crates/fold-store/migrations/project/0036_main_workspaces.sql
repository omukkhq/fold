-- The main agent changes main directly, on the user's word, from its own main workspace: a workspace of kind `main` that belongs to no task. Its changesets have no task either, and an integration of kind `direct` brings one of them onto main with the Git commit message the main agent wrote. SQLite cannot change a CHECK or make a column nullable in place, so `workspaces`, `changesets` and `integrations` are made again with their rows, ids, indexes and triggers. The rows that refer to them lose their parent for a moment between the drop and the copy back, so their foreign key checks wait for the commit.
PRAGMA defer_foreign_keys = ON;

CREATE TABLE workspaces_copy AS SELECT * FROM workspaces;

DROP TABLE workspaces;

CREATE TABLE workspaces (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL UNIQUE,
    task_id INTEGER REFERENCES tasks (id),
    worker_id INTEGER REFERENCES workers (id),
    kind TEXT NOT NULL CONSTRAINT workspaces_kind_values CHECK (kind IN ('task', 'review', 'main')),
    path TEXT NOT NULL,
    base_revision_id INTEGER NOT NULL REFERENCES revisions (id),
    proposal_id INTEGER REFERENCES proposals (id),
    state TEXT NOT NULL CONSTRAINT workspaces_state_values CHECK (state IN ('active', 'missing', 'archived', 'abandoned')),
    changed_files INTEGER NOT NULL DEFAULT 0 CONSTRAINT workspaces_changed_files_count CHECK (changed_files >= 0),
    last_activity_at INTEGER,
    created_at INTEGER NOT NULL,
    closed_at INTEGER,
    sync_checkpoint_id INTEGER REFERENCES checkpoints (id),
    version_id INTEGER REFERENCES proposal_versions (id),
    review_batch_id INTEGER REFERENCES review_batches (id),
    directory_removed_at INTEGER,
    CONSTRAINT workspaces_task_unless_main CHECK ((kind = 'main') = (task_id IS NULL)),
    CONSTRAINT workspaces_main_no_review CHECK (
        kind <> 'main' OR (proposal_id IS NULL AND version_id IS NULL AND review_batch_id IS NULL)
    )
);

INSERT INTO workspaces (
    id, number, task_id, worker_id, kind, path, base_revision_id, proposal_id, state, changed_files,
    last_activity_at, created_at, closed_at, sync_checkpoint_id, version_id, review_batch_id, directory_removed_at
)
SELECT
    id, number, task_id, worker_id, kind, path, base_revision_id, proposal_id, state, changed_files,
    last_activity_at, created_at, closed_at, sync_checkpoint_id, version_id, review_batch_id, directory_removed_at
FROM workspaces_copy;

DROP TABLE workspaces_copy;

CREATE UNIQUE INDEX workspaces_one_live_task_workspace ON workspaces (task_id)
    WHERE kind = 'task' AND state IN ('active', 'missing');
CREATE INDEX workspaces_worker ON workspaces (worker_id);
CREATE INDEX workspaces_task ON workspaces (task_id);
CREATE UNIQUE INDEX workspaces_path ON workspaces (path);
CREATE UNIQUE INDEX workspaces_one_active_review_checkout ON workspaces (version_id, worker_id)
    WHERE kind = 'review' AND state = 'active';
CREATE UNIQUE INDEX workspaces_one_active_batch_checkout ON workspaces (review_batch_id)
    WHERE review_batch_id IS NOT NULL AND state = 'active';

-- A project has one live main workspace, whichever session holds the main tag.
CREATE UNIQUE INDEX workspaces_one_live_main_workspace ON workspaces (kind)
    WHERE kind = 'main' AND state IN ('active', 'missing');

CREATE TRIGGER workspaces_review_target_insert BEFORE INSERT ON workspaces
WHEN NEW.review_batch_id IS NOT NULL AND (NEW.kind <> 'review' OR NEW.proposal_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'workspaces_review_target');
END;

CREATE TRIGGER workspaces_review_target_update BEFORE UPDATE OF review_batch_id, proposal_id, kind ON workspaces
WHEN NEW.review_batch_id IS NOT NULL AND (NEW.kind <> 'review' OR NEW.proposal_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'workspaces_review_target');
END;

CREATE TRIGGER workspaces_sync_checkpoint_own BEFORE UPDATE OF sync_checkpoint_id ON workspaces
WHEN NEW.sync_checkpoint_id IS NOT NULL
    AND (SELECT workspace_id FROM checkpoints WHERE id = NEW.sync_checkpoint_id) IS NOT NEW.id
BEGIN
    SELECT RAISE(ABORT, 'workspaces_sync_checkpoint_own');
END;

CREATE TRIGGER workspaces_task_needs_proposal_kind BEFORE INSERT ON workspaces
WHEN NEW.kind = 'task' AND (SELECT kind FROM tasks WHERE id = NEW.task_id) IN ('no_proposal', 'human')
BEGIN
    SELECT RAISE(ABORT, 'workspaces_task_needs_proposal_kind');
END;

-- A workspace never changes its task, so with `workspaces_task_unless_main` a main workspace never gains a task and never becomes another kind, and no other workspace becomes the main workspace.
CREATE TRIGGER workspaces_task_fixed BEFORE UPDATE OF task_id ON workspaces
WHEN NEW.task_id IS NOT OLD.task_id
BEGIN
    SELECT RAISE(ABORT, 'workspaces_task_fixed');
END;

CREATE TABLE changesets_copy AS SELECT * FROM changesets;

DROP TABLE changesets;

CREATE TABLE changesets (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL UNIQUE,
    workspace_id INTEGER NOT NULL REFERENCES workspaces (id),
    task_id INTEGER REFERENCES tasks (id),
    parent_changeset_id INTEGER REFERENCES changesets (id),
    base_revision_id INTEGER NOT NULL REFERENCES revisions (id),
    tree_hash TEXT NOT NULL,
    message TEXT NOT NULL,
    stats TEXT NOT NULL DEFAULT '{}',
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

INSERT INTO changesets (
    id, number, workspace_id, task_id, parent_changeset_id, base_revision_id, tree_hash, message, stats, created_by, created_at
)
SELECT
    id, number, workspace_id, task_id, parent_changeset_id, base_revision_id, tree_hash, message, stats, created_by, created_at
FROM changesets_copy;

DROP TABLE changesets_copy;

CREATE INDEX changesets_workspace ON changesets (workspace_id);
CREATE INDEX changesets_task ON changesets (task_id);

CREATE TRIGGER changesets_no_update BEFORE UPDATE ON changesets
BEGIN
    SELECT RAISE(ABORT, 'changesets is append-only');
END;

CREATE TRIGGER changesets_no_delete BEFORE DELETE ON changesets
BEGIN
    SELECT RAISE(ABORT, 'changesets is append-only');
END;

CREATE TRIGGER changesets_task_workspaces_only BEFORE INSERT ON changesets
WHEN (SELECT kind FROM workspaces WHERE id = NEW.workspace_id) = 'review'
BEGIN
    SELECT RAISE(ABORT, 'changesets_task_workspaces_only');
END;

-- The task of a changeset is its workspace's task, and none for a main workspace.
CREATE TRIGGER changesets_task_of_workspace BEFORE INSERT ON changesets
WHEN NEW.task_id IS NOT (SELECT task_id FROM workspaces WHERE id = NEW.workspace_id)
BEGIN
    SELECT RAISE(ABORT, 'changesets_task_of_workspace');
END;

CREATE TRIGGER changesets_parent_same_workspace BEFORE INSERT ON changesets
WHEN NEW.parent_changeset_id IS NOT NULL
    AND (SELECT workspace_id FROM changesets WHERE id = NEW.parent_changeset_id) IS NOT NEW.workspace_id
BEGIN
    SELECT RAISE(ABORT, 'changesets_parent_same_workspace');
END;

CREATE TABLE integrations_copy AS SELECT * FROM integrations;

DROP TABLE integrations;

CREATE TABLE integrations (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL CONSTRAINT integrations_kind_values CHECK (kind IN ('proposal', 'pull', 'direct')),
    proposal_id INTEGER REFERENCES proposals (id),
    version_id INTEGER REFERENCES proposal_versions (id),
    remote_commit TEXT,
    result TEXT NOT NULL CONSTRAINT integrations_result_values CHECK (result IN ('succeeded', 'conflicted', 'refused')),
    revision_id INTEGER REFERENCES revisions (id) DEFERRABLE INITIALLY DEFERRED,
    report TEXT NOT NULL DEFAULT '{}',
    actor TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    number INTEGER NOT NULL DEFAULT 0 CONSTRAINT integrations_number_positive CHECK (number >= 1),
    -- The changeset of the main workspace that a direct change brought onto main, and the Git commit message the main agent wrote for it.
    changeset_id INTEGER REFERENCES changesets (id),
    message TEXT,
    CONSTRAINT integrations_columns_match_kind CHECK (
        (kind = 'proposal' AND proposal_id IS NOT NULL AND version_id IS NOT NULL
            AND changeset_id IS NULL AND message IS NULL)
        OR (kind = 'pull' AND proposal_id IS NULL AND version_id IS NULL AND remote_commit IS NOT NULL
            AND changeset_id IS NULL AND message IS NULL)
        OR (kind = 'direct' AND proposal_id IS NULL AND version_id IS NULL AND remote_commit IS NULL
            AND changeset_id IS NOT NULL AND message IS NOT NULL AND trim(message) <> '' AND result = 'succeeded')
    )
);

INSERT INTO integrations (
    id, kind, proposal_id, version_id, remote_commit, result, revision_id, report, actor, created_at, number
)
SELECT
    id, kind, proposal_id, version_id, remote_commit, result, revision_id, report, actor, created_at, number
FROM integrations_copy;

DROP TABLE integrations_copy;

CREATE INDEX integrations_proposal ON integrations (proposal_id);
CREATE UNIQUE INDEX integrations_number ON integrations (number);
CREATE UNIQUE INDEX integrations_one_succeeded_per_proposal ON integrations (proposal_id)
    WHERE kind = 'proposal' AND result = 'succeeded';
CREATE INDEX integrations_version ON integrations (version_id) WHERE version_id IS NOT NULL;

-- A changeset reaches main directly at most once.
CREATE UNIQUE INDEX integrations_one_per_changeset ON integrations (changeset_id)
    WHERE changeset_id IS NOT NULL;

CREATE TRIGGER integrations_no_update BEFORE UPDATE ON integrations
BEGIN
    SELECT RAISE(ABORT, 'integrations is append-only');
END;

CREATE TRIGGER integrations_no_delete BEFORE DELETE ON integrations
BEGIN
    SELECT RAISE(ABORT, 'integrations is append-only');
END;

-- Only a changeset of a main workspace reaches main directly; task work goes through a proposal.
CREATE TRIGGER integrations_direct_main_changeset BEFORE INSERT ON integrations
WHEN NEW.kind = 'direct'
    AND (SELECT w.kind FROM changesets c JOIN workspaces w ON w.id = c.workspace_id WHERE c.id = NEW.changeset_id) IS NOT 'main'
BEGIN
    SELECT RAISE(ABORT, 'integrations_direct_main_changeset');
END;

-- A proposal offers task work, so its versions never hold a changeset of a main workspace.
CREATE TRIGGER proposal_versions_task_changeset BEFORE INSERT ON proposal_versions
WHEN (SELECT task_id FROM changesets WHERE id = NEW.head_changeset_id) IS NULL
BEGIN
    SELECT RAISE(ABORT, 'proposal_versions_task_changeset');
END;
