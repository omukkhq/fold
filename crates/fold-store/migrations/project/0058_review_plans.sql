-- A review plan: open proposals that nobody reviewed yet and that wait for related tasks to open their proposals, so one reviewer reviews them all as one batch. The review lead `lead_agent_id` makes it; its wait ends at `deadline_at`, an hour after `created_at`, and `wait_ended_at` records when fold ended it. Starting the plan gives its open proposals to one reviewer, `started_agent_id`, and records the batch they became in `review_batch_id` (empty when one proposal was left, which the reviewer reviews alone). The members of that batch are the plan's members released with the reason `started`.
CREATE TABLE review_plans (
    id INTEGER PRIMARY KEY,
    number INTEGER NOT NULL UNIQUE,
    lead_agent_id INTEGER NOT NULL REFERENCES agents (id),
    state TEXT NOT NULL DEFAULT 'open' CONSTRAINT review_plans_state_values CHECK (state IN ('open', 'started', 'ended')),
    created_at INTEGER NOT NULL,
    deadline_at INTEGER NOT NULL,
    wait_ended_at INTEGER,
    started_at INTEGER,
    started_agent_id INTEGER REFERENCES agents (id),
    review_batch_id INTEGER REFERENCES review_batches (id),
    ended_at INTEGER,
    CONSTRAINT review_plans_deadline_after_creation CHECK (deadline_at > created_at),
    CONSTRAINT review_plans_started_at_iff_started CHECK ((state = 'started') = (started_at IS NOT NULL)),
    CONSTRAINT review_plans_started_by_agent CHECK ((started_at IS NULL) = (started_agent_id IS NULL)),
    CONSTRAINT review_plans_batch_of_start CHECK (review_batch_id IS NULL OR started_at IS NOT NULL),
    CONSTRAINT review_plans_ended_at_iff_ended CHECK ((state = 'ended') = (ended_at IS NOT NULL))
);

-- The deadline job reads the open plans whose wait has not ended.
CREATE INDEX review_plans_waiting ON review_plans (deadline_at) WHERE state = 'open' AND wait_ended_at IS NULL;

CREATE INDEX review_plans_batch ON review_plans (review_batch_id) WHERE review_batch_id IS NOT NULL;

CREATE TRIGGER review_plans_no_delete BEFORE DELETE ON review_plans
BEGIN
    SELECT RAISE(ABORT, 'review_plans keeps every plan');
END;

-- An open plan ends its wait once, then starts or ends; a started or ended plan never changes.
CREATE TRIGGER review_plans_forward_only BEFORE UPDATE ON review_plans
WHEN OLD.state <> 'open'
    OR NEW.number <> OLD.number OR NEW.lead_agent_id <> OLD.lead_agent_id
    OR NEW.created_at <> OLD.created_at OR NEW.deadline_at <> OLD.deadline_at
    OR (OLD.wait_ended_at IS NOT NULL AND NEW.wait_ended_at IS NOT OLD.wait_ended_at)
BEGIN
    SELECT RAISE(ABORT, 'review_plans_forward_only');
END;

-- One task of a plan: awaited while `proposal_id` is empty, joined once its proposal is in the plan with the version it had then. `released_at` stays empty while the member is in the plan.
CREATE TABLE review_plan_members (
    id INTEGER PRIMARY KEY,
    plan_id INTEGER NOT NULL REFERENCES review_plans (id),
    task_id INTEGER NOT NULL REFERENCES tasks (id),
    proposal_id INTEGER REFERENCES proposals (id),
    version_id INTEGER REFERENCES proposal_versions (id),
    added_at INTEGER NOT NULL,
    joined_at INTEGER,
    released_at INTEGER,
    release_reason TEXT CONSTRAINT review_plan_members_release_reason_values CHECK (release_reason IN (
        'started', 'not_opened', 'deadline', 'removed', 'closed', 'reviewed', 'claimed', 'archived', 'not_proposal_kind',
        'main_area', 'human_review', 'blocker', 'depends_on_member', 'backlog', 'in_review', 'done'
    )),
    CONSTRAINT review_plan_members_version_with_proposal CHECK ((proposal_id IS NULL) = (version_id IS NULL)),
    CONSTRAINT review_plan_members_joined_with_proposal CHECK ((proposal_id IS NULL) = (joined_at IS NULL)),
    CONSTRAINT review_plan_members_released_with_reason CHECK ((released_at IS NULL) = (release_reason IS NULL)),
    UNIQUE (plan_id, task_id)
);

-- A task, and so its proposal, is in at most one plan at a time.
CREATE UNIQUE INDEX review_plan_members_one_live_task ON review_plan_members (task_id) WHERE released_at IS NULL;

CREATE UNIQUE INDEX review_plan_members_one_live_proposal ON review_plan_members (proposal_id)
    WHERE released_at IS NULL AND proposal_id IS NOT NULL;

CREATE INDEX review_plan_members_plan ON review_plan_members (plan_id);

-- A member joins only an open plan, an awaited member only before the plan's wait ended, and a plan holds at most five members at a time.
CREATE TRIGGER review_plan_members_open_plan BEFORE INSERT ON review_plan_members
WHEN (SELECT state FROM review_plans WHERE id = NEW.plan_id) IS NOT 'open'
    OR (NEW.proposal_id IS NULL AND (SELECT wait_ended_at FROM review_plans WHERE id = NEW.plan_id) IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT, 'review_plan_members_open_plan');
END;

CREATE TRIGGER review_plan_members_at_most_five BEFORE INSERT ON review_plan_members
WHEN (SELECT count(*) FROM review_plan_members WHERE plan_id = NEW.plan_id AND released_at IS NULL) >= 5
BEGIN
    SELECT RAISE(ABORT, 'review_plan_members_at_most_five');
END;

-- A member's proposal is its task's, and its version is the proposal's.
CREATE TRIGGER review_plan_members_proposal_of_task_insert BEFORE INSERT ON review_plan_members
WHEN NEW.proposal_id IS NOT NULL AND (
    (SELECT task_id FROM proposals WHERE id = NEW.proposal_id) IS NOT NEW.task_id
    OR (SELECT proposal_id FROM proposal_versions WHERE id = NEW.version_id) IS NOT NEW.proposal_id
)
BEGIN
    SELECT RAISE(ABORT, 'review_plan_members_proposal_of_task');
END;

CREATE TRIGGER review_plan_members_proposal_of_task_update BEFORE UPDATE OF proposal_id, version_id ON review_plan_members
WHEN NEW.proposal_id IS NOT NULL AND (
    (SELECT task_id FROM proposals WHERE id = NEW.proposal_id) IS NOT NEW.task_id
    OR (SELECT proposal_id FROM proposal_versions WHERE id = NEW.version_id) IS NOT NEW.proposal_id
)
BEGIN
    SELECT RAISE(ABORT, 'review_plan_members_proposal_of_task');
END;

CREATE TRIGGER review_plan_members_no_delete BEFORE DELETE ON review_plan_members
BEGIN
    SELECT RAISE(ABORT, 'review_plan_members keeps every member');
END;

-- A member changes twice at most: an awaited member joins with its proposal, and a member in the plan is released once.
CREATE TRIGGER review_plan_members_join_or_release BEFORE UPDATE ON review_plan_members
WHEN OLD.released_at IS NOT NULL
    OR NEW.plan_id <> OLD.plan_id OR NEW.task_id <> OLD.task_id OR NEW.added_at <> OLD.added_at
    OR (OLD.proposal_id IS NOT NULL AND (
        NEW.proposal_id IS NOT OLD.proposal_id OR NEW.version_id IS NOT OLD.version_id
        OR NEW.joined_at IS NOT OLD.joined_at
    ))
BEGIN
    SELECT RAISE(ABORT, 'review_plan_members_join_or_release');
END;
