-- The human's stash of the project folder, a stack: `fld stash` takes what the folder holds against the checkout record as one entry and puts those paths back, and `fld unstash` merges the newest entry into the folder again. `base_tree_hash` is the checkout record's tree and `tree_hash` the folder's tree at the stash, both in the object store before the row; `revision_id` is the main revision the folder held then. The stack is the `stashed` and `conflicted` rows, newest `number` first. A clean unstash closes the entry as `applied`. An unstash that conflicts writes markers into the folder and keeps the entry as `conflicted`; the `fld record` that finds no marker left closes it as `dropped`.
CREATE TABLE folder_stashes (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL CONSTRAINT folder_stashes_number_positive CHECK (number > 0),
    base_tree_hash TEXT NOT NULL,
    tree_hash TEXT NOT NULL,
    revision_id INTEGER NOT NULL REFERENCES revisions (id),
    actor TEXT NOT NULL CONSTRAINT folder_stashes_actor_human CHECK (actor GLOB 'human:?*'),
    state TEXT NOT NULL CONSTRAINT folder_stashes_state_values CHECK (state IN ('stashed', 'conflicted', 'applied', 'dropped')),
    created_at INTEGER NOT NULL,
    conflicted_at INTEGER,
    closed_at INTEGER,
    CONSTRAINT folder_stashes_a_change CHECK (base_tree_hash <> tree_hash),
    CONSTRAINT folder_stashes_conflicted_at CHECK ((state IN ('conflicted', 'dropped')) = (conflicted_at IS NOT NULL)),
    CONSTRAINT folder_stashes_closed_at CHECK ((state IN ('applied', 'dropped')) = (closed_at IS NOT NULL))
);

CREATE UNIQUE INDEX folder_stashes_number ON folder_stashes (number);
-- Only the newest entry is unstashed, and nothing is stashed while its markers wait, so one entry at most is conflicted.
CREATE UNIQUE INDEX folder_stashes_one_conflicted ON folder_stashes (state) WHERE state = 'conflicted';

-- An entry moves only forward: a stashed one is applied or conflicted, and a conflicted one is dropped.
CREATE TRIGGER folder_stashes_state_steps BEFORE UPDATE OF state ON folder_stashes
WHEN NOT ((OLD.state = 'stashed' AND NEW.state IN ('applied', 'conflicted'))
       OR (OLD.state = 'conflicted' AND NEW.state = 'dropped'))
BEGIN
    SELECT RAISE(ABORT, 'folder_stashes_state_steps');
END;

CREATE TRIGGER folder_stashes_fixed_columns BEFORE UPDATE OF number, base_tree_hash, tree_hash, revision_id, actor, created_at ON folder_stashes
BEGIN
    SELECT RAISE(ABORT, 'folder_stashes_fixed');
END;

CREATE TRIGGER folder_stashes_no_delete BEFORE DELETE ON folder_stashes
BEGIN
    SELECT RAISE(ABORT, 'folder_stashes_no_delete');
END;

-- The paths where an unstash conflicted with the folder, written with the entry's move to `conflicted`.
CREATE TABLE folder_stash_conflicts (
    stash_id INTEGER NOT NULL REFERENCES folder_stashes (id),
    path BLOB NOT NULL CONSTRAINT folder_stash_conflicts_path_bytes CHECK (typeof(path) = 'blob' AND length(path) > 0),
    kind TEXT NOT NULL CONSTRAINT folder_stash_conflicts_kind_values CHECK (kind IN ('both_modified_text', 'both_modified_binary', 'modify_delete', 'add_add', 'type_change')),
    PRIMARY KEY (stash_id, path)
) WITHOUT ROWID;

CREATE TRIGGER folder_stash_conflicts_no_update BEFORE UPDATE ON folder_stash_conflicts
BEGIN
    SELECT RAISE(ABORT, 'folder_stash_conflicts_append_only');
END;

CREATE TRIGGER folder_stash_conflicts_no_delete BEFORE DELETE ON folder_stash_conflicts
BEGIN
    SELECT RAISE(ABORT, 'folder_stash_conflicts_append_only');
END;
