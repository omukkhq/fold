-- A checklist is addressed by its name within its task.
ALTER TABLE checklists RENAME COLUMN title TO name;

CREATE UNIQUE INDEX checklists_task_name ON checklists (task_id, name);
CREATE UNIQUE INDEX checklists_task_position ON checklists (task_id, position);
DROP INDEX checklists_task;

-- Triggers stand in for a CHECK constraint: adding one would mean rebuilding `checklists`, which `checklist_items` refers to.
CREATE TRIGGER checklists_name_single_line_insert BEFORE INSERT ON checklists
WHEN trim(NEW.name) = '' OR instr(NEW.name, char(10)) > 0 OR instr(NEW.name, char(13)) > 0
BEGIN
    SELECT RAISE(ABORT, 'checklists_name_single_line');
END;

CREATE TRIGGER checklists_name_single_line_update BEFORE UPDATE OF name ON checklists
WHEN trim(NEW.name) = '' OR instr(NEW.name, char(10)) > 0 OR instr(NEW.name, char(13)) > 0
BEGIN
    SELECT RAISE(ABORT, 'checklists_name_single_line');
END;

-- `checklist_items` is rebuilt to take its public number (`ci-<n>`) and its constraints. Nothing refers to it, so dropping the old table touches no other row. A row that existed before keeps its row id as its number, and the counter continues after the highest one.
CREATE TABLE checklist_items_new (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL UNIQUE,
    checklist_id INTEGER NOT NULL REFERENCES checklists (id) ON DELETE CASCADE,
    text TEXT CONSTRAINT checklist_items_text_single_line CHECK (
        text IS NULL OR (trim(text) <> '' AND instr(text, char(10)) = 0 AND instr(text, char(13)) = 0)
    ),
    linked_task_id INTEGER REFERENCES tasks (id),
    position INTEGER NOT NULL,
    done_at INTEGER,
    done_by TEXT,
    created_at INTEGER NOT NULL,
    CONSTRAINT checklist_items_text_or_link CHECK ((text IS NULL) <> (linked_task_id IS NULL)),
    CONSTRAINT checklist_items_done_with_actor CHECK ((done_at IS NULL) = (done_by IS NULL)),
    CONSTRAINT checklist_items_one_per_position UNIQUE (checklist_id, position)
);

INSERT INTO checklist_items_new (id, number, checklist_id, text, linked_task_id, position, done_at, done_by, created_at)
SELECT id, id, checklist_id, text, linked_task_id, position, done_at, done_by, created_at FROM checklist_items;

INSERT INTO counters (kind, next)
SELECT 'checklist_item', MAX(id) + 1 FROM checklist_items HAVING COUNT(*) > 0;

DROP TABLE checklist_items;

ALTER TABLE checklist_items_new RENAME TO checklist_items;

CREATE INDEX checklist_items_linked ON checklist_items (linked_task_id) WHERE linked_task_id IS NOT NULL;

-- A linked item never links the task that owns it, as its edge could not be a self-dependency either.
CREATE TRIGGER checklist_items_no_self_link_insert BEFORE INSERT ON checklist_items
WHEN NEW.linked_task_id = (SELECT task_id FROM checklists WHERE id = NEW.checklist_id)
BEGIN
    SELECT RAISE(ABORT, 'checklist_items_no_self_link');
END;

CREATE TRIGGER checklist_items_no_self_link_update BEFORE UPDATE OF checklist_id, linked_task_id ON checklist_items
WHEN NEW.linked_task_id = (SELECT task_id FROM checklists WHERE id = NEW.checklist_id)
BEGIN
    SELECT RAISE(ABORT, 'checklist_items_no_self_link');
END;
