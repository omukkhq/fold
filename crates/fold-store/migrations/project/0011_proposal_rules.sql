-- When a proposal stopped being open: closed or integrated.
ALTER TABLE proposals ADD COLUMN closed_at INTEGER;

-- Who added the version: the author's worker. The default of '' fails the check, so every insert must name it.
ALTER TABLE proposal_versions ADD COLUMN created_by TEXT NOT NULL DEFAULT '' CONSTRAINT proposal_versions_created_by_present CHECK (created_by <> '');

-- Triggers stand in for CHECK constraints: adding a CHECK would mean rebuilding `proposals`, which other tables refer to.
CREATE TRIGGER proposals_title_single_line_insert BEFORE INSERT ON proposals
WHEN trim(NEW.title) = '' OR instr(NEW.title, char(10)) > 0 OR instr(NEW.title, char(13)) > 0
BEGIN
    SELECT RAISE(ABORT, 'proposals_title_single_line');
END;

CREATE TRIGGER proposals_title_single_line_update BEFORE UPDATE OF title ON proposals
WHEN trim(NEW.title) = '' OR instr(NEW.title, char(10)) > 0 OR instr(NEW.title, char(13)) > 0
BEGIN
    SELECT RAISE(ABORT, 'proposals_title_single_line');
END;

-- A closed proposal names why it was closed; an open or integrated one has no close reason.
CREATE TRIGGER proposals_close_reason_iff_closed_insert BEFORE INSERT ON proposals
WHEN (NEW.state = 'closed') <> (NEW.close_reason IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'proposals_close_reason_iff_closed');
END;

CREATE TRIGGER proposals_close_reason_iff_closed_update BEFORE UPDATE OF state, close_reason ON proposals
WHEN (NEW.state = 'closed') <> (NEW.close_reason IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'proposals_close_reason_iff_closed');
END;
