use serde_json::json;

use super::{Choice, MAX_QUESTION_HEADER_LEN, MessageChoices, chosen_answer, short_line};
use crate::error::{FoldError, codes};
use crate::text::shorten;

/// The most questions one set asks. Claude Code's `AskUserQuestion` asks at most 4 in one call.
pub const MAX_SET_QUESTIONS: usize = 4;

/// The longest question of a set, in characters.
pub const MAX_QUESTION_LEN: usize = 500;

/// The longest first line of an ask that a compact listing shows, in characters.
pub const FIRST_LINE_LEN: usize = 120;

/// One question of a set: its text, an optional short header and the options a human may pick. A human may always answer with free text, and a question without options takes free text only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetQuestion {
    pub text: String,
    pub header: Option<String>,
    pub choices: Vec<Choice>,
}

/// The questions an agent asks a human in one message: 1 to 4, which the human answers together in one reply, so the agent gets everything it needs from one wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionSet {
    questions: Vec<SetQuestion>,
}

fn invalid_set(message: &str) -> FoldError {
    FoldError::new(codes::INVALID_REQUEST, message)
        .with_details([("reason", json!("questions"))])
        .with_hint(format!(
            "pass 1 to {MAX_SET_QUESTIONS} questions, each a text of at most {MAX_QUESTION_LEN} characters with no choices or 2 to 6 short choices"
        ))
}

impl QuestionSet {
    /// Trims each text and header; an empty header counts as none. A set of no question or of more than 4, an empty or too long text, or the choices of one question that [`MessageChoices::parse`] refuses is `invalid_request`: `details.reason` is `questions`, or `choices` for the choices.
    pub fn parse(questions: &[SetQuestion]) -> Result<QuestionSet, FoldError> {
        if !(1..=MAX_SET_QUESTIONS).contains(&questions.len()) {
            return Err(invalid_set("a question set has 1 to 4 questions"));
        }
        let mut kept = Vec::with_capacity(questions.len());
        for question in questions {
            let text = question.text.trim();
            if text.is_empty() {
                return Err(invalid_set("a question has text"));
            }
            if text.chars().count() > MAX_QUESTION_LEN {
                return Err(invalid_set("a question is at most 500 characters"));
            }
            let (header, choices) = if question.choices.is_empty() {
                let header = match question.header.as_deref().map(str::trim) {
                    None | Some("") => None,
                    Some(header) => {
                        Some(short_line(header, MAX_QUESTION_HEADER_LEN).ok_or_else(|| {
                            invalid_set("a question's header is one line of at most 40 characters")
                        })?)
                    }
                };
                (header, Vec::new())
            } else {
                let parsed = MessageChoices::parse(&question.choices, question.header.as_deref())?;
                (
                    parsed.header().map(str::to_owned),
                    parsed.options().to_vec(),
                )
            };
            kept.push(SetQuestion {
                text: text.to_owned(),
                header,
                choices,
            });
        }
        Ok(QuestionSet { questions: kept })
    }

    pub fn questions(&self) -> &[SetQuestion] {
        &self.questions
    }
}

/// What a stored message holds that makes it an ask, as [`ask_questions`] reads it.
#[derive(Debug, Clone, Copy)]
pub struct AskParts<'a> {
    /// The questions of a set, in order; empty for any other message.
    pub stored: &'a [SetQuestion],
    pub body: &'a str,
    /// The options of a single question with choices.
    pub choices: &'a [Choice],
    pub header: Option<&'a str>,
    /// The message is an ask without questions or choices: marked `asks_human`, or a worker's `AskUserQuestion` whose options fold could not keep.
    pub marked: bool,
}

/// The questions of an ask, as a set: the stored questions of a set; else the set of one that a single question with choices or a marked ask is, with the body as its question; empty for a message that asks nothing.
pub fn ask_questions(parts: AskParts<'_>) -> Vec<SetQuestion> {
    if !parts.stored.is_empty() {
        return parts.stored.to_vec();
    }
    if parts.choices.is_empty() && !parts.marked {
        return Vec::new();
    }
    vec![SetQuestion {
        text: parts.body.trim().to_owned(),
        header: parts.header.map(str::to_owned),
        choices: parts.choices.to_vec(),
    }]
}

/// The answers a human gives to the set `questions`: one per question in order, each trimmed and not empty. An answer that names one of its question's choices, without case and surrounding space, takes the choice's own label. A missing, extra or empty answer is `invalid_request` with `details.reason = "answers"`.
pub fn set_answers(
    questions: &[SetQuestion],
    answers: &[String],
) -> Result<Vec<String>, FoldError> {
    let invalid = |message: &str| {
        FoldError::new(codes::INVALID_REQUEST, message)
            .with_details([
                ("reason", json!("answers")),
                ("questions", json!(questions.len())),
            ])
            .with_hint("give one answer per question, in the order of the questions")
    };
    if questions.is_empty() {
        return Err(invalid("the message asks no question"));
    }
    if answers.len() != questions.len() {
        return Err(invalid("the answers do not match the questions one to one"));
    }
    questions
        .iter()
        .zip(answers)
        .map(|(question, answer)| {
            if answer.trim().is_empty() {
                return Err(invalid("an answer has no text"));
            }
            let labels: Vec<String> = question
                .choices
                .iter()
                .map(|choice| choice.label.clone())
                .collect();
            Ok(chosen_answer(&labels, answer))
        })
        .collect()
}

/// The text of the one reply that answers a set: the answer alone for a set of one, as a reply that names a choice always was; else each question with its number and, on the next line, its answer, with a blank line between the questions.
pub fn answers_body(questions: &[SetQuestion], answers: &[String]) -> String {
    if let ([_], [answer]) = (questions, answers) {
        return answer.clone();
    }
    questions
        .iter()
        .zip(answers)
        .enumerate()
        .map(|(index, (question, answer))| format!("{}. {}\n{answer}", index + 1, question.text))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The first line of a message's text that has any, cut to [`FIRST_LINE_LEN`] characters, for a compact listing of asks.
pub fn first_line(body: &str) -> String {
    let line = body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    shorten(line, FIRST_LINE_LEN)
}
