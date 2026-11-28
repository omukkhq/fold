-- The main agent's direct change runs the integration checks on its merged tree before it reaches main: a run of purpose `integrate_direct` checks the changeset `changeset_id` of the main workspace and belongs to no task, proposal or version. Every other run keeps its task and has no changeset. SQLite cannot change a CHECK or make a column nullable in place, so `check_runs` is made again with its rows, ids, indexes and triggers; `check_results`, `check_run_members` and `review_batch_integrations` refer to its rows and wait for the commit for their foreign key checks.
PRAGMA defer_foreign_keys = ON;

CREATE TABLE check_runs_copy AS SELECT * FROM check_runs;

DROP TABLE check_runs;

CREATE TABLE check_runs (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL UNIQUE,
    stage TEXT NOT NULL CONSTRAINT check_runs_stage_values CHECK (stage IN ('proposal', 'integration')),
    purpose TEXT NOT NULL CONSTRAINT check_runs_purpose_values CHECK (purpose IN ('check', 'create_proposal', 'update_proposal', 'integrate', 'review_fix', 'integrate_direct')),
    task_id INTEGER REFERENCES tasks (id),
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
    review_batch_id INTEGER REFERENCES review_batches (id),
    intent TEXT
        CONSTRAINT check_runs_intent_shape CHECK (
            intent IS NULL OR (
                json_valid(intent)
                AND json_type(intent, '$.summary') = 'text'
                AND length(json_extract(intent, '$.summary')) BETWEEN 1 AND 1200
                AND json_type(intent, '$.parts') = 'array'
                AND json_array_length(intent, '$.parts') BETWEEN 1 AND 12
            )
        ),
    changeset_id INTEGER REFERENCES changesets (id),
    CONSTRAINT check_runs_finished_unless_running CHECK ((state = 'running') = (finished_at IS NULL)),
    CONSTRAINT check_runs_integration_purpose CHECK ((stage = 'integration') = (purpose IN ('integrate', 'integrate_direct'))),
    CONSTRAINT check_runs_columns_match_stage CHECK (
        (stage = 'proposal' AND task_id IS NOT NULL AND changeset_id IS NULL
            AND workspace_id IS NOT NULL AND worker_id IS NOT NULL AND version_id IS NULL)
        OR (stage = 'integration' AND purpose = 'integrate' AND task_id IS NOT NULL AND changeset_id IS NULL
            AND workspace_id IS NULL AND proposal_id IS NOT NULL AND version_id IS NOT NULL)
        OR (stage = 'integration' AND purpose = 'integrate_direct' AND changeset_id IS NOT NULL
            AND task_id IS NULL AND workspace_id IS NULL AND proposal_id IS NULL AND version_id IS NULL
            AND review_batch_id IS NULL)
    ),
    CONSTRAINT check_runs_create_has_title CHECK ((purpose = 'create_proposal') = (title IS NOT NULL)),
    CONSTRAINT check_runs_version_names_proposal CHECK (purpose NOT IN ('update_proposal', 'review_fix') OR proposal_id IS NOT NULL)
);

INSERT INTO check_runs (
    id, number, stage, purpose, task_id, workspace_id, worker_id, proposal_id, version_id, base_revision_id,
    tree_hash, actor, title, evidence, state, reason, created_at, finished_at, review_batch_id, intent
)
SELECT
    id, number, stage, purpose, task_id, workspace_id, worker_id, proposal_id, version_id, base_revision_id,
    tree_hash, actor, title, evidence, state, reason, created_at, finished_at, review_batch_id, intent
FROM check_runs_copy;

DROP TABLE check_runs_copy;

-- One run at a time in a task workspace, and one integration run at a time in the project.
CREATE UNIQUE INDEX check_runs_one_running_per_workspace ON check_runs (workspace_id)
    WHERE state = 'running' AND workspace_id IS NOT NULL;
CREATE UNIQUE INDEX check_runs_one_running_integration ON check_runs (stage)
    WHERE state = 'running' AND stage = 'integration';
CREATE INDEX check_runs_task ON check_runs (task_id);
CREATE INDEX check_runs_workspace ON check_runs (workspace_id) WHERE workspace_id IS NOT NULL;
CREATE INDEX check_runs_version ON check_runs (version_id) WHERE version_id IS NOT NULL;
CREATE INDEX check_runs_tree_hash ON check_runs (tree_hash);
CREATE INDEX check_runs_changeset ON check_runs (changeset_id) WHERE changeset_id IS NOT NULL;

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
    OR NEW.changeset_id IS NOT OLD.changeset_id
BEGIN
    SELECT RAISE(ABORT, 'check_runs_end_once');
END;

CREATE TRIGGER check_runs_review_batch_shape BEFORE INSERT ON check_runs
WHEN NEW.review_batch_id IS NOT NULL AND NEW.stage <> 'integration'
BEGIN
    SELECT RAISE(ABORT, 'check_runs_review_batch_shape');
END;

CREATE TRIGGER check_runs_review_batch_fixed BEFORE UPDATE ON check_runs
WHEN NEW.review_batch_id IS NOT OLD.review_batch_id
BEGIN
    SELECT RAISE(ABORT, 'check_runs_review_batch_fixed');
END;

CREATE TRIGGER check_runs_intent_fixed BEFORE UPDATE ON check_runs
WHEN NEW.intent IS NOT OLD.intent
BEGIN
    SELECT RAISE(ABORT, 'check_runs_intent_fixed');
END;

-- Only a changeset of a main workspace is a direct change; task work goes through a proposal.
CREATE TRIGGER check_runs_direct_main_changeset BEFORE INSERT ON check_runs
WHEN NEW.changeset_id IS NOT NULL
    AND (SELECT w.kind FROM changesets c JOIN workspaces w ON w.id = c.workspace_id WHERE c.id = NEW.changeset_id) IS NOT 'main'
BEGIN
    SELECT RAISE(ABORT, 'check_runs_direct_main_changeset');
END;
