use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use crate::error::{FoldError, codes};

/// What an object is. The byte is part of the hash input, so a blob and a tree with the same payload never share a hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum ObjectKind {
    Blob = 1,
    Tree = 2,
    Revision = 3,
}

impl ObjectKind {
    pub fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Blob),
            2 => Some(Self::Tree),
            3 => Some(Self::Revision),
            _ => None,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ParseHashError {
    #[error("object hash is not 64 characters")]
    WrongLength,
    #[error("object hash holds a character other than 0-9 and a-f")]
    NotHex,
}

impl ParseHashError {
    pub fn code(&self) -> &'static str {
        codes::INVALID_HASH
    }
}

impl From<ParseHashError> for FoldError {
    fn from(error: ParseHashError) -> Self {
        FoldError::new(error.code(), error.to_string())
    }
}

/// A BLAKE3 hash of an object, printed as 64 lowercase hex characters.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectHash([u8; 32]);

impl ObjectHash {
    pub const LEN: usize = 32;

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for ObjectHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for ObjectHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ObjectHash({self})")
    }
}

impl FromStr for ObjectHash {
    type Err = ParseHashError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let digits = text.as_bytes();
        if digits.len() != 64 {
            return Err(ParseHashError::WrongLength);
        }
        let mut bytes = [0u8; 32];
        for (slot, pair) in bytes.iter_mut().zip(digits.as_chunks::<2>().0) {
            *slot = (nibble(pair[0])? << 4) | nibble(pair[1])?;
        }
        Ok(Self(bytes))
    }
}

fn nibble(digit: u8) -> Result<u8, ParseHashError> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        _ => Err(ParseHashError::NotHex),
    }
}

/// BLAKE3 over the kind byte followed by the raw payload.
pub fn hash_object(kind: ObjectKind, payload: &[u8]) -> ObjectHash {
    let mut hasher = ObjectHasher::new(kind);
    hasher.update(payload);
    hasher.finish()
}

/// The incremental form of [`hash_object`], so a large file never has to fit in memory.
pub struct ObjectHasher(blake3::Hasher);

impl ObjectHasher {
    pub fn new(kind: ObjectKind) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&[kind.as_u8()]);
        Self(hasher)
    }

    pub fn update(&mut self, chunk: &[u8]) {
        self.0.update(chunk);
    }

    pub fn finish(self) -> ObjectHash {
        ObjectHash(*self.0.finalize().as_bytes())
    }
}

/// Unsigned LEB128. One value has exactly one byte string.
pub mod varint {
    use thiserror::Error;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
    pub enum VarintError {
        #[error("varint is truncated")]
        Truncated,
        #[error("varint is larger than u64::MAX")]
        Overflow,
        #[error("varint is not in its shortest form")]
        NonMinimal,
    }

    pub fn encode(mut value: u64, out: &mut Vec<u8>) {
        while value >= 0x80 {
            out.push((value & 0x7f) as u8 | 0x80);
            value >>= 7;
        }
        out.push(value as u8);
    }

    /// Returns the value and the number of bytes it used.
    pub fn decode(input: &[u8]) -> Result<(u64, usize), VarintError> {
        let mut value: u64 = 0;
        for (index, &byte) in input.iter().enumerate() {
            let shift = 7 * index as u32;
            let bits = u64::from(byte & 0x7f);
            if shift == 63 && bits > 1 || shift > 63 {
                return Err(VarintError::Overflow);
            }
            value |= bits << shift;
            if byte & 0x80 == 0 {
                if byte == 0 && index > 0 {
                    return Err(VarintError::NonMinimal);
                }
                return Ok((value, index + 1));
            }
        }
        Err(VarintError::Truncated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn golden_hashes() {
        assert_eq!(
            hash_object(ObjectKind::Blob, b"").to_string(),
            GOLDEN_EMPTY_BLOB
        );
        assert_eq!(
            hash_object(ObjectKind::Blob, b"hello\n").to_string(),
            GOLDEN_HELLO_BLOB
        );
    }

    const GOLDEN_EMPTY_BLOB: &str =
        "48fc721fbbc172e0925fa27af1671de225ba927134802998b10a1568a188652b";
    const GOLDEN_HELLO_BLOB: &str =
        "60200926b98f21cddd0dc4fb5c05e6a4d7ab691b9c8b01295ebefe1daca3bbe5";

    #[test]
    fn kind_is_part_of_the_hash() {
        assert_ne!(
            hash_object(ObjectKind::Blob, b"x"),
            hash_object(ObjectKind::Tree, b"x")
        );
    }

    #[test]
    fn chunked_hasher_matches_one_shot() {
        let mut hasher = ObjectHasher::new(ObjectKind::Blob);
        hasher.update(b"hel");
        hasher.update(b"");
        hasher.update(b"lo\n");
        assert_eq!(hasher.finish(), hash_object(ObjectKind::Blob, b"hello\n"));
    }

    #[test]
    fn kind_bytes() {
        for kind in [ObjectKind::Blob, ObjectKind::Tree, ObjectKind::Revision] {
            assert_eq!(ObjectKind::from_u8(kind.as_u8()), Some(kind));
        }
        assert_eq!(ObjectKind::from_u8(0), None);
        assert_eq!(ObjectKind::from_u8(4), None);
    }

    #[test]
    fn hash_text_round_trip_and_refusals() {
        let hash = hash_object(ObjectKind::Blob, b"x");
        let text = hash.to_string();
        assert_eq!(text.len(), 64);
        assert_eq!(text.parse::<ObjectHash>(), Ok(hash));
        assert_eq!(format!("{hash:?}"), format!("ObjectHash({text})"));
        assert_eq!(
            text[..63].parse::<ObjectHash>(),
            Err(ParseHashError::WrongLength)
        );
        assert_eq!(
            text.to_uppercase().parse::<ObjectHash>(),
            Err(ParseHashError::NotHex)
        );
        assert_eq!(
            "g".repeat(64).parse::<ObjectHash>(),
            Err(ParseHashError::NotHex)
        );
        let error = FoldError::from(ParseHashError::NotHex);
        assert_eq!(error.code(), "invalid_hash");
    }

    #[test]
    fn varint_known_values() {
        let mut out = Vec::new();
        varint::encode(0, &mut out);
        varint::encode(127, &mut out);
        varint::encode(128, &mut out);
        varint::encode(300, &mut out);
        assert_eq!(out, [0x00, 0x7f, 0x80, 0x01, 0xac, 0x02]);
        let mut max = Vec::new();
        varint::encode(u64::MAX, &mut max);
        assert_eq!(max.len(), 10);
        assert_eq!(varint::decode(&max), Ok((u64::MAX, 10)));
    }

    #[test]
    fn varint_refusals() {
        use varint::VarintError;
        assert_eq!(varint::decode(&[]), Err(VarintError::Truncated));
        assert_eq!(varint::decode(&[0x80]), Err(VarintError::Truncated));
        assert_eq!(varint::decode(&[0xff, 0xff]), Err(VarintError::Truncated));
        assert_eq!(varint::decode(&[0x80, 0x00]), Err(VarintError::NonMinimal));
        assert_eq!(varint::decode(&[0xff, 0x00]), Err(VarintError::NonMinimal));
        let mut too_big = vec![0xff; 9];
        too_big.push(0x02);
        assert_eq!(varint::decode(&too_big), Err(VarintError::Overflow));
        let too_long = vec![0x80; 11];
        assert_eq!(varint::decode(&too_long), Err(VarintError::Overflow));
    }

    proptest! {
        #[test]
        fn varint_round_trip(value in any::<u64>(), tail in proptest::collection::vec(any::<u8>(), 0..4)) {
            let mut bytes = Vec::new();
            varint::encode(value, &mut bytes);
            let used = bytes.len();
            bytes.extend(tail);
            prop_assert_eq!(varint::decode(&bytes), Ok((value, used)));
        }

        #[test]
        fn chunked_hash_equals_one_shot(payload in proptest::collection::vec(any::<u8>(), 0..2048), split in 0usize..2048) {
            let split = split.min(payload.len());
            let mut hasher = ObjectHasher::new(ObjectKind::Tree);
            hasher.update(&payload[..split]);
            hasher.update(&payload[split..]);
            prop_assert_eq!(hasher.finish(), hash_object(ObjectKind::Tree, &payload));
        }
    }
}
