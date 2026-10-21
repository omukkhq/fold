-- When fold started to remove a closed workspace's directory: the final checkpoint of its unrecorded files, when it needed one, committed with it, so a removal that stops part way only deletes when it runs again and never checkpoints a half-deleted directory.
ALTER TABLE workspaces ADD COLUMN removal_started_at INTEGER;
