-- The entries a human dropped from the stash with `fld stash drop` or `fld stash clear`: one row per entry, which hides it from the stack, so `fld stash list` and `fld unstash` never see it again, and lets `fld gc` collect its trees. The entry's own row keeps its state `stashed`, because a drop is a record of its own and `folder_stashes` keeps its forward-only states. Only a `stashed` entry is dropped: a `conflicted` one waits for its markers in the folder.
CREATE TABLE folder_stash_drops (
    stash_id INTEGER PRIMARY KEY REFERENCES folder_stashes (id),
    actor TEXT NOT NULL CONSTRAINT folder_stash_drops_actor_human CHECK (actor GLOB 'human:?*'),
    created_at INTEGER NOT NULL
);

CREATE TRIGGER folder_stash_drops_stashed BEFORE INSERT ON folder_stash_drops
WHEN (SELECT state FROM folder_stashes WHERE id = NEW.stash_id) IS NOT 'stashed'
BEGIN
    SELECT RAISE(ABORT, 'folder_stash_drops_stashed');
END;

CREATE TRIGGER folder_stash_drops_no_update BEFORE UPDATE ON folder_stash_drops
BEGIN
    SELECT RAISE(ABORT, 'folder_stash_drops is append-only');
END;

CREATE TRIGGER folder_stash_drops_no_delete BEFORE DELETE ON folder_stash_drops
BEGIN
    SELECT RAISE(ABORT, 'folder_stash_drops is append-only');
END;

-- A dropped entry is never applied or conflicted.
CREATE TRIGGER folder_stashes_dropped_fixed BEFORE UPDATE OF state ON folder_stashes
WHEN EXISTS (SELECT 1 FROM folder_stash_drops WHERE stash_id = OLD.id)
BEGIN
    SELECT RAISE(ABORT, 'folder_stashes_dropped');
END;
