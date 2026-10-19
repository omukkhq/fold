use std::fmt;
use std::num::NonZeroU32;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::error::{FoldError, codes};

/// The messages never repeat the rejected text, so an error is safe to log and to return to an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum IdParseError {
    #[error("id has the wrong prefix")]
    WrongPrefix,
    #[error("id number is not made of ASCII digits")]
    NotANumber,
    #[error("id number has a leading zero")]
    LeadingZero,
    #[error("id number is zero")]
    Zero,
    #[error("id number is larger than 4294967295")]
    OutOfRange,
}

impl IdParseError {
    pub fn code(&self) -> &'static str {
        codes::INVALID_ID
    }
}

impl From<IdParseError> for FoldError {
    fn from(error: IdParseError) -> Self {
        FoldError::new(error.code(), error.to_string())
    }
}

/// The kinds of public id. `counter_key` is the value of `counters.kind` in the project database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdKind {
    Task,
    Proposal,
    Changeset,
    Revision,
    Workspace,
    Checkpoint,
    Message,
    Comment,
    Integration,
    ChecklistItem,
    Request,
    ReviewBatch,
    PermissionQuestion,
    CheckRun,
}

impl IdKind {
    pub const ALL: [IdKind; 14] = [
        IdKind::Task,
        IdKind::Proposal,
        IdKind::Changeset,
        IdKind::Revision,
        IdKind::Workspace,
        IdKind::Checkpoint,
        IdKind::Message,
        IdKind::Comment,
        IdKind::Integration,
        IdKind::ChecklistItem,
        IdKind::Request,
        IdKind::ReviewBatch,
        IdKind::PermissionQuestion,
        IdKind::CheckRun,
    ];

    pub const fn prefix(self) -> &'static str {
        match self {
            IdKind::Task => "T-",
            IdKind::Proposal => "P-",
            IdKind::Changeset => "C-",
            IdKind::Revision => "r",
            IdKind::Workspace => "ws-",
            IdKind::Checkpoint => "ck-",
            IdKind::Message => "m-",
            IdKind::Comment => "rc-",
            IdKind::Integration => "i-",
            IdKind::ChecklistItem => "ci-",
            IdKind::Request => "RQ-",
            IdKind::ReviewBatch => "rb-",
            IdKind::PermissionQuestion => "pq-",
            IdKind::CheckRun => "cr-",
        }
    }

    pub fn counter_key(self) -> &'static str {
        match self {
            IdKind::Task => "task",
            IdKind::Proposal => "proposal",
            IdKind::Changeset => "changeset",
            IdKind::Revision => "revision",
            IdKind::Workspace => "workspace",
            IdKind::Checkpoint => "checkpoint",
            IdKind::Message => "message",
            IdKind::Comment => "comment",
            IdKind::Integration => "integration",
            IdKind::ChecklistItem => "checklist_item",
            IdKind::Request => "request",
            IdKind::ReviewBatch => "review_batch",
            IdKind::PermissionQuestion => "permission_question",
            IdKind::CheckRun => "check_run",
        }
    }
}

fn parse_number(rest: &str) -> Result<NonZeroU32, IdParseError> {
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return Err(IdParseError::NotANumber);
    }
    if rest == "0" {
        return Err(IdParseError::Zero);
    }
    if rest.starts_with('0') {
        return Err(IdParseError::LeadingZero);
    }
    // More than ten digits cannot fit in u32, and the cap keeps the u64 parse from overflowing.
    if rest.len() > 10 {
        return Err(IdParseError::OutOfRange);
    }
    let value: u64 = rest.parse().map_err(|_| IdParseError::NotANumber)?;
    u32::try_from(value)
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or(IdParseError::OutOfRange)
}

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident, $kind:expr) => {
        define_id!(@prefixed $(#[$meta])* $name, $kind.prefix());

        impl $name {
            pub const KIND: IdKind = $kind;
        }
    };
    (@prefixed $(#[$meta:meta])* $name:ident, $prefix:expr) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(NonZeroU32);

        impl $name {
            pub const PREFIX: &'static str = $prefix;

            pub fn number(self) -> u32 {
                self.0.get()
            }

            pub fn from_number(number: u32) -> Option<Self> {
                NonZeroU32::new(number).map(Self)
            }

            /// The value of the `number` column.
            pub fn to_db(self) -> i64 {
                i64::from(self.0.get())
            }

            pub fn try_from_db(value: i64) -> Option<Self> {
                u32::try_from(value).ok().and_then(Self::from_number)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}", Self::PREFIX, self.0)
            }
        }

        impl FromStr for $name {
            type Err = IdParseError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                let rest = text
                    .strip_prefix(Self::PREFIX)
                    .ok_or(IdParseError::WrongPrefix)?;
                parse_number(rest).map(Self)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                text.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

define_id!(TaskId, IdKind::Task);
define_id!(ProposalId, IdKind::Proposal);
define_id!(ChangesetId, IdKind::Changeset);

/// The public id of a revision. Unlike the other ids it starts at `r0`, the genesis revision every project starts from, so its number may be zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RevisionId(u32);

impl RevisionId {
    pub const KIND: IdKind = IdKind::Revision;
    pub const PREFIX: &'static str = IdKind::Revision.prefix();
    /// `r0`, the genesis revision.
    pub const GENESIS: Self = Self(0);

    pub fn number(self) -> u32 {
        self.0
    }

    /// Every number is a revision number; the `Option` keeps the signature of the other ids.
    pub fn from_number(number: u32) -> Option<Self> {
        Some(Self(number))
    }

    /// The value of the `number` column.
    pub fn to_db(self) -> i64 {
        i64::from(self.0)
    }

    pub fn try_from_db(value: i64) -> Option<Self> {
        u32::try_from(value).ok().map(Self)
    }

    /// The revision after this one, `None` past the last number.
    pub fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

impl fmt::Display for RevisionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", Self::PREFIX, self.0)
    }
}

impl FromStr for RevisionId {
    type Err = IdParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let rest = text
            .strip_prefix(Self::PREFIX)
            .ok_or(IdParseError::WrongPrefix)?;
        if rest == "0" {
            return Ok(Self::GENESIS);
        }
        parse_number(rest).map(|number| Self(number.get()))
    }
}

impl Serialize for RevisionId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for RevisionId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

define_id!(WorkspaceId, IdKind::Workspace);
define_id!(CheckpointId, IdKind::Checkpoint);
define_id!(MessageId, IdKind::Message);
define_id!(
    /// The public id of a review comment; the `comment` counter numbers them.
    CommentId,
    IdKind::Comment
);
define_id!(
    /// The public integration number that a revision records; the `integration` counter numbers them.
    IntegrationId,
    IdKind::Integration
);
define_id!(
    /// The public id of a checklist item, plain step or linked; the `checklist_item` counter numbers them.
    ChecklistItemId,
    IdKind::ChecklistItem
);
define_id!(
    /// The public id of a request; the `request` counter numbers them.
    RequestId,
    IdKind::Request
);
define_id!(
    /// The public id of a review batch; the `review_batch` counter numbers them.
    ReviewBatchId,
    IdKind::ReviewBatch
);
define_id!(
    /// The public id of a question a worker's permission check put to a human; the `permission_question` counter numbers them.
    PermissionQuestionId,
    IdKind::PermissionQuestion
);
define_id!(
    /// The public id of one run of a project's checks by fold; the `check_run` counter numbers them.
    CheckRunId,
    IdKind::CheckRun
);
define_id!(
    @prefixed
    /// The public id of a web sign-in token: `web_tokens.number`, unique across the host registry rather than one project. A revoked token keeps its row and its number, so a number is never handed out twice.
    WebTokenId,
    "wt-"
);

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    macro_rules! round_trip {
        ($ty:ident, $n:expr) => {{
            let id = $ty::from_number($n).unwrap();
            let text = id.to_string();
            assert!(text.starts_with($ty::KIND.prefix()));
            assert_eq!(text.parse::<$ty>(), Ok(id));
            assert_eq!(id.number(), $n);
            assert_eq!($ty::try_from_db(id.to_db()), Some(id));
            let json = serde_json::to_string(&id).unwrap();
            assert_eq!(serde_json::from_str::<$ty>(&json).unwrap(), id);
        }};
    }

    proptest! {
        #[test]
        fn ids_round_trip(n in 1..=u32::MAX) {
            round_trip!(TaskId, n);
            round_trip!(ProposalId, n);
            round_trip!(ChangesetId, n);
            round_trip!(RevisionId, n);
            round_trip!(WorkspaceId, n);
            round_trip!(CheckpointId, n);
            round_trip!(MessageId, n);
            round_trip!(CommentId, n);
            round_trip!(IntegrationId, n);
            round_trip!(ChecklistItemId, n);
            round_trip!(RequestId, n);
            round_trip!(ReviewBatchId, n);
            round_trip!(PermissionQuestionId, n);
            round_trip!(CheckRunId, n);
        }
    }

    #[test]
    fn ids_reject_malformed_text() {
        let cases: [(&str, IdParseError); 10] = [
            ("T-0", IdParseError::Zero),
            ("T-012", IdParseError::LeadingZero),
            ("T-", IdParseError::NotANumber),
            ("T-+1", IdParseError::NotANumber),
            (" T-1", IdParseError::WrongPrefix),
            ("t-1", IdParseError::WrongPrefix),
            ("T-4294967296", IdParseError::OutOfRange),
            ("P-3", IdParseError::WrongPrefix),
            ("", IdParseError::WrongPrefix),
            ("T-99999999999999999999", IdParseError::OutOfRange),
        ];
        for (text, expected) in cases {
            assert_eq!(text.parse::<TaskId>(), Err(expected), "{text:?}");
        }
        assert_eq!("rc-0".parse::<CommentId>(), Err(IdParseError::Zero));
        assert_eq!("r0".parse::<RevisionId>(), Ok(RevisionId::GENESIS));
        assert_eq!(RevisionId::GENESIS.to_string(), "r0");
        assert_eq!(RevisionId::try_from_db(0), Some(RevisionId::GENESIS));
        assert_eq!("r00".parse::<RevisionId>(), Err(IdParseError::LeadingZero));
        assert_eq!("r01".parse::<RevisionId>(), Err(IdParseError::LeadingZero));
        assert_eq!("r".parse::<RevisionId>(), Err(IdParseError::NotANumber));
        assert_eq!(RevisionId::GENESIS.next(), RevisionId::from_number(1));
        assert_eq!(RevisionId::from_number(u32::MAX).unwrap().next(), None);
        assert_eq!("i-0".parse::<IntegrationId>(), Err(IdParseError::Zero));
        assert_eq!(
            "r3".parse::<IntegrationId>(),
            Err(IdParseError::WrongPrefix)
        );
        assert_eq!(IdKind::Integration.counter_key(), "integration");
        assert_eq!("rc-5".parse::<RevisionId>(), Err(IdParseError::NotANumber));
        assert_eq!("T-1 ".parse::<TaskId>(), Err(IdParseError::NotANumber));
        assert_eq!(
            "T-\u{0661}".parse::<TaskId>(),
            Err(IdParseError::NotANumber)
        );
        assert_eq!(
            "T-4294967295".parse::<TaskId>().map(TaskId::number),
            Ok(u32::MAX)
        );
        assert_eq!(TaskId::try_from_db(0), None);
        assert_eq!(TaskId::try_from_db(-1), None);
        assert_eq!(TaskId::try_from_db(i64::from(u32::MAX) + 1), None);
        assert_eq!(TaskId::from_number(0), None);
    }

    #[test]
    fn ids_serialise_as_text() {
        let task = TaskId::from_number(12).unwrap();
        assert_eq!(serde_json::to_string(&task).unwrap(), "\"T-12\"");
        assert_eq!(serde_json::from_str::<TaskId>("\"T-12\"").unwrap(), task);
        assert!(serde_json::from_str::<TaskId>("12").is_err());
        assert!(serde_json::from_str::<TaskId>("\"P-12\"").is_err());
        let comment = CommentId::from_number(5).unwrap();
        assert_eq!(serde_json::to_string(&comment).unwrap(), "\"rc-5\"");
        let integration = IntegrationId::from_number(7).unwrap();
        assert_eq!(serde_json::to_string(&integration).unwrap(), "\"i-7\"");
        let item = ChecklistItemId::from_number(3).unwrap();
        assert_eq!(serde_json::to_string(&item).unwrap(), "\"ci-3\"");
        assert_eq!(
            "c-3".parse::<ChecklistItemId>(),
            Err(IdParseError::WrongPrefix)
        );
        let request = RequestId::from_number(4).unwrap();
        assert_eq!(serde_json::to_string(&request).unwrap(), "\"RQ-4\"");
        assert_eq!("rq-4".parse::<RequestId>(), Err(IdParseError::WrongPrefix));
        assert_eq!("RQ-4".parse::<RevisionId>(), Err(IdParseError::WrongPrefix));
        assert_eq!(IdKind::Request.counter_key(), "request");
        let batch = ReviewBatchId::from_number(3).unwrap();
        assert_eq!(serde_json::to_string(&batch).unwrap(), "\"rb-3\"");
        assert_eq!("rb-3".parse::<RevisionId>(), Err(IdParseError::NotANumber));
        assert_eq!(
            "rc-3".parse::<ReviewBatchId>(),
            Err(IdParseError::WrongPrefix)
        );
        assert_eq!(IdKind::ReviewBatch.counter_key(), "review_batch");
        assert_eq!(
            IdKind::PermissionQuestion.counter_key(),
            "permission_question"
        );
        assert_eq!(
            "pq-4".parse::<PermissionQuestionId>().unwrap().to_string(),
            "pq-4"
        );
    }

    #[test]
    fn kinds_list_every_prefix_and_counter_once() {
        let mut prefixes: Vec<_> = IdKind::ALL.iter().map(|k| k.prefix()).collect();
        let mut keys: Vec<_> = IdKind::ALL.iter().map(|k| k.counter_key()).collect();
        prefixes.sort_unstable();
        prefixes.dedup();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(prefixes.len(), IdKind::ALL.len());
        assert_eq!(keys.len(), IdKind::ALL.len());
    }

    #[test]
    fn a_web_token_id_has_its_own_prefix() {
        let id: WebTokenId = "wt-12".parse().unwrap();
        assert_eq!(id.number(), 12);
        assert_eq!(id.to_string(), "wt-12");
        assert_eq!(WebTokenId::try_from_db(12), Some(id));
        assert_eq!(
            "ws-12".parse::<WebTokenId>(),
            Err(IdParseError::WrongPrefix)
        );
        assert_eq!(
            "wt-012".parse::<WebTokenId>(),
            Err(IdParseError::LeadingZero)
        );
        assert!(
            IdKind::ALL
                .iter()
                .all(|kind| !WebTokenId::PREFIX.starts_with(kind.prefix())
                    && !kind.prefix().starts_with(WebTokenId::PREFIX))
        );
    }

    #[test]
    fn parse_error_becomes_fold_error_without_input() {
        let error = "T-secret".parse::<TaskId>().unwrap_err();
        let fold = FoldError::from(error);
        assert_eq!(fold.code(), "invalid_id");
        assert!(!fold.message().contains("secret"));
    }
}
