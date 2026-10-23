-- The fingerprint of the skill texts that the row's current Claude Code session last read: a hash of the role's skill and the shared skill files. A resume whose fld has another fingerprint tells the session to load its role skill again and records the new one; null is a session that a fold without fingerprints started.
ALTER TABLE worker_processes ADD COLUMN fold_fingerprint TEXT
    CONSTRAINT worker_processes_fold_fingerprint_length CHECK (fold_fingerprint IS NULL OR length(fold_fingerprint) BETWEEN 1 AND 200);
