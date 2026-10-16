-- The label of the area whose lead builds the card; an approval gives the card's task this label. NULL leaves the task without an area, so it goes to the main agent.
ALTER TABLE request_draft_cards ADD COLUMN label_id INTEGER REFERENCES labels (id);

-- The main agent is one agent: the user's own interactive session.
CREATE UNIQUE INDEX agent_tags_one_main ON agent_tags (tag) WHERE tag = 'main';
