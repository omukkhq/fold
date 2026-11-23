-- Reads of the workers of one role, with or without the ended ones, go through this index instead of the whole table, which keeps every ended worker.
CREATE INDEX worker_processes_role_state ON worker_processes (role, state);
