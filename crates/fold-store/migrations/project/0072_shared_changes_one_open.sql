-- Each agent has at most one open change of shared files, which gains the paths of each later change until a human, or the main agent when the human asks, dismisses it. Open rows of one agent from before this rule are dismissed into its newest one.
UPDATE shared_changes
SET dismissed_at = created_at, dismissed_by = 'fold'
WHERE dismissed_at IS NULL
  AND id < (SELECT MAX(newer.id) FROM shared_changes newer WHERE newer.agent_id = shared_changes.agent_id AND newer.dismissed_at IS NULL);

CREATE UNIQUE INDEX shared_changes_one_open ON shared_changes (agent_id) WHERE dismissed_at IS NULL;
