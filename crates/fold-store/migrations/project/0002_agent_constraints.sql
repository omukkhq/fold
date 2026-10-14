CREATE UNIQUE INDEX workers_one_main ON workers (agent_id) WHERE parent_worker_id IS NULL;

CREATE INDEX sessions_agent_open ON sessions (agent_id, ended_at);
DROP INDEX sessions_agent;

CREATE TRIGGER agents_name_pattern_insert BEFORE INSERT ON agents
WHEN length(NEW.name) NOT BETWEEN 1 AND 32
    OR NEW.name GLOB '*[^a-z0-9_-]*'
    OR NEW.name NOT GLOB '[a-z0-9]*'
BEGIN
    SELECT RAISE(ABORT, 'agents_name_pattern');
END;

CREATE TRIGGER agents_name_pattern_update BEFORE UPDATE OF name ON agents
WHEN length(NEW.name) NOT BETWEEN 1 AND 32
    OR NEW.name GLOB '*[^a-z0-9_-]*'
    OR NEW.name NOT GLOB '[a-z0-9]*'
BEGIN
    SELECT RAISE(ABORT, 'agents_name_pattern');
END;

-- A child worker belongs to its parent's agent and is named `<parent>/<segment>`.
CREATE TRIGGER workers_child_of_parent BEFORE INSERT ON workers
WHEN NEW.parent_worker_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM workers parent
    WHERE parent.id = NEW.parent_worker_id
      AND parent.agent_id = NEW.agent_id
      AND length(NEW.name) > length(parent.name) + 1
      AND substr(NEW.name, 1, length(parent.name) + 1) = parent.name || '/'
      AND instr(substr(NEW.name, length(parent.name) + 2), '/') = 0
)
BEGIN
    SELECT RAISE(ABORT, 'workers_child_of_parent');
END;

CREATE TRIGGER workers_identity_fixed BEFORE UPDATE OF agent_id, name, parent_worker_id ON workers
BEGIN
    SELECT RAISE(ABORT, 'workers_identity_fixed');
END;
