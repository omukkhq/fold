use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use crate::error::{FoldError, codes};

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
        }
    }
}

impl From<TaskValueError> for FoldError {
    fn from(error: TaskValueError) -> Self {
        FoldError::new(error.code(), error.to_string())
    }
}

fn single_line(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty() && !trimmed.contains(['\n', '\r'])).then(|| trimmed.to_owned())
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

macro_rules! define_names {
    ($(#[$meta:meta])* $name:ident, $error:expr, { $($variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }
        }

        impl FromStr for $name {
            type Err = TaskValueError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                match text {
                    $($text => Ok($name::$variant),)+
                    _ => Err($error),
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

define_names!(
    /// The closed set of label colours. The web tokens `label-<name>` and `label-<name>-soft` use the same names.
    LabelColor,
    TaskValueError::InvalidLabelColor,
    {
        Sage = "sage",
        Slate = "slate",
        Teal = "teal",
        Ink = "ink",
        Gold = "gold",
        Moss = "moss",
        Plum = "plum",
        Stone = "stone",
    }
);

/// How an edge came to exist. A checklist edge belongs to a linked checklist item and goes away only with that item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DependencyOrigin {
    Direct,
    Checklist,
}

impl DependencyOrigin {
    pub const ALL: &'static [DependencyOrigin] =
        &[DependencyOrigin::Direct, DependencyOrigin::Checklist];

    pub fn as_str(self) -> &'static str {
        match self {
            DependencyOrigin::Direct => "direct",
            DependencyOrigin::Checklist => "checklist",
        }
    }

    pub fn parse(text: &str) -> Option<DependencyOrigin> {
        Self::ALL
            .iter()
            .copied()
            .find(|origin| origin.as_str() == text)
    }
}

impl fmt::Display for DependencyOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::TaskStatus;

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
            assert_eq!(color.as_str().parse::<LabelColor>(), Ok(*color));
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
    fn dependency_origins_round_trip() {
        for origin in DependencyOrigin::ALL {
            assert_eq!(DependencyOrigin::parse(origin.as_str()), Some(*origin));
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
            assert_eq!(status.as_str().parse::<TaskStatus>(), Ok(*status));
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
        ];
        for (error, code) in cases {
            assert_eq!(error.code(), code);
            assert_eq!(FoldError::from(error).code(), code);
        }
    }
}
