use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use crate::error::{FoldError, codes};
use crate::events::EventActor;
use crate::text::one_line;
use crate::time::Timestamp;

pub mod claims;
pub mod container;
pub mod dag;
mod plan;
pub mod readiness;
pub mod requests;
pub mod status;

pub use plan::{
    MAX_PLAN_TASKS, PlanRef, PlannedChecklistItem, PlannedReview, PlannedTask, check_task_plan,
};

/// The messages never repeat the rejected text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TaskValueError {
    #[error("a task title is not empty and has no line break")]
    InvalidTitle,
    #[error("a label name is not empty and has no line break")]
    InvalidLabelName,
    #[error("a label colour is one of sage, slate, teal, ink, gold, moss, plum, stone")]
    InvalidLabelColor,
    #[error("a task status is one of backlog, todo, in_progress, review, done")]
    InvalidStatus,
    #[error("a checklist name is not empty and has no line break")]
    InvalidChecklistName,
    #[error("a checklist step is not empty and has no line break")]
    InvalidStepText,
    #[error("a task kind is proposal, no_proposal or human")]
    InvalidKind,
    #[error("a blocker reason is not empty and has at most 500 characters")]
    InvalidHoldReason,
}

impl TaskValueError {
    pub fn code(&self) -> &'static str {
        match self {
            TaskValueError::InvalidTitle => codes::INVALID_TASK_TITLE,
            TaskValueError::InvalidLabelName => codes::INVALID_LABEL_NAME,
            TaskValueError::InvalidLabelColor => codes::INVALID_LABEL_COLOR,
            TaskValueError::InvalidStatus => codes::INVALID_TASK_STATUS,
            TaskValueError::InvalidChecklistName => codes::INVALID_CHECKLIST_NAME,
            TaskValueError::InvalidStepText => codes::INVALID_STEP_TEXT,
            TaskValueError::InvalidKind => codes::INVALID_TASK_KIND,
            TaskValueError::InvalidHoldReason => codes::INVALID_HOLD_REASON,
        }
    }
}

impl From<TaskValueError> for FoldError {
    fn from(error: TaskValueError) -> Self {
        FoldError::new(error.code(), error.to_string())
    }
}

fn single_line(text: &str) -> Option<String> {
    one_line(text, usize::MAX).ok().map(str::to_owned)
}

/// A label name is a path segment of the label routes, and URL normalisation removes the segments `.` and `..` before a request is sent.
fn not_a_dot_segment(line: &str) -> bool {
    line != "." && line != ".."
}

macro_rules! define_line {
    ($(#[$meta:meta])* $name:ident, $error:expr $(, $accept:expr)?) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn parse(text: &str) -> Result<Self, TaskValueError> {
                single_line(text)
                    $(.filter(|line| $accept(line)))?
                    .map(Self)
                    .ok_or($error)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl FromStr for $name {
            type Err = TaskValueError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                Self::parse(text)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

define_line!(
    /// One trimmed line of text.
    TaskTitle,
    TaskValueError::InvalidTitle
);
define_line!(
    /// One trimmed line of text other than `.` and `..`; names are compared exactly, so `Bug` and `bug` are two labels.
    LabelName,
    TaskValueError::InvalidLabelName,
    not_a_dot_segment
);
define_line!(
    /// One trimmed line of text, unique within its task. A task's first checklist is [`ChecklistName::subtasks`].
    ChecklistName,
    TaskValueError::InvalidChecklistName
);
define_line!(
    /// The text of a plain checklist step: one trimmed line.
    StepText,
    TaskValueError::InvalidStepText
);

impl ChecklistName {
    /// The name of the checklist that an item added without a checklist name creates.
    pub fn subtasks() -> Self {
        Self("Subtasks".to_owned())
    }
}

/// The checklist of an item that names none, from the task's checklists in position order; `None` when the caller must name one. A plain step goes to the only checklist, or to `Subtasks` when there is none, so the add command and a plan place the same steps alike. A linked item goes to the first checklist, else `Subtasks`.
pub fn unnamed_item_checklist(checklists: &[ChecklistName], linked: bool) -> Option<ChecklistName> {
    match checklists {
        [] => Some(ChecklistName::subtasks()),
        [only] => Some(only.clone()),
        [first, ..] if linked => Some(first.clone()),
        _ => None,
    }
}

/// The longest hold reason, in characters.
pub const MAX_HOLD_REASON_CHARS: usize = 500;

/// Why a task is on hold: trimmed text of 1 to [`MAX_HOLD_REASON_CHARS`] characters, line breaks allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoldReason(String);

impl HoldReason {
    pub fn parse(text: &str) -> Result<Self, TaskValueError> {
        let text = text.trim();
        if text.is_empty() || text.chars().count() > MAX_HOLD_REASON_CHARS {
            return Err(TaskValueError::InvalidHoldReason);
        }
        Ok(Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HoldReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The hold mark of a task, which the board labels Blocker: while it is on, no agent claims the task and no lead starts a coder for it, until a person says the work can start. It is set by hand with a reason, unlike `tasks.blocked`, which recompute calculates from unfinished prerequisites. It never changes the task's status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoldMark {
    pub reason: HoldReason,
    pub set_by: EventActor,
    pub set_at: Timestamp,
}

/// Markdown text, kept exactly as given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskDescription(String);

impl TaskDescription {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A closed set of names: the enum, `ALL` in declaration order, `as_str`, `parse` and `Display`, all from one list, so a new variant cannot miss its text or its place in `ALL`. With `error = …` it also implements `FromStr` with that error.
macro_rules! define_names {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident { $($(#[$vmeta:meta])* $variant:ident = $text:literal),+ $(,)? }
        $(, error = $error:expr)? $(,)?
    ) => {
        $(#[$meta])*
        $vis enum $name {
            $($(#[$vmeta])* $variant),+
        }

        impl $name {
            pub const ALL: [$name; [$($text),+].len()] = [$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }

            pub fn parse(text: &str) -> Option<$name> {
                match text {
                    $($text => Some($name::$variant),)+
                    _ => None,
                }
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        $(
            impl ::std::str::FromStr for $name {
                type Err = $crate::tasks::TaskValueError;

                fn from_str(text: &str) -> Result<Self, Self::Err> {
                    $name::parse(text).ok_or($error)
                }
            }
        )?
    };
}

pub(crate) use define_names;

define_names!(
    /// The closed set of label colours. The web tokens `label-<name>` and `label-<name>-soft` use the same names.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum LabelColor {
        Sage = "sage",
        Slate = "slate",
        Teal = "teal",
        Ink = "ink",
        Gold = "gold",
        Moss = "moss",
        Plum = "plum",
        Stone = "stone",
    },
    error = TaskValueError::InvalidLabelColor
);

define_names!(
    /// How a task's work reaches Done. A `proposal` task is done when its proposal integrates; a `no_proposal` task changes no project file, such as design work done through an MCP server, so its coder reports the work and a lead check completes it; a `human` task is work only a person can do outside every agent, such as verifying an account with a provider, so no agent takes it and a person marks it Done.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum TaskKind {
        Proposal = "proposal",
        NoProposal = "no_proposal",
        Human = "human",
    },
    error = TaskValueError::InvalidKind
);

define_names!(
    /// How an edge came to exist. A checklist edge belongs to a linked checklist item and goes away only with that item.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum DependencyOrigin {
        Direct = "direct",
        Checklist = "checklist",
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::status::TaskStatus;

    #[test]
    fn titles_and_names_are_trimmed_single_lines() {
        assert_eq!(
            TaskTitle::parse("  Build it \t").unwrap().as_str(),
            "Build it"
        );
        assert_eq!(LabelName::parse(" bug ").unwrap().to_string(), "bug");
        for bad in ["", "   ", "\n", "a\nb", "a\r\nb", "a\rb"] {
            assert_eq!(TaskTitle::parse(bad), Err(TaskValueError::InvalidTitle));
            assert_eq!(LabelName::parse(bad), Err(TaskValueError::InvalidLabelName));
            assert_eq!(
                ChecklistName::parse(bad),
                Err(TaskValueError::InvalidChecklistName)
            );
            assert_eq!(StepText::parse(bad), Err(TaskValueError::InvalidStepText));
        }
        assert_eq!(ChecklistName::parse(" QA ").unwrap().to_string(), "QA");
        assert_eq!(StepText::parse(" Wire it ").unwrap().as_str(), "Wire it");
        assert_eq!(ChecklistName::subtasks().as_str(), "Subtasks");
        assert_eq!(
            "Subtasks".parse::<ChecklistName>(),
            Ok(ChecklistName::subtasks())
        );
        assert_eq!(
            "a\nb".parse::<LabelName>(),
            Err(TaskValueError::InvalidLabelName)
        );
    }

    #[test]
    fn an_unnamed_plain_step_needs_a_name_only_when_the_task_has_several_checklists() {
        let qa = ChecklistName::parse("QA").unwrap();
        let later = ChecklistName::parse("Later").unwrap();
        let both = [qa.clone(), later];
        for linked in [false, true] {
            assert_eq!(
                unnamed_item_checklist(&[], linked),
                Some(ChecklistName::subtasks())
            );
            assert_eq!(
                unnamed_item_checklist(std::slice::from_ref(&qa), linked),
                Some(qa.clone())
            );
        }
        assert_eq!(unnamed_item_checklist(&both, false), None);
        assert_eq!(unnamed_item_checklist(&both, true), Some(qa));
    }

    #[test]
    fn a_label_name_is_never_a_dot_segment() {
        for bad in [".", "..", " .. "] {
            assert_eq!(LabelName::parse(bad), Err(TaskValueError::InvalidLabelName));
        }
        for good in ["...", ".x", "a.b", "./a"] {
            assert_eq!(LabelName::parse(good).unwrap().as_str(), good);
        }
        assert_eq!(TaskTitle::parse("..").unwrap().as_str(), "..");
        assert_eq!(ChecklistName::parse(".").unwrap().as_str(), ".");
    }

    #[test]
    fn label_names_keep_their_case() {
        assert_ne!(LabelName::parse("Bug"), LabelName::parse("bug"));
    }

    #[test]
    fn descriptions_pass_through_unchanged() {
        let text = "  # Title\n\n- item  \n";
        assert_eq!(TaskDescription::new(text).as_str(), text);
        assert_eq!(TaskDescription::default().as_str(), "");
    }

    #[test]
    fn every_colour_round_trips_and_others_are_refused() {
        assert_eq!(LabelColor::ALL.len(), 8);
        for color in LabelColor::ALL {
            assert_eq!(color.as_str().parse::<LabelColor>(), Ok(color));
            assert_eq!(color.to_string(), color.as_str());
        }
        for other in ["green", "red", "black-ish", "Sage", "", "lime"] {
            assert_eq!(
                other.parse::<LabelColor>(),
                Err(TaskValueError::InvalidLabelColor)
            );
        }
    }

    #[test]
    fn task_kinds_round_trip() {
        for kind in TaskKind::ALL {
            assert_eq!(kind.as_str().parse::<TaskKind>(), Ok(kind));
            assert_eq!(kind.to_string(), kind.as_str());
        }
        assert_eq!(TaskKind::NoProposal.as_str(), "no_proposal");
        assert_eq!(TaskKind::Human.as_str(), "human");
        assert_eq!(TaskKind::ALL.len(), 3);
        assert_eq!(
            "design".parse::<TaskKind>(),
            Err(TaskValueError::InvalidKind)
        );
    }

    #[test]
    fn hold_reasons_are_trimmed_and_bounded() {
        assert_eq!(
            HoldReason::parse("  Wait for the Meta review \n")
                .unwrap()
                .as_str(),
            "Wait for the Meta review"
        );
        assert_eq!(
            HoldReason::parse("line one\nline two").unwrap().to_string(),
            "line one\nline two"
        );
        let longest = "a".repeat(MAX_HOLD_REASON_CHARS);
        assert_eq!(HoldReason::parse(&longest).unwrap().as_str(), longest);
        for bad in [
            String::new(),
            " \n ".to_owned(),
            "a".repeat(MAX_HOLD_REASON_CHARS + 1),
        ] {
            assert_eq!(
                HoldReason::parse(&bad),
                Err(TaskValueError::InvalidHoldReason)
            );
        }
    }

    #[test]
    fn dependency_origins_round_trip() {
        for origin in DependencyOrigin::ALL {
            assert_eq!(DependencyOrigin::parse(origin.as_str()), Some(origin));
            assert_eq!(origin.to_string(), origin.as_str());
        }
        assert_eq!(DependencyOrigin::Checklist.as_str(), "checklist");
        assert_eq!(DependencyOrigin::parse("Direct"), None);
        assert_eq!(DependencyOrigin::parse(""), None);
    }

    #[test]
    fn statuses_round_trip() {
        assert_eq!(TaskStatus::ALL.len(), 5);
        for status in TaskStatus::ALL {
            assert_eq!(status.as_str().parse::<TaskStatus>(), Ok(status));
            assert_eq!(status.to_string(), status.as_str());
        }
        assert_eq!(TaskStatus::InProgress.as_str(), "in_progress");
        assert_eq!(
            "blocked".parse::<TaskStatus>(),
            Err(TaskValueError::InvalidStatus)
        );
    }

    #[test]
    fn errors_carry_stable_codes() {
        let cases = [
            (TaskValueError::InvalidTitle, "invalid_task_title"),
            (TaskValueError::InvalidLabelName, "invalid_label_name"),
            (TaskValueError::InvalidLabelColor, "invalid_label_color"),
            (TaskValueError::InvalidStatus, "invalid_task_status"),
            (
                TaskValueError::InvalidChecklistName,
                "invalid_checklist_name",
            ),
            (TaskValueError::InvalidStepText, "invalid_step_text"),
            (TaskValueError::InvalidKind, "invalid_task_kind"),
            (TaskValueError::InvalidHoldReason, "invalid_hold_reason"),
        ];
        for (error, code) in cases {
            assert_eq!(error.code(), code);
            assert_eq!(FoldError::from(error).code(), code);
        }
    }
}
