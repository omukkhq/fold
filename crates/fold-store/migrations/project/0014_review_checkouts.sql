-- The proposal version a review checkout holds. Two versions can share a base revision, so the base alone cannot tell which files a checkout has.
ALTER TABLE workspaces ADD COLUMN version_id INTEGER REFERENCES proposal_versions (id);

-- One active checkout per version and worker: a second create for the same pair returns the first, and this index decides a race between two.
CREATE UNIQUE INDEX workspaces_one_active_review_checkout ON workspaces (version_id, worker_id)
    WHERE kind = 'review' AND state = 'active';

-- A review checkout is read, built and tested, never recorded.
CREATE TRIGGER checkpoints_task_workspaces_only BEFORE INSERT ON checkpoints
WHEN (SELECT kind FROM workspaces WHERE id = NEW.workspace_id) = 'review'
BEGIN
    SELECT RAISE(ABORT, 'checkpoints_task_workspaces_only');
END;

CREATE TRIGGER changesets_task_workspaces_only BEFORE INSERT ON changesets
WHEN (SELECT kind FROM workspaces WHERE id = NEW.workspace_id) = 'review'
BEGIN
    SELECT RAISE(ABORT, 'changesets_task_workspaces_only');
END;
