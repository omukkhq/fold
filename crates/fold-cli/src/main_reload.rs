use std::collections::BTreeMap;

use fold_core::roles::{Role, needs_update_note, reload_note};
use fold_core::tool_schemas::changed_tools;
use serde_json::Value;

/// The Claude Code event of the hook that repeats the reload note at each turn of the main agent's session.
pub const PROMPT_HOOK_EVENT: &str = "UserPromptSubmit";
/// The command of [`PROMPT_HOOK_EVENT`].
pub const PROMPT_HOOK_COMMAND: &str = "fld hook user-prompt-submit";
/// The Claude Code event of the hook that sees the main agent load a skill.
pub const SKILL_HOOK_EVENT: &str = "PostToolUse";
/// The command of [`SKILL_HOOK_EVENT`].
pub const SKILL_HOOK_COMMAND: &str = "fld hook post-tool-use";
/// The Claude Code tool through which a model loads a skill.
const SKILL_TOOL: &str = "Skill";

/// What the main agent's session last loaded from fld: its session id, the fingerprint of the skill texts and the hash of each tool schema. The fingerprint record holds it as `<session> <fingerprint>` on the first line and one `<tool> <hash>` line for each tool; a record of an older fld has the first line only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub session: String,
    pub fingerprint: String,
    pub tools: BTreeMap<String, String>,
}

impl Loaded {
    /// The record in `text`, or `None` when a line does not have two words.
    pub fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines().filter(|line| !line.trim().is_empty());
        let (session, fingerprint) = lines.next()?.trim().split_once(' ')?;
        let mut tools = BTreeMap::new();
        for line in lines {
            let (tool, hash) = line.trim().split_once(' ')?;
            tools.insert(tool.to_owned(), hash.to_owned());
        }
        Some(Self {
            session: session.to_owned(),
            fingerprint: fingerprint.to_owned(),
            tools,
        })
    }

    /// The text of the record, which [`Loaded::parse`] reads back.
    pub fn render(&self) -> String {
        let mut text = format!("{} {}\n", self.session, self.fingerprint);
        for (tool, hash) in &self.tools {
            text.push_str(&format!("{tool} {hash}\n"));
        }
        text
    }
}

/// What the installed fld gives the main agent's session: the fingerprint of its skill texts and the hash of each tool schema.
#[derive(Debug, Clone, Copy)]
pub struct Current<'a> {
    pub fingerprint: &'a str,
    pub tools: &'a BTreeMap<String, String>,
}

impl Current<'_> {
    /// The record of `session` once it loaded these texts.
    pub fn loaded_by(&self, session: &str) -> Loaded {
        Loaded {
            session: session.to_owned(),
            fingerprint: self.fingerprint.to_owned(),
            tools: self.tools.clone(),
        }
    }
}

/// The reload note for `session` when it has not loaded what `current` gives, by `record`: its skill fingerprint differs, or a tool schema changed. The note names the changed tools; a record without tool hashes, or of another session, cannot name them. A tool change alone also needs the note, because Claude Code keeps the schemas it listed and only the skill tells the agent to read them again.
pub fn reload_due(record: Option<&Loaded>, session: &str, current: Current<'_>) -> Option<String> {
    let loaded = record.filter(|record| record.session == session);
    let changed = loaded
        .filter(|loaded| !loaded.tools.is_empty())
        .map(|loaded| changed_tools(&loaded.tools, current.tools));
    let skill_stale = needs_update_note(
        loaded.map(|loaded| loaded.fingerprint.as_str()),
        Some(current.fingerprint),
    );
    let tools_stale = changed.as_ref().is_some_and(|changed| !changed.is_empty());
    (skill_stale || tools_stale).then(|| reload_note(Role::Main, changed.as_deref()))
}

/// What the prompt hook does at one turn of the main agent's session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Turn {
    /// The session loads its skill with this turn, so the record takes the current texts.
    Loads,
    /// The session has not loaded its skill since the update: the turn gets the note.
    Remind(String),
    /// Nothing to tell.
    Quiet,
}

/// The prompt hook's decision for `prompt` in `session`. A prompt that is the note itself, the first turn that `fld claude` gives, gets it no second time; a prompt that types the skill's command loads it.
pub fn prompt_turn(
    record: Option<&Loaded>,
    session: &str,
    prompt: &str,
    current: Current<'_>,
) -> Turn {
    let Some(note) = reload_due(record, session, current) else {
        return Turn::Quiet;
    };
    let command = format!("/{}", Role::Main.skill());
    if prompt.split_whitespace().next() == Some(command.as_str()) {
        Turn::Loads
    } else if prompt.trim() == note {
        Turn::Quiet
    } else {
        Turn::Remind(note)
    }
}

/// Whether a tool call that the skill hook sees, `tool` with `input`, loads the main agent's skill.
pub fn loads_main_skill(tool: &str, input: &Value) -> bool {
    tool == SKILL_TOOL
        && input["skill"]
            .as_str()
            .is_some_and(|skill| skill.trim_start_matches('/') == Role::Main.skill())
}

/// The hooks that only the main agent's session gets, which `fld claude` passes with `--settings`: they merge with the user's own hooks.
pub fn main_hooks() -> Value {
    serde_json::json!({
        PROMPT_HOOK_EVENT: [{"hooks": [{"type": "command", "command": PROMPT_HOOK_COMMAND}]}],
        SKILL_HOOK_EVENT: [{"matcher": SKILL_TOOL, "hooks": [{"type": "command", "command": SKILL_HOOK_COMMAND}]}],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "0b6f8a2e-1c3d-4e5f-8a9b-0c1d2e3f4a5b";

    fn tools(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(tool, hash)| ((*tool).to_owned(), (*hash).to_owned()))
            .collect()
    }

    fn record(fingerprint: &str, pairs: &[(&str, &str)]) -> Loaded {
        Loaded {
            session: SESSION.to_owned(),
            fingerprint: fingerprint.to_owned(),
            tools: tools(pairs),
        }
    }

    #[test]
    fn a_record_reads_back_what_it_wrote_and_an_older_one_has_no_tools() {
        let loaded = record("aaaa", &[("get_task", "11"), ("create_proposal", "22")]);
        let text = loaded.render();
        assert_eq!(
            text,
            format!("{SESSION} aaaa\ncreate_proposal 22\nget_task 11\n")
        );
        assert_eq!(Loaded::parse(&text), Some(loaded));
        assert_eq!(
            Loaded::parse(&format!("{SESSION} aaaa\n")),
            Some(record("aaaa", &[]))
        );
        assert_eq!(Loaded::parse("garbage"), None);
        assert_eq!(Loaded::parse(&format!("{SESSION} aaaa\nbroken\n")), None);
        assert_eq!(Loaded::parse(""), None);
    }

    #[test]
    fn the_reload_message_names_a_changed_tool() {
        let now = tools(&[("get_task", "12"), ("create_proposal", "22")]);
        let current = Current {
            fingerprint: "bbbb",
            tools: &now,
        };
        let old = record("aaaa", &[("get_task", "11"), ("create_proposal", "22")]);
        let note = reload_due(Some(&old), SESSION, current).unwrap();
        assert!(note.contains("These tools changed: `get_task`."), "{note}");
        assert!(note.contains("`fold-main`"), "{note}");

        let same_skill = record("bbbb", &[("get_task", "11"), ("create_proposal", "22")]);
        let note = reload_due(Some(&same_skill), SESSION, current).unwrap();
        assert!(
            note.contains("`get_task`"),
            "a tool change alone needs the note: {note}"
        );

        let older = record("aaaa", &[]);
        let note = reload_due(Some(&older), SESSION, current).unwrap();
        assert!(note.contains("cannot name the tools"), "{note}");
        let note = reload_due(None, SESSION, current).unwrap();
        assert!(note.contains("cannot name the tools"), "{note}");

        assert_eq!(
            reload_due(Some(&current.loaded_by(SESSION)), SESSION, current),
            None
        );
        assert_eq!(
            reload_due(Some(&record("bbbb", &[])), SESSION, current),
            None,
            "an older record of the same skill text cannot show a tool change"
        );
        assert!(
            reload_due(Some(&current.loaded_by("other")), SESSION, current).is_some(),
            "a record of another session says nothing of this one"
        );
    }

    #[test]
    fn a_second_turn_without_a_load_repeats_the_message_and_a_turn_after_the_load_does_not() {
        let now = tools(&[("get_task", "12")]);
        let current = Current {
            fingerprint: "bbbb",
            tools: &now,
        };
        let old = record("aaaa", &[("get_task", "11")]);
        let note = reload_due(Some(&old), SESSION, current).unwrap();
        assert_eq!(
            prompt_turn(Some(&old), SESSION, &note, current),
            Turn::Quiet,
            "the first turn of fld claude is the note itself"
        );
        for prompt in ["what is next?", "and now?"] {
            assert_eq!(
                prompt_turn(Some(&old), SESSION, prompt, current),
                Turn::Remind(note.clone()),
                "{prompt}"
            );
        }
        assert_eq!(
            prompt_turn(Some(&old), SESSION, "/fold-main", current),
            Turn::Loads
        );
        assert_eq!(
            prompt_turn(Some(&old), SESSION, "/fold-mainly", current),
            Turn::Remind(note)
        );

        assert!(loads_main_skill(
            "Skill",
            &serde_json::json!({"skill": "fold-main"})
        ));
        let loaded = current.loaded_by(SESSION);
        assert_eq!(
            prompt_turn(Some(&loaded), SESSION, "what is next?", current),
            Turn::Quiet
        );
        assert_eq!(
            prompt_turn(Some(&loaded), SESSION, "/fold-main", current),
            Turn::Quiet
        );
    }

    #[test]
    fn only_a_skill_call_of_fold_main_is_a_load() {
        assert!(loads_main_skill(
            "Skill",
            &serde_json::json!({"skill": "/fold-main"})
        ));
        assert!(!loads_main_skill(
            "Skill",
            &serde_json::json!({"skill": "fold"})
        ));
        assert!(!loads_main_skill(
            "Read",
            &serde_json::json!({"skill": "fold-main"})
        ));
        assert!(!loads_main_skill("Skill", &serde_json::json!({})));
    }

    #[test]
    fn the_main_hooks_run_at_each_prompt_and_after_a_skill_call() {
        let hooks = main_hooks();
        assert_eq!(
            hooks[PROMPT_HOOK_EVENT][0]["hooks"][0]["command"],
            PROMPT_HOOK_COMMAND
        );
        assert_eq!(hooks[SKILL_HOOK_EVENT][0]["matcher"], "Skill");
        assert_eq!(
            hooks[SKILL_HOOK_EVENT][0]["hooks"][0]["command"],
            SKILL_HOOK_COMMAND
        );
    }
}
