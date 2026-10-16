// A file in this directory is compiled and run only through its `mod` line here.
#[cfg(test)]
pub mod adoption_support;
#[cfg(test)]
pub mod common;
#[cfg(test)]
pub mod daemon_support;
#[cfg(test)]
pub mod git_support;
#[cfg(test)]
pub mod proposal_tools_support;
#[cfg(test)]
pub mod reviews_support;
#[cfg(test)]
pub mod workspaces_support;

mod acceptance_10_requests;
mod acceptance_1_three_sessions;
mod acceptance_2_messaging;
mod acceptance_3_parallel_workspaces;
mod acceptance_4_live_diff;
mod acceptance_5_review_and_integrate;
mod acceptance_6_dependency_unblocks;
mod acceptance_7_conflicting_proposals;
mod acceptance_8_restart_recovery;
mod acceptance_9_offline;
mod adoption;
mod attention_question;
mod auto_checkpoints;
mod auto_integrate;
#[cfg(feature = "failpoints")]
mod auto_integrate_failpoints;
mod checklists_service;
mod claims_service;
mod claude_probe;
mod conflicts;
#[cfg(feature = "failpoints")]
mod crash_checkpoint;
#[cfg(feature = "failpoints")]
mod crash_git_commit;
#[cfg(feature = "failpoints")]
mod crash_init;
#[cfg(feature = "failpoints")]
mod crash_integration;
#[cfg(feature = "failpoints")]
mod crash_messages;
mod daemon_shell;
mod delete_archive;
mod dependencies_service;
mod diff_routes;
mod event_bus;
mod events_service;
mod gc;
mod git_commits;
mod http_agents;
mod http_assets;
mod http_auth;
mod http_events_history;
mod http_human_review;
mod http_messages;
mod http_project_overview;
mod http_requests;
mod http_review_batches;
mod http_review_checkouts;
mod http_reviews;
mod http_tasks;
mod http_workspaces;
mod human_review;
mod inbox_probe;
mod integration;
mod lease_sweeper;
mod live_state;
mod mcp_activity;
mod mcp_footer;
mod mcp_handshake;
mod mcp_identity;
mod mcp_messages;
mod mcp_requests;
mod mcp_review_batches;
mod mcp_reviews;
mod mcp_tasks;
mod mcp_wait_for_events;
mod mcp_workspace_tools;
mod mcp_workspaces;
mod notifications;
mod openapi;
mod presence_events;
mod project_git;
mod projects;
mod proposals;
mod pull;
mod push;
mod push_requests;
mod ready;
mod recompute;
mod recompute_facts;
mod recompute_guard;
mod reconcile;
#[cfg(feature = "failpoints")]
mod reconcile_failpoints;
mod requests;
mod restart_sessions;
mod review_batches;
mod review_checkouts;
mod review_claims;
mod reviews;
mod services_agents;
mod services_inbox;
mod services_messages;
mod session_context;
mod session_push;
mod sse_stream;
mod takeover;
mod task_read;
mod task_views;
mod tasks_service;
mod web_tokens;
mod workspaces_compare;
#[cfg(feature = "failpoints")]
mod workspaces_failpoints;
mod workspaces_git;
mod workspaces_repository;
mod workspaces_service;
#[cfg(feature = "failpoints")]
mod workspaces_tools_failpoints;
