-- A human change is the commit `fld record` makes of the uncommitted changes in the project folder for a human, which fold brings onto main. `base_tree_hash` is the tree of the newest commit it shares with main and `tree_hash` its own tree; both are in the object store before the row is written. A clean one is `recorded` with the `pull` integration of its commit. A conflicting one is `conflicted` until the main agent integrates the merge it prepared in its main workspace (`resolved`, with that `direct` integration), or until a newer human change takes its place (`superseded`).
CREATE TABLE human_changes (
    id INTEGER PRIMARY KEY,
    commit_id TEXT NOT NULL CONSTRAINT human_changes_commit_hex CHECK (length(commit_id) IN (40, 64) AND commit_id NOT GLOB '*[^0-9a-f]*'),
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

-- One human change waits for the main agent at a time; a newer one takes its place.
CREATE UNIQUE INDEX human_changes_one_conflicted ON human_changes (state) WHERE state = 'conflicted';
CREATE UNIQUE INDEX human_changes_integration ON human_changes (integration_id) WHERE integration_id IS NOT NULL;
CREATE INDEX human_changes_commit ON human_changes (commit_id);
CREATE INDEX human_changes_workspace ON human_changes (workspace_id) WHERE workspace_id IS NOT NULL;

-- A recorded change came in through the pull integration of its own commit, and a resolved one through a direct change.
CREATE TRIGGER human_changes_integration_kind_insert BEFORE INSERT ON human_changes
WHEN NEW.integration_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM integrations i
    WHERE i.id = NEW.integration_id AND i.result = 'succeeded'
      AND ((NEW.state = 'recorded' AND i.kind = 'pull' AND i.remote_commit = NEW.commit_id)
        OR (NEW.state = 'resolved' AND i.kind = 'direct'))
)
BEGIN
    SELECT RAISE(ABORT, 'human_changes_integration_kind');
END;

CREATE TRIGGER human_changes_integration_kind_update BEFORE UPDATE OF integration_id, state ON human_changes
WHEN NEW.integration_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM integrations i
    WHERE i.id = NEW.integration_id AND i.result = 'succeeded'
      AND ((NEW.state = 'recorded' AND i.kind = 'pull' AND i.remote_commit = NEW.commit_id)
        OR (NEW.state = 'resolved' AND i.kind = 'direct'))
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

-- The paths that conflicted with main when the change was recorded.
CREATE TABLE human_change_conflicts (
    human_change_id INTEGER NOT NULL REFERENCES human_changes (id),
    path BLOB NOT NULL CONSTRAINT human_change_conflicts_path_bytes CHECK (typeof(path) = 'blob' AND length(path) > 0),
    kind TEXT NOT NULL CONSTRAINT human_change_conflicts_kind_values CHECK (kind IN ('both_modified_text', 'both_modified_binary', 'modify_delete', 'add_add', 'type_change')),
    PRIMARY KEY (human_change_id, path)
) WITHOUT ROWID;

CREATE TRIGGER human_change_conflicts_no_update BEFORE UPDATE ON human_change_conflicts
BEGIN
    SELECT RAISE(ABORT, 'human_change_conflicts_append_only');
END;

CREATE TRIGGER human_change_conflicts_no_delete BEFORE DELETE ON human_change_conflicts
BEGIN
    SELECT RAISE(ABORT, 'human_change_conflicts_append_only');
END;
