-- An area's profile: the kind of work its area lead and task coders start with. A null field falls back to the role's settings of `.fold/config.toml`, and a start's own model and instructions come first.
ALTER TABLE areas ADD COLUMN instructions TEXT
    CONSTRAINT areas_instructions_present CHECK (instructions IS NULL OR trim(instructions) <> '');
ALTER TABLE areas ADD COLUMN model TEXT
    CONSTRAINT areas_model_present CHECK (model IS NULL OR model <> '');

-- The MCP servers an area's workers load, in the order given. An area with no row leaves the servers to the role.
CREATE TABLE area_mcp_servers (
    area_id INTEGER NOT NULL REFERENCES areas (id),
    position INTEGER NOT NULL,
    name TEXT NOT NULL CONSTRAINT area_mcp_servers_name_present CHECK (name <> ''),
    PRIMARY KEY (area_id, position),
    CONSTRAINT area_mcp_servers_once UNIQUE (area_id, name)
);

-- The profile changes; the lead is still set once and the area's identity never changes.
DROP TRIGGER areas_lead_once;

CREATE TRIGGER areas_lead_once BEFORE UPDATE ON areas
WHEN (OLD.lead_agent_id IS NOT NULL AND NEW.lead_agent_id IS NOT OLD.lead_agent_id)
    OR NEW.name <> OLD.name OR NEW.label_id <> OLD.label_id
    OR NEW.created_by <> OLD.created_by OR NEW.created_at <> OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'areas_lead_once');
END;
