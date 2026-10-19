-- A conflicted human change is `resolved` by the integration that brings its changes onto main: the direct change of the merge the main agent prepared, or any other integration whose tree holds them, such as a proposal or a pull that took the same lines. A recorded change still came in through the pull integration of its own commit.
DROP TRIGGER human_changes_integration_kind_insert;
DROP TRIGGER human_changes_integration_kind_update;

CREATE TRIGGER human_changes_integration_kind_insert BEFORE INSERT ON human_changes
WHEN NEW.integration_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM integrations i
    WHERE i.id = NEW.integration_id AND i.result = 'succeeded'
      AND ((NEW.state = 'recorded' AND i.kind = 'pull' AND i.remote_commit = NEW.commit_id)
        OR NEW.state = 'resolved')
)
BEGIN
    SELECT RAISE(ABORT, 'human_changes_integration_kind');
END;

CREATE TRIGGER human_changes_integration_kind_update BEFORE UPDATE OF integration_id, state ON human_changes
WHEN NEW.integration_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM integrations i
    WHERE i.id = NEW.integration_id AND i.result = 'succeeded'
      AND ((NEW.state = 'recorded' AND i.kind = 'pull' AND i.remote_commit = NEW.commit_id)
        OR NEW.state = 'resolved')
)
BEGIN
    SELECT RAISE(ABORT, 'human_changes_integration_kind');
END;
