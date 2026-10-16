-- A session acts for the human of the token it was made from.
CREATE TRIGGER web_sessions_human_of_token_insert BEFORE INSERT ON web_sessions
WHEN NEW.human_id IS NOT (SELECT human_id FROM web_tokens WHERE id = NEW.token_id)
BEGIN
    SELECT RAISE(ABORT, 'web_sessions_human_of_token');
END;

CREATE TRIGGER web_sessions_human_of_token_update BEFORE UPDATE OF human_id, token_id ON web_sessions
WHEN NEW.human_id IS NOT (SELECT human_id FROM web_tokens WHERE id = NEW.token_id)
BEGIN
    SELECT RAISE(ABORT, 'web_sessions_human_of_token');
END;

CREATE TRIGGER web_tokens_human_fixed BEFORE UPDATE OF human_id ON web_tokens
WHEN NEW.human_id <> OLD.human_id
BEGIN
    SELECT RAISE(ABORT, 'web_tokens_human_fixed');
END;

-- A human name follows the agent name pattern, so `human:<name>` is a valid event actor.
CREATE TRIGGER humans_name_pattern_insert BEFORE INSERT ON humans
WHEN length(NEW.name) NOT BETWEEN 1 AND 32
    OR NEW.name GLOB '*[^a-z0-9_-]*'
    OR NEW.name NOT GLOB '[a-z0-9]*'
BEGIN
    SELECT RAISE(ABORT, 'humans_name_pattern');
END;

CREATE TRIGGER humans_name_pattern_update BEFORE UPDATE OF name ON humans
WHEN length(NEW.name) NOT BETWEEN 1 AND 32
    OR NEW.name GLOB '*[^a-z0-9_-]*'
    OR NEW.name NOT GLOB '[a-z0-9]*'
BEGIN
    SELECT RAISE(ABORT, 'humans_name_pattern');
END;

-- A slug is lowercase ASCII letters and digits in runs joined by single hyphens.
CREATE TRIGGER projects_slug_pattern_insert BEFORE INSERT ON projects
WHEN NEW.slug = ''
    OR NEW.slug GLOB '*[^a-z0-9-]*'
    OR NEW.slug GLOB '-*'
    OR NEW.slug GLOB '*-'
    OR NEW.slug GLOB '*--*'
BEGIN
    SELECT RAISE(ABORT, 'projects_slug_pattern');
END;

CREATE TRIGGER projects_slug_pattern_update BEFORE UPDATE OF slug ON projects
WHEN NEW.slug = ''
    OR NEW.slug GLOB '*[^a-z0-9-]*'
    OR NEW.slug GLOB '-*'
    OR NEW.slug GLOB '*-'
    OR NEW.slug GLOB '*--*'
BEGIN
    SELECT RAISE(ABORT, 'projects_slug_pattern');
END;
