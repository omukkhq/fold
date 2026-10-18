-- The checkout record of the project folder: `tree_hash` is a tree object of, for each path, the entry fold last wrote there or knows the folder holds, which is in the object store before the row names it. The folder follows main through it: fold writes a path only while the folder still holds the record's entry, so a path the human changed stays theirs. `revision_id` is the main revision the folder last followed, and `human_change_id` the newest human change whose entries the record holds.
CREATE TABLE checkout (
    id INTEGER PRIMARY KEY CONSTRAINT checkout_one_row CHECK (id = 1),
    tree_hash TEXT NOT NULL,
    revision_id INTEGER NOT NULL REFERENCES revisions (id),
    human_change_id INTEGER REFERENCES human_changes (id),
    updated_at INTEGER NOT NULL
);
