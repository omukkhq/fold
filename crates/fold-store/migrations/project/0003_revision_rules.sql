-- `r1` is the only revision without a parent, and every other revision has one.
CREATE TRIGGER revisions_first_has_no_parent BEFORE INSERT ON revisions
WHEN (NEW.number = 1) <> (NEW.parent_id IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'revisions_number_one_iff_no_parent');
END;

-- The number of an integration. The revision encoding records it, so it can never be the row id. The default of 0 fails the check, so every insert must name a number.
ALTER TABLE integrations ADD COLUMN number INTEGER NOT NULL DEFAULT 0 CONSTRAINT integrations_number_positive CHECK (number >= 1);
CREATE UNIQUE INDEX integrations_number ON integrations (number);
