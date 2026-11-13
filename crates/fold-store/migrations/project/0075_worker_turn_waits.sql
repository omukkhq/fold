-- A worker's last turn that ended for a cause outside the worker, which fold runs again in the same session once the cause has passed: `network` when Claude Code could not reach the model, `usage_limit` when the account reached a usage limit, of the kind `wait_limit`. `wait_since` is the end of the first turn of the wait and stays through the re-runs that fail again; `wait_tried_at` is the end of the newest of them and `wait_tries` counts them. A limit's reset is Claude Code's wording (`wait_reset`), or an instant (`wait_resets_at`) when its message gives one. A turn that ends for any other reason clears the wait.
ALTER TABLE worker_processes ADD COLUMN wait_reason TEXT
    CONSTRAINT worker_processes_wait_reason CHECK (wait_reason IS NULL OR wait_reason IN ('network', 'usage_limit'));

ALTER TABLE worker_processes ADD COLUMN wait_limit TEXT
    CONSTRAINT worker_processes_wait_limit CHECK (
        (wait_limit IS NULL) = (wait_reason IS NULL OR wait_reason = 'network')
        AND (wait_limit IS NULL OR wait_limit IN ('session', 'weekly', 'opus', 'sonnet', 'usage'))
    );

ALTER TABLE worker_processes ADD COLUMN wait_since INTEGER
    CONSTRAINT worker_processes_wait_since_iff_reason CHECK ((wait_since IS NULL) = (wait_reason IS NULL));

ALTER TABLE worker_processes ADD COLUMN wait_tried_at INTEGER
    CONSTRAINT worker_processes_wait_tried_at_iff_reason CHECK (
        (wait_tried_at IS NULL) = (wait_reason IS NULL) AND (wait_tried_at IS NULL OR wait_tried_at >= wait_since)
    );

ALTER TABLE worker_processes ADD COLUMN wait_tries INTEGER NOT NULL DEFAULT 0
    CONSTRAINT worker_processes_wait_tries_count CHECK (wait_tries >= 0 AND (wait_reason IS NOT NULL OR wait_tries = 0));

ALTER TABLE worker_processes ADD COLUMN wait_reset TEXT
    CONSTRAINT worker_processes_wait_reset CHECK (
        wait_reset IS NULL OR (wait_limit IS NOT NULL AND length(wait_reset) BETWEEN 1 AND 64)
    );

ALTER TABLE worker_processes ADD COLUMN wait_resets_at INTEGER
    CONSTRAINT worker_processes_wait_resets_at CHECK (wait_resets_at IS NULL OR wait_limit IS NOT NULL);
