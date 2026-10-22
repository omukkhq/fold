use fold_core::Role;
use rmcp::model::{MetaObject, Tool};
use serde_json::Value;

/// The `_meta` key of a tool whose schema Claude Code loads with the session, where it would otherwise defer the tool until the agent finds it with its tool search. The mark goes on each tool, not on fold's server, because one server serves every role and the main agent's server entry is the user's own.
pub const ALWAYS_LOAD: &str = "anthropic/alwaysLoad";

/// What a session that lists the tools acts as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seat {
    /// No agent yet: a session opened by hand before `register_agent`.
    Unbound,
    /// An agent that follows the steps of a role, the interactive main agent as [`Role::Main`].
    Role(Role),
    /// An agent without a role, such as a session opened by hand after `register_agent`, which does no fixed steps.
    NoRole,
}

/// The fold tools that a seat calls in most of its turns, from the steps of its role prompt and its role skill. Every other tool stays deferred, so a session pays for its schema only when it looks the tool up.
pub fn always_loaded(seat: Seat) -> &'static [&'static str] {
    match seat {
        Seat::Unbound => &["register_agent"],
        Seat::NoRole => &[],
        Seat::Role(Role::Main) => &[
            "get_task_graph",
            "list_requests",
            "get_inbox",
            "list_workers",
            "get_repository_state",
            "get_task",
            "get_activity",
            "create_task_plan",
            "start_worker",
            "wait_for_events",
            "send_message",
            "acknowledge_message",
        ],
        Seat::Role(Role::AreaLead) => &[
            "get_task_graph",
            "get_task",
            "get_ready_tasks",
            "create_task_plan",
            "start_worker",
            "list_workers",
            "wait_for_events",
            "send_message",
            "get_inbox",
            "acknowledge_message",
        ],
        Seat::Role(Role::ReviewLead) => &[
            "list_proposals",
            "get_proposal",
            "get_proposal_diff",
            "get_task_graph",
            "start_worker",
            "list_workers",
            "create_review_plan",
            "wait_for_events",
            "send_message",
            "get_inbox",
            "acknowledge_message",
        ],
        Seat::Role(Role::TaskWorker) => &[
            "get_task",
            "get_workspace",
            "create_workspace",
            "get_workspace_changes",
            "record_checkpoint",
            "run_checks",
            "wait_for_events",
            "get_check_run",
            "update_checklist_item",
            "record_changes",
            "create_proposal",
            "update_proposal",
            "sync_workspace",
            "get_review_comments",
            "send_message",
        ],
        Seat::Role(Role::Reviewer) => &[
            "get_proposal",
            "get_proposal_diff",
            "get_review_comments",
            "create_review_checkout",
            "get_check_run",
            "submit_review",
            "submit_review_fix",
            "wait_for_events",
            "create_batch_checkout",
            "submit_batch_review",
            "send_message",
        ],
    }
}

/// Marks the tools of `seat` from [`always_loaded`] to load at once and leaves the others as they are.
pub fn mark(tools: &mut [Tool], seat: Seat) {
    let loaded = always_loaded(seat);
    for tool in tools
        .iter_mut()
        .filter(|tool| loaded.contains(&tool.name.as_ref()))
    {
        let mut meta = tool.meta.take().unwrap_or_default();
        meta.0.insert(ALWAYS_LOAD.to_owned(), Value::Bool(true));
        tool.meta = Some(meta);
    }
}

/// Whether a listed tool carries the mark of [`mark`].
pub fn is_always_loaded(tool: &Tool) -> bool {
    tool.meta
        .as_ref()
        .and_then(|meta: &MetaObject| meta.0.get(ALWAYS_LOAD))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_seat() -> Vec<Seat> {
        [Seat::Unbound, Seat::NoRole]
            .into_iter()
            .chain(Role::ALL.into_iter().map(Seat::Role))
            .collect()
    }

    fn listed() -> Vec<Tool> {
        let mut tools = crate::mcp::identity::tools();
        tools.extend(crate::mcp::agents::tools());
        tools.extend(crate::mcp::events::tools());
        tools.extend(crate::mcp::workspaces::tools());
        tools.extend(crate::mcp::tasks::tools());
        tools.extend(crate::mcp::tasks::requests::tools());
        tools.extend(crate::mcp::messages::tools());
        tools.extend(crate::mcp::reviews::tools());
        tools
    }

    #[test]
    fn every_always_loaded_tool_is_a_listed_tool_once() {
        let names: Vec<String> = listed()
            .into_iter()
            .map(|tool| tool.name.into_owned())
            .collect();
        for seat in every_seat() {
            let loaded = always_loaded(seat);
            for name in loaded {
                assert!(
                    names.iter().any(|listed| listed == name),
                    "{seat:?}: {name}"
                );
                assert_eq!(
                    loaded.iter().filter(|other| *other == name).count(),
                    1,
                    "{seat:?}: {name}"
                );
            }
        }
    }

    #[test]
    fn the_mark_goes_only_on_the_seats_tools_and_keeps_other_meta() {
        for seat in every_seat() {
            let mut tools = listed();
            if let Some(first) = tools.first_mut() {
                let mut meta = MetaObject::new();
                meta.0.insert("other".to_owned(), Value::Bool(false));
                first.meta = Some(meta);
            }
            mark(&mut tools, seat);
            let marked: Vec<&str> = tools
                .iter()
                .filter(|tool| is_always_loaded(tool))
                .map(|tool| tool.name.as_ref())
                .collect();
            let mut expected = always_loaded(seat).to_vec();
            expected.sort_unstable();
            let mut sorted = marked.clone();
            sorted.sort_unstable();
            assert_eq!(sorted, expected, "{seat:?}");
            assert!(tools[0].meta.as_ref().unwrap().0.contains_key("other"));
        }
        assert_eq!(always_loaded(Seat::Unbound), ["register_agent"]);
        assert!(always_loaded(Seat::NoRole).is_empty());
    }
}
