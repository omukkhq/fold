-- A member that gets its verdict alone leaves its batch with the release reason `reviewed_alone`. The reason list lives in the table definition, so the table is made again with its rows and ids. `review_batch_lines` refers to its rows and loses its parent for a moment between the drop and the copy back, so the foreign key checks wait for the commit.
PRAGMA defer_foreign_keys = ON;

DROP TRIGGER review_batch_members_version_of_proposal;
DROP TRIGGER review_batch_members_no_delete;
DROP TRIGGER review_batch_members_release_only;
DROP INDEX review_batch_members_one_open;

CREATE TABLE review_batch_members_copy AS SELECT * FROM review_batch_members;

DROP TABLE review_batch_members;

CREATE TABLE review_batch_members (
    id INTEGER PRIMARY KEY,
    batch_id INTEGER NOT NULL REFERENCES review_batches (id),
    proposal_id INTEGER NOT NULL REFERENCES proposals (id),
    version_id INTEGER NOT NULL REFERENCES proposal_versions (id),
    position INTEGER NOT NULL CONSTRAINT review_batch_members_position_positive CHECK (position >= 1),
    released_at INTEGER,
    release_reason TEXT CONSTRAINT review_batch_members_release_reason_values CHECK (release_reason IN ('removed', 'submitted', 'closed', 'reviewed_alone')),
    CONSTRAINT review_batch_members_released_with_reason CHECK ((released_at IS NULL) = (release_reason IS NULL)),
    UNIQUE (batch_id, proposal_id),
    UNIQUE (batch_id, position)
);

INSERT INTO review_batch_members (id, batch_id, proposal_id, version_id, position, released_at, release_reason)
SELECT id, batch_id, proposal_id, version_id, position, released_at, release_reason FROM review_batch_members_copy;

DROP TABLE review_batch_members_copy;

-- A proposal is in at most one batch at a time.
CREATE UNIQUE INDEX review_batch_members_one_open ON review_batch_members (proposal_id) WHERE released_at IS NULL;

CREATE TRIGGER review_batch_members_version_of_proposal BEFORE INSERT ON review_batch_members
WHEN (SELECT proposal_id FROM proposal_versions WHERE id = NEW.version_id) IS NOT NEW.proposal_id
BEGIN
    SELECT RAISE(ABORT, 'review_batch_members_version_of_proposal');
END;

CREATE TRIGGER review_batch_members_no_delete BEFORE DELETE ON review_batch_members
BEGIN
    SELECT RAISE(ABORT, 'review_batch_members keeps every member');
END;

CREATE TRIGGER review_batch_members_release_only BEFORE UPDATE ON review_batch_members
WHEN OLD.released_at IS NOT NULL OR NEW.released_at IS NULL
    OR NEW.batch_id <> OLD.batch_id OR NEW.proposal_id <> OLD.proposal_id
    OR NEW.version_id <> OLD.version_id OR NEW.position <> OLD.position
BEGIN
    SELECT RAISE(ABORT, 'review_batch_members_release_only');
END;
