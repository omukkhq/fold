-- Every project has the built-in area `main` over the label `main`: the main agent's own tasks, which no lead, task worker or review agent takes. A project that already has a label `main` keeps it with its colour, and its tasks join the area.
INSERT OR IGNORE INTO labels (name, color, created_at)
VALUES ('main', 'ink', CAST(strftime('%s', 'now') AS INTEGER) * 1000);

INSERT OR IGNORE INTO areas (name, label_id, lead_agent_id, created_by, created_at)
SELECT 'main', id, NULL, 'fold', CAST(strftime('%s', 'now') AS INTEGER) * 1000
FROM labels
WHERE name = 'main';

-- The main agent takes the tasks of `main` itself, so the area never gets a lead or a profile.
CREATE TRIGGER areas_main_without_lead BEFORE UPDATE ON areas
WHEN OLD.name = 'main'
    AND (NEW.lead_agent_id IS NOT NULL OR NEW.instructions IS NOT NULL OR NEW.model IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'areas_main_without_lead');
END;

CREATE TRIGGER area_mcp_servers_not_main BEFORE INSERT ON area_mcp_servers
WHEN (SELECT name FROM areas WHERE id = NEW.area_id) = 'main'
BEGIN
    SELECT RAISE(ABORT, 'areas_main_without_lead');
END;

-- The label of `main` is the area's, so it keeps its name and is never deleted.
CREATE TRIGGER labels_main_kept_delete BEFORE DELETE ON labels
WHEN OLD.name = 'main'
BEGIN
    SELECT RAISE(ABORT, 'labels_main_kept');
END;

CREATE TRIGGER labels_main_kept_rename BEFORE UPDATE OF name ON labels
WHEN OLD.name = 'main' AND NEW.name <> 'main'
BEGIN
    SELECT RAISE(ABORT, 'labels_main_kept');
END;
