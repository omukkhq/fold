-- A human edit is a new proposal version that keeps the agent approvals of the version it was made from; a human approval is never kept. Each kept approval is a copy on the new version that names the approval it copies, because approvals belong to the version they reviewed.
ALTER TABLE reviews ADD COLUMN kept_from_review_id INTEGER REFERENCES reviews (id);

CREATE TRIGGER reviews_kept_shape BEFORE INSERT ON reviews
WHEN NEW.kept_from_review_id IS NOT NULL
    AND (NEW.verdict <> 'approve'
        OR NEW.reviewer_worker_id IS NULL
        OR NEW.review_batch_id IS NOT NULL
        OR NOT EXISTS (
            SELECT 1 FROM reviews kept
            WHERE kept.id = NEW.kept_from_review_id AND kept.verdict = 'approve' AND kept.version_id <> NEW.version_id
        ))
BEGIN
    SELECT RAISE(ABORT, 'reviews_kept_shape');
END;

-- A human sends a human edit for review again at most once per version. From then on the kept approvals of that version no longer count.
CREATE TABLE review_again_requests (
    id INTEGER PRIMARY KEY,
    version_id INTEGER NOT NULL UNIQUE REFERENCES proposal_versions (id),
    requested_by_human TEXT NOT NULL CONSTRAINT review_again_requests_human_present CHECK (requested_by_human <> ''),
    created_at INTEGER NOT NULL
);

CREATE TRIGGER review_again_requests_no_update BEFORE UPDATE ON review_again_requests
BEGIN
    SELECT RAISE(ABORT, 'review_again_requests is append-only');
END;

CREATE TRIGGER review_again_requests_no_delete BEFORE DELETE ON review_again_requests
BEGIN
    SELECT RAISE(ABORT, 'review_again_requests is append-only');
END;

-- The latest verdict other than a comment on each proposal's current version, without the kept approvals of a version sent for review again.
DROP VIEW proposal_verdicts;

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
             AND (r.kept_from_review_id IS NULL
                 OR NOT EXISTS (SELECT 1 FROM review_again_requests q WHERE q.version_id = r.version_id))
           ORDER BY r.created_at DESC, r.id DESC
           LIMIT 1
       ) AS verdict
FROM proposals p;
