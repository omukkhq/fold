-- Flaky checks are found by looking up the later runs of a failed run's tree, so the lookup by tree hash needs its own index.
CREATE INDEX check_runs_tree_hash ON check_runs (tree_hash);
