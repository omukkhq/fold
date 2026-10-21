-- The worker whose agent fold sent the message on to because the recipient, a stopped task worker, could not start to answer it, and when. NULL while the message was never sent on.
ALTER TABLE message_recipients ADD COLUMN forwarded_to INTEGER REFERENCES workers (id);
ALTER TABLE message_recipients ADD COLUMN forwarded_at INTEGER;
