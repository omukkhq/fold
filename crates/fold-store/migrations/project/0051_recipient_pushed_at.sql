-- The first time a notice about the message reached a session or the worker process of the recipient's agent: the message then waits in that session until the agent reads its inbox. NULL while no notice got through.
ALTER TABLE message_recipients ADD COLUMN pushed_at INTEGER;
