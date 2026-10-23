use super::Role;

/// How many hex digits of the hash a fingerprint keeps. Two skill texts differ, never by chance in these digits.
const HASH_DIGITS: usize = 16;

/// What a fold session of one role read from the installed fld: a hash of the skill texts the role reads, such as `3f2a9c0b1d4e5f60`. A session keeps the skill text it loaded for as long as it lives, so a resumed session whose recorded fingerprint differs from the installed one works by an older skill until it loads the skill again. Only the skill texts count: every start passes the role's prompt again, and a release that leaves the skills as they are changes nothing a session holds.
pub fn session_fingerprint(skills: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new();
    for text in skills {
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

/// `turn` with [`update_note`] of `role` before it.
pub fn with_update_note(role: Role, turn: &str) -> String {
    format!("{}\n\n{turn}", update_note(role))
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
    fn the_note_names_the_skill_of_the_role_and_comes_first() {
        for role in Role::ALL {
            let note = update_note(role);
            assert!(
                note.contains(&format!("Load the `{}` skill again", role.skill())),
                "{note}"
            );
        }
        assert_eq!(
            with_update_note(Role::ReviewLead, "Continue."),
            format!("{}\n\nContinue.", update_note(Role::ReviewLead))
        );
        assert!(update_note(Role::ReviewLead).contains("`fold-review-lead`"));
    }
}
