use std::fmt;

use serde_json::json;

use crate::error::{FoldError, codes};
use crate::ids::{ProposalId, RequestId, TaskId, WorkspaceId};
use crate::text::{LineFault, one_line};

/// The longest note title, in characters.
pub const MAX_NOTE_TITLE: usize = 200;

/// The longest decision or reason of a note, in characters.
pub const MAX_NOTE_TEXT: usize = 4000;

/// The most links one note carries.
pub const MAX_NOTE_LINKS: usize = 20;

/// The longest file path a link names, in characters.
pub const MAX_LINK_PATH: usize = 512;

/// What a note links to, in fold's mention forms: `#T-12`, `#P-3`, `#RQ-2`, `@web/src/board.ts` or `@ws-9:web/src/board.ts`. A link is checked for its form only, as a mention is: the item or file it names may change or go away later, and the note keeps what its author wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteLink {
    Task(TaskId),
    Proposal(ProposalId),
    Request(RequestId),
    /// A file or, with a trailing `/`, a folder of main, or of a workspace when one is named.
    File {
        workspace: Option<WorkspaceId>,
        path: String,
    },
}

impl NoteLink {
    pub fn parse(text: &str) -> Result<NoteLink, FoldError> {
        let text = text.trim();
        if let Some(id) = text.strip_prefix('#') {
            if let Ok(task) = id.parse::<TaskId>() {
                return Ok(NoteLink::Task(task));
            }
            if let Ok(proposal) = id.parse::<ProposalId>() {
                return Ok(NoteLink::Proposal(proposal));
            }
            if let Ok(request) = id.parse::<RequestId>() {
                return Ok(NoteLink::Request(request));
            }
            return Err(invalid_link());
        }
        let Some(rest) = text.strip_prefix('@') else {
            return Err(invalid_link());
        };
        let (workspace, path) = match rest.split_once(':') {
            Some((workspace, path)) => (
                Some(
                    workspace
                        .parse::<WorkspaceId>()
                        .map_err(|_| invalid_link())?,
                ),
                path,
            ),
            None => (None, rest),
        };
        if !is_link_path(path) {
            return Err(invalid_link());
        }
        Ok(NoteLink::File {
            workspace,
            path: path.to_owned(),
        })
    }
}

impl fmt::Display for NoteLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NoteLink::Task(task) => write!(f, "#{task}"),
            NoteLink::Proposal(proposal) => write!(f, "#{proposal}"),
            NoteLink::Request(request) => write!(f, "#{request}"),
            NoteLink::File {
                workspace: Some(workspace),
                path,
            } => write!(f, "@{workspace}:{path}"),
            NoteLink::File {
                workspace: None,
                path,
            } => write!(f, "@{path}"),
        }
    }
}

/// A relative, normalised path of a file, or of a folder with a trailing `/`; `./` may lead it, as in the mention of a file that has a person's name.
fn is_link_path(path: &str) -> bool {
    let path = path.strip_prefix("./").unwrap_or(path);
    let path = path.strip_suffix('/').unwrap_or(path);
    !path.is_empty()
        && path.chars().count() <= MAX_LINK_PATH
        && !path
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
        && path
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

fn invalid_link() -> FoldError {
    FoldError::new(
        codes::INVALID_REQUEST,
        "a link is a task, proposal or request such as #T-12, or a file such as @web/src/board.ts or @ws-9:web/src/board.ts",
    )
    .with_details([("field", json!("links"))])
    .with_hint("write each link in fold's mention form, with no space after the mark")
}

/// A decision note as its author writes it: a one-line title, the decision and its reason, and links to what it concerns.
#[derive(Clone, PartialEq, Eq)]
pub struct NoteDraft {
    title: String,
    decision: String,
    reason: String,
    links: Vec<NoteLink>,
}

impl fmt::Debug for NoteDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NoteDraft")
            .field("title_len", &self.title.len())
            .field("links", &self.links.len())
            .finish_non_exhaustive()
    }
}

impl NoteDraft {
    /// Refuses an empty or multi-line title, an empty decision or reason, text over its limit and a link in any other form than [`NoteLink`] takes, each with `invalid_request` and `details.field`.
    pub fn parse(
        title: &str,
        decision: &str,
        reason: &str,
        links: &[String],
    ) -> Result<NoteDraft, FoldError> {
        let title = one_line(title, MAX_NOTE_TITLE).map_err(|fault| {
            let message = match fault {
                LineFault::Empty => "a note needs a title".to_owned(),
                LineFault::NotOneLine => "a note title is one line".to_owned(),
                LineFault::TooLong => {
                    format!("a note title holds at most {MAX_NOTE_TITLE} characters")
                }
            };
            invalid_field("title", message)
        })?;
        let decision = text_field("decision", decision)?;
        let reason = text_field("reason", reason)?;
        if links.len() > MAX_NOTE_LINKS {
            return Err(invalid_field(
                "links",
                format!("a note holds at most {MAX_NOTE_LINKS} links"),
            ));
        }
        let links = links
            .iter()
            .map(|link| NoteLink::parse(link))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(NoteDraft {
            title: title.to_owned(),
            decision,
            reason,
            links,
        })
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn decision(&self) -> &str {
        &self.decision
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    pub fn links(&self) -> &[NoteLink] {
        &self.links
    }
}

fn text_field(field: &'static str, text: &str) -> Result<String, FoldError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(invalid_field(field, format!("a note needs a {field}")));
    }
    if text.chars().count() > MAX_NOTE_TEXT {
        return Err(invalid_field(
            field,
            format!("a note's {field} holds at most {MAX_NOTE_TEXT} characters"),
        ));
    }
    Ok(text.to_owned())
}

fn invalid_field(field: &'static str, message: String) -> FoldError {
    FoldError::new(codes::INVALID_REQUEST, message)
        .with_details([("field", json!(field))])
        .with_hint("write a short title, the decision and the reason in plain words")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(error: &FoldError) -> &serde_json::Value {
        &error.details().unwrap()["field"]
    }

    #[test]
    fn a_note_takes_a_title_a_decision_a_reason_and_links() {
        let links: Vec<String> = [
            "#T-12",
            " #P-3 ",
            "#RQ-2",
            "@web/src/board.ts",
            "@ws-9:web/src/",
            "@./tanim",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let draft = NoteDraft::parse(
            " Sort by due date ",
            " Cards sort by due date. ",
            "The board reads that way.",
            &links,
        )
        .unwrap();
        assert_eq!(draft.title(), "Sort by due date");
        assert_eq!(draft.decision(), "Cards sort by due date.");
        assert_eq!(draft.reason(), "The board reads that way.");
        let written: Vec<String> = draft.links().iter().map(ToString::to_string).collect();
        assert_eq!(
            written,
            [
                "#T-12",
                "#P-3",
                "#RQ-2",
                "@web/src/board.ts",
                "@ws-9:web/src/",
                "@./tanim"
            ]
        );
        assert!(!format!("{draft:?}").contains("due date"));
    }

    #[test]
    fn a_note_refuses_empty_long_or_malformed_parts() {
        let ok = |title: &str, decision: &str, reason: &str, links: &[&str]| {
            let links: Vec<String> = links.iter().map(|link| (*link).to_owned()).collect();
            NoteDraft::parse(title, decision, reason, &links)
        };
        let cases = [
            (ok(" ", "d", "r", &[]).unwrap_err(), "title"),
            (ok("a\nb", "d", "r", &[]).unwrap_err(), "title"),
            (
                ok(&"t".repeat(MAX_NOTE_TITLE + 1), "d", "r", &[]).unwrap_err(),
                "title",
            ),
            (ok("t", "", "r", &[]).unwrap_err(), "decision"),
            (ok("t", "d", " \n", &[]).unwrap_err(), "reason"),
            (
                ok("t", &"d".repeat(MAX_NOTE_TEXT + 1), "r", &[]).unwrap_err(),
                "decision",
            ),
            (ok("t", "d", "r", &["T-12"]).unwrap_err(), "links"),
            (ok("t", "d", "r", &["#m-4"]).unwrap_err(), "links"),
            (ok("t", "d", "r", &["@/etc/passwd"]).unwrap_err(), "links"),
            (ok("t", "d", "r", &["@a/../b"]).unwrap_err(), "links"),
            (ok("t", "d", "r", &["@a//b"]).unwrap_err(), "links"),
            (ok("t", "d", "r", &["@a b"]).unwrap_err(), "links"),
            (ok("t", "d", "r", &["@ws-x:a"]).unwrap_err(), "links"),
            (ok("t", "d", "r", &["@"]).unwrap_err(), "links"),
            (
                ok(
                    "t",
                    "d",
                    "r",
                    &[&format!("@{}", "a".repeat(MAX_LINK_PATH + 1))],
                )
                .unwrap_err(),
                "links",
            ),
            (
                ok("t", "d", "r", &["#T-1"; MAX_NOTE_LINKS + 1]).unwrap_err(),
                "links",
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(error.code(), codes::INVALID_REQUEST);
            assert_eq!(field(&error), expected, "{error:?}");
        }
        assert!(ok("t", "d", "r", &["#T-1"; MAX_NOTE_LINKS]).is_ok());
    }
}
