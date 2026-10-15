-- The full remote commit a sync task resolves, set in the transaction that creates the task; null for every other task. A pull finds the open sync task of its tip by it, and the integration of the task's proposal copies it into `integrations.remote_commit`.
ALTER TABLE tasks ADD COLUMN remote_commit TEXT;

CREATE INDEX tasks_remote_commit ON tasks (remote_commit) WHERE remote_commit IS NOT NULL;

CREATE TRIGGER tasks_remote_commit_fixed
BEFORE UPDATE OF remote_commit ON tasks
WHEN OLD.remote_commit IS NOT NULL AND NEW.remote_commit IS NOT OLD.remote_commit
BEGIN
    SELECT RAISE(ABORT, 'tasks.remote_commit never changes once set');
END;
