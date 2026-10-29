-- The code a request was made on: a human selected a run of lines of one file and wrote the request about them. The lines belong to a revision of main (`revision_id`) or to one side of a proposal version (`version_id` and `side`: its base tree `old` or its own tree `new`). `excerpt` holds those lines as the tree has them, joined by newlines; `excerpt_cut` marks an excerpt that holds only the first of them.
CREATE TABLE request_code_refs (
    request_id INTEGER PRIMARY KEY REFERENCES requests (id),
    revision_id INTEGER REFERENCES revisions (id),
    version_id INTEGER REFERENCES proposal_versions (id),
    side TEXT CONSTRAINT request_code_refs_side_values CHECK (side IS NULL OR side IN ('old', 'new')),
    path TEXT NOT NULL CONSTRAINT request_code_refs_path_present CHECK (path <> ''),
    start_line INTEGER NOT NULL,
    end_line INTEGER NOT NULL,
    excerpt TEXT NOT NULL,
    excerpt_cut INTEGER NOT NULL CONSTRAINT request_code_refs_excerpt_cut_flag CHECK (excerpt_cut IN (0, 1)),
    CONSTRAINT request_code_refs_one_anchor CHECK ((revision_id IS NULL) <> (version_id IS NULL)),
    CONSTRAINT request_code_refs_side_with_version CHECK ((version_id IS NULL) = (side IS NULL)),
    CONSTRAINT request_code_refs_range_order CHECK (start_line >= 1 AND start_line <= end_line)
);

CREATE INDEX request_code_refs_revision ON request_code_refs (revision_id) WHERE revision_id IS NOT NULL;
CREATE INDEX request_code_refs_version ON request_code_refs (version_id) WHERE version_id IS NOT NULL;

CREATE TRIGGER request_code_refs_no_update BEFORE UPDATE ON request_code_refs
BEGIN
    SELECT RAISE(ABORT, 'request_code_refs is append-only');
END;

CREATE TRIGGER request_code_refs_no_delete BEFORE DELETE ON request_code_refs
BEGIN
    SELECT RAISE(ABORT, 'request_code_refs is append-only');
END;
