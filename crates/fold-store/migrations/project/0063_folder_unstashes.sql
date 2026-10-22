-- The journal of `fld unstash`: before an unstash writes its first file into the project folder, one row names the entry, `folder_tree_hash`, the folder's tree as fold read it, `target_tree_hash`, the tree the folder holds once every write is done, and `revision_id`, the main revision the folder held then; both trees are in the object store before the row. The paths where the merge conflicts are in `folder_unstash_conflicts`. The row is `open` until the transaction that moves the entry closes it as `finished`, or until the unstash wrote nothing and it closes as `abandoned`. So a stop between the files and the entry's row leaves an open row, which the next `fld stash`, `fld unstash` or `fld record` finishes: the entry is applied or conflicted as the row says, never by what the files hold.
CREATE TABLE folder_unstashes (
    id INTEGER PRIMARY KEY,
    stash_id INTEGER NOT NULL REFERENCES folder_stashes (id),
    folder_tree_hash TEXT NOT NULL,
    target_tree_hash TEXT NOT NULL,
    revision_id INTEGER NOT NULL REFERENCES revisions (id),
    actor TEXT NOT NULL CONSTRAINT folder_unstashes_actor_human CHECK (actor GLOB 'human:?*'),
    state TEXT NOT NULL CONSTRAINT folder_unstashes_state_values CHECK (state IN ('open', 'finished', 'abandoned')),
    started_at INTEGER NOT NULL,
    closed_at INTEGER,
    CONSTRAINT folder_unstashes_a_write CHECK (folder_tree_hash <> target_tree_hash),
    CONSTRAINT folder_unstashes_closed_at CHECK ((state = 'open') = (closed_at IS NULL))
);

-- The project folder takes one unstash at a time, under its folder lock.
CREATE UNIQUE INDEX folder_unstashes_one_open ON folder_unstashes (state) WHERE state = 'open';

-- A row only closes, once.
CREATE TRIGGER folder_unstashes_state_steps BEFORE UPDATE OF state ON folder_unstashes
WHEN NOT (OLD.state = 'open' AND NEW.state IN ('finished', 'abandoned'))
BEGIN
    SELECT RAISE(ABORT, 'folder_unstashes_state_steps');
END;

CREATE TRIGGER folder_unstashes_fixed_columns BEFORE UPDATE OF stash_id, folder_tree_hash, target_tree_hash, revision_id, actor, started_at ON folder_unstashes
BEGIN
    SELECT RAISE(ABORT, 'folder_unstashes_fixed');
END;

CREATE TRIGGER folder_unstashes_no_delete BEFORE DELETE ON folder_unstashes
BEGIN
    SELECT RAISE(ABORT, 'folder_unstashes_no_delete');
END;

-- The paths where the unstash's merge conflicts, written with its journal row; none is a clean merge.
CREATE TABLE folder_unstash_conflicts (
    unstash_id INTEGER NOT NULL REFERENCES folder_unstashes (id),
    path BLOB NOT NULL CONSTRAINT folder_unstash_conflicts_path_bytes CHECK (typeof(path) = 'blob' AND length(path) > 0),
    kind TEXT NOT NULL CONSTRAINT folder_unstash_conflicts_kind_values CHECK (kind IN ('both_modified_text', 'both_modified_binary', 'modify_delete', 'add_add', 'type_change')),
    PRIMARY KEY (unstash_id, path)
) WITHOUT ROWID;

CREATE TRIGGER folder_unstash_conflicts_no_update BEFORE UPDATE ON folder_unstash_conflicts
BEGIN
    SELECT RAISE(ABORT, 'folder_unstash_conflicts_append_only');
END;

CREATE TRIGGER folder_unstash_conflicts_no_delete BEFORE DELETE ON folder_unstash_conflicts
BEGIN
    SELECT RAISE(ABORT, 'folder_unstash_conflicts_append_only');
END;
