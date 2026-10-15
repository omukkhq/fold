-- The message a message event is about, as its public number. It has a foreign key because a message is never deleted. The recipient rule of wait_for_events decides on this column.
ALTER TABLE events ADD COLUMN message_id INTEGER REFERENCES messages (number);

CREATE INDEX events_message ON events (message_id) WHERE message_id IS NOT NULL;

-- A thread is read in number order, and awaiting_human looks for a later message in the same thread.
CREATE INDEX messages_thread_number ON messages (thread_id, number);
DROP INDEX messages_thread;
