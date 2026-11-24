-- How the card's task reaches Done: `proposal`, `no_proposal` or `human`. Cards written before the column existed are proposal cards.
ALTER TABLE request_draft_cards ADD COLUMN kind TEXT NOT NULL DEFAULT 'proposal'
    CONSTRAINT request_draft_cards_kind_values CHECK (kind IN ('proposal', 'no_proposal', 'human'));

-- An existing task the card's task needs, as a dependency edge an approval adds from the created task to the existing one. Cards need other cards through `request_draft_edges`.
CREATE TABLE request_draft_card_needs (
    id INTEGER PRIMARY KEY,
    card_id INTEGER NOT NULL REFERENCES request_draft_cards (id),
    task_id INTEGER NOT NULL REFERENCES tasks (id),
    UNIQUE (card_id, task_id)
);

CREATE INDEX request_draft_card_needs_task ON request_draft_card_needs (task_id);

CREATE TRIGGER request_draft_card_needs_no_update BEFORE UPDATE ON request_draft_card_needs
BEGIN
    SELECT RAISE(ABORT, 'request_draft_card_needs is append-only');
END;

CREATE TRIGGER request_draft_card_needs_no_delete BEFORE DELETE ON request_draft_card_needs
BEGIN
    SELECT RAISE(ABORT, 'request_draft_card_needs is append-only');
END;
