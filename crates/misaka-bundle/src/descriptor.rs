//! `misaka-bundle.json`, schema `misaka/torrent-bundle/v1` (RFC-0001 §2.2).
//!
//! Canonical form: UTF-8, keys sorted, two-space indent, one trailing newline, `files` sorted by
//! `path` — the components manifest's form (misakas `docs/components-manifest.md:31-33`). Struct
//! fields below are declared in key order, so serde writes them sorted; a test pins that.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::commitment::{BundleCommitment, bundle_commitment};
use crate::hexfmt::{Hex32, Hex64};
use crate::kind::BundleKind;

/// The descriptor's file name, at the bundle's root.
pub const DESCRIPTOR_FILE_NAME: &str = "misaka-bundle.json";
/// The schema string. A new key, or a new torrent rule, is a new schema string.
pub const DESCRIPTOR_SCHEMA: &str = "misaka/torrent-bundle/v1";
/// A descriptor is at most 1 MiB (§5.3).
pub const MAX_DESCRIPTOR_BYTES: usize = 1 << 20;

/// The descriptor of one bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub files: Vec<FileEntry>,
    pub kind: BundleKind,
    pub license: License,
    pub schema: String,
    pub title: String,
}

/// One file of the bundle. The descriptor does not list itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    /// BEP 52 Merkle root of the file's 16 KiB blocks.
    pub btv2_pieces_root: Hex32,
    /// Present exactly for the PALW container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub palw: Option<PalwInfo>,
    pub path: String,
    pub role: Role,
    /// SHA-256 of the file's own bytes: the value a components-manifest row carries.
    pub sha256: Hex32,
    pub size: u64,
}

/// What a PALW container is, copied from the registry. L3 checks it by recomputation, never by
/// reading this field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PalwInfo {
    /// The container's own digest (`PalwArtifactDigestV1`), when it defines one. Never a root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_digest: Option<Hex64>,
    pub magic: PalwMagic,
    /// `PalwInventoryRootV1` per class, sorted by `class_id`.
    pub roots: Vec<PalwRoot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PalwRoot {
    pub class_id: Hex64,
    pub inventory_root: Hex64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct License {
    pub file: String,
    pub spdx: String,
}

/// A file's role. The policy derives it from the name; a descriptor whose role disagrees is
/// refused, so two packagers write the same descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    /// `*.palwart`, `*.palwq36`, `*.palwtir`.
    PalwContainer,
    /// `*.palwmanifest`.
    PalwManifest,
    /// `*.gguf`, `*.safetensors`.
    Weights,
    /// `model.safetensors.index.json`.
    WeightsIndex,
    /// `config.json`, `generation_config.json`.
    Config,
    /// `tokenizer.json`, `tokenizer_config.json`, `special_tokens_map.json`, `vocab.json`,
    /// `tokenizer.model`, `*.tiktoken`, `merges.txt`.
    Tokenizer,
    /// `LICENSE`, `LICENSE.txt`.
    License,
    /// `NOTICE`.
    Notice,
    /// `README.md`.
    Readme,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::PalwContainer => "palw-container",
            Role::PalwManifest => "palw-manifest",
            Role::Weights => "weights",
            Role::WeightsIndex => "weights-index",
            Role::Config => "config",
            Role::Tokenizer => "tokenizer",
            Role::License => "license",
            Role::Notice => "notice",
            Role::Readme => "readme",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The leading eight bytes of a PALW container (misakas's own magics, RFC-0001 §2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PalwMagic {
    /// `.palwart`, current.
    PalwB0A2,
    /// `.palwart`, previous.
    PalwB0A1,
    /// `.palwq36`.
    PalwQ361,
    /// `.palwtir`.
    PalwTir1,
}

impl PalwMagic {
    pub const ALL: [PalwMagic; 4] =
        [PalwMagic::PalwB0A2, PalwMagic::PalwB0A1, PalwMagic::PalwQ361, PalwMagic::PalwTir1];

    pub fn bytes(self) -> &'static [u8; 8] {
        match self {
            PalwMagic::PalwB0A2 => b"PALWB0A2",
            PalwMagic::PalwB0A1 => b"PALWB0A1",
            PalwMagic::PalwQ361 => b"PALWQ361",
            PalwMagic::PalwTir1 => b"PALWTIR1",
        }
    }

    pub fn as_str(self) -> &'static str {
        std::str::from_utf8(self.bytes()).expect("ASCII")
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.bytes().as_slice() == b)
    }

    /// The file extension (without the dot) a container with this magic carries.
    pub fn extension(self) -> &'static str {
        match self {
            PalwMagic::PalwB0A2 | PalwMagic::PalwB0A1 => "palwart",
            PalwMagic::PalwQ361 => "palwq36",
            PalwMagic::PalwTir1 => "palwtir",
        }
    }
}

impl fmt::Display for PalwMagic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PalwMagic {
    type Err = DescriptorError;
    fn from_str(s: &str) -> Result<Self, DescriptorError> {
        Self::from_bytes(s.as_bytes()).ok_or_else(|| DescriptorError::UnknownMagic(s.to_owned()))
    }
}

impl Serialize for PalwMagic {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PalwMagic {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Why a descriptor was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DescriptorError {
    #[error("descriptor is {0} bytes; at most {MAX_DESCRIPTOR_BYTES}")]
    TooLarge(usize),
    #[error("descriptor does not parse: {0}")]
    Parse(String),
    #[error("descriptor is not in canonical form (keys sorted, two-space indent, one trailing newline)")]
    NotCanonical,
    #[error("schema is {0:?}, expected {DESCRIPTOR_SCHEMA:?}")]
    Schema(String),
    #[error("kind {0} is reserved and not accepted by schema v1")]
    ReservedKind(BundleKind),
    #[error("title {0:?} is not [a-z0-9][a-z0-9._-]{{0,63}}")]
    Title(String),
    #[error("license.spdx {0:?} is not an SPDX expression")]
    Spdx(String),
    #[error("license.file {0:?} is not a file of the bundle with role `license`")]
    LicenseFile(String),
    #[error("the bundle lists no files")]
    NoFiles,
    #[error("files are not sorted by path, or a path repeats, at {0:?}")]
    Unsorted(String),
    #[error("the descriptor lists itself")]
    ListsItself,
    #[error("file {0:?} is empty")]
    EmptyFile(String),
    #[error("file {0:?}: `palw` is present exactly for the role `palw-container`")]
    PalwPlacement(String),
    #[error("file {0:?}: `palw.roots` is empty, or not sorted and unique by class_id")]
    PalwRoots(String),
    #[error("unknown PALW magic {0:?}")]
    UnknownMagic(String),
}

impl Descriptor {
    /// The one serialization: sorted keys, two-space indent, one trailing newline.
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        let mut out = serde_json::to_vec_pretty(self).expect("a descriptor always serializes");
        out.push(b'\n');
        out
    }

    /// The commitment over [`Self::to_canonical_bytes`].
    pub fn commitment(&self) -> BundleCommitment {
        bundle_commitment(&self.to_canonical_bytes())
    }

    /// Accepts exactly canonical, valid descriptor bytes. Unknown keys are refused at every level.
    pub fn parse_canonical(bytes: &[u8]) -> Result<Self, DescriptorError> {
        if bytes.len() > MAX_DESCRIPTOR_BYTES {
            return Err(DescriptorError::TooLarge(bytes.len()));
        }
        let d: Descriptor = serde_json::from_slice(bytes).map_err(|e| DescriptorError::Parse(e.to_string()))?;
        d.validate()?;
        if d.to_canonical_bytes() != bytes {
            return Err(DescriptorError::NotCanonical);
        }
        Ok(d)
    }

    /// The structural rules of schema v1. Name syntax, roles against names and kind constraints
    /// are the Safe Model Profile's (`misaka-transport-policy`).
    pub fn validate(&self) -> Result<(), DescriptorError> {
        if self.schema != DESCRIPTOR_SCHEMA {
            return Err(DescriptorError::Schema(self.schema.clone()));
        }
        if !self.kind.is_active() {
            return Err(DescriptorError::ReservedKind(self.kind));
        }
        if !is_valid_title(&self.title) {
            return Err(DescriptorError::Title(self.title.clone()));
        }
        if !is_valid_spdx(&self.license.spdx) {
            return Err(DescriptorError::Spdx(self.license.spdx.clone()));
        }
        if self.files.is_empty() {
            return Err(DescriptorError::NoFiles);
        }
        for pair in self.files.windows(2) {
            if pair[0].path.as_bytes() >= pair[1].path.as_bytes() {
                return Err(DescriptorError::Unsorted(pair[1].path.clone()));
            }
        }
        for f in &self.files {
            if f.path == DESCRIPTOR_FILE_NAME {
                return Err(DescriptorError::ListsItself);
            }
            if f.size == 0 {
                return Err(DescriptorError::EmptyFile(f.path.clone()));
            }
            match (&f.palw, f.role == Role::PalwContainer) {
                (Some(p), true) => {
                    let sorted = p.roots.windows(2).all(|w| w[0].class_id < w[1].class_id);
                    if p.roots.is_empty() || !sorted {
                        return Err(DescriptorError::PalwRoots(f.path.clone()));
                    }
                }
                (None, false) => {}
                _ => return Err(DescriptorError::PalwPlacement(f.path.clone())),
            }
        }
        if !self.files.iter().any(|f| f.path == self.license.file && f.role == Role::License) {
            return Err(DescriptorError::LicenseFile(self.license.file.clone()));
        }
        Ok(())
    }

    /// The file entry at `path`.
    pub fn file(&self, path: &str) -> Option<&FileEntry> {
        self.files.binary_search_by(|f| f.path.as_bytes().cmp(path.as_bytes())).ok().map(|i| &self.files[i])
    }

    /// The bundle's size: every listed file plus the descriptor itself.
    pub fn total_bytes(&self) -> u64 {
        let listed: u64 = self.files.iter().map(|f| f.size).sum();
        listed + self.to_canonical_bytes().len() as u64
    }

    /// The torrent's file set: `{misaka-bundle.json} ∪ files[].path`.
    pub fn torrent_file_names(&self) -> BTreeSet<&str> {
        let mut s: BTreeSet<&str> = self.files.iter().map(|f| f.path.as_str()).collect();
        s.insert(DESCRIPTOR_FILE_NAME);
        s
    }

    /// The PALW container entry, if any.
    pub fn palw_container(&self) -> Option<&FileEntry> {
        self.files.iter().find(|f| f.role == Role::PalwContainer)
    }
}

/// `[a-z0-9][a-z0-9._-]{0,63}`.
pub fn is_valid_title(t: &str) -> bool {
    let b = t.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-'))
}

/// An SPDX license expression without parentheses: identifiers (`Apache-2.0`, `GPL-2.0+`,
/// `LicenseRef-…`) joined by single spaces with `AND`, `OR` or `WITH`.
pub fn is_valid_spdx(s: &str) -> bool {
    if s.is_empty() || s.len() > 256 {
        return false;
    }
    let ident = |t: &str| {
        let core = t.strip_suffix('+').unwrap_or(t);
        !core.is_empty()
            && core.as_bytes()[0].is_ascii_alphanumeric()
            && core.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-'))
    };
    let tokens: Vec<&str> = s.split(' ').collect();
    tokens.len() % 2 == 1
        && tokens.iter().enumerate().all(|(i, t)| {
            if i % 2 == 0 {
                ident(t) && !matches!(*t, "AND" | "OR" | "WITH")
            } else {
                matches!(*t, "AND" | "OR" | "WITH")
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> Descriptor {
        Descriptor {
            files: vec![
                FileEntry {
                    btv2_pieces_root: Hex32([1; 32]),
                    palw: None,
                    path: "LICENSE".into(),
                    role: Role::License,
                    sha256: Hex32([2; 32]),
                    size: 11357,
                },
                FileEntry {
                    btv2_pieces_root: Hex32([3; 32]),
                    palw: Some(PalwInfo {
                        artifact_digest: Some(Hex64([4; 64])),
                        magic: PalwMagic::PalwB0A2,
                        roots: vec![PalwRoot { class_id: Hex64([5; 64]), inventory_root: Hex64([6; 64]) }],
                    }),
                    path: "qwen25-1.5b-a16.palwart".into(),
                    role: Role::PalwContainer,
                    sha256: Hex32([7; 32]),
                    size: 1_795_427_276,
                },
            ],
            kind: BundleKind::PalwArtifact,
            license: License { file: "LICENSE".into(), spdx: "Apache-2.0".into() },
            schema: DESCRIPTOR_SCHEMA.into(),
            title: "qwen25-1.5b-a16-graph-v5-512".into(),
        }
    }

    #[test]
    fn round_trip_is_canonical() {
        let d = sample();
        let bytes = d.to_canonical_bytes();
        assert_eq!(Descriptor::parse_canonical(&bytes).unwrap(), d);
        assert!(bytes.ends_with(b"}\n") && !bytes.ends_with(b"\n\n"));
    }

    #[test]
    fn keys_are_sorted_at_every_level() {
        // A serde_json::Value map is a BTreeMap: re-serializing through it sorts every key.
        let bytes = sample().to_canonical_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let mut resorted = serde_json::to_vec_pretty(&v).unwrap();
        resorted.push(b'\n');
        assert_eq!(resorted, bytes);
    }

    #[test]
    fn refuses_noncanonical_and_unknown() {
        let bytes = sample().to_canonical_bytes();
        let s = String::from_utf8(bytes.clone()).unwrap();
        // No trailing newline.
        assert_eq!(Descriptor::parse_canonical(&bytes[..bytes.len() - 1]), Err(DescriptorError::NotCanonical));
        // Compact JSON.
        let compact = serde_json::to_vec(&sample()).unwrap();
        assert_eq!(Descriptor::parse_canonical(&compact), Err(DescriptorError::NotCanonical));
        // An unknown key, top level and nested.
        let top = s.replacen("\"files\"", "\"extra\": 1,\n  \"files\"", 1);
        assert!(matches!(Descriptor::parse_canonical(top.as_bytes()), Err(DescriptorError::Parse(_))));
        let nested = s.replacen("\"path\": \"LICENSE\"", "\"path\": \"LICENSE\",\n      \"url\": \"x\"", 1);
        assert!(matches!(Descriptor::parse_canonical(nested.as_bytes()), Err(DescriptorError::Parse(_))));
        // `"palw": null` parses but is not canonical.
        let null = s.replacen("\"path\": \"LICENSE\"", "\"palw\": null,\n      \"path\": \"LICENSE\"", 1);
        assert!(Descriptor::parse_canonical(null.as_bytes()).is_err());
        // Uppercase hex.
        let upper = s.replacen(&"01".repeat(32), &"0A".repeat(32), 1);
        assert!(matches!(Descriptor::parse_canonical(upper.as_bytes()), Err(DescriptorError::Parse(_))));
    }

    #[test]
    fn structural_rules() {
        let mut d = sample();
        d.files.swap(0, 1);
        assert!(matches!(d.validate(), Err(DescriptorError::Unsorted(_))));

        let mut d = sample();
        d.kind = BundleKind::Shard;
        assert_eq!(d.validate(), Err(DescriptorError::ReservedKind(BundleKind::Shard)));

        let mut d = sample();
        d.title = "Qwen".into();
        assert!(matches!(d.validate(), Err(DescriptorError::Title(_))));

        let mut d = sample();
        d.files[1].palw = None;
        assert!(matches!(d.validate(), Err(DescriptorError::PalwPlacement(_))));

        let mut d = sample();
        d.files[1].palw.as_mut().unwrap().roots.clear();
        assert!(matches!(d.validate(), Err(DescriptorError::PalwRoots(_))));

        let mut d = sample();
        d.license.file = "README.md".into();
        assert!(matches!(d.validate(), Err(DescriptorError::LicenseFile(_))));

        let mut d = sample();
        d.files[0].size = 0;
        assert!(matches!(d.validate(), Err(DescriptorError::EmptyFile(_))));
    }

    #[test]
    fn spdx_and_title() {
        for ok in [
            "Apache-2.0",
            "MIT",
            "GPL-2.0+",
            "LicenseRef-qwen",
            "Apache-2.0 OR MIT",
            "GPL-2.0 WITH Classpath-exception-2.0",
        ] {
            assert!(is_valid_spdx(ok), "{ok}");
        }
        for bad in ["", "Apache 2.0", "MIT  OR Apache-2.0", "AND", "MIT OR", "(MIT)", "MIT/Apache"] {
            assert!(!is_valid_spdx(bad), "{bad}");
        }
        assert!(is_valid_title("a"));
        assert!(is_valid_title(&"a".repeat(64)));
        assert!(!is_valid_title(&"a".repeat(65)));
        assert!(!is_valid_title("-a"));
        assert!(!is_valid_title("a/b"));
    }

    #[test]
    fn total_bytes_includes_descriptor() {
        let d = sample();
        assert_eq!(d.total_bytes(), 11357 + 1_795_427_276 + d.to_canonical_bytes().len() as u64);
        assert!(d.torrent_file_names().contains(DESCRIPTOR_FILE_NAME));
    }
}
