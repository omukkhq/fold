-- The sending agent marked the message as asking a human for an answer. A question with choices and a worker's `AskUserQuestion` message ask without the mark; any other message to a human is a report that waits for no one.
ALTER TABLE messages ADD COLUMN asks_human INTEGER NOT NULL DEFAULT 0 CONSTRAINT messages_asks_human_flag CHECK (asks_human IN (0, 1));

-- An ask that stopped waiting without a human's reply in its thread: the asking agent or the main agent recorded that the user answered elsewhere (`answered_elsewhere`), or a human marked it answered (`dismissed`). `closed_by` is the actor as events name it. An `AskUserQuestion` message is never closed here, because its worker waits for the human's reply itself.
CREATE TABLE message_ask_closures (
    message_id INTEGER PRIMARY KEY REFERENCES messages (id),
    reason TEXT NOT NULL CONSTRAINT message_ask_closures_reason_values CHECK (reason IN ('answered_elsewhere', 'dismissed')),
    closed_by TEXT NOT NULL CONSTRAINT message_ask_closures_closed_by_present CHECK (closed_by <> ''),
    created_at INTEGER NOT NULL
);

CREATE TRIGGER message_ask_closures_no_update BEFORE UPDATE ON message_ask_closures
BEGIN
    SELECT RAISE(ABORT, 'message_ask_closures is append-only');
END;

CREATE TRIGGER message_ask_closures_no_delete BEFORE DELETE ON message_ask_closures
BEGIN
    SELECT RAISE(ABORT, 'message_ask_closures is append-only');
END;

CREATE TRIGGER message_ask_closures_not_worker_question BEFORE INSERT ON message_ask_closures
WHEN EXISTS (SELECT 1 FROM worker_question_messages WHERE message_id = NEW.message_id)
BEGIN
    SELECT RAISE(ABORT, 'message_ask_closures_not_worker_question');
END;

-- The asks from a worker to a human that still wait: a message marked as an ask, a question with choices or an `AskUserQuestion` message, addressed to a human or `humans`, with no later message from a human in its thread and no closure. The board's question mark, Needs you and the Messages count all read it.
DROP VIEW open_human_questions;

CREATE VIEW open_human_questions AS
SELECT m.id, m.number, m.thread_id, m.task_id
FROM messages m
WHERE m.sender_kind = 'worker'
    AND (
        m.asks_human = 1
        OR EXISTS (SELECT 1 FROM message_choices c WHERE c.message_id = m.id)
        OR EXISTS (SELECT 1 FROM worker_question_messages q WHERE q.message_id = m.id)
    )
    AND EXISTS (
        SELECT 1 FROM message_recipients r
        WHERE r.message_id = m.id AND r.addressed_as IN ('human', 'humans')
    )
    AND NOT EXISTS (
        SELECT 1 FROM messages later
        WHERE later.thread_id = m.thread_id AND later.number > m.number AND later.sender_kind = 'human'
    )
    AND NOT EXISTS (SELECT 1 FROM message_ask_closures x WHERE x.message_id = m.id);

CREATE TRIGGER message_ask_closures_open_only BEFORE INSERT ON message_ask_closures
WHEN NOT EXISTS (SELECT 1 FROM open_human_questions WHERE id = NEW.message_id)
BEGIN
    SELECT RAISE(ABORT, 'message_ask_closures_open_only');
END;
