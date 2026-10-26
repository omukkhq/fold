-- A change of the project folder may carry the title it was made with, such as the one `fld untrack` gives the revision that stops tracking files: a `record` integration may have a `message`, which is never blank. SQLite cannot change a CHECK in place, so `integrations` is made again with its rows, ids, indexes and triggers; the rows that refer to it wait for the commit for their foreign key checks.
PRAGMA defer_foreign_keys = ON;

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
    -- The changeset of the main workspace that a direct change brought onto main, and the Git commit message the main agent wrote for it, or the title of a change of the project folder.
    changeset_id INTEGER REFERENCES changesets (id),
    message TEXT,
    review_batch_id INTEGER REFERENCES review_batches (id),
    CONSTRAINT integrations_columns_match_kind CHECK (
        (kind = 'proposal' AND proposal_id IS NOT NULL AND version_id IS NOT NULL
            AND changeset_id IS NULL AND message IS NULL)
        OR (kind = 'pull' AND proposal_id IS NULL AND version_id IS NULL AND remote_commit IS NOT NULL
            AND changeset_id IS NULL AND message IS NULL)
        OR (kind = 'direct' AND proposal_id IS NULL AND version_id IS NULL AND remote_commit IS NULL
            AND changeset_id IS NOT NULL AND message IS NOT NULL AND trim(message) <> '' AND result = 'succeeded')
        OR (kind = 'record' AND proposal_id IS NULL AND version_id IS NULL
            AND changeset_id IS NULL AND (message IS NULL OR trim(message) <> '') AND result = 'succeeded')
    )
);

INSERT INTO integrations (
    id, kind, proposal_id, version_id, remote_commit, result, revision_id, report, actor, created_at, number,
    changeset_id, message, review_batch_id
)
SELECT
    id, kind, proposal_id, version_id, remote_commit, result, revision_id, report, actor, created_at, number,
    changeset_id, message, review_batch_id
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

CREATE INDEX integrations_review_batch ON integrations (review_batch_id) WHERE review_batch_id IS NOT NULL;

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

-- A batch publishes proposals only, and only a submitted batch is integrated.
CREATE TRIGGER integrations_review_batch_shape BEFORE INSERT ON integrations
WHEN NEW.review_batch_id IS NOT NULL AND (
    NEW.kind <> 'proposal' OR NEW.result <> 'succeeded'
    OR (SELECT state FROM review_batches WHERE id = NEW.review_batch_id) IS NOT 'submitted'
)
BEGIN
    SELECT RAISE(ABORT, 'integrations_review_batch_shape');
END;
