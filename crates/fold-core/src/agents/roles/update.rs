use super::Role;

/// How many hex digits of the text hash a fingerprint keeps after the version. Two releases of one version differ in their texts, never by chance in these digits.
const HASH_DIGITS: usize = 16;

/// What a fold session of one role read from the installed fld: the version, then a hash of the role's shipped prompt and of its skill, such as `0.5.0+3f2a9c0b1d4e5f60`. A session keeps the skill text it loaded for as long as it lives, so a resumed session whose recorded fingerprint differs from the installed one works by an older skill until it loads the skill again. The hash covers the texts as well as the version, because a build between two releases changes them under one version.
pub fn session_fingerprint(version: &str, prompt: &str, skill: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    for text in [prompt, skill] {
        hasher.update(&(text.len() as u64).to_le_bytes());
        hasher.update(text.as_bytes());
    }
    let hash = hasher.finalize().to_hex();
    format!("{version}+{}", &hash[..HASH_DIGITS])
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
    fn a_fingerprint_starts_with_the_version_and_changes_with_each_text() {
        let base = session_fingerprint("0.5.0", "prompt", "skill");
        assert!(base.starts_with("0.5.0+"), "{base}");
        assert_eq!(base.len(), "0.5.0+".len() + HASH_DIGITS);
        assert_eq!(base, session_fingerprint("0.5.0", "prompt", "skill"));
        assert_ne!(base, session_fingerprint("0.6.0", "prompt", "skill"));
        assert_ne!(base, session_fingerprint("0.5.0", "prompt.", "skill"));
        assert_ne!(base, session_fingerprint("0.5.0", "prompt", "skill."));
        assert_ne!(
            session_fingerprint("0.5.0", "ab", "c"),
            session_fingerprint("0.5.0", "a", "bc"),
            "the texts are kept apart"
        );
    }

    #[test]
    fn only_a_session_of_another_fingerprint_gets_the_note() {
        assert!(!needs_update_note(Some("0.5.0+a"), Some("0.5.0+a")));
        assert!(needs_update_note(Some("0.4.0+b"), Some("0.5.0+a")));
        assert!(needs_update_note(None, Some("0.5.0+a")));
        assert!(!needs_update_note(Some("0.4.0+b"), None));
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
