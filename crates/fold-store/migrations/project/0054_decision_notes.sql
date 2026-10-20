-- A decision note records a decision that the main agent or a lead took without asking the human, so the human reads it later and asks its author about it. A note belongs to the area of the area lead that wrote it; a note of the main agent or of the review lead belongs to no area. The links are the mention forms the author wrote, which nothing filters on.
CREATE TABLE notes (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL UNIQUE CONSTRAINT notes_number_positive CHECK (number >= 1),
    author_worker_id INTEGER NOT NULL REFERENCES workers (id),
    area_id INTEGER REFERENCES areas (id),
    title TEXT NOT NULL CONSTRAINT notes_title_present CHECK (trim(title) <> ''),
    decision TEXT NOT NULL CONSTRAINT notes_decision_present CHECK (trim(decision) <> ''),
    reason TEXT NOT NULL CONSTRAINT notes_reason_present CHECK (trim(reason) <> ''),
    links TEXT NOT NULL DEFAULT '[]' CONSTRAINT notes_links_list CHECK (json_valid(links) AND json_type(links) = 'array'),
    created_at INTEGER NOT NULL
);

CREATE INDEX notes_area ON notes (area_id, number);

CREATE INDEX notes_author ON notes (author_worker_id, number);

CREATE TRIGGER notes_no_update BEFORE UPDATE ON notes
BEGIN
    SELECT RAISE(ABORT, 'notes is append-only');
END;

CREATE TRIGGER notes_no_delete BEFORE DELETE ON notes
BEGIN
    SELECT RAISE(ABORT, 'notes is append-only');
END;

-- How far each human has read the notes: the number of the newest note the human has seen. A human without a row has read none.
CREATE TABLE note_reads (
    human TEXT PRIMARY KEY CONSTRAINT note_reads_human_present CHECK (human <> ''),
    through INTEGER NOT NULL CONSTRAINT note_reads_through_positive CHECK (through >= 1),
    read_at INTEGER NOT NULL
);
