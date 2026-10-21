-- The review batch whose integration published this proposal. The approved members of a submitted batch integrate together after one run of the integration checks, with one integration row and one revision for each member, all in one transaction.
ALTER TABLE integrations ADD COLUMN review_batch_id INTEGER REFERENCES review_batches (id);

CREATE INDEX integrations_review_batch ON integrations (review_batch_id) WHERE review_batch_id IS NOT NULL;

-- A batch publishes proposals only, and only a submitted batch is integrated.
CREATE TRIGGER integrations_review_batch_shape BEFORE INSERT ON integrations
WHEN NEW.review_batch_id IS NOT NULL AND (
    NEW.kind <> 'proposal' OR NEW.result <> 'succeeded'
    OR (SELECT state FROM review_batches WHERE id = NEW.review_batch_id) IS NOT 'submitted'
)
BEGIN
    SELECT RAISE(ABORT, 'integrations_review_batch_shape');
END;

-- The end of a submitted batch's integration turn, once per batch. `together` published the members that were ready as one integration, `one_at_a_time` followed a failed check run (`check_run_id`) with one integration for each member, and `none` found no member ready. After it, every member that did not integrate is reviewed and integrated on its own rules.
CREATE TABLE review_batch_integrations (
    id INTEGER PRIMARY KEY,
    batch_id INTEGER NOT NULL UNIQUE REFERENCES review_batches (id),
    outcome TEXT NOT NULL CONSTRAINT review_batch_integrations_outcome_values CHECK (outcome IN ('together', 'one_at_a_time', 'none')),
    check_run_id INTEGER REFERENCES check_runs (id),
    created_at INTEGER NOT NULL,
    CONSTRAINT review_batch_integrations_run_of_fallback CHECK (outcome <> 'one_at_a_time' OR check_run_id IS NOT NULL)
);

CREATE TRIGGER review_batch_integrations_submitted BEFORE INSERT ON review_batch_integrations
WHEN (SELECT state FROM review_batches WHERE id = NEW.batch_id) IS NOT 'submitted'
BEGIN
    SELECT RAISE(ABORT, 'review_batch_integrations_submitted');
END;

-- The members of a batch submitted before batches integrated together integrated on their own, so those batches have ended their turn.
INSERT INTO review_batch_integrations (batch_id, outcome, created_at)
SELECT id, 'none', submitted_at FROM review_batches WHERE state = 'submitted';

CREATE TRIGGER review_batch_integrations_no_update BEFORE UPDATE ON review_batch_integrations
BEGIN
    SELECT RAISE(ABORT, 'review_batch_integrations is append-only');
END;

CREATE TRIGGER review_batch_integrations_no_delete BEFORE DELETE ON review_batch_integrations
BEGIN
    SELECT RAISE(ABORT, 'review_batch_integrations is append-only');
END;

-- A request to integrate a submitted batch while the project has integration checks, which waits its turn in the integration queue beside the requests of single proposals. A request ends once its turn ran, was refused or found nothing to do, so a daemon start serves again every request without an end. `actor` is the event actor of the caller, whose rights the gates check when the turn comes.
CREATE TABLE review_batch_requests (
    id INTEGER PRIMARY KEY,
    batch_id INTEGER NOT NULL REFERENCES review_batches (id),
    actor TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    ended_at INTEGER
);

-- One waiting request per batch: a second request waits for nothing the first does not.
CREATE UNIQUE INDEX review_batch_requests_one_waiting ON review_batch_requests (batch_id) WHERE ended_at IS NULL;

CREATE TRIGGER review_batch_requests_no_delete BEFORE DELETE ON review_batch_requests
BEGIN
    SELECT RAISE(ABORT, 'review_batch_requests is append-only');
END;

-- A request ends once; nothing else of it changes.
CREATE TRIGGER review_batch_requests_end_once BEFORE UPDATE ON review_batch_requests
WHEN OLD.ended_at IS NOT NULL
    OR NEW.ended_at IS NULL
    OR NEW.id IS NOT OLD.id
    OR NEW.batch_id IS NOT OLD.batch_id
    OR NEW.actor IS NOT OLD.actor
    OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'review_batch_requests_end_once');
END;
