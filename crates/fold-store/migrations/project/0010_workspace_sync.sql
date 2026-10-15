-- The base revision the workspace had when the checkpoint was taken, so a restore puts back the base together with the files and a sync can be undone. Rows from before this column have no base, and a restore of one keeps the current base.
ALTER TABLE checkpoints ADD COLUMN base_revision_id INTEGER REFERENCES revisions (id);

-- The checkpoint of the last complete state before a sync or a restore began to write files: set in the transaction that records the checkpoint taken before it, unless a mark is already set, and cleared in the transaction that finishes the sync or the restore. A value left behind means a sync or a restore stopped after it began to write files, so record_changes refuses and the next sync first restores this checkpoint.
ALTER TABLE workspaces ADD COLUMN sync_checkpoint_id INTEGER REFERENCES checkpoints (id);
