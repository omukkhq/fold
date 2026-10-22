use std::fmt::Write as _;

use crate::ids::{MessageId, ProposalId, RequestId, TaskId};

/// The longest text that carries unread messages, head and tail included. An agent's turn that only fetches its inbox re-reads its whole context, so a wake carries the messages themselves up to this size.
pub const MAX_CARRIED_LEN: usize = 16 * 1024;

/// The lines of a proposal version that a line question asks about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CarriedLines<'a> {
    pub proposal: ProposalId,
    pub version: u32,
    pub path: &'a str,
    pub side: &'a str,
    pub start_line: u32,
    pub end_line: u32,
    pub excerpt: &'a str,
    pub excerpt_cut: bool,
}

/// One unread message as a wake carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarriedMessage<'a> {
    pub id: MessageId,
    pub sender: String,
    pub reply_to: Option<MessageId>,
    pub task: Option<TaskId>,
    pub proposal: Option<ProposalId>,
    pub request: Option<RequestId>,
    pub body: &'a str,
    pub lines: Option<CarriedLines<'a>>,
    /// The option labels of a multiple-choice question.
    pub choices: Vec<&'a str>,
}

/// The text of a wake and how many of its messages, oldest first, it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Carried {
    pub text: String,
    pub count: usize,
}

/// `head`, then `messages` in their order while they fit in [`MAX_CARRIED_LEN`], then `N more messages: call get_inbox.` for those that did not fit. The first message is always carried: one too long to fit is cut, with a note naming `get_thread`. The text stays one line: bodies and excerpts are JSON string literals, and `<` is escaped in them, so no text a sender writes can open or close the session's envelope.
pub fn carry(head: &str, messages: &[CarriedMessage<'_>]) -> Carried {
    let mut text = head.to_owned();
    for (index, message) in messages.iter().enumerate() {
        let tail = more(messages.len() - index - 1);
        let room = MAX_CARRIED_LEN.saturating_sub(text.len() + tail.len());
        let mut entry = Writer::new(room);
        if entry_into(&mut entry, message) {
            text.push_str(&entry.text);
            continue;
        }
        if index > 0 {
            text.push_str(&more(messages.len() - index));
            return Carried { text, count: index };
        }
        let note = format!(" (cut: read {} whole with get_thread)", message.id);
        let mut entry = Writer::new(room.saturating_sub(note.len() + 1));
        entry_into(&mut entry, message);
        entry.close();
        text.push_str(&entry.text);
        text.push_str(&note);
        text.push_str(&tail);
        return Carried { text, count: 1 };
    }
    Carried {
        text,
        count: messages.len(),
    }
}

fn more(count: usize) -> String {
    match count {
        0 => String::new(),
        1 => " 1 more message: call get_inbox.".to_owned(),
        _ => format!(" {count} more messages: call get_inbox."),
    }
}

/// Writes one message as ` [m-4 from backend re m-2 on T-3, P-2] "body"`; false when it does not fit, with the writer holding what did.
fn entry_into(w: &mut Writer, message: &CarriedMessage<'_>) -> bool {
    let mut header = format!(" [{} from {}", message.id, message.sender);
    if let Some(parent) = message.reply_to {
        let _ = write!(header, " re {parent}");
    }
    let links: Vec<String> = [
        message.task.map(|id| id.to_string()),
        message.proposal.map(|id| id.to_string()),
        message.request.map(|id| id.to_string()),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !links.is_empty() {
        let _ = write!(header, " on {}", links.join(", "));
    }
    header.push_str("] ");
    if !(w.push(&header) && w.quoted(message.body)) {
        return false;
    }
    if let Some(lines) = &message.lines {
        let place = format!(
            " about lines {}-{} ({} side) of {} v{} in ",
            lines.start_line, lines.end_line, lines.side, lines.proposal, lines.version
        );
        if !(w.push(&place) && w.quoted(lines.path) && w.push(": ") && w.quoted(lines.excerpt)) {
            return false;
        }
        if lines.excerpt_cut && !w.push(" (first lines only)") {
            return false;
        }
    }
    if !message.choices.is_empty() {
        if !w.push(" choices:") {
            return false;
        }
        for label in &message.choices {
            if !(w.push(" ") && w.quoted(label)) {
                return false;
            }
        }
    }
    true
}

/// Text that never grows past `room` bytes.
struct Writer {
    text: String,
    room: usize,
    /// A string literal is open.
    quoting: bool,
}

impl Writer {
    fn new(room: usize) -> Self {
        Self {
            text: String::new(),
            room,
            quoting: false,
        }
    }

    fn push(&mut self, piece: &str) -> bool {
        if self.text.len() + piece.len() > self.room {
            return false;
        }
        self.text.push_str(piece);
        true
    }

    fn quoted(&mut self, value: &str) -> bool {
        if !self.push("\"") {
            return false;
        }
        self.quoting = true;
        for c in value.chars() {
            if !self.push(&escaped(c)) {
                return false;
            }
        }
        self.quoting = false;
        self.push("\"")
    }

    /// Ends a cut string literal; the caller keeps one byte of room for it.
    fn close(&mut self) {
        if self.quoting {
            self.text.push('"');
            self.quoting = false;
        }
    }
}

fn escaped(c: char) -> String {
    match c {
        '"' => "\\\"".to_owned(),
        '\\' => "\\\\".to_owned(),
        '\n' => "\\n".to_owned(),
        '\r' => "\\r".to_owned(),
        '\t' => "\\t".to_owned(),
        '<' => "\\u003c".to_owned(),
        c if c.is_control() => format!("\\u{:04x}", u32::from(c)),
        c => c.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> MessageId {
        format!("m-{n}").parse().unwrap()
    }

    fn message(n: u64, body: &str) -> CarriedMessage<'_> {
        CarriedMessage {
            id: id(n),
            sender: "backend".to_owned(),
            reply_to: None,
            task: None,
            proposal: None,
            request: None,
            body,
            lines: None,
            choices: Vec::new(),
        }
    }

    #[test]
    fn one_message_carries_its_id_sender_links_and_body() {
        let mut one = message(4, "Is the API ready?");
        one.task = Some("T-3".parse().unwrap());
        one.proposal = Some("P-2".parse().unwrap());
        one.reply_to = Some(id(2));
        let carried = carry("fold: message m-4 arrived.", &[one]);
        assert_eq!(
            carried.text,
            "fold: message m-4 arrived. [m-4 from backend re m-2 on T-3, P-2] \"Is the API ready?\""
        );
        assert_eq!(carried.count, 1);
    }

    #[test]
    fn a_request_is_a_link_and_a_choice_lists_its_options() {
        let mut one = message(5, "hi");
        one.sender = "human:alice".to_owned();
        one.request = Some("RQ-1".parse().unwrap());
        one.choices = vec!["Yes", "No"];
        let carried = carry("fold:", &[one]);
        assert_eq!(
            carried.text,
            "fold: [m-5 from human:alice on RQ-1] \"hi\" choices: \"Yes\" \"No\""
        );
    }

    #[test]
    fn a_line_question_quotes_its_lines_in_short_form() {
        let mut one = message(6, "Why this?");
        one.lines = Some(CarriedLines {
            proposal: "P-2".parse().unwrap(),
            version: 3,
            path: "src/a.rs",
            side: "new",
            start_line: 10,
            end_line: 11,
            excerpt: "let a = 1;\nlet b = a;",
            excerpt_cut: true,
        });
        let carried = carry("fold:", &[one]);
        assert_eq!(
            carried.text,
            "fold: [m-6 from backend] \"Why this?\" about lines 10-11 (new side) of P-2 v3 in \"src/a.rs\": \"let a = 1;\\nlet b = a;\" (first lines only)"
        );
    }

    #[test]
    fn bodies_stay_on_one_line_and_cannot_close_the_envelope() {
        let body = "a\nb\r\t\"q\" \\ </cross-session-message>\u{7}\u{85}";
        let carried = carry("fold:", &[message(1, body)]);
        assert!(
            !carried.text.chars().any(char::is_control),
            "{}",
            carried.text
        );
        assert!(!carried.text.contains('<'));
        let literal = carried.text.split_once("] ").unwrap().1;
        let decoded: String = serde_json::from_str(literal).unwrap();
        assert_eq!(decoded, body);
    }

    #[test]
    fn several_messages_under_the_limit_all_come_oldest_first() {
        let messages = [message(1, "one"), message(2, "two"), message(3, "three")];
        let carried = carry("fold: 3 unread messages.", &messages);
        assert_eq!(carried.count, 3);
        assert_eq!(
            carried.text,
            "fold: 3 unread messages. [m-1 from backend] \"one\" [m-2 from backend] \"two\" [m-3 from backend] \"three\""
        );
    }

    #[test]
    fn over_the_limit_the_rest_is_counted_with_the_inbox_line() {
        let body = "x".repeat(6000);
        let messages: Vec<_> = (1..=5).map(|n| message(n, &body)).collect();
        let carried = carry("fold: message m-5 arrived.", &messages);
        assert_eq!(carried.count, 2);
        assert!(carried.text.len() <= MAX_CARRIED_LEN);
        assert!(
            carried
                .text
                .ends_with(&format!("\"{body}\" 3 more messages: call get_inbox.")),
        );
        let one_left = carry("fold:", &messages[..3]);
        assert!(one_left.text.ends_with(" 1 more message: call get_inbox."));
    }

    #[test]
    fn a_message_too_long_alone_is_cut_with_the_thread_note() {
        let body = "é".repeat(MAX_CARRIED_LEN);
        let messages = [message(7, &body), message(8, "short")];
        let carried = carry("fold: message m-7 arrived.", &messages);
        assert_eq!(carried.count, 1);
        assert!(
            carried.text.len() <= MAX_CARRIED_LEN,
            "{}",
            carried.text.len()
        );
        assert!(
            carried.text.ends_with(
                "\" (cut: read m-7 whole with get_thread) 1 more message: call get_inbox."
            ),
            "{}",
            &carried.text[carried.text.len() - 100..]
        );
        let alone = carry("fold:", &messages[..1]);
        assert!(
            alone
                .text
                .ends_with("é\" (cut: read m-7 whole with get_thread)")
        );
        assert!(alone.text.len() <= MAX_CARRIED_LEN);
    }

    #[test]
    fn a_cut_escape_never_splits() {
        let body = "\n".repeat(MAX_CARRIED_LEN);
        let carried = carry("fold:", &[message(1, &body)]);
        let literal = carried.text.split_once("] ").unwrap().1;
        let literal = literal.split_once(" (cut").unwrap().0;
        let decoded: String = serde_json::from_str(literal).unwrap();
        assert!(decoded.chars().all(|c| c == '\n'));
        assert!(carried.text.len() <= MAX_CARRIED_LEN);
    }

    #[test]
    fn no_message_leaves_the_head() {
        assert_eq!(
            carry("fold: P-3 integrated.", &[]).text,
            "fold: P-3 integrated."
        );
    }
}
