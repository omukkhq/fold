// A file in this directory is compiled and run only through its `mod` line here.
#[cfg(test)]
pub mod common;
#[cfg(test)]
pub mod revisions_support;
#[cfg(test)]
pub mod seed;

mod agents_schema;
mod batch_integration_schema;
mod changesets;
mod check_runs_schema;
mod checklists;
mod checkpoints;
mod claims;
mod completions;
mod counters;
mod dependencies;
mod events;
mod folder_stashes_schema;
mod gc;
mod handoffs_schema;
mod human_changes_schema;
mod human_tasks_schema;
mod humans;
mod integrations_schema;
mod main_workspaces_schema;
mod messages_schema;
mod proposals_schema;
mod registry;
mod removal;
mod requests_schema;
mod review_batches_schema;
mod review_plans_schema;
mod reviews_schema;
mod revision_reads;
mod revisions;
mod schema;
mod store;
mod store_rules;
mod task_reports_schema;
mod tasks;
mod test_project;
mod worker_cursors_schema;
mod worker_questions_schema;
mod workers_schema;
mod workspace_conflicts;
mod workspaces;
