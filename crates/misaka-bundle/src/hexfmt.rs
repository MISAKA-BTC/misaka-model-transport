//! Fixed-width lowercase hex, the only spelling of a hash in a descriptor.

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};

/// A hex string was not exactly `2 × N` lowercase hex digits.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("expected {expected} lowercase hex digits")]
pub struct HexError {
    pub expected: usize,
}

/// Decodes exactly `N` bytes from `2 × N` lowercase hex digits. Uppercase is refused, so that a
/// value has one spelling.
pub fn decode_fixed<const N: usize>(s: &str) -> Result<[u8; N], HexError> {
    let err = HexError { expected: 2 * N };
    if s.len() != 2 * N || !s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(err);
    }
    let mut out = [0u8; N];
    hex::decode_to_slice(s, &mut out).map_err(|_| err)?;
    Ok(out)
}

macro_rules! fixed_hex {
    ($name:ident, $n:expr, $doc:expr) => {
        #[doc = $doc]
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub [u8; $n]);

        impl $name {
            pub const LEN: usize = $n;

            pub fn as_bytes(&self) -> &[u8; $n] {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&hex::encode(self.0))
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self)
            }
        }

        impl FromStr for $name {
            type Err = HexError;
            fn from_str(s: &str) -> Result<Self, HexError> {
                decode_fixed::<$n>(s).map($name)
            }
        }

        impl From<[u8; $n]> for $name {
            fn from(b: [u8; $n]) -> Self {
                $name(b)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = <std::borrow::Cow<'de, str>>::deserialize(d)?;
                s.parse().map_err(de::Error::custom)
            }
        }
    };
}

fixed_hex!(Hex32, 32, "32 bytes, written as 64 lowercase hex digits (SHA-256, a BEP 52 root, an infohash).");
fixed_hex!(Hex64, 64, "64 bytes, written as 128 lowercase hex digits (the chain's hashes, a commitment).");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_spelling() {
        let lower = "ab".repeat(32);
        assert!(lower.parse::<Hex32>().is_ok());
        assert!("AB".repeat(32).parse::<Hex32>().is_err());
        assert!("ab".repeat(31).parse::<Hex32>().is_err());
        assert!(format!("0x{}", "ab".repeat(31)).parse::<Hex32>().is_err());
        assert_eq!(lower.parse::<Hex32>().unwrap().to_string(), lower);
    }
}
