-- The intent of a proposal version: what the change does, why, what it leaves as it is and its risks, as its author, the reviewer that fixed it or the human that edited it wrote it. A version made before intents existed has none.
ALTER TABLE proposal_versions ADD COLUMN intent TEXT
    CONSTRAINT proposal_versions_intent_size CHECK (intent IS NULL OR (intent <> '' AND length(intent) <= 2000));

-- The intent of the version a proposal-stage run waits to make. Like the run's other inputs, it never changes.
ALTER TABLE check_runs ADD COLUMN intent TEXT
    CONSTRAINT check_runs_intent_size CHECK (intent IS NULL OR (intent <> '' AND length(intent) <= 2000));

CREATE TRIGGER check_runs_intent_fixed BEFORE UPDATE ON check_runs
WHEN NEW.intent IS NOT OLD.intent
BEGIN
    SELECT RAISE(ABORT, 'check_runs_intent_fixed');
END;
