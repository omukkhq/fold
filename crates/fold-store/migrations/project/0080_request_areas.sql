-- The area whose lead hears of a new request first; NULL for a request in no area, which goes to the main agent and every lead at once.
ALTER TABLE requests ADD COLUMN area_id INTEGER REFERENCES areas (id);

-- When fold told the main agent and every lead of an area request that nobody claimed in time; set once, so each request escalates once.
ALTER TABLE requests ADD COLUMN escalated_at INTEGER;

CREATE TRIGGER requests_area_fixed
BEFORE UPDATE OF area_id ON requests
WHEN OLD.area_id IS NOT NULL AND NEW.area_id IS NOT OLD.area_id
BEGIN
    SELECT RAISE(ABORT, 'requests.area_id never changes once set');
END;

CREATE TRIGGER requests_escalated_once
BEFORE UPDATE OF escalated_at ON requests
WHEN OLD.escalated_at IS NOT NULL AND NEW.escalated_at IS NOT OLD.escalated_at
BEGIN
    SELECT RAISE(ABORT, 'requests.escalated_at is set once');
END;

CREATE INDEX requests_waiting_in_area ON requests (created_at)
    WHERE area_id IS NOT NULL AND state = 'open' AND escalated_at IS NULL;
