-- When a person last opened the project, through the web interface or a `fld` command that opens it; the project list orders recent projects by it and by the project's newest event.
ALTER TABLE projects ADD COLUMN last_used_at INTEGER;

UPDATE projects SET last_used_at = created_at;
