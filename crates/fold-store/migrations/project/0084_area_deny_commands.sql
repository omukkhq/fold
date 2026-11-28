-- The Claude Code permission rules that an area's workers get as `permissions.deny`, in the order given, after the project's own list. An area with no row adds none.
CREATE TABLE area_deny_commands (
    area_id INTEGER NOT NULL REFERENCES areas (id),
    position INTEGER NOT NULL,
    rule TEXT NOT NULL CONSTRAINT area_deny_commands_rule_present CHECK (rule <> ''),
    PRIMARY KEY (area_id, position),
    CONSTRAINT area_deny_commands_once UNIQUE (area_id, rule)
);

CREATE TRIGGER area_deny_commands_not_main BEFORE INSERT ON area_deny_commands
WHEN (SELECT name FROM areas WHERE id = NEW.area_id) = 'main'
BEGIN
    SELECT RAISE(ABORT, 'areas_main_without_lead');
END;
