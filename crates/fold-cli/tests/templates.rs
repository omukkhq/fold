use std::collections::BTreeSet;

const SKILL: &str = include_str!("../templates/SKILL.md");
const AGENTS_SECTION: &str = include_str!("../templates/agents_section.md");

const FOLD_TOOLS: [&str; 74] = [
    "register_agent",
    "register_worker",
    "list_agents",
    "define_area",
    "start_worker",
    "stop_worker",
    "list_workers",
    "set_worker_limits",
    "send_message",
    "get_inbox",
    "get_thread",
    "acknowledge_message",
    "get_events",
    "wait_for_events",
    "get_activity",
    "get_task_graph",
    "get_ready_tasks",
    "get_task",
    "create_task",
    "update_task",
    "delete_task",
    "archive_task",
    "add_dependency",
    "remove_dependency",
    "add_checklist_item",
    "update_checklist_item",
    "move_checklist_item",
    "remove_checklist_item",
    "create_label",
    "claim_task",
    "release_task",
    "takeover_claim",
    "get_repository_state",
    "get_revision",
    "compare_revisions",
    "create_workspace",
    "get_workspace",
    "get_workspace_changes",
    "record_checkpoint",
    "list_checkpoints",
    "restore_checkpoint",
    "record_changes",
    "sync_workspace",
    "abandon_workspace",
    "create_proposal",
    "list_proposals",
    "get_proposal",
    "get_proposal_diff",
    "update_proposal",
    "close_proposal",
    "claim_review",
    "release_review",
    "create_review_checkout",
    "submit_review",
    "get_review_comments",
    "integrate_proposal",
    "require_human_review",
    "create_review_batch",
    "list_review_batches",
    "get_review_batch",
    "remove_from_batch",
    "create_batch_checkout",
    "submit_batch_review",
    "create_request",
    "list_requests",
    "get_request",
    "claim_request",
    "release_request",
    "submit_request_draft",
    "request_draft_changes",
    "edit_request_draft",
    "approve_request_draft",
    "close_request",
    "write_handoff",
];

/// The tools of `FOLD_TOOLS` that the MCP server lists and the skill does not teach yet. A tool leaves this list in the change that teaches it.
const PENDING_TOOLS: [&str; 0] = [];

/// Every code of the error table and the named `invalid_request` reasons.
const FOLD_ERROR_CODES: [&str; 136] = [
    "invalid_id",
    "invalid_timestamp",
    "invalid_request",
    "invalid_name",
    "invalid_config",
    "invalid_handshake",
    "protocol_version_unsupported",
    "invalid_host_paths",
    "not_found",
    "auth_required",
    "invalid_token",
    "token_not_found",
    "human_name_required",
    "human_required",
    "unknown_human",
    "host_not_allowed",
    "agent_not_registered",
    "agent_not_found",
    "agent_already_bound",
    "worker_not_found",
    "project_unavailable",
    "internal_error",
    "handshake_unavailable",
    "project_exists",
    "project_not_found",
    "daemon_already_running",
    "runtime_dir_missing",
    "schema_too_new",
    "database_locked",
    "web_not_built",
    "seed_exists",
    "doctor_checks_failed",
    "service_install_failed",
    "task_not_found",
    "task_closed",
    "label_exists",
    "label_not_found",
    "invalid_label_color",
    "dependency_cycle",
    "dependency_exists",
    "dependency_not_found",
    "already_claimed",
    "already_holder",
    "claim_not_found",
    "claim_not_stale",
    "not_claimed",
    "not_claim_holder",
    "actor_cannot_claim",
    "invalid_checklist_name",
    "invalid_step_text",
    "checklist_item_not_found",
    "workspace_exists",
    "workspace_not_found",
    "workspace_not_active",
    "not_workspace_owner",
    "unresolved_conflicts",
    "checkpoint_not_found",
    "workspace_write_unfinished",
    "already_current",
    "workspace_changed_during_sync",
    "object_missing",
    "target_not_empty",
    "too_large",
    "revision_not_found",
    "path_not_in_comparison",
    "invalid_path",
    "invalid_state_ref",
    "git_failed",
    "git_sync_map_invalid",
    "git_not_found",
    "remote_not_set",
    "remote_ahead",
    "object_corrupt",
    "object_io",
    "workspace_io",
    "unrecorded_changes",
    "invalid_hash",
    "tree_invalid",
    "revision_invalid",
    "export_diverged",
    "gc_in_progress",
    "message_not_found",
    "recipient_not_found",
    "invalid_event_filter",
    "invalid_last_event_id",
    "invalid_push_target",
    "proposal_not_found",
    "proposal_exists",
    "proposal_not_open",
    "not_proposal_author",
    "version_unchanged",
    "stale_version",
    "author_cannot_review",
    "author_cannot_integrate",
    "integrator_required",
    "review_claim_required",
    "empty_review",
    "unknown_comment_path",
    "invalid_comment_range",
    "checkout_failed",
    "human_review_already_set",
    "not_approved",
    "changes_requested",
    "human_approval_missing",
    "prerequisites_not_done",
    "head_moved",
    "integration_conflicted",
    "integration_failed",
    "request_not_found",
    "request_not_open",
    "requester_required",
    "drafter_required",
    "orchestrator_required",
    "invalid_draft",
    "review_batch_not_found",
    "review_batch_not_open",
    "already_in_batch",
    "not_batch_member",
    "batch_conflict",
    "unattributed_lines",
    "area_not_found",
    "area_exists",
    "area_has_lead",
    "main_agent_required",
    "worker_start_denied",
    "worker_stop_denied",
    "task_outside_area",
    "worker_limit_reached",
    "worker_exists",
    "worker_process_not_found",
    "main_tag_denied",
    "handoff_denied",
    "permission_question_not_found",
    "permission_question_closed",
    "worker_peer_refused",
    "client_header_required",
];

/// The codes of `FOLD_ERROR_CODES` that `fold_core::codes` does not define yet. A code leaves this list in the change that adds it.
const PENDING_ERROR_CODES: [&str; 1] = ["too_large"];

/// The `invalid_request` reasons that have a name.
const INVALID_REQUEST_REASONS: [&str; 9] = [
    "self_dependency",
    "checklist_origin",
    "linked_item",
    "task_has_history",
    "proposal_open",
    "malformed_claude_file",
    "no_service_manager",
    "too_many_attachments",
    "attachment_too_large",
];

const NON_TOOL_NAMES: [&str; 20] = [
    "stall_minutes",
    "lease_minutes",
    "request_changes",
    "stop_hook_active",
    "linked_task",
    "start_line",
    "end_line",
    "up_to",
    "expected_version",
    "timeout_s",
    "reply_to",
    "area_lead",
    "review_lead",
    "task_coder",
    "batch_reviewer",
    "idle_minutes",
    "handoff_at",
    "clear_max_per_area",
    "max_per_area",
    "last_reviewed",
];

const SECTIONS: [&str; 14] = [
    "Identity",
    "Messaging",
    "Noticing news",
    "Orchestrating",
    "Orchestrator",
    "Worker",
    "Reviewer",
    "Mentions",
    "Requests",
    "Task template",
    "Subagents",
    "Worker agents",
    "Errors",
    "Never",
];

/// Each lowercase identifier with an underscore inside a backtick span.
fn backticked_identifiers(text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for (index, span) in text.split('`').enumerate() {
        if index % 2 == 0 {
            continue;
        }
        let mut word = String::new();
        for character in span.chars().chain(std::iter::once(' ')) {
            if character.is_ascii_alphanumeric() || character == '_' {
                word.push(character);
            } else {
                let lower = word
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
                if lower && word.contains('_') && !word.starts_with('_') {
                    found.insert(word.clone());
                }
                word.clear();
            }
        }
    }
    found
}

fn known_names() -> BTreeSet<&'static str> {
    FOLD_TOOLS
        .into_iter()
        .chain(FOLD_ERROR_CODES)
        .chain(INVALID_REQUEST_REASONS)
        .chain(NON_TOOL_NAMES)
        .collect()
}

#[test]
fn every_backticked_identifier_is_a_tool_a_code_or_an_allowed_name() {
    let known = known_names();
    for (name, text) in [("SKILL.md", SKILL), ("agents_section.md", AGENTS_SECTION)] {
        for identifier in backticked_identifiers(text) {
            assert!(
                known.contains(identifier.as_str()),
                "{name} names `{identifier}`, which is not a tool, a code or an allowed name"
            );
        }
    }
}

fn server_tools() -> BTreeSet<String> {
    use fold_daemon::mcp::{agents, events, identity, messages, reviews, tasks, workspaces};
    [
        identity::tools(),
        agents::tools(),
        events::tools(),
        workspaces::tools(),
        tasks::tools(),
        tasks::requests::tools(),
        messages::tools(),
        reviews::tools(),
    ]
    .into_iter()
    .flatten()
    .map(|tool| tool.name.into_owned())
    .collect()
}

#[test]
fn the_tool_list_is_the_server_list_and_the_skill_teaches_no_pending_tool() {
    let server = server_tools();
    for tool in PENDING_TOOLS {
        assert!(
            FOLD_TOOLS.contains(&tool),
            "pending `{tool}` is not in FOLD_TOOLS"
        );
        assert!(
            !backticked_identifiers(SKILL).contains(tool),
            "SKILL.md teaches `{tool}`: remove it from PENDING_TOOLS"
        );
    }
    let listed: BTreeSet<String> = FOLD_TOOLS.into_iter().map(str::to_owned).collect();
    assert_eq!(listed, server);
}

#[test]
fn the_code_list_is_the_core_codes_and_the_pending_codes() {
    let defined: BTreeSet<&str> = fold_core::codes::ALL.iter().copied().collect();
    for code in PENDING_ERROR_CODES {
        assert!(
            FOLD_ERROR_CODES.contains(&code),
            "pending `{code}` is not in FOLD_ERROR_CODES"
        );
        assert!(
            !defined.contains(code),
            "fold_core defines `{code}`: remove it from PENDING_ERROR_CODES"
        );
    }
    for code in FOLD_ERROR_CODES {
        assert!(
            defined.contains(code) || PENDING_ERROR_CODES.contains(&code),
            "`{code}` is neither a fold_core code nor pending"
        );
    }
    for reason in INVALID_REQUEST_REASONS {
        assert!(
            !defined.contains(reason),
            "`{reason}` is a code, not a reason"
        );
    }
}

#[test]
fn the_skill_names_every_tool() {
    let named = backticked_identifiers(SKILL);
    for tool in FOLD_TOOLS
        .into_iter()
        .filter(|tool| !PENDING_TOOLS.contains(tool))
    {
        assert!(named.contains(tool), "SKILL.md does not name `{tool}`");
    }
}

#[test]
fn the_tool_and_code_lists_have_no_duplicates() {
    assert_eq!(
        FOLD_TOOLS.into_iter().collect::<BTreeSet<_>>().len(),
        FOLD_TOOLS.len()
    );
    assert_eq!(
        FOLD_ERROR_CODES.into_iter().collect::<BTreeSet<_>>().len(),
        FOLD_ERROR_CODES.len()
    );
}

#[test]
fn the_agents_section_is_short_and_has_no_imports_or_design_documents() {
    assert!(AGENTS_SECTION.lines().count() <= 40);
    for text in [AGENTS_SECTION, SKILL] {
        assert!(text.lines().all(|line| !line.starts_with('@')));
        assert!(!text.contains("ARCHITECTURE.md"));
        assert!(!text.contains("IMPLEMENTATION_PLAN.md"));
    }
}

#[test]
fn the_agents_section_points_to_the_skill_and_the_repository_rules() {
    assert!(AGENTS_SECTION.starts_with("## fold\n"));
    assert!(AGENTS_SECTION.contains("`fld claude`"));
    assert!(AGENTS_SECTION.contains("FOLD_AGENT=<name> claude --name <name>"));
    assert!(AGENTS_SECTION.contains("`fold` skill"));
    assert!(AGENTS_SECTION.contains("`.fold/ws/`"));
    assert!(AGENTS_SECTION.contains("`AGENTS.md` or `CLAUDE.md`"));
}

#[test]
fn neither_template_uses_the_banned_word_for_a_proposal() {
    for text in [AGENTS_SECTION, SKILL] {
        assert!(!text.to_lowercase().contains("pull request"));
        let abbreviation = text
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| word.eq_ignore_ascii_case("pr") || word.eq_ignore_ascii_case("prs"));
        assert!(!abbreviation);
    }
}

#[test]
fn both_templates_keep_agents_out_of_the_project_folder() {
    assert!(AGENTS_SECTION.contains("Never edit the files of the project folder"));
    assert!(SKILL.contains("Never edit the files of the project folder"));
}

#[test]
fn the_skill_has_the_frontmatter_keys_and_every_section() {
    let rest = SKILL.strip_prefix("---\n").expect("frontmatter");
    let (frontmatter, _) = rest.split_once("\n---\n").expect("frontmatter end");
    let keys: Vec<&str> = frontmatter
        .lines()
        .filter_map(|line| line.split_once(": ").map(|(key, _)| key))
        .collect();
    assert_eq!(keys, ["name", "description"]);
    assert!(frontmatter.contains("name: fold\n"));
    let headings: Vec<&str> = SKILL
        .lines()
        .filter_map(|line| line.strip_prefix("## "))
        .collect();
    assert_eq!(headings, SECTIONS);
}

#[test]
fn the_skill_uses_exactly_the_four_verdicts() {
    for verdict in ["comment", "request_changes", "approve", "reject"] {
        assert!(SKILL.contains(&format!("`{verdict}`")), "{verdict}");
    }
}

const SUBCOMMANDS: [&str; 15] = [
    "init", "serve", "mcp", "doctor", "setup", "gc", "agent", "claude", "new", "token", "remote",
    "push", "pull", "hook", "dev",
];

/// Lines that name the old binary on purpose: the hook prefix that `fld init --refresh` replaces in files written for it, and the fixture that proves the refresh replaces its server and hooks.
const LEGACY_LINES: [(&str, &str); 4] = [
    ("crates/fold-cli/src/claude_files.rs", "const HOOK_PREFIXES"),
    (
        "crates/fold-cli/src/claude_files.rs",
        "/// `fold hook` is what files",
    ),
    (
        "crates/fold-cli/tests/claude_files.rs",
        r#""fold": {"command": "fold", "args": ["serve"]}"#,
    ),
    (
        "crates/fold-cli/tests/claude_files.rs",
        r#""command": "fold hook old-start""#,
    ),
];

/// Spellings, with the spaces taken out, that run a plain `fold` executable from JSON, TOML or Rust.
const PLAIN_LAUNCHES: [&str; 3] = [
    "\"command\":\"fold\"",
    "command=\"fold\"",
    "Command::new(\"fold\")",
];

fn text_files(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name != "target" && name != "node_modules" && name != "dist" {
                text_files(&path, found);
            }
        } else if name != "templates.rs" {
            found.push(path);
        }
    }
}

/// Whether `line` names a plain `fold` executable: `fold` as a word followed by a subcommand, a flag or the word binary, a path that ends in `bin/fold`, `release/fold` or `debug/fold`, or a launch from [`PLAIN_LAUNCHES`].
fn names_plain_fold(line: &str) -> bool {
    let compact: String = line.split_whitespace().collect();
    if PLAIN_LAUNCHES.iter().any(|launch| compact.contains(launch)) {
        return true;
    }
    let word_char = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    for dir in ["bin/fold", "release/fold", "debug/fold"] {
        for (at, _) in line.match_indices(dir) {
            if !line[at + dir.len()..].starts_with(|c: char| word_char(c) || c == '.') {
                return true;
            }
        }
    }
    if line.contains("`fold` binary") {
        return true;
    }
    line.match_indices("fold ").any(|(at, _)| {
        let named = line[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !(word_char(c) || "./".contains(c)));
        let after = &line[at + 5..];
        let word: String = after.chars().take_while(|c| word_char(*c)).collect();
        named
            && (SUBCOMMANDS.contains(&word.as_str()) || word == "binary" || after.starts_with("--"))
    })
}

#[test]
fn the_plain_fold_check_tells_commands_from_product_prose() {
    for command in [
        "run `fold init` first",
        "fold mcp",
        "cd x && fold init",
        "`fold --version` prints it",
        r#"  "command": "fold","#,
        r#"[mcp_servers.fold] command = "fold""#,
        r#"Command::new("fold")"#,
        "mv fld-x86_64-linux ~/.local/bin/fold",
        "target/release/fold is the binary",
        "the `fold` binary",
        "find the fold binary",
        r#"{"command": "fold hook stop"}"#,
    ] {
        assert!(names_plain_fold(command), "{command}");
    }
    for prose in [
        "fold plans, builds, reviews and integrates",
        "the `fold` MCP server",
        "a message from `fold` that names the file",
        "Whether fold pushes to this remote",
        "`.fold/git`, `FOLD_AGENT` and `mcp__fold`",
        "$XDG_RUNTIME_DIR/fold/fold.sock",
        "fold-native proposals, fold-cli and fold's crates",
        "~/.local/bin/folder",
        r#"  "command": "fld","#,
        "run fld init in the project root",
        "fld hook session-start",
    ] {
        assert!(!names_plain_fold(prose), "{prose}");
    }
}

/// The executable is `fld`: no generated file, hint, doc or test names a plain `fold` binary with a subcommand.
#[test]
fn no_text_calls_a_plain_fold_binary() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = [
        "README.md",
        "AGENTS.md",
        "Makefile",
        ".pre-commit-config.yaml",
    ]
    .map(|name| root.join(name))
    .to_vec();
    for part in ["crates", "docs", "scripts", "web/src"] {
        text_files(&root.join(part), &mut files);
    }
    let mut offenders = Vec::new();
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        let relative = file.strip_prefix(&root).unwrap_or(file);
        for (number, line) in text.lines().enumerate() {
            let legacy = LEGACY_LINES
                .iter()
                .any(|(path, part)| relative == std::path::Path::new(path) && line.contains(part));
            if !legacy && names_plain_fold(line) {
                offenders.push(format!("{}:{}", relative.display(), number + 1));
            }
        }
    }
    assert!(offenders.is_empty(), "plain `fold` command: {offenders:?}");
}
