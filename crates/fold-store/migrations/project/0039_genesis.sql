-- Every project starts from a genesis revision, `r0`: the empty tree for a plain folder, or the `HEAD` commit of a Git repository. The genesis is the one revision without a parent. A plain folder's files then reach main as the human's change, an integration of the new kind `record`, which also brings in each later change of the project folder: one that `fld record` reads from the folder, with no Git commit, or a Git commit the human made, whose id is `remote_commit`. SQLite cannot change a CHECK or make a column nullable in place, so `integrations` and `human_changes` are made again with their rows, ids, indexes and triggers; the rows that refer to them wait for the commit for their foreign key checks.
PRAGMA defer_foreign_keys = ON;

DROP TRIGGER revisions_first_has_no_parent;

CREATE TRIGGER revisions_first_has_no_parent BEFORE INSERT ON revisions
WHEN (NEW.parent_id IS NULL) <> (NOT EXISTS (SELECT 1 FROM revisions))
BEGIN
    SELECT RAISE(ABORT, 'revisions_genesis_only_without_parent');
END;

CREATE TABLE integrations_copy AS SELECT * FROM integrations;

DROP TABLE integrations;

CREATE TABLE integrations (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL CONSTRAINT integrations_kind_values CHECK (kind IN ('proposal', 'pull', 'direct', 'record')),
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
        OR (kind = 'record' AND proposal_id IS NULL AND version_id IS NULL
            AND changeset_id IS NULL AND message IS NULL AND result = 'succeeded')
    )
);

INSERT INTO integrations (
    id, kind, proposal_id, version_id, remote_commit, result, revision_id, report, actor, created_at, number,
    changeset_id, message
)
SELECT
    id, kind, proposal_id, version_id, remote_commit, result, revision_id, report, actor, created_at, number,
    changeset_id, message
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

-- A human change is a change of the project folder by a human: one that `fld record` read from the folder has no commit, and a Git commit the human made has its id. `base_tree_hash` is the tree the change is based on and `tree_hash` its own tree; both are in the object store before the row is written. A clean one is `recorded` with the `record` integration that brought it onto main. A conflicting one is `conflicted` until an integration brings its changes onto main (`resolved`), or until a newer human change takes its place (`superseded`).
CREATE TABLE human_changes_copy AS SELECT * FROM human_changes;

DROP TABLE human_changes;

CREATE TABLE human_changes (
    id INTEGER PRIMARY KEY,
    commit_id TEXT CONSTRAINT human_changes_commit_hex CHECK (commit_id IS NULL OR (length(commit_id) IN (40, 64) AND commit_id NOT GLOB '*[^0-9a-f]*')),
    human TEXT NOT NULL CONSTRAINT human_changes_human_named CHECK (trim(human) <> ''),
    base_tree_hash TEXT NOT NULL,
    tree_hash TEXT NOT NULL,
    state TEXT NOT NULL CONSTRAINT human_changes_state_values CHECK (state IN ('recorded', 'conflicted', 'resolved', 'superseded')),
    integration_id INTEGER REFERENCES integrations (id),
    workspace_id INTEGER REFERENCES workspaces (id),
    created_at INTEGER NOT NULL,
    closed_at INTEGER,
    CONSTRAINT human_changes_integration_when_integrated CHECK ((state IN ('recorded', 'resolved')) = (integration_id IS NOT NULL)),
    CONSTRAINT human_changes_closed_unless_conflicted CHECK ((state = 'conflicted') = (closed_at IS NULL))
);

INSERT INTO human_changes (
    id, commit_id, human, base_tree_hash, tree_hash, state, integration_id, workspace_id, created_at, closed_at
)
SELECT id, commit_id, human, base_tree_hash, tree_hash, state, integration_id, workspace_id, created_at, closed_at
FROM human_changes_copy;

DROP TABLE human_changes_copy;

-- One human change waits for the main agent at a time; a newer one takes its place.
CREATE UNIQUE INDEX human_changes_one_conflicted ON human_changes (state) WHERE state = 'conflicted';
CREATE UNIQUE INDEX human_changes_integration ON human_changes (integration_id) WHERE integration_id IS NOT NULL;
CREATE INDEX human_changes_commit ON human_changes (commit_id) WHERE commit_id IS NOT NULL;
CREATE INDEX human_changes_workspace ON human_changes (workspace_id) WHERE workspace_id IS NOT NULL;

-- A recorded change came in through the `record` integration of its own commit, or of no commit, and a resolved one through any integration whose tree holds it.
CREATE TRIGGER human_changes_integration_kind_insert BEFORE INSERT ON human_changes
WHEN NEW.integration_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM integrations i
    WHERE i.id = NEW.integration_id AND i.result = 'succeeded'
      AND ((NEW.state = 'recorded' AND i.kind = 'record' AND i.remote_commit IS NEW.commit_id)
        OR NEW.state = 'resolved')
)
BEGIN
    SELECT RAISE(ABORT, 'human_changes_integration_kind');
END;

CREATE TRIGGER human_changes_integration_kind_update BEFORE UPDATE OF integration_id, state ON human_changes
WHEN NEW.integration_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM integrations i
    WHERE i.id = NEW.integration_id AND i.result = 'succeeded'
      AND ((NEW.state = 'recorded' AND i.kind = 'record' AND i.remote_commit IS NEW.commit_id)
        OR NEW.state = 'resolved')
)
BEGIN
    SELECT RAISE(ABORT, 'human_changes_integration_kind');
END;

-- The merge of a conflicted change is prepared only in a main workspace.
CREATE TRIGGER human_changes_workspace_kind_insert BEFORE INSERT ON human_changes
WHEN NEW.workspace_id IS NOT NULL
    AND NOT EXISTS (SELECT 1 FROM workspaces w WHERE w.id = NEW.workspace_id AND w.kind = 'main')
BEGIN
    SELECT RAISE(ABORT, 'human_changes_workspace_kind');
END;

CREATE TRIGGER human_changes_workspace_kind_update BEFORE UPDATE OF workspace_id ON human_changes
WHEN NEW.workspace_id IS NOT NULL
    AND NOT EXISTS (SELECT 1 FROM workspaces w WHERE w.id = NEW.workspace_id AND w.kind = 'main')
BEGIN
    SELECT RAISE(ABORT, 'human_changes_workspace_kind');
END;

-- A change that left `conflicted` stays as it closed.
CREATE TRIGGER human_changes_closed_update BEFORE UPDATE ON human_changes
WHEN OLD.state <> 'conflicted'
BEGIN
    SELECT RAISE(ABORT, 'human_changes_closed');
END;

CREATE TRIGGER human_changes_fixed_columns BEFORE UPDATE OF commit_id, human, base_tree_hash, tree_hash, created_at ON human_changes
BEGIN
    SELECT RAISE(ABORT, 'human_changes_fixed');
END;

CREATE TRIGGER human_changes_no_delete BEFORE DELETE ON human_changes
BEGIN
    SELECT RAISE(ABORT, 'human_changes_no_delete');
END;

-- Git sync is on once this table has its row: fold then writes a Git commit of every revision into the project folder's own repository and keeps its branch and index on main. A project started from a Git repository has it from the start; a plain folder gets it when a human turns Git on. It never goes off.
CREATE TABLE git_sync (
    id INTEGER PRIMARY KEY CONSTRAINT git_sync_one_row CHECK (id = 1),
    -- The event actor label of the human who turned Git sync on.
    enabled_by TEXT NOT NULL CONSTRAINT git_sync_enabled_by_present CHECK (trim(enabled_by) <> ''),
    enabled_at INTEGER NOT NULL
);

CREATE TRIGGER git_sync_no_update BEFORE UPDATE ON git_sync
BEGIN
    SELECT RAISE(ABORT, 'git_sync is append-only');
END;

CREATE TRIGGER git_sync_no_delete BEFORE DELETE ON git_sync
BEGIN
    SELECT RAISE(ABORT, 'git_sync is append-only');
END;
