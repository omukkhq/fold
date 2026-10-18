-- One run of a project's checks by fold: the setup and the checks of one tree's fold-checks.toml. A proposal-stage run checks a task workspace's recorded files (`tree_hash` on the workspace's base `base_revision_id`) for the worker `worker_id`; an integration run checks the merged tree of the version `version_id` on main's head `base_revision_id` in fold's check directory. `title` and `evidence` are what the waiting proposal or version takes when the run passes. The row is written when the run starts and ends once, in the transaction that writes its results, so a run without an end is one that is still going or that a stopped daemon cut short.
CREATE TABLE check_runs (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL UNIQUE,
    stage TEXT NOT NULL CONSTRAINT check_runs_stage_values CHECK (stage IN ('proposal', 'integration')),
    purpose TEXT NOT NULL CONSTRAINT check_runs_purpose_values CHECK (purpose IN ('check', 'create_proposal', 'update_proposal', 'integrate')),
    task_id INTEGER NOT NULL REFERENCES tasks (id),
    workspace_id INTEGER REFERENCES workspaces (id),
    worker_id INTEGER REFERENCES workers (id),
    proposal_id INTEGER REFERENCES proposals (id),
    version_id INTEGER REFERENCES proposal_versions (id),
    base_revision_id INTEGER NOT NULL REFERENCES revisions (id),
    tree_hash TEXT NOT NULL,
    actor TEXT NOT NULL,
    title TEXT,
    evidence TEXT,
    state TEXT NOT NULL DEFAULT 'running' CONSTRAINT check_runs_state_values CHECK (state IN ('running', 'passed', 'failed', 'stopped')),
    reason TEXT,
    created_at INTEGER NOT NULL,
    finished_at INTEGER,
    CONSTRAINT check_runs_finished_unless_running CHECK ((state = 'running') = (finished_at IS NULL)),
    CONSTRAINT check_runs_integration_purpose CHECK ((stage = 'integration') = (purpose = 'integrate')),
    CONSTRAINT check_runs_columns_match_stage CHECK (
        (stage = 'proposal' AND workspace_id IS NOT NULL AND worker_id IS NOT NULL AND version_id IS NULL)
        OR (stage = 'integration' AND workspace_id IS NULL AND proposal_id IS NOT NULL AND version_id IS NOT NULL)
    ),
    CONSTRAINT check_runs_create_has_title CHECK ((purpose = 'create_proposal') = (title IS NOT NULL)),
    CONSTRAINT check_runs_update_names_proposal CHECK (purpose <> 'update_proposal' OR proposal_id IS NOT NULL)
);

-- One run at a time in a task workspace, and one integration run at a time in the project.
CREATE UNIQUE INDEX check_runs_one_running_per_workspace ON check_runs (workspace_id)
    WHERE state = 'running' AND workspace_id IS NOT NULL;
CREATE UNIQUE INDEX check_runs_one_running_integration ON check_runs (stage)
    WHERE state = 'running' AND stage = 'integration';
CREATE INDEX check_runs_task ON check_runs (task_id);
CREATE INDEX check_runs_workspace ON check_runs (workspace_id) WHERE workspace_id IS NOT NULL;
CREATE INDEX check_runs_version ON check_runs (version_id) WHERE version_id IS NOT NULL;

CREATE TRIGGER check_runs_no_delete BEFORE DELETE ON check_runs
BEGIN
    SELECT RAISE(ABORT, 'check_runs is append-only');
END;

-- A run ends once: only its state, reason and end time change, and only from `running`.
CREATE TRIGGER check_runs_end_once BEFORE UPDATE ON check_runs
WHEN OLD.state <> 'running'
    OR NEW.id IS NOT OLD.id
    OR NEW.number IS NOT OLD.number
    OR NEW.stage IS NOT OLD.stage
    OR NEW.purpose IS NOT OLD.purpose
    OR NEW.task_id IS NOT OLD.task_id
    OR NEW.workspace_id IS NOT OLD.workspace_id
    OR NEW.worker_id IS NOT OLD.worker_id
    OR NEW.proposal_id IS NOT OLD.proposal_id
    OR NEW.version_id IS NOT OLD.version_id
    OR NEW.base_revision_id IS NOT OLD.base_revision_id
    OR NEW.tree_hash IS NOT OLD.tree_hash
    OR NEW.actor IS NOT OLD.actor
    OR NEW.title IS NOT OLD.title
    OR NEW.evidence IS NOT OLD.evidence
    OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'check_runs_end_once');
END;

-- What one command of a run did, in run order: the setup first, then each check of the stage. `log` is the end of the command's combined output, at most the runner's limit; `log_bytes` is the whole output's size, so `truncated` says the start was dropped.
CREATE TABLE check_results (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES check_runs (id),
    position INTEGER NOT NULL CONSTRAINT check_results_position_count CHECK (position >= 0),
    kind TEXT NOT NULL CONSTRAINT check_results_kind_values CHECK (kind IN ('setup', 'check')),
    name TEXT NOT NULL,
    required INTEGER NOT NULL CONSTRAINT check_results_required_flag CHECK (required IN (0, 1)),
    outcome TEXT NOT NULL CONSTRAINT check_results_outcome_values CHECK (outcome IN ('passed', 'failed', 'timed_out', 'skipped', 'not_run')),
    exit_code INTEGER,
    duration_ms INTEGER NOT NULL CONSTRAINT check_results_duration_count CHECK (duration_ms >= 0),
    log BLOB NOT NULL,
    log_bytes INTEGER NOT NULL CONSTRAINT check_results_log_bytes_count CHECK (log_bytes >= length(log)),
    truncated INTEGER NOT NULL CONSTRAINT check_results_truncated_flag CHECK (truncated IN (0, 1)),
    CONSTRAINT check_results_one_per_position UNIQUE (run_id, position)
);

CREATE TRIGGER check_results_no_update BEFORE UPDATE ON check_results
BEGIN
    SELECT RAISE(ABORT, 'check_results is append-only');
END;

CREATE TRIGGER check_results_no_delete BEFORE DELETE ON check_results
BEGIN
    SELECT RAISE(ABORT, 'check_results is append-only');
END;

-- Results are written only while their run is going, in the transaction that ends it.
CREATE TRIGGER check_results_running_only BEFORE INSERT ON check_results
WHEN (SELECT state FROM check_runs WHERE id = NEW.run_id) IS NOT 'running'
BEGIN
    SELECT RAISE(ABORT, 'check_results_running_only');
END;
