-- Each worker reads the events after its own cursor, so two waits of one agent, such as a main worker and its child, never take each other's events. Every worker of an agent starts where the agent's cursor stood.
CREATE TABLE worker_cursors (
    worker_id INTEGER PRIMARY KEY REFERENCES workers (id),
    last_seq INTEGER NOT NULL DEFAULT 0
);

INSERT INTO worker_cursors (worker_id, last_seq)
SELECT w.id, c.last_seq
FROM workers w
JOIN agent_cursors c ON c.agent_id = w.agent_id;

DROP TABLE agent_cursors;

-- A new child worker has read what its parent has read, so its first read is not the project's whole history.
CREATE TRIGGER worker_cursors_from_parent AFTER INSERT ON workers
WHEN NEW.parent_worker_id IS NOT NULL
BEGIN
    INSERT INTO worker_cursors (worker_id, last_seq)
    SELECT NEW.id, last_seq FROM worker_cursors WHERE worker_id = NEW.parent_worker_id;
END;
