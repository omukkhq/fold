-- An option's one-line description, as Claude Code's `AskUserQuestion` gives it; null for an option without one and for the options stored before this column.
ALTER TABLE message_choices ADD COLUMN description TEXT
    CONSTRAINT message_choices_description_present CHECK (description IS NULL OR trim(description) <> '');

-- The short header of a multiple-choice question message, such as `Chart library`, shown as a chip above the question.
CREATE TABLE message_question_headers (
    message_id INTEGER PRIMARY KEY REFERENCES messages (id),
    header TEXT NOT NULL CONSTRAINT message_question_headers_header_present CHECK (trim(header) <> '')
);

CREATE TRIGGER message_question_headers_no_update BEFORE UPDATE ON message_question_headers
BEGIN
    SELECT RAISE(ABORT, 'message_question_headers is append-only');
END;

CREATE TRIGGER message_question_headers_no_delete BEFORE DELETE ON message_question_headers
BEGIN
    SELECT RAISE(ABORT, 'message_question_headers is append-only');
END;
