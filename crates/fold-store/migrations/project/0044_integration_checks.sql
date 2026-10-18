-- The integration checks of the proposal's current version failed on its merge with main, so the proposal is with its author until a new version, which clears the flag as a new version clears `conflicted`.
ALTER TABLE proposals ADD COLUMN checks_failed INTEGER NOT NULL DEFAULT 0 CONSTRAINT proposals_checks_failed_flag CHECK (checks_failed IN (0, 1));

-- An integration a caller asked for while the project has integration checks, which waits its turn in the queue. The queue serves the requests in `id` order; a request ends once its integration ran, was refused or found nothing to do, so a daemon start serves again every request without an end. `version` is the proposal's current version when the request was made, and `expected_version` the one the caller named, if any. `actor` is the event actor of the caller, whose rights the gates check when the turn comes.
CREATE TABLE integration_requests (
    id INTEGER PRIMARY KEY,
    proposal_id INTEGER NOT NULL REFERENCES proposals (id),
    version INTEGER NOT NULL CONSTRAINT integration_requests_current_positive CHECK (version >= 1),
    expected_version INTEGER CONSTRAINT integration_requests_version_positive CHECK (expected_version >= 1),
    actor TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    ended_at INTEGER
);

CREATE INDEX integration_requests_waiting ON integration_requests (id) WHERE ended_at IS NULL;

-- One waiting request per proposal version: a second request waits for nothing the first does not.
CREATE UNIQUE INDEX integration_requests_one_waiting ON integration_requests (proposal_id, version) WHERE ended_at IS NULL;

CREATE TRIGGER integration_requests_no_delete BEFORE DELETE ON integration_requests
BEGIN
    SELECT RAISE(ABORT, 'integration_requests is append-only');
END;

-- A request ends once; nothing else of it changes.
CREATE TRIGGER integration_requests_end_once BEFORE UPDATE ON integration_requests
WHEN OLD.ended_at IS NOT NULL
    OR NEW.ended_at IS NULL
    OR NEW.id IS NOT OLD.id
    OR NEW.proposal_id IS NOT OLD.proposal_id
    OR NEW.version IS NOT OLD.version
    OR NEW.expected_version IS NOT OLD.expected_version
    OR NEW.actor IS NOT OLD.actor
    OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'integration_requests_end_once');
END;
