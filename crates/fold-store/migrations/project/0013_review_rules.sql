-- SQLite cannot change a CHECK in place, so `reviews` is rebuilt to accept the `reject` verdict and `review_comments` to tie a comment's path, lines and side together. `review_comments` refers to `reviews`, and the migration runs inside a transaction with foreign keys on, so both tables are copied aside and dropped child first, then recreated and refilled parent first: no row ever refers to a missing review.
CREATE TABLE reviews_copy AS SELECT * FROM reviews;

CREATE TABLE review_comments_copy AS SELECT * FROM review_comments;

DROP TABLE review_comments;

DROP TABLE reviews;

CREATE TABLE reviews (
    id INTEGER PRIMARY KEY,
    version_id INTEGER NOT NULL REFERENCES proposal_versions (id),
    reviewer_worker_id INTEGER REFERENCES workers (id),
    reviewer_human TEXT,
    verdict TEXT NOT NULL CONSTRAINT reviews_verdict_values CHECK (verdict IN ('approve', 'request_changes', 'comment', 'reject')),
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    CONSTRAINT reviews_exactly_one_reviewer CHECK ((reviewer_worker_id IS NULL) <> (reviewer_human IS NULL)),
    -- A reject closes the proposal, and its body is the close reason.
    CONSTRAINT reviews_reject_has_body CHECK (verdict <> 'reject' OR trim(body) <> '')
);

CREATE TABLE review_comments (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL UNIQUE,
    version_id INTEGER NOT NULL REFERENCES proposal_versions (id),
    review_id INTEGER NOT NULL REFERENCES reviews (id),
    author_worker_id INTEGER REFERENCES workers (id),
    author_human TEXT,
    path TEXT,
    start_line INTEGER,
    end_line INTEGER,
    side TEXT CONSTRAINT review_comments_side_values CHECK (side IN ('old', 'new')),
    reply_to INTEGER REFERENCES review_comments (number),
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    CONSTRAINT review_comments_exactly_one_author CHECK ((author_worker_id IS NULL) <> (author_human IS NULL)),
    CONSTRAINT review_comments_lines_both_or_neither CHECK ((start_line IS NULL) = (end_line IS NULL)),
    CONSTRAINT review_comments_range_order CHECK (start_line IS NULL OR (start_line >= 1 AND start_line <= end_line)),
    -- A comment without a path is about the whole version and has no lines and no side.
    CONSTRAINT review_comments_location_complete CHECK ((path IS NULL) = (start_line IS NULL) AND (path IS NULL) = (side IS NULL))
);

INSERT INTO reviews (id, version_id, reviewer_worker_id, reviewer_human, verdict, body, created_at)
SELECT id, version_id, reviewer_worker_id, reviewer_human, verdict, body, created_at FROM reviews_copy;

-- A reply names an earlier comment, so id order inserts every parent before its replies.
INSERT INTO review_comments (id, number, version_id, review_id, author_worker_id, author_human, path, start_line, end_line, side, reply_to, body, created_at)
SELECT id, number, version_id, review_id, author_worker_id, author_human, path, start_line, end_line, side, reply_to, body, created_at
FROM review_comments_copy ORDER BY id;

DROP TABLE review_comments_copy;

DROP TABLE reviews_copy;

CREATE INDEX reviews_version ON reviews (version_id);

CREATE INDEX review_comments_version ON review_comments (version_id);

CREATE INDEX review_comments_review ON review_comments (review_id);

CREATE TRIGGER reviews_no_update BEFORE UPDATE ON reviews
BEGIN
    SELECT RAISE(ABORT, 'reviews is append-only');
END;

CREATE TRIGGER reviews_no_delete BEFORE DELETE ON reviews
BEGIN
    SELECT RAISE(ABORT, 'reviews is append-only');
END;

CREATE TRIGGER review_comments_no_update BEFORE UPDATE ON review_comments
BEGIN
    SELECT RAISE(ABORT, 'review_comments is append-only');
END;

CREATE TRIGGER review_comments_no_delete BEFORE DELETE ON review_comments
BEGIN
    SELECT RAISE(ABORT, 'review_comments is append-only');
END;
