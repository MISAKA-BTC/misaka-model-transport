//! Bundle kinds (RFC-0001 §2.1).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// What a bundle holds. The id is the `kind: u8` a declaration carries (§3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BundleKind {
    /// Exactly one PALW container, its optional `.palwmanifest`, `LICENSE`, `README.md`. Id 0.
    PalwArtifact,
    /// The public weights a container is converted from. Id 1.
    SourceWeights,
    /// Reserved: misakas RFC-0004's unmerged adapters. Id 2. Refused by schema v1.
    Adapter,
    /// Reserved: a layer range for misakas RFC-0006's seats. Id 3. Refused by schema v1.
    Shard,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown bundle kind {0:?}")]
pub struct UnknownKind(pub String);

impl BundleKind {
    pub const ALL: [BundleKind; 4] =
        [BundleKind::PalwArtifact, BundleKind::SourceWeights, BundleKind::Adapter, BundleKind::Shard];

    pub fn id(self) -> u8 {
        match self {
            BundleKind::PalwArtifact => 0,
            BundleKind::SourceWeights => 1,
            BundleKind::Adapter => 2,
            BundleKind::Shard => 3,
        }
    }

    pub fn from_id(id: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.id() == id)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            BundleKind::PalwArtifact => "palw-artifact",
            BundleKind::SourceWeights => "source-weights",
            BundleKind::Adapter => "adapter",
            BundleKind::Shard => "shard",
        }
    }

    /// Whether schema v1 (and a declaration, PALW-DIST-2) accepts this kind: 0 or 1.
    pub fn is_active(self) -> bool {
        matches!(self, BundleKind::PalwArtifact | BundleKind::SourceWeights)
    }
}

impl fmt::Display for BundleKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for BundleKind {
    type Err = UnknownKind;
    fn from_str(s: &str) -> Result<Self, UnknownKind> {
        Self::ALL.into_iter().find(|k| k.as_str() == s).ok_or_else(|| UnknownKind(s.to_owned()))
    }
}
