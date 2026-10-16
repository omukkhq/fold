-- A proposal is integrated once, so it has at most one succeeded integration.
CREATE UNIQUE INDEX integrations_one_succeeded_per_proposal ON integrations (proposal_id)
    WHERE kind = 'proposal' AND result = 'succeeded';

CREATE INDEX integrations_version ON integrations (version_id) WHERE version_id IS NOT NULL;

CREATE INDEX messages_task ON messages (task_id, number) WHERE task_id IS NOT NULL;

-- The rows `prune_auto` and the gc job's over-limit read look at.
CREATE INDEX checkpoints_auto_unpruned ON checkpoints (workspace_id) WHERE kind = 'auto' AND pruned_at IS NULL;

-- A reply belongs to the proposal of the comment it answers; it can be on another version of it.
CREATE TRIGGER review_comments_reply_same_proposal BEFORE INSERT ON review_comments
WHEN NEW.reply_to IS NOT NULL
    AND (SELECT v.proposal_id FROM review_comments c JOIN proposal_versions v ON v.id = c.version_id WHERE c.number = NEW.reply_to)
        <> (SELECT proposal_id FROM proposal_versions WHERE id = NEW.version_id)
BEGIN
    SELECT RAISE(ABORT, 'review_comments_reply_same_proposal');
END;

-- Every checkpoint has a reason. The column stays nullable because SQLite cannot add NOT NULL to an existing column without a rebuild.
CREATE TRIGGER checkpoints_reason_required BEFORE INSERT ON checkpoints
WHEN NEW.reason IS NULL
BEGIN
    SELECT RAISE(ABORT, 'checkpoints_reason_required');
END;

-- A checkpoint never changes except that it is pruned once.
CREATE TRIGGER checkpoints_prune_only BEFORE UPDATE ON checkpoints
WHEN OLD.pruned_at IS NOT NULL OR NEW.pruned_at IS NULL
    OR NEW.id IS NOT OLD.id OR NEW.number IS NOT OLD.number OR NEW.workspace_id IS NOT OLD.workspace_id
    OR NEW.tree_hash IS NOT OLD.tree_hash OR NEW.kind IS NOT OLD.kind OR NEW.reason IS NOT OLD.reason
    OR NEW.label IS NOT OLD.label OR NEW.created_at IS NOT OLD.created_at
    OR NEW.base_revision_id IS NOT OLD.base_revision_id
BEGIN
    SELECT RAISE(ABORT, 'checkpoints_prune_only');
END;

-- The checkpoint a sync or a restore returns to is one of the workspace's own.
CREATE TRIGGER workspaces_sync_checkpoint_own BEFORE UPDATE OF sync_checkpoint_id ON workspaces
WHEN NEW.sync_checkpoint_id IS NOT NULL
    AND (SELECT workspace_id FROM checkpoints WHERE id = NEW.sync_checkpoint_id) IS NOT NEW.id
BEGIN
    SELECT RAISE(ABORT, 'workspaces_sync_checkpoint_own');
END;

-- `main` is the only ref. It is created once and then only moves.
CREATE TRIGGER refs_main_only BEFORE INSERT ON refs
WHEN NEW.name <> 'main'
BEGIN
    SELECT RAISE(ABORT, 'refs_main_only');
END;

CREATE TRIGGER refs_name_fixed BEFORE UPDATE OF name ON refs
WHEN NEW.name <> OLD.name
BEGIN
    SELECT RAISE(ABORT, 'refs_name_fixed');
END;

CREATE TRIGGER refs_no_delete BEFORE DELETE ON refs
BEGIN
    SELECT RAISE(ABORT, 'refs keeps main');
END;

-- A changeset records the workspace's own task, and its parent is an earlier changeset of the same workspace.
CREATE TRIGGER changesets_task_of_workspace BEFORE INSERT ON changesets
WHEN NEW.task_id IS NOT (SELECT task_id FROM workspaces WHERE id = NEW.workspace_id)
BEGIN
    SELECT RAISE(ABORT, 'changesets_task_of_workspace');
END;

CREATE TRIGGER changesets_parent_same_workspace BEFORE INSERT ON changesets
WHEN NEW.parent_changeset_id IS NOT NULL
    AND (SELECT workspace_id FROM changesets WHERE id = NEW.parent_changeset_id) IS NOT NEW.workspace_id
BEGIN
    SELECT RAISE(ABORT, 'changesets_parent_same_workspace');
END;

-- The human review mark: on, it has a reason and the actor who set it; off, all three columns are empty. The board reads every task, so one row out of this shape would fail the whole read.
CREATE TRIGGER tasks_human_review_shape_insert BEFORE INSERT ON tasks
WHEN (NEW.human_review = 1 AND (NEW.human_review_reason IS NULL OR NEW.human_review_set_by IS NULL))
    OR (NEW.human_review = 0
        AND (NEW.human_reviewer IS NOT NULL OR NEW.human_review_reason IS NOT NULL OR NEW.human_review_set_by IS NOT NULL))
BEGIN
    SELECT RAISE(ABORT, 'tasks_human_review_shape');
END;

CREATE TRIGGER tasks_human_review_shape_update
BEFORE UPDATE OF human_review, human_reviewer, human_review_reason, human_review_set_by ON tasks
WHEN (NEW.human_review = 1 AND (NEW.human_review_reason IS NULL OR NEW.human_review_set_by IS NULL))
    OR (NEW.human_review = 0
        AND (NEW.human_reviewer IS NOT NULL OR NEW.human_review_reason IS NOT NULL OR NEW.human_review_set_by IS NOT NULL))
BEGIN
    SELECT RAISE(ABORT, 'tasks_human_review_shape');
END;

-- When gc removed the directory of a closed workspace, so a later run does not visit it again.
ALTER TABLE workspaces ADD COLUMN directory_removed_at INTEGER;

-- The latest verdict other than a comment on each proposal's current version: what decides whether it is approved or has changes requested.
CREATE VIEW proposal_verdicts AS
SELECT p.id AS proposal_id,
       (
           SELECT r.verdict FROM reviews r
           WHERE r.version_id = (
                     SELECT v.id FROM proposal_versions v
                     WHERE v.proposal_id = p.id
                     ORDER BY v.version DESC
                     LIMIT 1
                 )
             AND r.verdict <> 'comment'
           ORDER BY r.created_at DESC, r.id DESC
           LIMIT 1
       ) AS verdict
FROM proposals p;

-- The messages from a worker to a human with no later message from a human in their thread: the questions that wait for a human.
CREATE VIEW open_human_questions AS
SELECT m.id, m.number, m.thread_id, m.task_id
FROM messages m
WHERE m.sender_kind = 'worker'
    AND EXISTS (
        SELECT 1 FROM message_recipients r
        WHERE r.message_id = m.id AND r.addressed_as IN ('human', 'humans')
    )
    AND NOT EXISTS (
        SELECT 1 FROM messages later
        WHERE later.thread_id = m.thread_id AND later.number > m.number AND later.sender_kind = 'human'
    );
