-- The Git commit each main revision became in the project's repository `.fold/git`, on `branch`, the `[git]` branch when the row was written. A row is written only after `.fold/git-sync.map` gained the same line, so the map is the recovery record and a missing row is filled in from it. An imported `r1`, or a pull that fast-forwarded, is the source or remote commit itself.
CREATE TABLE revision_commits (
    revision_id INTEGER PRIMARY KEY REFERENCES revisions (id),
    branch TEXT NOT NULL CONSTRAINT revision_commits_branch_present CHECK (branch <> ''),
    "commit" TEXT NOT NULL CONSTRAINT revision_commits_commit_sha1 CHECK (
        length("commit") = 40 AND "commit" NOT GLOB '*[^0-9a-f]*'
    ),
    created_at INTEGER NOT NULL
);

CREATE TRIGGER revision_commits_no_update BEFORE UPDATE ON revision_commits
BEGIN
    SELECT RAISE(ABORT, 'revision_commits is append-only');
END;

CREATE TRIGGER revision_commits_no_delete BEFORE DELETE ON revision_commits
BEGIN
    SELECT RAISE(ABORT, 'revision_commits is append-only');
END;
