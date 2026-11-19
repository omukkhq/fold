-- A worker's turn can also wait for a working login (`login`): the API refused the account's token or Claude Code's own login. A login wait has no limit, as a network wait has none, and names the account whose login was refused (`wait_account`, `default` for Claude Code's own login), which the project's account can differ from after a switch. SQLite changes no CHECK of a column, so the wait columns are dropped and added again with the new checks, the dependent ones first, and each row keeps its wait through a copy.
CREATE TEMP TABLE worker_waits_copy AS
SELECT id, wait_reason, wait_limit, wait_since, wait_tried_at, wait_tries, wait_reset, wait_resets_at
FROM worker_processes
WHERE wait_reason IS NOT NULL;

ALTER TABLE worker_processes DROP COLUMN wait_resets_at;
ALTER TABLE worker_processes DROP COLUMN wait_reset;
ALTER TABLE worker_processes DROP COLUMN wait_tries;
ALTER TABLE worker_processes DROP COLUMN wait_tried_at;
ALTER TABLE worker_processes DROP COLUMN wait_since;
ALTER TABLE worker_processes DROP COLUMN wait_limit;
ALTER TABLE worker_processes DROP COLUMN wait_reason;

ALTER TABLE worker_processes ADD COLUMN wait_reason TEXT
    CONSTRAINT worker_processes_wait_reason CHECK (wait_reason IS NULL OR wait_reason IN ('network', 'usage_limit', 'login'));

ALTER TABLE worker_processes ADD COLUMN wait_limit TEXT
    CONSTRAINT worker_processes_wait_limit CHECK (
        (wait_limit IS NULL) = (wait_reason IS NULL OR wait_reason IN ('network', 'login'))
        AND (wait_limit IS NULL OR wait_limit IN ('session', 'weekly', 'opus', 'sonnet', 'usage'))
    );

ALTER TABLE worker_processes ADD COLUMN wait_since INTEGER
    CONSTRAINT worker_processes_wait_since_iff_reason CHECK ((wait_since IS NULL) = (wait_reason IS NULL));

ALTER TABLE worker_processes ADD COLUMN wait_tried_at INTEGER
    CONSTRAINT worker_processes_wait_tried_at_iff_reason CHECK ((wait_tried_at IS NULL) = (wait_reason IS NULL));

ALTER TABLE worker_processes ADD COLUMN wait_tries INTEGER NOT NULL DEFAULT 0
    CONSTRAINT worker_processes_wait_tries_count CHECK (wait_tries >= 0 AND (wait_reason IS NOT NULL OR wait_tries = 0));

ALTER TABLE worker_processes ADD COLUMN wait_reset TEXT
    CONSTRAINT worker_processes_wait_reset CHECK (
        wait_reset IS NULL OR (wait_limit IS NOT NULL AND length(wait_reset) BETWEEN 1 AND 64)
    );

ALTER TABLE worker_processes ADD COLUMN wait_resets_at INTEGER
    CONSTRAINT worker_processes_wait_resets_at CHECK (wait_resets_at IS NULL OR wait_limit IS NOT NULL);

ALTER TABLE worker_processes ADD COLUMN wait_account TEXT
    CONSTRAINT worker_processes_wait_account CHECK (
        (wait_account IS NULL) = (wait_reason IS NOT 'login')
        AND (wait_account IS NULL OR length(wait_account) BETWEEN 1 AND 32)
    );

UPDATE worker_processes
SET (wait_reason, wait_limit, wait_since, wait_tried_at, wait_tries, wait_reset, wait_resets_at) = (
    SELECT c.wait_reason, c.wait_limit, c.wait_since, c.wait_tried_at, c.wait_tries, c.wait_reset, c.wait_resets_at
    FROM worker_waits_copy c
    WHERE c.id = worker_processes.id
)
WHERE id IN (SELECT id FROM worker_waits_copy);

-- A wait ends a row's run of failed turns, so a worker that waits is never one that cannot reach the model.
UPDATE worker_processes SET failed_turns = 0 WHERE wait_reason IS NOT NULL;

DROP TABLE worker_waits_copy;
