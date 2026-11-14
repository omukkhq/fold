-- An agent stopped the worker with `stop_worker`'s `wait_for` to wait for its go: `parked_by` is that agent, `parked_for` what the worker waits for in its words, `parked_at` when. While the agent that parked it runs, fold renews the worker's claims, so none goes stale, and every view shows the wait instead of a stall. Every start of the row clears the park.
ALTER TABLE worker_processes ADD COLUMN parked_by INTEGER REFERENCES agents (id);

ALTER TABLE worker_processes ADD COLUMN parked_for TEXT;

ALTER TABLE worker_processes ADD COLUMN parked_at INTEGER
    CONSTRAINT worker_processes_parked_only_ended CHECK (
        (parked_at IS NULL AND parked_by IS NULL AND parked_for IS NULL)
        OR (parked_at IS NOT NULL AND parked_by IS NOT NULL AND parked_for IS NOT NULL AND parked_for <> '' AND state IN ('stopping', 'stopped', 'failed'))
    );

-- When a stale claim started to need a person: at its mark when no agent can act on it, or once the agent fold told, the holder's lead or else the main agent, let one more stall window pass. A renewal clears it with the stale mark.
ALTER TABLE claims ADD COLUMN stale_needs_person_at INTEGER
    CONSTRAINT claims_needs_person_only_stale CHECK (stale_needs_person_at IS NULL OR stale_at IS NOT NULL);
