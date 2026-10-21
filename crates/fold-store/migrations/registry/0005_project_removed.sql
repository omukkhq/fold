-- When a person removed the project from fold. The row stays, with the project's data in its folder, so `fld init`, `fld claude` or `fld new` there, or the projects page, brings the project back under its slug; the daemon opens no removed project.
ALTER TABLE projects ADD COLUMN removed_at INTEGER;
