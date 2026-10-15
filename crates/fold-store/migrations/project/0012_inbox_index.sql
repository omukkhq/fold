-- The inbox and the unread count read a worker's unacknowledged rows. This index also serves every lookup by worker alone, so the worker-only index goes.
CREATE INDEX message_recipients_worker_acked ON message_recipients (worker_id, acked_at) WHERE worker_id IS NOT NULL;
DROP INDEX message_recipients_worker;
