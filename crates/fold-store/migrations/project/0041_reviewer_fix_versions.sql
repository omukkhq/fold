-- A reviewer may fix what it finds in its own review checkout; the fix becomes the next version under the reviewer's name. The version names the checkout its files came from, so a reader can tell a reviewer's fix from the author's work even after the proposal changes author.
ALTER TABLE proposal_versions ADD COLUMN fixed_in_checkout_id INTEGER REFERENCES workspaces (id);

CREATE TRIGGER proposal_versions_fixed_in_checkout BEFORE INSERT ON proposal_versions
WHEN NEW.fixed_in_checkout_id IS NOT NULL
    AND NOT EXISTS (
        SELECT 1 FROM workspaces w
        WHERE w.id = NEW.fixed_in_checkout_id AND w.kind = 'review' AND w.proposal_id = NEW.proposal_id
    )
BEGIN
    SELECT RAISE(ABORT, 'proposal_versions_fixed_in_checkout');
END;
