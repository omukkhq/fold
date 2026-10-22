-- The worker whose agent fold sent the message on to because the recipient, a stopped worker, could not start to answer it. NULL while the message was not sent on, and when nobody could take it.
ALTER TABLE message_recipients ADD COLUMN forwarded_to INTEGER REFERENCES workers (id);
-- When fold settled the row because the stopped worker could not start to answer it: sent on to forwarded_to, or held with nobody to take it. A stopped worker starts again only for a row without it.
ALTER TABLE message_recipients ADD COLUMN forwarded_at INTEGER;
-- The rows of agents' workers that wait unread when this migration runs are settled, so the first start after it starts no worker for an old message.
UPDATE message_recipients
SET forwarded_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)
WHERE worker_id IS NOT NULL AND delivered_at IS NULL AND acked_at IS NULL AND pushed_at IS NULL;
