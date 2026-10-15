// A file in this directory is compiled and run only through its `mod` line here.
#[cfg(test)]
pub mod common;
#[cfg(test)]
pub mod revisions_support;
#[cfg(test)]
pub mod seed;

mod agents_schema;
mod changesets;
mod checklists;
mod checkpoints;
mod claims;
mod completions;
mod counters;
mod dependencies;
mod events;
mod gc;
mod humans;
mod integrations_schema;
mod messages_schema;
mod proposals_schema;
mod registry;
mod removal;
mod requests_schema;
mod review_batches_schema;
mod reviews_schema;
mod revision_reads;
mod revisions;
mod schema;
mod store;
mod tasks;
mod test_project;
mod workspace_conflicts;
mod workspaces;
