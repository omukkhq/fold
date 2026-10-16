-- A tool call that a worker's Claude Code put to fold with `can_use_tool`. Kind `tool` is a call that a permission rule marks `ask` and auto mode would not decide; a human allows or denies it, and `number` is its public id. Kind `ask` is an `AskUserQuestion` call: its questions go to the humans as messages with choices, and the humans' replies answer it. fold keeps the request because Claude Code drops an open request when its process ends. `input` is shown to the human and never logged.
CREATE TABLE worker_permission_questions (
    id INTEGER PRIMARY KEY,
    number INTEGER UNIQUE CONSTRAINT worker_permission_questions_number_positive CHECK (number >= 1),
    kind TEXT NOT NULL CONSTRAINT worker_permission_questions_kind_values CHECK (kind IN ('tool', 'ask')),
    worker_process_id INTEGER NOT NULL REFERENCES worker_processes (id),
    session_id TEXT NOT NULL CONSTRAINT worker_permission_questions_session_present CHECK (session_id <> ''),
    request_id TEXT NOT NULL CONSTRAINT worker_permission_questions_request_present CHECK (request_id <> ''),
    tool_use_id TEXT,
    tool TEXT NOT NULL CONSTRAINT worker_permission_questions_tool_present CHECK (tool <> ''),
    input TEXT NOT NULL CONSTRAINT worker_permission_questions_input_json CHECK (json_valid(input)),
    created_at INTEGER NOT NULL,
    CONSTRAINT worker_permission_questions_number_of_tool CHECK ((kind = 'tool') = (number IS NOT NULL))
);

CREATE INDEX worker_permission_questions_worker ON worker_permission_questions (worker_process_id);

CREATE TRIGGER worker_permission_questions_no_update BEFORE UPDATE ON worker_permission_questions
BEGIN
    SELECT RAISE(ABORT, 'worker_permission_questions is append-only');
END;

CREATE TRIGGER worker_permission_questions_no_delete BEFORE DELETE ON worker_permission_questions
BEGIN
    SELECT RAISE(ABORT, 'worker_permission_questions is append-only');
END;

-- The one answer to a question. A `tool` question is answered by a named human; an `ask` question is allowed with the humans' replies once each of its messages has one, and `answered_by` is the human of the last reply. `message` is the human's note to the worker.
CREATE TABLE worker_permission_answers (
    id INTEGER PRIMARY KEY,
    question_id INTEGER NOT NULL UNIQUE REFERENCES worker_permission_questions (id),
    behavior TEXT NOT NULL CONSTRAINT worker_permission_answers_behavior_values CHECK (behavior IN ('allow', 'deny')),
    answered_by TEXT NOT NULL CONSTRAINT worker_permission_answers_by_human CHECK (answered_by LIKE 'human:_%'),
    message TEXT,
    created_at INTEGER NOT NULL
);

-- A question that ended without an answer because fold stopped the worker or its process ended.
CREATE TABLE worker_permission_closures (
    question_id INTEGER PRIMARY KEY REFERENCES worker_permission_questions (id),
    created_at INTEGER NOT NULL
);

CREATE TRIGGER worker_permission_answers_no_update BEFORE UPDATE ON worker_permission_answers
BEGIN
    SELECT RAISE(ABORT, 'worker_permission_answers is append-only');
END;

CREATE TRIGGER worker_permission_answers_no_delete BEFORE DELETE ON worker_permission_answers
BEGIN
    SELECT RAISE(ABORT, 'worker_permission_answers is append-only');
END;

CREATE TRIGGER worker_permission_answers_open_only BEFORE INSERT ON worker_permission_answers
WHEN EXISTS (SELECT 1 FROM worker_permission_closures WHERE question_id = NEW.question_id)
BEGIN
    SELECT RAISE(ABORT, 'worker_permission_answers_open_only');
END;

CREATE TRIGGER worker_permission_answers_ask_allows BEFORE INSERT ON worker_permission_answers
WHEN NEW.behavior <> 'allow'
    AND (SELECT kind FROM worker_permission_questions WHERE id = NEW.question_id) = 'ask'
BEGIN
    SELECT RAISE(ABORT, 'worker_permission_answers_ask_allows');
END;

CREATE TRIGGER worker_permission_closures_no_update BEFORE UPDATE ON worker_permission_closures
BEGIN
    SELECT RAISE(ABORT, 'worker_permission_closures is append-only');
END;

CREATE TRIGGER worker_permission_closures_no_delete BEFORE DELETE ON worker_permission_closures
BEGIN
    SELECT RAISE(ABORT, 'worker_permission_closures is append-only');
END;

CREATE TRIGGER worker_permission_closures_open_only BEFORE INSERT ON worker_permission_closures
WHEN EXISTS (SELECT 1 FROM worker_permission_answers WHERE question_id = NEW.question_id)
BEGIN
    SELECT RAISE(ABORT, 'worker_permission_closures_open_only');
END;

-- The question messages of an `ask` question: one per question of the call, at its position in the call's `questions`.
CREATE TABLE worker_question_messages (
    question_id INTEGER NOT NULL REFERENCES worker_permission_questions (id),
    position INTEGER NOT NULL CONSTRAINT worker_question_messages_position_positive CHECK (position >= 1),
    message_id INTEGER NOT NULL UNIQUE REFERENCES messages (id),
    PRIMARY KEY (question_id, position)
);

CREATE TRIGGER worker_question_messages_no_update BEFORE UPDATE ON worker_question_messages
BEGIN
    SELECT RAISE(ABORT, 'worker_question_messages is append-only');
END;

CREATE TRIGGER worker_question_messages_no_delete BEFORE DELETE ON worker_question_messages
BEGIN
    SELECT RAISE(ABORT, 'worker_question_messages is append-only');
END;

CREATE TRIGGER worker_question_messages_ask_only BEFORE INSERT ON worker_question_messages
WHEN (SELECT kind FROM worker_permission_questions WHERE id = NEW.question_id) IS NOT 'ask'
BEGIN
    SELECT RAISE(ABORT, 'worker_question_messages_ask_only');
END;

-- The options of a multiple-choice question message, in their order. The human answers with a reply that names one, or with free text.
CREATE TABLE message_choices (
    message_id INTEGER NOT NULL REFERENCES messages (id),
    position INTEGER NOT NULL CONSTRAINT message_choices_position_range CHECK (position BETWEEN 1 AND 6),
    label TEXT NOT NULL CONSTRAINT message_choices_label_present CHECK (trim(label) <> ''),
    PRIMARY KEY (message_id, position)
);

CREATE TRIGGER message_choices_no_update BEFORE UPDATE ON message_choices
BEGIN
    SELECT RAISE(ABORT, 'message_choices is append-only');
END;

CREATE TRIGGER message_choices_no_delete BEFORE DELETE ON message_choices
BEGIN
    SELECT RAISE(ABORT, 'message_choices is append-only');
END;
