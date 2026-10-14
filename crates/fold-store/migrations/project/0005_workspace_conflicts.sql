-- Paths are the workspace's path bytes, so a name that is not UTF-8 round-trips and the primary key orders rows the way trees order paths. Only blobs are accepted: SQLite sorts every TEXT value before every BLOB.
CREATE TABLE workspace_conflicts (
    workspace_id INTEGER NOT NULL REFERENCES workspaces (id),
    path BLOB NOT NULL CONSTRAINT workspace_conflicts_path_bytes CHECK (typeof(path) = 'blob' AND length(path) > 0),
    kind TEXT NOT NULL CONSTRAINT workspace_conflicts_kind_values CHECK (kind IN ('both_modified_text', 'both_modified_binary', 'modify_delete', 'add_add', 'type_change')),
    PRIMARY KEY (workspace_id, path)
) WITHOUT ROWID;

-- How many paths are conflicted is the number of rows above.
ALTER TABLE workspaces DROP COLUMN conflicted_paths;
