-- The intent of a proposal version: how the change works in the code, so a human understands and judges it without reading every line, as its author, the reviewer that fixed it or the human that edited it wrote it. The version holds the summary; its parts, their files and its other sections are rows below. A version made before intents existed has none.
ALTER TABLE proposal_versions ADD COLUMN intent_summary TEXT
    CONSTRAINT proposal_versions_intent_summary_size CHECK (intent_summary IS NULL OR (intent_summary <> '' AND length(intent_summary) <= 1200));

-- One part of a version's diff: a short name and what that part of the code does, in the order the author wrote them.
CREATE TABLE proposal_intent_parts (
    id INTEGER PRIMARY KEY,
    version_id INTEGER NOT NULL REFERENCES proposal_versions (id),
    position INTEGER NOT NULL CONSTRAINT proposal_intent_parts_position_range CHECK (position BETWEEN 1 AND 12),
    name TEXT NOT NULL CONSTRAINT proposal_intent_parts_name_size CHECK (name <> '' AND length(name) <= 80),
    does TEXT NOT NULL CONSTRAINT proposal_intent_parts_does_size CHECK (does <> '' AND length(does) <= 800),
    UNIQUE (version_id, position),
    UNIQUE (version_id, name),
    UNIQUE (id, version_id)
);

-- A changed file of a version and the one part it belongs to. `version_id` repeats the part's version so that a file belongs to at most one part of a version.
CREATE TABLE proposal_intent_files (
    part_id INTEGER NOT NULL,
    version_id INTEGER NOT NULL,
    position INTEGER NOT NULL CONSTRAINT proposal_intent_files_position_positive CHECK (position >= 1),
    path TEXT NOT NULL CONSTRAINT proposal_intent_files_path_present CHECK (path <> ''),
    FOREIGN KEY (part_id, version_id) REFERENCES proposal_intent_parts (id, version_id),
    UNIQUE (version_id, path),
    UNIQUE (part_id, position)
);

-- One entry of a version's other sections: a kind of contract the change adds or changes, a decision, or what it does not change.
CREATE TABLE proposal_intent_entries (
    version_id INTEGER NOT NULL REFERENCES proposal_versions (id),
    section TEXT NOT NULL CONSTRAINT proposal_intent_entries_section_values CHECK (section IN ('routes', 'database', 'rights', 'config', 'errors', 'events', 'commands', 'decisions', 'not_changed')),
    position INTEGER NOT NULL CONSTRAINT proposal_intent_entries_position_range CHECK (position BETWEEN 1 AND 20),
    text TEXT NOT NULL CONSTRAINT proposal_intent_entries_text_size CHECK (text <> '' AND length(text) <= 300),
    UNIQUE (version_id, section, position)
);

-- Only a version with a summary has an intent, so only it has parts and entries.
CREATE TRIGGER proposal_intent_parts_need_summary BEFORE INSERT ON proposal_intent_parts
WHEN (SELECT intent_summary FROM proposal_versions WHERE id = NEW.version_id) IS NULL
BEGIN
    SELECT RAISE(ABORT, 'proposal_intent_parts_need_summary');
END;

CREATE TRIGGER proposal_intent_entries_need_summary BEFORE INSERT ON proposal_intent_entries
WHEN (SELECT intent_summary FROM proposal_versions WHERE id = NEW.version_id) IS NULL
BEGIN
    SELECT RAISE(ABORT, 'proposal_intent_entries_need_summary');
END;

CREATE TRIGGER proposal_intent_parts_no_update BEFORE UPDATE ON proposal_intent_parts
BEGIN
    SELECT RAISE(ABORT, 'proposal_intent_parts is append-only');
END;

CREATE TRIGGER proposal_intent_parts_no_delete BEFORE DELETE ON proposal_intent_parts
BEGIN
    SELECT RAISE(ABORT, 'proposal_intent_parts is append-only');
END;

CREATE TRIGGER proposal_intent_files_no_update BEFORE UPDATE ON proposal_intent_files
BEGIN
    SELECT RAISE(ABORT, 'proposal_intent_files is append-only');
END;

CREATE TRIGGER proposal_intent_files_no_delete BEFORE DELETE ON proposal_intent_files
BEGIN
    SELECT RAISE(ABORT, 'proposal_intent_files is append-only');
END;

CREATE TRIGGER proposal_intent_entries_no_update BEFORE UPDATE ON proposal_intent_entries
BEGIN
    SELECT RAISE(ABORT, 'proposal_intent_entries is append-only');
END;

CREATE TRIGGER proposal_intent_entries_no_delete BEFORE DELETE ON proposal_intent_entries
BEGIN
    SELECT RAISE(ABORT, 'proposal_intent_entries is append-only');
END;

-- The intent of each version that has one, as the JSON every read of it decodes: the summary, the parts with their files, each contract kind, the decisions and what it does not change, every list in the author's order and empty when the section does not apply.
CREATE VIEW proposal_version_intents AS
SELECT v.id AS version_id,
       json_object(
           'summary', v.intent_summary,
           'parts', json((
               SELECT json_group_array(json_object(
                   'name', p.name,
                   'does', p.does,
                   'files', json((
                       SELECT json_group_array(f.path ORDER BY f.position)
                       FROM proposal_intent_files f WHERE f.part_id = p.id
                   ))
               ) ORDER BY p.position)
               FROM proposal_intent_parts p WHERE p.version_id = v.id
           )),
           'contracts', json_object(
               'routes', json((SELECT json_group_array(e.text ORDER BY e.position) FROM proposal_intent_entries e WHERE e.version_id = v.id AND e.section = 'routes')),
               'database', json((SELECT json_group_array(e.text ORDER BY e.position) FROM proposal_intent_entries e WHERE e.version_id = v.id AND e.section = 'database')),
               'rights', json((SELECT json_group_array(e.text ORDER BY e.position) FROM proposal_intent_entries e WHERE e.version_id = v.id AND e.section = 'rights')),
               'config', json((SELECT json_group_array(e.text ORDER BY e.position) FROM proposal_intent_entries e WHERE e.version_id = v.id AND e.section = 'config')),
               'errors', json((SELECT json_group_array(e.text ORDER BY e.position) FROM proposal_intent_entries e WHERE e.version_id = v.id AND e.section = 'errors')),
               'events', json((SELECT json_group_array(e.text ORDER BY e.position) FROM proposal_intent_entries e WHERE e.version_id = v.id AND e.section = 'events')),
               'commands', json((SELECT json_group_array(e.text ORDER BY e.position) FROM proposal_intent_entries e WHERE e.version_id = v.id AND e.section = 'commands'))
           ),
           'decisions', json((SELECT json_group_array(e.text ORDER BY e.position) FROM proposal_intent_entries e WHERE e.version_id = v.id AND e.section = 'decisions')),
           'not_changed', json((SELECT json_group_array(e.text ORDER BY e.position) FROM proposal_intent_entries e WHERE e.version_id = v.id AND e.section = 'not_changed'))
       ) AS intent
FROM proposal_versions v
WHERE v.intent_summary IS NOT NULL;

-- The intent of the version a proposal-stage run waits to make, as the JSON of the checked intent: the run only carries it to that version, which stores it as the rows above, and nothing filters or decides on it here. Like the run's other inputs, it never changes.
ALTER TABLE check_runs ADD COLUMN intent TEXT
    CONSTRAINT check_runs_intent_shape CHECK (
        intent IS NULL OR (
            json_valid(intent)
            AND json_type(intent, '$.summary') = 'text'
            AND length(json_extract(intent, '$.summary')) BETWEEN 1 AND 1200
            AND json_type(intent, '$.parts') = 'array'
            AND json_array_length(intent, '$.parts') BETWEEN 1 AND 12
        )
    );

CREATE TRIGGER check_runs_intent_fixed BEFORE UPDATE ON check_runs
WHEN NEW.intent IS NOT OLD.intent
BEGIN
    SELECT RAISE(ABORT, 'check_runs_intent_fixed');
END;
