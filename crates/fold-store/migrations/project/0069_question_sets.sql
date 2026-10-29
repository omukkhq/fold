-- The questions of a question set: one message from an agent that asks a human 1 to 4 questions, each answered with one of its choices or free text, all in one reply. A single question with choices keeps its rows in `message_choices` and a message never holds both.
CREATE TABLE message_questions (
    message_id INTEGER NOT NULL REFERENCES messages (id),
    position INTEGER NOT NULL CONSTRAINT message_questions_position_range CHECK (position BETWEEN 1 AND 4),
    text TEXT NOT NULL CONSTRAINT message_questions_text_present CHECK (trim(text) <> ''),
    header TEXT CONSTRAINT message_questions_header_present CHECK (header IS NULL OR trim(header) <> ''),
    PRIMARY KEY (message_id, position)
);

CREATE TRIGGER message_questions_no_update BEFORE UPDATE ON message_questions
BEGIN
    SELECT RAISE(ABORT, 'message_questions is append-only');
END;

CREATE TRIGGER message_questions_no_delete BEFORE DELETE ON message_questions
BEGIN
    SELECT RAISE(ABORT, 'message_questions is append-only');
END;

CREATE TRIGGER message_questions_not_choices BEFORE INSERT ON message_questions
WHEN EXISTS (SELECT 1 FROM message_choices WHERE message_id = NEW.message_id)
BEGIN
    SELECT RAISE(ABORT, 'message_questions_not_choices');
END;

CREATE TRIGGER message_choices_not_questions BEFORE INSERT ON message_choices
WHEN EXISTS (SELECT 1 FROM message_questions WHERE message_id = NEW.message_id)
BEGIN
    SELECT RAISE(ABORT, 'message_choices_not_questions');
END;

-- The options of one question of a set, in their order.
CREATE TABLE message_question_choices (
    message_id INTEGER NOT NULL,
    question INTEGER NOT NULL,
    position INTEGER NOT NULL CONSTRAINT message_question_choices_position_range CHECK (position BETWEEN 1 AND 6),
    label TEXT NOT NULL CONSTRAINT message_question_choices_label_present CHECK (trim(label) <> ''),
    description TEXT CONSTRAINT message_question_choices_description_present CHECK (description IS NULL OR trim(description) <> ''),
    PRIMARY KEY (message_id, question, position),
    FOREIGN KEY (message_id, question) REFERENCES message_questions (message_id, position)
);

CREATE TRIGGER message_question_choices_no_update BEFORE UPDATE ON message_question_choices
BEGIN
    SELECT RAISE(ABORT, 'message_question_choices is append-only');
END;

CREATE TRIGGER message_question_choices_no_delete BEFORE DELETE ON message_question_choices
BEGIN
    SELECT RAISE(ABORT, 'message_question_choices is append-only');
END;

-- A human's answers to an ask, one row per question in the order of the set, all written with the one reply that carries them. A single question with choices and a marked ask are sets of one, so their answer is question 1. Each question of an ask has one answer.
CREATE TABLE message_answers (
    reply_id INTEGER NOT NULL REFERENCES messages (id),
    ask_id INTEGER NOT NULL REFERENCES messages (id),
    question INTEGER NOT NULL CONSTRAINT message_answers_question_range CHECK (question BETWEEN 1 AND 4),
    answer TEXT NOT NULL CONSTRAINT message_answers_answer_present CHECK (trim(answer) <> ''),
    PRIMARY KEY (reply_id, question),
    CONSTRAINT message_answers_once UNIQUE (ask_id, question)
);

CREATE INDEX message_answers_ask ON message_answers (ask_id);

CREATE TRIGGER message_answers_no_update BEFORE UPDATE ON message_answers
BEGIN
    SELECT RAISE(ABORT, 'message_answers is append-only');
END;

CREATE TRIGGER message_answers_no_delete BEFORE DELETE ON message_answers
BEGIN
    SELECT RAISE(ABORT, 'message_answers is append-only');
END;

-- The reply is a human's later message in the ask's thread, and it answers one ask only.
CREATE TRIGGER message_answers_reply_in_thread BEFORE INSERT ON message_answers
WHEN NOT EXISTS (
    SELECT 1 FROM messages r JOIN messages a ON a.id = NEW.ask_id
    WHERE r.id = NEW.reply_id AND r.sender_kind = 'human' AND r.thread_id = a.thread_id AND r.number > a.number
)
    OR EXISTS (SELECT 1 FROM message_answers x WHERE x.reply_id = NEW.reply_id AND x.ask_id <> NEW.ask_id)
BEGIN
    SELECT RAISE(ABORT, 'message_answers_reply_in_thread');
END;

-- The asks from a worker to a human that still wait: a message marked as an ask, a question with choices, a question set or an `AskUserQuestion` message, addressed to a human or `humans`, with no later message from a human in its thread and no closure. A human's reply that answers another ask of the thread leaves this one waiting. The board's question mark, Needs you and the Messages count all read it.
DROP VIEW open_human_questions;

CREATE VIEW open_human_questions AS
SELECT m.id, m.number, m.thread_id, m.task_id
FROM messages m
WHERE m.sender_kind = 'worker'
    AND (
        m.asks_human = 1
        OR EXISTS (SELECT 1 FROM message_choices c WHERE c.message_id = m.id)
        OR EXISTS (SELECT 1 FROM message_questions s WHERE s.message_id = m.id)
        OR EXISTS (SELECT 1 FROM worker_question_messages q WHERE q.message_id = m.id)
    )
    AND EXISTS (
        SELECT 1 FROM message_recipients r
        WHERE r.message_id = m.id AND r.addressed_as IN ('human', 'humans')
    )
    AND NOT EXISTS (
        SELECT 1 FROM messages later
        WHERE later.thread_id = m.thread_id AND later.number > m.number AND later.sender_kind = 'human'
            AND NOT EXISTS (SELECT 1 FROM message_answers o WHERE o.reply_id = later.id AND o.ask_id <> m.id)
    )
    AND NOT EXISTS (SELECT 1 FROM message_ask_closures x WHERE x.message_id = m.id);
