-- The canonical working directory the adapter sent when it opened the session.
ALTER TABLE sessions ADD COLUMN cwd TEXT;

-- The actor label of the human that granted the tag, such as `human:tanim`; null for a tag the agent registered with.
ALTER TABLE agent_tags ADD COLUMN granted_by TEXT;
