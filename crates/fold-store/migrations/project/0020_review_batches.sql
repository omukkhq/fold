-- Related open proposals one worker reviews together: one checkout with every member merged onto main, one verdict per member. The batch holds the review claim of each member for `holder_worker_id`.
CREATE TABLE review_batches (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL UNIQUE,
    holder_worker_id INTEGER NOT NULL REFERENCES workers (id),
    state TEXT NOT NULL DEFAULT 'open' CONSTRAINT review_batches_state_values CHECK (state IN ('open', 'submitted', 'closed')),
    created_at INTEGER NOT NULL,
    submitted_at INTEGER,
    closed_at INTEGER,
    CONSTRAINT review_batches_submitted_at_iff_submitted CHECK ((state = 'submitted') = (submitted_at IS NOT NULL)),
    CONSTRAINT review_batches_closed_at_iff_closed CHECK ((state = 'closed') = (closed_at IS NOT NULL))
);

-- A batch is never deleted, and the only change it takes is its end: submitted with its verdicts, or closed when no member is left to review.
CREATE TRIGGER review_batches_no_delete BEFORE DELETE ON review_batches
BEGIN
    SELECT RAISE(ABORT, 'review_batches keeps every batch');
END;

CREATE TRIGGER review_batches_end_only BEFORE UPDATE ON review_batches
WHEN OLD.state <> 'open' OR NEW.state = 'open'
    OR NEW.number <> OLD.number OR NEW.holder_worker_id <> OLD.holder_worker_id OR NEW.created_at <> OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'review_batches_end_only');
END;

-- One proposal in a batch, with the version it had when it joined and its place in dependency order. A member leaves when the holder removes it or when the batch is submitted or closed; `released_at` stays empty while it is in.
CREATE TABLE review_batch_members (
    id INTEGER PRIMARY KEY,
    batch_id INTEGER NOT NULL REFERENCES review_batches (id),
    proposal_id INTEGER NOT NULL REFERENCES proposals (id),
    version_id INTEGER NOT NULL REFERENCES proposal_versions (id),
    position INTEGER NOT NULL CONSTRAINT review_batch_members_position_positive CHECK (position >= 1),
    released_at INTEGER,
    release_reason TEXT CONSTRAINT review_batch_members_release_reason_values CHECK (release_reason IN ('removed', 'submitted', 'closed')),
    CONSTRAINT review_batch_members_released_with_reason CHECK ((released_at IS NULL) = (release_reason IS NULL)),
    UNIQUE (batch_id, proposal_id),
    UNIQUE (batch_id, position)
);

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

-- The batch whose checkout a review workspace is. A batch checkout is a review workspace and holds no single proposal.
ALTER TABLE workspaces ADD COLUMN review_batch_id INTEGER REFERENCES review_batches (id);

CREATE TRIGGER workspaces_review_target_insert BEFORE INSERT ON workspaces
WHEN NEW.review_batch_id IS NOT NULL AND (NEW.kind <> 'review' OR NEW.proposal_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'workspaces_review_target');
END;

CREATE TRIGGER workspaces_review_target_update BEFORE UPDATE OF review_batch_id, proposal_id, kind ON workspaces
WHEN NEW.review_batch_id IS NOT NULL AND (NEW.kind <> 'review' OR NEW.proposal_id IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'workspaces_review_target');
END;

-- One active checkout per batch: a repeat create returns it, and this index decides a race between two.
CREATE UNIQUE INDEX workspaces_one_active_batch_checkout ON workspaces (review_batch_id)
    WHERE review_batch_id IS NOT NULL AND state = 'active';

-- Which member brought a run of lines of a file into a batch checkout, and where the run starts in that member's version, so a line comment on the checkout is stored on the member's own lines.
CREATE TABLE review_batch_lines (
    id INTEGER PRIMARY KEY,
    workspace_id INTEGER NOT NULL REFERENCES workspaces (id),
    member_id INTEGER NOT NULL REFERENCES review_batch_members (id),
    path TEXT NOT NULL CONSTRAINT review_batch_lines_path_present CHECK (path <> ''),
    start_line INTEGER NOT NULL CONSTRAINT review_batch_lines_start_positive CHECK (start_line >= 1),
    end_line INTEGER NOT NULL,
    member_start_line INTEGER NOT NULL CONSTRAINT review_batch_lines_member_start_positive CHECK (member_start_line >= 1),
    CONSTRAINT review_batch_lines_range_order CHECK (start_line <= end_line)
);

CREATE INDEX review_batch_lines_lookup ON review_batch_lines (workspace_id, path, start_line);

CREATE TRIGGER review_batch_lines_no_update BEFORE UPDATE ON review_batch_lines
BEGIN
    SELECT RAISE(ABORT, 'review_batch_lines is append-only');
END;

CREATE TRIGGER review_batch_lines_no_delete BEFORE DELETE ON review_batch_lines
BEGIN
    SELECT RAISE(ABORT, 'review_batch_lines is append-only');
END;

-- The batch a review was submitted with, so a batch shows its verdicts. A review on its own has none.
ALTER TABLE reviews ADD COLUMN review_batch_id INTEGER REFERENCES review_batches (id);

CREATE INDEX reviews_batch ON reviews (review_batch_id) WHERE review_batch_id IS NOT NULL;

-- A comment of a batch review about members together. Its body is stored once here; each member it names gets a whole-version comment row that points to it, so every member's author sees it and a reader sees the other members it names.
CREATE TABLE review_batch_comments (
    id INTEGER PRIMARY KEY,
    batch_id INTEGER NOT NULL REFERENCES review_batches (id),
    number INTEGER NOT NULL CONSTRAINT review_batch_comments_number_positive CHECK (number >= 1),
    body TEXT NOT NULL CONSTRAINT review_batch_comments_body_present CHECK (trim(body) <> ''),
    created_at INTEGER NOT NULL,
    UNIQUE (batch_id, number)
);

CREATE TRIGGER review_batch_comments_no_update BEFORE UPDATE ON review_batch_comments
BEGIN
    SELECT RAISE(ABORT, 'review_batch_comments is append-only');
END;

CREATE TRIGGER review_batch_comments_no_delete BEFORE DELETE ON review_batch_comments
BEGIN
    SELECT RAISE(ABORT, 'review_batch_comments is append-only');
END;

ALTER TABLE review_comments ADD COLUMN batch_comment_id INTEGER REFERENCES review_batch_comments (id);

CREATE INDEX review_comments_batch_comment ON review_comments (batch_comment_id) WHERE batch_comment_id IS NOT NULL;

-- A member's row of a batch comment is about the whole version, replies to nothing, leaves the body to the batch comment, and comes with a review of the same batch.
CREATE TRIGGER review_comments_batch_comment_shape BEFORE INSERT ON review_comments
WHEN NEW.batch_comment_id IS NOT NULL AND (
    NEW.body <> '' OR NEW.path IS NOT NULL OR NEW.reply_to IS NOT NULL
    OR (SELECT review_batch_id FROM reviews WHERE id = NEW.review_id)
        IS NOT (SELECT batch_id FROM review_batch_comments WHERE id = NEW.batch_comment_id)
)
BEGIN
    SELECT RAISE(ABORT, 'review_comments_batch_comment_shape');
END;
