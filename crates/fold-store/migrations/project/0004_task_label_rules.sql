-- SQLite cannot change a foreign key in place, so `task_labels` is rebuilt to cascade from both ends. Nothing refers to it, so dropping the old table touches no other row.
CREATE TABLE task_labels_new (
    task_id INTEGER NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    label_id INTEGER NOT NULL REFERENCES labels (id) ON DELETE CASCADE,
    PRIMARY KEY (task_id, label_id)
);

INSERT INTO task_labels_new (task_id, label_id) SELECT task_id, label_id FROM task_labels;

DROP TABLE task_labels;

ALTER TABLE task_labels_new RENAME TO task_labels;

CREATE INDEX task_labels_label ON task_labels (label_id);

-- Triggers stand in for CHECK constraints: adding a CHECK would mean rebuilding `tasks` and `labels`, which other tables refer to.
CREATE TRIGGER tasks_title_single_line_insert BEFORE INSERT ON tasks
WHEN trim(NEW.title) = '' OR instr(NEW.title, char(10)) > 0 OR instr(NEW.title, char(13)) > 0
BEGIN
    SELECT RAISE(ABORT, 'tasks_title_single_line');
END;

CREATE TRIGGER tasks_title_single_line_update BEFORE UPDATE OF title ON tasks
WHEN trim(NEW.title) = '' OR instr(NEW.title, char(10)) > 0 OR instr(NEW.title, char(13)) > 0
BEGIN
    SELECT RAISE(ABORT, 'tasks_title_single_line');
END;

CREATE TRIGGER labels_name_single_line_insert BEFORE INSERT ON labels
WHEN trim(NEW.name) = '' OR instr(NEW.name, char(10)) > 0 OR instr(NEW.name, char(13)) > 0
BEGIN
    SELECT RAISE(ABORT, 'labels_name_single_line');
END;

CREATE TRIGGER labels_name_single_line_update BEFORE UPDATE OF name ON labels
WHEN trim(NEW.name) = '' OR instr(NEW.name, char(10)) > 0 OR instr(NEW.name, char(13)) > 0
BEGIN
    SELECT RAISE(ABORT, 'labels_name_single_line');
END;

CREATE TRIGGER labels_color_values_insert BEFORE INSERT ON labels
WHEN NEW.color NOT IN ('sage', 'slate', 'teal', 'ink', 'gold', 'moss', 'plum', 'stone')
BEGIN
    SELECT RAISE(ABORT, 'labels_color_values');
END;

CREATE TRIGGER labels_color_values_update BEFORE UPDATE OF color ON labels
WHEN NEW.color NOT IN ('sage', 'slate', 'teal', 'ink', 'gold', 'moss', 'plum', 'stone')
BEGIN
    SELECT RAISE(ABORT, 'labels_color_values');
END;
