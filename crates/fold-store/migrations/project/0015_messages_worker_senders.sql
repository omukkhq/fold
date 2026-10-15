-- The awaiting-a-human count starts from the messages a worker sent; this lets it skip human and system messages.
CREATE INDEX messages_worker_senders ON messages (thread_id, number) WHERE sender_kind = 'worker';
