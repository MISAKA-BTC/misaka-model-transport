//! The chain-anchored deep link, `misaka-model://<network>/<line_id>/<version>?kind=<kind>`
//! (RFC-0001 §8.2).
//!
//! The link is an identifier, not an instruction: a handler resolves everything it shows from the
//! chain, through the operator's own node, and fetches nothing before a confirmation (MT-9).
//! Parsing is strict:
//! - `network` is a known network name (`mainnet`, `devnet`, `simnet`, `testnet-<n>`);
//! - `line_id` is 128 lowercase hex;
//! - `version` is a decimal `u32` without leading zeros;
//! - `kind` is `palw-artifact` or `source-weights`, and there are no other parameters;
//! - the whole link is at most 256 bytes.

use std::fmt;
use std::str::FromStr;

use crate::hexfmt::Hex64;
use crate::kind::BundleKind;

pub const SCHEME: &str = "misaka-model";
pub const MAX_LINK_BYTES: usize = 256;

/// A network name as misakas spells it (`consensus/core/src/network.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Network {
    Mainnet,
    Testnet(u32),
    Devnet,
    Simnet,
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Network::Mainnet => f.write_str("mainnet"),
            Network::Testnet(n) => write!(f, "testnet-{n}"),
            Network::Devnet => f.write_str("devnet"),
            Network::Simnet => f.write_str("simnet"),
        }
    }
}

impl FromStr for Network {
    type Err = LinkError;
    fn from_str(s: &str) -> Result<Self, LinkError> {
        match s {
            "mainnet" => Ok(Network::Mainnet),
            "devnet" => Ok(Network::Devnet),
            "simnet" => Ok(Network::Simnet),
            _ => s
                .strip_prefix("testnet-")
                .and_then(parse_u32_strict)
                .map(Network::Testnet)
                .ok_or_else(|| LinkError::Network(s.to_owned())),
        }
    }
}

/// A parsed `misaka-model://` link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelLink {
    pub network: Network,
    pub line_id: Hex64,
    pub version: u32,
    pub kind: BundleKind,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    #[error("the link is longer than {MAX_LINK_BYTES} bytes")]
    TooLong,
    #[error("the link does not start with `misaka-model://`")]
    Scheme,
    #[error("the link is not <network>/<line_id>/<version>?kind=<kind>")]
    Shape,
    #[error("unknown network {0:?}")]
    Network(String),
    #[error("line_id is not 128 lowercase hex digits")]
    LineId,
    #[error("version is not a decimal u32 without leading zeros")]
    Version,
    #[error("kind {0:?} is not palw-artifact or source-weights")]
    Kind(String),
}

impl FromStr for ModelLink {
    type Err = LinkError;

    fn from_str(s: &str) -> Result<Self, LinkError> {
        if s.len() > MAX_LINK_BYTES {
            return Err(LinkError::TooLong);
        }
        let rest = s.strip_prefix("misaka-model://").ok_or(LinkError::Scheme)?;
        let (path, query) = rest.split_once('?').ok_or(LinkError::Shape)?;
        let kind_s = query.strip_prefix("kind=").ok_or(LinkError::Shape)?;
        if kind_s.contains(['&', '?', '#', '=']) {
            return Err(LinkError::Shape);
        }
        let mut parts = path.split('/');
        let (Some(net), Some(line), Some(ver), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
            return Err(LinkError::Shape);
        };
        let network = net.parse()?;
        let line_id = line.parse::<Hex64>().map_err(|_| LinkError::LineId)?;
        let version = parse_u32_strict(ver).ok_or(LinkError::Version)?;
        let kind = kind_s
            .parse::<BundleKind>()
            .ok()
            .filter(|k| k.is_active())
            .ok_or_else(|| LinkError::Kind(kind_s.to_owned()))?;
        Ok(ModelLink { network, line_id, version, kind })
    }
}

impl fmt::Display for ModelLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{SCHEME}://{}/{}/{}?kind={}", self.network, self.line_id, self.version, self.kind)
    }
}

/// A decimal `u32` with no sign, no leading zero (but `0` itself) and no other characters.
fn parse_u32_strict(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0')) {
        return None;
    }
    s.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> String {
        "5b".repeat(64)
    }

    #[test]
    fn round_trip() {
        let s = format!("misaka-model://testnet-12/{}/3?kind=palw-artifact", line());
        let l: ModelLink = s.parse().unwrap();
        assert_eq!(l.network, Network::Testnet(12));
        assert_eq!(l.version, 3);
        assert_eq!(l.kind, BundleKind::PalwArtifact);
        assert_eq!(l.to_string(), s);
    }

    #[test]
    fn strict() {
        let l = line();
        let bad = [
            format!("misaka://testnet-12/{l}/3?kind=palw-artifact"),
            format!("misaka-model://testnet-12/{l}/3"),
            format!("misaka-model://testnet-12/{l}/03?kind=palw-artifact"),
            format!("misaka-model://testnet-12/{l}/-1?kind=palw-artifact"),
            format!("misaka-model://testnet-12/{l}/4294967296?kind=palw-artifact"),
            format!("misaka-model://testnet-012/{l}/3?kind=palw-artifact"),
            format!("misaka-model://moonnet/{l}/3?kind=palw-artifact"),
            format!("misaka-model://testnet-12/{}/3?kind=palw-artifact", l.to_uppercase()),
            format!("misaka-model://testnet-12/{l}/3/?kind=palw-artifact"),
            format!("misaka-model://testnet-12/{l}/3?kind=palw-artifact&x=1"),
            format!("misaka-model://testnet-12/{l}/3?kind=shard"),
            format!("misaka-model://testnet-12/{l}/3?kind=palw-artifact#frag"),
            format!("misaka-model://testnet-12/{l}/3?kind=palw-artifact{}", " ".repeat(100)),
        ];
        for b in bad {
            assert!(b.parse::<ModelLink>().is_err(), "{b}");
        }
    }
}
