-- An area of the project's own that holds the name or the label `main`, or an area `main` with a lead or a profile, would split the built-in area, so the migration refuses it and names what to change.
CREATE TEMP TABLE main_area_conflicts (conflict TEXT NOT NULL);

CREATE TEMP TRIGGER main_area_conflicts_label BEFORE INSERT ON main_area_conflicts
WHEN NEW.conflict = 'label'
BEGIN
    SELECT RAISE(ABORT, 'main_area_conflict: fold reserves the label main for its built-in area main, but another area of this project owns that label. Rename the label main in .fold/fold.db with sqlite3, such as UPDATE labels SET name = ''old-main'' WHERE name = ''main'', then open the project again.');
END;

CREATE TEMP TRIGGER main_area_conflicts_area BEFORE INSERT ON main_area_conflicts
WHEN NEW.conflict = 'area'
BEGIN
    SELECT RAISE(ABORT, 'main_area_conflict: fold reserves the area name main for its built-in area, but this project has its own area main with another label, a lead or a profile, and an area never changes its name. Make the project again with fld init and define that area under another name.');
END;

INSERT INTO main_area_conflicts (conflict)
SELECT 'label' FROM areas JOIN labels ON labels.id = areas.label_id
WHERE labels.name = 'main' AND areas.name <> 'main';

INSERT INTO main_area_conflicts (conflict)
SELECT 'area' FROM areas JOIN labels ON labels.id = areas.label_id
WHERE areas.name = 'main'
    AND (labels.name <> 'main' OR areas.lead_agent_id IS NOT NULL OR areas.instructions IS NOT NULL
        OR areas.model IS NOT NULL OR EXISTS (SELECT 1 FROM area_mcp_servers WHERE area_id = areas.id));

DROP TABLE main_area_conflicts;

-- Every project has the built-in area `main` over the label `main`: the main agent's own tasks, which no lead, task worker or review agent takes. A project that already has a label `main` keeps it with its colour, and its tasks join the area.
INSERT OR IGNORE INTO labels (name, color, created_at)
VALUES ('main', 'ink', CAST(strftime('%s', 'now') AS INTEGER) * 1000);

INSERT OR IGNORE INTO areas (name, label_id, lead_agent_id, created_by, created_at)
SELECT 'main', id, NULL, 'fold', CAST(strftime('%s', 'now') AS INTEGER) * 1000
FROM labels
WHERE name = 'main';

-- A human approves the proposal of a task of `main` until a human clears its mark, so an open task that had the label before the area existed gets the mark fold sets on every task that comes into `main`.
UPDATE tasks
SET human_review = 1,
    human_reviewer = NULL,
    human_review_reason = 'a task of the main agent''s own area main, so a human approves its proposal',
    human_review_set_by = 'fold'
WHERE human_review = 0
    AND archived_at IS NULL
    AND status <> 'done'
    AND id IN (
        SELECT task_labels.task_id
        FROM task_labels
        JOIN labels ON labels.id = task_labels.label_id
        WHERE labels.name = 'main'
    );

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
