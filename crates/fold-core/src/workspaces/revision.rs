use thiserror::Error;

use crate::error::{FoldError, codes};
use crate::ids::{IntegrationId, RevisionId};
use crate::time::Timestamp;
use crate::workspaces::object::{ObjectHash, ObjectKind, hash_object, varint};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RevisionError {
    #[error("revision bytes end early")]
    Truncated,
    #[error("revision has a flag byte other than 0 or 1")]
    BadFlag,
    #[error("revision has a number or integration number that is not a valid varint")]
    BadVarint,
    #[error("revision number or integration number is zero or larger than 4294967295")]
    BadNumber,
    #[error("revision creation time is out of range")]
    BadTime,
    #[error("revision bytes have trailing data")]
    TrailingBytes,
}

impl RevisionError {
    pub fn code(&self) -> &'static str {
        codes::REVISION_INVALID
    }
}

impl From<RevisionError> for FoldError {
    fn from(error: RevisionError) -> Self {
        FoldError::new(error.code(), error.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Revision {
    pub tree: ObjectHash,
    /// `None` only for `r1`.
    pub parent: Option<ObjectHash>,
    pub number: RevisionId,
    /// The number of the integration that made this revision; never a row id. `None` for `r1`.
    pub integration: Option<IntegrationId>,
    pub created_at: Timestamp,
}

impl Revision {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + 33 + 5 + 10 + 8);
        out.extend_from_slice(self.tree.as_bytes());
        match &self.parent {
            Some(parent) => {
                out.push(1);
                out.extend_from_slice(parent.as_bytes());
            }
            None => out.push(0),
        }
        varint::encode(u64::from(self.number.number()), &mut out);
        match self.integration {
            Some(integration) => {
                out.push(1);
                varint::encode(u64::from(integration.number()), &mut out);
            }
            None => out.push(0),
        }
        out.extend_from_slice(&self.created_at.as_millis().to_be_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, RevisionError> {
        let mut reader = Reader(bytes);
        let tree = reader.hash()?;
        let parent = match reader.byte()? {
            0 => None,
            1 => Some(reader.hash()?),
            _ => return Err(RevisionError::BadFlag),
        };
        let number = u32::try_from(reader.varint()?)
            .ok()
            .and_then(RevisionId::from_number)
            .ok_or(RevisionError::BadNumber)?;
        let integration = match reader.byte()? {
            0 => None,
            1 => Some(
                u32::try_from(reader.varint()?)
                    .ok()
                    .and_then(IntegrationId::from_number)
                    .ok_or(RevisionError::BadNumber)?,
            ),
            _ => return Err(RevisionError::BadFlag),
        };
        let millis = i64::from_be_bytes(reader.take::<8>()?);
        let created_at = Timestamp::from_millis(millis).map_err(|_| RevisionError::BadTime)?;
        if !reader.0.is_empty() {
            return Err(RevisionError::TrailingBytes);
        }
        Ok(Self {
            tree,
            parent,
            number,
            integration,
            created_at,
        })
    }

    pub fn hash(&self) -> ObjectHash {
        hash_object(ObjectKind::Revision, &self.encode())
    }
}

/// A revision whose object the object store holds. The revision row functions take this instead of a bare hash, so a row can only be inserted for a stored object; the object store builds it after writing or finding the object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredRevision {
    revision: Revision,
    hash: ObjectHash,
}

impl StoredRevision {
    /// Called by the object store once it holds the object of `revision`.
    pub fn confirmed(revision: Revision) -> Self {
        Self {
            hash: revision.hash(),
            revision,
        }
    }

    pub fn revision(&self) -> &Revision {
        &self.revision
    }

    pub fn hash(&self) -> ObjectHash {
        self.hash
    }
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], RevisionError> {
        let (head, rest) = self
            .0
            .split_first_chunk::<N>()
            .ok_or(RevisionError::Truncated)?;
        self.0 = rest;
        Ok(*head)
    }

    fn byte(&mut self) -> Result<u8, RevisionError> {
        Ok(self.take::<1>()?[0])
    }

    fn hash(&mut self) -> Result<ObjectHash, RevisionError> {
        Ok(ObjectHash::from_bytes(self.take::<32>()?))
    }

    fn varint(&mut self) -> Result<u64, RevisionError> {
        let (value, used) = varint::decode(self.0).map_err(|error| match error {
            varint::VarintError::Truncated => RevisionError::Truncated,
            _ => RevisionError::BadVarint,
        })?;
        self.0 = &self.0[used..];
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn hash(seed: u8) -> ObjectHash {
        ObjectHash::from_bytes([seed; 32])
    }

    fn number(n: u32) -> RevisionId {
        RevisionId::from_number(n).unwrap()
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn fixed() -> Revision {
        Revision {
            tree: hash(1),
            parent: Some(hash(2)),
            number: number(300),
            integration: IntegrationId::from_number(5),
            created_at: Timestamp::from_millis(1_700_000_000_123).unwrap(),
        }
    }

    #[test]
    fn golden_revision_bytes_and_hash() {
        assert_eq!(hex(&fixed().encode()), GOLDEN_REVISION_BYTES);
        assert_eq!(fixed().hash().to_string(), GOLDEN_REVISION_HASH);
        let first = Revision {
            tree: hash(9),
            parent: None,
            number: number(1),
            integration: None,
            created_at: Timestamp::from_millis(0).unwrap(),
        };
        assert_eq!(hex(&first.encode()), GOLDEN_FIRST_BYTES);
        assert_eq!(first.hash().to_string(), GOLDEN_FIRST_HASH);
    }

    const GOLDEN_REVISION_BYTES: &str = "0101010101010101010101010101010101010101010101010101010101010101010202020202020202020202020202020202020202020202020202020202020202ac0201050000018bcfe5687b";
    const GOLDEN_REVISION_HASH: &str =
        "f1a3f5e9d55dd3888b19315c66925f35466fa82be775ff02b7668404a03957ce";
    const GOLDEN_FIRST_BYTES: &str =
        "09090909090909090909090909090909090909090909090909090909090909090001000000000000000000";
    const GOLDEN_FIRST_HASH: &str =
        "f2bfbaf8f806088c23eccc30677a76b052cb99e45029e230e38bf2b59021e1cd";

    #[test]
    fn decode_refusals() {
        let bytes = fixed().encode();
        assert_eq!(Revision::decode(&bytes), Ok(fixed()));
        for cut in 0..bytes.len() {
            assert!(Revision::decode(&bytes[..cut]).is_err(), "cut {cut}");
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(
            Revision::decode(&trailing),
            Err(RevisionError::TrailingBytes)
        );
        let mut bad_parent_flag = bytes.clone();
        bad_parent_flag[32] = 2;
        assert_eq!(
            Revision::decode(&bad_parent_flag),
            Err(RevisionError::BadFlag)
        );
        let first = Revision {
            parent: None,
            integration: None,
            ..fixed()
        }
        .encode();
        let mut bad_integration_flag = first.clone();
        bad_integration_flag[first.len() - 9] = 2;
        assert_eq!(
            Revision::decode(&bad_integration_flag),
            Err(RevisionError::BadFlag)
        );
    }

    #[test]
    fn number_and_time_are_checked() {
        let mut zero = Revision {
            parent: None,
            integration: None,
            ..fixed()
        }
        .encode();
        // tree(32) + parent flag(1) then the number varint.
        zero.splice(33..35, [0u8]);
        assert_eq!(Revision::decode(&zero), Err(RevisionError::BadNumber));
        let mut non_minimal = Revision {
            parent: None,
            integration: None,
            ..fixed()
        }
        .encode();
        non_minimal.splice(33..35, [0x81, 0x00]);
        assert_eq!(
            Revision::decode(&non_minimal),
            Err(RevisionError::BadVarint)
        );
        let mut big = Revision {
            parent: None,
            integration: None,
            ..fixed()
        }
        .encode();
        big.splice(33..35, [0x80, 0x80, 0x80, 0x80, 0x10]);
        assert_eq!(Revision::decode(&big), Err(RevisionError::BadNumber));
        // The integration number is the single varint byte before the 8 time bytes.
        let integrated = fixed().encode();
        let at = integrated.len() - 9;
        let mut zero_integration = integrated.clone();
        zero_integration[at] = 0;
        assert_eq!(
            Revision::decode(&zero_integration),
            Err(RevisionError::BadNumber)
        );
        let mut big_integration = integrated.clone();
        big_integration.splice(at..=at, [0x80, 0x80, 0x80, 0x80, 0x10]);
        assert_eq!(
            Revision::decode(&big_integration),
            Err(RevisionError::BadNumber)
        );
        let mut late = Revision {
            parent: None,
            integration: None,
            ..fixed()
        }
        .encode();
        let len = late.len();
        late[len - 8..].copy_from_slice(&i64::MAX.to_be_bytes());
        assert_eq!(Revision::decode(&late), Err(RevisionError::BadTime));
        assert_eq!(
            FoldError::from(RevisionError::BadTime).code(),
            "revision_invalid"
        );
    }

    proptest! {
        #[test]
        fn revision_round_trip(
            tree in any::<[u8; 32]>(),
            parent in proptest::option::of(any::<[u8; 32]>()),
            number in 1u32..,
            integration in proptest::option::of(1u32..),
            millis in -62_167_219_200_000i64..=253_402_300_799_999,
        ) {
            let revision = Revision {
                tree: ObjectHash::from_bytes(tree),
                parent: parent.map(ObjectHash::from_bytes),
                number: RevisionId::from_number(number).unwrap(),
                integration: integration.and_then(IntegrationId::from_number),
                created_at: Timestamp::from_millis(millis).unwrap(),
            };
            prop_assert_eq!(Revision::decode(&revision.encode()), Ok(revision));
        }
    }
}
