-- The lines of a proposal version a message asks about: a human's question to the proposal's author about a run of lines of one file, in the version's base tree (`old`) or its own tree (`new`). `excerpt` holds those lines as the tree has them, joined by newlines; `excerpt_cut` marks an excerpt that holds only the first of them. The question creates no review, verdict or comment.
CREATE TABLE message_line_refs (
    message_id INTEGER PRIMARY KEY REFERENCES messages (id),
    version_id INTEGER NOT NULL REFERENCES proposal_versions (id),
    path TEXT NOT NULL CONSTRAINT message_line_refs_path_present CHECK (path <> ''),
    side TEXT NOT NULL CONSTRAINT message_line_refs_side_values CHECK (side IN ('old', 'new')),
    start_line INTEGER NOT NULL,
    end_line INTEGER NOT NULL,
    excerpt TEXT NOT NULL,
    excerpt_cut INTEGER NOT NULL CONSTRAINT message_line_refs_excerpt_cut_flag CHECK (excerpt_cut IN (0, 1)),
    CONSTRAINT message_line_refs_range_order CHECK (start_line >= 1 AND start_line <= end_line)
);

CREATE INDEX message_line_refs_version ON message_line_refs (version_id);

CREATE TRIGGER message_line_refs_no_update BEFORE UPDATE ON message_line_refs
BEGIN
    SELECT RAISE(ABORT, 'message_line_refs is append-only');
END;

CREATE TRIGGER message_line_refs_no_delete BEFORE DELETE ON message_line_refs
BEGIN
    SELECT RAISE(ABORT, 'message_line_refs is append-only');
END;

-- The version belongs to the proposal the message is linked to.
CREATE TRIGGER message_line_refs_same_proposal BEFORE INSERT ON message_line_refs
WHEN NOT EXISTS (
    SELECT 1 FROM messages m JOIN proposal_versions v ON v.proposal_id = m.proposal_id
    WHERE m.id = NEW.message_id AND v.id = NEW.version_id
)
BEGIN
    SELECT RAISE(ABORT, 'message_line_refs_same_proposal');
END;
