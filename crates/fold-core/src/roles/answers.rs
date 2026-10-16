use serde_json::Value;

/// A human's answer to a worker's permission question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionBehavior {
    Allow,
    Deny,
}

impl PermissionBehavior {
    pub fn as_str(self) -> &'static str {
        match self {
            PermissionBehavior::Allow => "allow",
            PermissionBehavior::Deny => "deny",
        }
    }

    pub fn parse(text: &str) -> Option<PermissionBehavior> {
        match text {
            "allow" => Some(PermissionBehavior::Allow),
            "deny" => Some(PermissionBehavior::Deny),
            _ => None,
        }
    }
}

/// The first user turn of a worker that fold resumed after a restart with a permission question answered: Claude Code drops an open `can_use_tool` request when its process ends and the resumed model sees the tool call as interrupted, so the answer arrives as text. `message` is the human's note on the answer.
pub fn permission_answer_turn(
    tool: &str,
    input: &Value,
    allowed: bool,
    message: Option<&str>,
) -> String {
    let asked = format!("Before fold restarted you asked to use {tool} with this input: {input}.");
    let decision = if allowed {
        "The human allowed it. Run it again now."
    } else {
        "The human denied it. Do not run it; continue without it."
    };
    match message.map(str::trim).filter(|text| !text.is_empty()) {
        Some(note) => format!("{asked} {decision} The human's note: {note}"),
        None => format!("{asked} {decision}"),
    }
}

/// The first user turn of a worker that fold resumed after a restart with its `AskUserQuestion` call answered. `answers` holds each question with the human's answer, in the call's order.
pub fn ask_answer_turn(answers: &[(String, String)]) -> String {
    let lines: Vec<String> = answers
        .iter()
        .map(|(question, answer)| format!("\"{question}\": \"{answer}\""))
        .collect();
    format!(
        "Before fold restarted you asked the human a question. The human answered: {}. Continue with these answers in mind.",
        lines.join("; ")
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn behaviors_round_trip_their_names() {
        for behavior in [PermissionBehavior::Allow, PermissionBehavior::Deny] {
            assert_eq!(PermissionBehavior::parse(behavior.as_str()), Some(behavior));
        }
        assert_eq!(PermissionBehavior::parse("maybe"), None);
    }

    #[test]
    fn a_permission_answer_names_the_call_and_the_decision() {
        let input = json!({"command": "make deploy"});
        let allowed = permission_answer_turn("Bash", &input, true, None);
        assert_eq!(
            allowed,
            r#"Before fold restarted you asked to use Bash with this input: {"command":"make deploy"}. The human allowed it. Run it again now."#
        );
        let denied = permission_answer_turn("Bash", &input, false, Some(" use staging "));
        assert!(
            denied.ends_with("Do not run it; continue without it. The human's note: use staging")
        );
        let blank = permission_answer_turn("Bash", &input, false, Some("  "));
        assert!(blank.ends_with("continue without it."));
    }

    #[test]
    fn an_ask_answer_lists_each_question_and_answer() {
        let turn = ask_answer_turn(&[
            ("Which colour?".to_owned(), "Red".to_owned()),
            ("Which size?".to_owned(), "large".to_owned()),
        ]);
        assert_eq!(
            turn,
            r#"Before fold restarted you asked the human a question. The human answered: "Which colour?": "Red"; "Which size?": "large". Continue with these answers in mind."#
        );
    }
}
