use super::Role;

/// How many hex digits of the hash a fingerprint keeps. Two skill texts differ, never by chance in these digits.
const HASH_DIGITS: usize = 16;

/// What a fold session of one role read from the installed fld: a hash of `texts`, such as `3f2a9c0b1d4e5f60`. The interactive main agent's fingerprint covers the skill texts it reads; a worker's covers them, its role's prompt and the tool schemas. A session keeps the text it loaded and the tool schemas it listed for as long as it lives, so a resumed session whose recorded fingerprint differs from the installed one works by older text until it loads it again.
pub fn session_fingerprint(texts: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new();
    for text in texts {
        hasher.update(&(text.len() as u64).to_le_bytes());
        hasher.update(text.as_bytes());
    }
    hasher.finalize().to_hex()[..HASH_DIGITS].to_owned()
}

/// Whether a session that recorded `recorded` gets [`update_note`] before its next turn: the installed fld has a fingerprint, and it is not the recorded one. A session with no record was started by a fold that recorded none, which is an older one. Without a `current` fingerprint fold cannot compare, and says nothing.
pub fn needs_update_note(recorded: Option<&str>, current: Option<&str>) -> bool {
    current.is_some_and(|current| recorded != Some(current))
}

/// The line that starts the first turn of a resumed session whose fingerprint is not the installed one, which tells the agent to load its role skill again.
pub fn update_note(role: Role) -> String {
    let skill = role.skill();
    format!(
        "fold was updated since this session last loaded its skill. Load the `{skill}` skill again now, and where it differs from what you remember, follow the skill."
    )
}

/// The update note of a session that keeps its session after an update, a task worker or a reviewer: [`update_note`] and the tools whose schemas changed, `changed`, which the session must read again because Claude Code keeps the schemas it listed at its start. `None` is a session whose older fold recorded no tool schemas, so fold cannot name them.
pub fn reload_note(role: Role, changed: Option<&[String]>) -> String {
    let note = update_note(role);
    match changed {
        None => format!(
            "{note} fold cannot name the tools whose schemas changed, so read the schema of each fold tool again before you call it."
        ),
        Some([]) => note,
        Some(tools) => {
            let names = tools
                .iter()
                .map(|tool| format!("`{tool}`"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{note} These tools changed: {names}. Read their schemas again before you call them."
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fingerprint_changes_with_each_skill_text_and_only_with_them() {
        let base = session_fingerprint(&["skill", "shared"]);
        assert_eq!(base.len(), HASH_DIGITS);
        assert!(base.bytes().all(|byte| byte.is_ascii_hexdigit()), "{base}");
        assert_eq!(base, session_fingerprint(&["skill", "shared"]));
        assert_ne!(base, session_fingerprint(&["skill.", "shared"]));
        assert_ne!(base, session_fingerprint(&["skill", "shared."]));
        assert_ne!(base, session_fingerprint(&["skill"]));
        assert_ne!(
            session_fingerprint(&["ab", "c"]),
            session_fingerprint(&["a", "bc"]),
            "the texts are kept apart"
        );
    }

    #[test]
    fn only_a_session_of_another_fingerprint_gets_the_note() {
        assert!(!needs_update_note(Some("aaaa"), Some("aaaa")));
        assert!(needs_update_note(Some("bbbb"), Some("aaaa")));
        assert!(needs_update_note(None, Some("aaaa")));
        assert!(!needs_update_note(Some("bbbb"), None));
        assert!(!needs_update_note(None, None));
    }

    #[test]
    fn the_reload_note_names_the_changed_tools() {
        let base = update_note(Role::TaskWorker);
        let tools = ["create_proposal".to_owned(), "get_task".to_owned()];
        let named = reload_note(Role::TaskWorker, Some(&tools));
        assert!(named.starts_with(&base), "{named}");
        assert!(
            named.ends_with(
                "These tools changed: `create_proposal`, `get_task`. Read their schemas again before you call them."
            ),
            "{named}"
        );
        assert_eq!(
            reload_note(Role::Reviewer, Some(&[])),
            update_note(Role::Reviewer)
        );
        let unknown = reload_note(Role::Reviewer, None);
        assert!(
            unknown.starts_with(&update_note(Role::Reviewer)),
            "{unknown}"
        );
        assert!(unknown.contains("cannot name the tools"), "{unknown}");
    }

    #[test]
    fn the_note_names_the_skill_of_the_role_and_comes_first() {
        for role in Role::ALL {
            let note = update_note(role);
            assert!(
                note.contains(&format!("Load the `{}` skill again", role.skill())),
                "{note}"
            );
        }
        assert!(update_note(Role::ReviewLead).contains("`fold-review-lead`"));
    }
}
