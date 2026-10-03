//! The canonical torrent, rule v1 (RFC-0001 §2.3).
//!
//! 1. `meta version` is 2; v2-only: no `pieces`, no padding files.
//! 2. `name` is the descriptor's `title`.
//! 3. `piece length` is `P(T) = clamp(2^⌈log2 ⌈T / 4096⌉⌉, 1 MiB, 16 MiB)`.
//! 4. `file tree` holds non-empty files at depth 1, with no `attr` and no `symlink path`.
//! 5. The info dictionary holds those four keys and no other.
//! 6. Outside `info`: `piece layers` for every file longer than `P`; `announce`,
//!    `announce-list` and `url-list` optional; no `creation date`, `created by` or `comment`.
//! 7. `btv2_infohash = SHA-256(bencode(info))`.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use sha2::{Digest, Sha256};

use crate::bencode::{self, DecodeError, Limits, Value};
use crate::merkle::{FileDigest, Hash, root_from_piece_layer};

pub const MIN_PIECE_LENGTH: u64 = 1 << 20;
pub const MAX_PIECE_LENGTH: u64 = 16 << 20;
/// Rule v1 aims at about this many pieces.
pub const TARGET_PIECES: u64 = 4096;

/// `P(T)`.
pub fn piece_length_for(total_bytes: u64) -> u64 {
    total_bytes.div_ceil(TARGET_PIECES).next_power_of_two().clamp(MIN_PIECE_LENGTH, MAX_PIECE_LENGTH)
}

/// A v2 infohash: the SHA-256 of the bencoded info dictionary. The wire truncates it to 20 bytes
/// where it needs 20; this is always the full 32.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Infohash(pub [u8; 32]);

impl Infohash {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The first 20 bytes, as the wire's v1-sized fields carry a v2 hash.
    pub fn truncated(&self) -> [u8; 20] {
        self.0[..20].try_into().expect("32 ≥ 20")
    }

    /// `9f2c…e1`, for people.
    pub fn short(&self) -> String {
        let h = self.to_string();
        format!("{}…{}", &h[..4], &h[62..])
    }
}

impl fmt::Display for Infohash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

impl fmt::Debug for Infohash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Infohash({self})")
    }
}

impl FromStr for Infohash {
    type Err = TorrentError;
    fn from_str(s: &str) -> Result<Self, TorrentError> {
        if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(TorrentError::InfohashSyntax);
        }
        let mut out = [0u8; 32];
        hex::decode_to_slice(s, &mut out).map_err(|_| TorrentError::InfohashSyntax)?;
        Ok(Infohash(out))
    }
}

/// `SHA-256(info_bytes)`.
pub fn infohash(info_bytes: &[u8]) -> Infohash {
    Infohash(Sha256::digest(info_bytes).into())
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TorrentError {
    #[error("bencode: {0}")]
    Bencode(#[from] DecodeError),
    #[error("not a dictionary: {0}")]
    NotDict(&'static str),
    #[error("info dictionary key {0:?} is not allowed by rule v1")]
    InfoKey(String),
    #[error("info dictionary lacks {0:?}")]
    MissingKey(&'static str),
    #[error("meta version is not 2")]
    MetaVersion,
    #[error("name is not a non-empty UTF-8 string")]
    Name,
    #[error("piece length {got} is not P(T) = {want} for T = {total}")]
    PieceLength { got: i64, want: u64, total: u64 },
    #[error("file tree is empty")]
    NoFiles,
    #[error("file tree entry {0:?} is a directory; rule v1 is depth 1")]
    Directory(String),
    #[error("file tree entry {0:?} is not a UTF-8 name")]
    FileName(String),
    #[error("file {name:?}: key {key:?} is not allowed (no attr, no symlink path)")]
    FileKey { name: String, key: String },
    #[error("file {0:?}: length is missing, zero or negative")]
    Length(String),
    #[error("file {0:?}: pieces root is missing or not 32 bytes")]
    PiecesRoot(String),
    #[error("total size overflows")]
    Overflow,
    #[error("torrent key {0:?} is not allowed")]
    TorrentKey(String),
    #[error("torrent lacks `info`")]
    NoInfo,
    #[error("piece layers: {0}")]
    PieceLayers(String),
    #[error("{0} is malformed")]
    Field(&'static str),
    #[error("infohash is not 64 lowercase hex digits")]
    InfohashSyntax,
}

/// One file of a rule-v1 info dictionary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoFile {
    pub name: String,
    pub length: u64,
    pub pieces_root: Hash,
}

/// A rule-v1 info dictionary. Files are sorted by name as raw bytes, which is bencode's order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoDict {
    pub name: String,
    pub piece_length: u64,
    pub files: Vec<InfoFile>,
}

impl InfoDict {
    /// The rule-v1 dictionary for `name` and `files`; `piece length` follows from their sizes.
    pub fn new(name: impl Into<String>, mut files: Vec<InfoFile>) -> Self {
        files.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
        let total: u64 = files.iter().map(|f| f.length).sum();
        InfoDict { name: name.into(), piece_length: piece_length_for(total), files }
    }

    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.length).sum()
    }

    pub fn file(&self, name: &str) -> Option<&InfoFile> {
        self.files.iter().find(|f| f.name == name)
    }

    pub fn to_value(&self) -> Value {
        let tree = Value::dict(self.files.iter().map(|f| {
            let leaf = Value::dict([
                ("length", Value::Int(f.length as i64)),
                ("pieces root", Value::bytes(f.pieces_root.to_vec())),
            ]);
            (f.name.as_bytes().to_vec(), Value::dict([("", leaf)]))
        }));
        Value::dict([
            ("file tree", tree),
            ("meta version", Value::Int(2)),
            ("name", Value::bytes(self.name.as_bytes())),
            ("piece length", Value::Int(self.piece_length as i64)),
        ])
    }

    pub fn encode(&self) -> Vec<u8> {
        bencode::encode(&self.to_value())
    }

    pub fn infohash(&self) -> Infohash {
        infohash(&self.encode())
    }

    /// Parses `bytes` and checks every clause of rule v1. Accepts exactly what [`Self::encode`]
    /// writes.
    pub fn parse_canonical(bytes: &[u8], limits: Limits) -> Result<Self, TorrentError> {
        let v = bencode::decode(bytes, limits)?;
        Self::from_value(&v)
    }

    fn from_value(v: &Value) -> Result<Self, TorrentError> {
        let d = v.as_dict().ok_or(TorrentError::NotDict("info"))?;
        for k in d.keys() {
            if !matches!(k.as_slice(), b"file tree" | b"meta version" | b"name" | b"piece length") {
                return Err(TorrentError::InfoKey(String::from_utf8_lossy(k).into_owned()));
            }
        }
        let get = |k: &'static str| d.get(k.as_bytes()).ok_or(TorrentError::MissingKey(k));
        if get("meta version")?.as_int() != Some(2) {
            return Err(TorrentError::MetaVersion);
        }
        let name = get("name")?.as_str().filter(|s| !s.is_empty()).ok_or(TorrentError::Name)?.to_owned();
        let tree = get("file tree")?.as_dict().ok_or(TorrentError::NotDict("file tree"))?;
        if tree.is_empty() {
            return Err(TorrentError::NoFiles);
        }
        let mut files = Vec::with_capacity(tree.len());
        for (k, entry) in tree {
            let lossy = String::from_utf8_lossy(k).into_owned();
            let fname = std::str::from_utf8(k)
                .ok()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| TorrentError::FileName(lossy.clone()))?;
            let entry = entry.as_dict().ok_or(TorrentError::NotDict("file tree entry"))?;
            let leaf = match (entry.len(), entry.get(b"".as_slice())) {
                (1, Some(leaf)) => leaf.as_dict().ok_or(TorrentError::NotDict("file"))?,
                _ => return Err(TorrentError::Directory(lossy)),
            };
            for key in leaf.keys() {
                if !matches!(key.as_slice(), b"length" | b"pieces root") {
                    return Err(TorrentError::FileKey { name: lossy, key: String::from_utf8_lossy(key).into_owned() });
                }
            }
            let length = leaf
                .get(b"length".as_slice())
                .and_then(Value::as_int)
                .filter(|&l| l > 0)
                .ok_or_else(|| TorrentError::Length(lossy.clone()))? as u64;
            let pieces_root: Hash = leaf
                .get(b"pieces root".as_slice())
                .and_then(Value::as_bytes)
                .and_then(|b| b.try_into().ok())
                .ok_or_else(|| TorrentError::PiecesRoot(lossy.clone()))?;
            files.push(InfoFile { name: fname.to_owned(), length, pieces_root });
        }
        let total = files.iter().try_fold(0u64, |acc, f| acc.checked_add(f.length)).ok_or(TorrentError::Overflow)?;
        let got = get("piece length")?.as_int().ok_or(TorrentError::Field("piece length"))?;
        let want = piece_length_for(total);
        if got != want as i64 {
            return Err(TorrentError::PieceLength { got, want, total });
        }
        Ok(InfoDict { name, piece_length: want, files })
    }
}

/// A `.torrent` file: the info dictionary plus what lies outside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Torrent {
    pub info: InfoDict,
    /// `pieces root → concatenated piece-layer hashes`, for every file longer than `P`.
    pub piece_layers: BTreeMap<Hash, Vec<u8>>,
    pub announce: Option<String>,
    pub announce_list: Vec<Vec<String>>,
    /// BEP 19 web seeds.
    pub url_list: Vec<String>,
}

impl Torrent {
    /// Builds the canonical torrent from file digests. `files` pairs a name with its digest.
    pub fn build(name: impl Into<String>, files: &[(String, FileDigest)]) -> Self {
        let info = InfoDict::new(
            name,
            files
                .iter()
                .map(|(n, d)| InfoFile { name: n.clone(), length: d.size, pieces_root: d.pieces_root })
                .collect(),
        );
        let mut piece_layers = BTreeMap::new();
        for (_, d) in files {
            if let Some(layer) = d.piece_layer(info.piece_length) {
                piece_layers.insert(d.pieces_root, layer.concat());
            }
        }
        Torrent { info, piece_layers, announce: None, announce_list: Vec::new(), url_list: Vec::new() }
    }

    pub fn infohash(&self) -> Infohash {
        self.info.infohash()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut top = BTreeMap::new();
        top.insert(b"info".to_vec(), self.info.to_value());
        top.insert(
            b"piece layers".to_vec(),
            Value::Dict(self.piece_layers.iter().map(|(k, v)| (k.to_vec(), Value::Bytes(v.clone()))).collect()),
        );
        if let Some(a) = &self.announce {
            top.insert(b"announce".to_vec(), Value::bytes(a.as_bytes()));
        }
        if !self.announce_list.is_empty() {
            let tiers = self
                .announce_list
                .iter()
                .map(|tier| Value::List(tier.iter().map(|u| Value::bytes(u.as_bytes())).collect()))
                .collect();
            top.insert(b"announce-list".to_vec(), Value::List(tiers));
        }
        if !self.url_list.is_empty() {
            top.insert(
                b"url-list".to_vec(),
                Value::List(self.url_list.iter().map(|u| Value::bytes(u.as_bytes())).collect()),
            );
        }
        bencode::encode(&Value::Dict(top))
    }

    /// Parses a `.torrent`: rule v1 for `info`, and `piece layers` checked against every pieces
    /// root. Keys a client may add outside `info` and that carry no identity (`creation date`,
    /// `created by`, `comment`, `encoding`, `nodes`) are ignored; any other is refused.
    pub fn parse(bytes: &[u8], limits: Limits) -> Result<Self, TorrentError> {
        let v = bencode::decode(bytes, limits)?;
        let top = v.as_dict().ok_or(TorrentError::NotDict("torrent"))?;
        let mut t = Torrent {
            info: InfoDict::from_value(top.get(b"info".as_slice()).ok_or(TorrentError::NoInfo)?)?,
            piece_layers: BTreeMap::new(),
            announce: None,
            announce_list: Vec::new(),
            url_list: Vec::new(),
        };
        let strings = |v: &Value, what: &'static str| -> Result<Vec<String>, TorrentError> {
            v.as_list()
                .ok_or(TorrentError::Field(what))?
                .iter()
                .map(|s| s.as_str().map(str::to_owned).ok_or(TorrentError::Field(what)))
                .collect()
        };
        for (k, val) in top {
            match k.as_slice() {
                b"info" => {}
                b"piece layers" => {
                    for (root, layer) in val.as_dict().ok_or(TorrentError::NotDict("piece layers"))? {
                        let root: Hash = root
                            .as_slice()
                            .try_into()
                            .map_err(|_| TorrentError::PieceLayers("a key is not 32 bytes".into()))?;
                        let layer = layer.as_bytes().ok_or(TorrentError::Field("piece layers"))?;
                        t.piece_layers.insert(root, layer.to_vec());
                    }
                }
                b"announce" => t.announce = Some(val.as_str().ok_or(TorrentError::Field("announce"))?.to_owned()),
                b"announce-list" => {
                    t.announce_list = val
                        .as_list()
                        .ok_or(TorrentError::Field("announce-list"))?
                        .iter()
                        .map(|tier| strings(tier, "announce-list"))
                        .collect::<Result<_, _>>()?
                }
                b"url-list" => {
                    t.url_list = match val {
                        Value::Bytes(_) => vec![val.as_str().ok_or(TorrentError::Field("url-list"))?.to_owned()],
                        _ => strings(val, "url-list")?,
                    }
                }
                b"creation date" | b"created by" | b"comment" | b"encoding" | b"nodes" => {}
                other => return Err(TorrentError::TorrentKey(String::from_utf8_lossy(other).into_owned())),
            }
        }
        t.check_piece_layers()?;
        Ok(t)
    }

    /// Every file longer than `P` has a layer of `⌈length / P⌉` hashes implying its pieces root;
    /// no other layer is present.
    pub fn check_piece_layers(&self) -> Result<(), TorrentError> {
        let p = self.info.piece_length;
        for f in &self.info.files {
            if f.length <= p {
                continue;
            }
            let layer = self
                .piece_layers
                .get(&f.pieces_root)
                .ok_or_else(|| TorrentError::PieceLayers(format!("missing for {:?}", f.name)))?;
            let pieces = f.length.div_ceil(p) as usize;
            if layer.len() != 32 * pieces {
                return Err(TorrentError::PieceLayers(format!(
                    "{:?} has {} bytes, expected {}",
                    f.name,
                    layer.len(),
                    32 * pieces
                )));
            }
            let nodes: Vec<Hash> = layer.chunks(32).map(|c| c.try_into().expect("32")).collect();
            if root_from_piece_layer(&nodes, p) != f.pieces_root {
                return Err(TorrentError::PieceLayers(format!("{:?} does not imply its pieces root", f.name)));
            }
        }
        // Identical files share a root and a layer.
        let distinct: std::collections::BTreeSet<_> =
            self.info.files.iter().filter(|f| f.length > p).map(|f| f.pieces_root).collect();
        if self.piece_layers.len() != distinct.len() {
            return Err(TorrentError::PieceLayers(format!(
                "{} layers, expected {}",
                self.piece_layers.len(),
                distinct.len()
            )));
        }
        Ok(())
    }

    /// The piece-layer hashes of `file`, if it is longer than a piece.
    pub fn layer_of(&self, file: &InfoFile) -> Option<Vec<Hash>> {
        self.piece_layers.get(&file.pieces_root).map(|l| l.chunks(32).map(|c| c.try_into().expect("32")).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merkle::FileHasher;

    #[test]
    fn piece_rule_matches_the_rfc_table() {
        let mib = 1u64 << 20;
        assert_eq!(piece_length_for(0), mib);
        assert_eq!(piece_length_for(1_795_427_276), mib);
        assert_eq!(piece_length_for(23_938_321_728), 8 * mib);
        assert_eq!(piece_length_for(36_492_831_232), 16 * mib);
        assert_eq!(piece_length_for(38_639_790_592), 16 * mib);
        assert_eq!(piece_length_for(322_122_547_200), 16 * mib);
        // Pieces of the large file, as the table counts them.
        assert_eq!(1_795_427_276u64.div_ceil(mib), 1_713);
        assert_eq!(23_938_321_728u64.div_ceil(8 * mib), 2_854);
        assert_eq!(36_492_831_232u64.div_ceil(16 * mib), 2_176);
        assert_eq!(322_122_547_200u64.div_ceil(16 * mib), 19_200);
        // Exactly 4096 MiB stays at 1 MiB; one byte more doubles.
        assert_eq!(piece_length_for(4096 * mib), mib);
        assert_eq!(piece_length_for(4096 * mib + 1), 2 * mib);
    }

    fn digest(bytes: &[u8]) -> FileDigest {
        let mut h = FileHasher::new();
        h.update(bytes);
        h.finish()
    }

    fn sample() -> Torrent {
        let big = vec![7u8; (3 << 20) + 5];
        Torrent::build(
            "sample",
            &[("b.gguf".to_string(), digest(&big)), ("LICENSE".to_string(), digest(b"license text"))],
        )
    }

    #[test]
    fn info_round_trip_and_order() {
        let t = sample();
        let bytes = t.info.encode();
        assert_eq!(InfoDict::parse_canonical(&bytes, Limits::default()).unwrap(), t.info);
        // `LICENSE` sorts before `b.gguf` as raw bytes.
        assert_eq!(t.info.files[0].name, "LICENSE");
        assert!(bytes.starts_with(b"d9:file treed7:LICENSEd0:d6:lengthi12e11:pieces root32:"));
    }

    #[test]
    fn torrent_round_trip() {
        let mut t = sample();
        t.url_list = vec!["https://example.invalid/x".into()];
        let bytes = t.encode();
        let back = Torrent::parse(&bytes, Limits::default()).unwrap();
        assert_eq!(back, t);
        assert_eq!(back.piece_layers.len(), 1);
    }

    fn info_with(f: impl FnOnce(&mut BTreeMap<Vec<u8>, Value>)) -> Vec<u8> {
        let mut v = sample().info.to_value();
        if let Value::Dict(d) = &mut v {
            f(d);
        }
        bencode::encode(&v)
    }

    fn file_leaf(d: &mut BTreeMap<Vec<u8>, Value>) -> &mut BTreeMap<Vec<u8>, Value> {
        let Value::Dict(tree) = d.get_mut(b"file tree".as_slice()).unwrap() else { panic!() };
        let Value::Dict(entry) = tree.get_mut(b"LICENSE".as_slice()).unwrap() else { panic!() };
        let Value::Dict(leaf) = entry.get_mut(b"".as_slice()).unwrap() else { panic!() };
        leaf
    }

    #[test]
    fn refuses_what_rule_v1_refuses() {
        let l = Limits::default();
        let cases: Vec<(Vec<u8>, &str)> = vec![
            (
                info_with(|d| {
                    d.insert(b"private".to_vec(), Value::Int(1));
                }),
                "private",
            ),
            (
                info_with(|d| {
                    d.insert(b"pieces".to_vec(), Value::bytes(vec![0; 20]));
                }),
                "hybrid",
            ),
            (
                info_with(|d| {
                    d.insert(b"meta version".to_vec(), Value::Int(1));
                }),
                "v1",
            ),
            (
                info_with(|d| {
                    d.insert(b"piece length".to_vec(), Value::Int(1 << 21));
                }),
                "piece length",
            ),
            (
                info_with(|d| {
                    file_leaf(d).insert(b"attr".to_vec(), Value::bytes("x"));
                }),
                "attr",
            ),
            (
                info_with(|d| {
                    file_leaf(d).insert(b"symlink path".to_vec(), Value::List(vec![]));
                }),
                "symlink",
            ),
            (
                info_with(|d| {
                    file_leaf(d).insert(b"length".to_vec(), Value::Int(0));
                }),
                "empty",
            ),
            (
                info_with(|d| {
                    file_leaf(d).insert(b"pieces root".to_vec(), Value::bytes(vec![0; 31]));
                }),
                "root",
            ),
            (
                info_with(|d| {
                    let Value::Dict(tree) = d.get_mut(b"file tree".as_slice()).unwrap() else { panic!() };
                    let file = tree.remove(b"LICENSE".as_slice()).unwrap();
                    tree.insert(b"sub".to_vec(), Value::dict([("LICENSE", file)]));
                }),
                "directory",
            ),
        ];
        for (bytes, what) in cases {
            assert!(InfoDict::parse_canonical(&bytes, l).is_err(), "{what}");
        }
    }

    #[test]
    fn refuses_bad_piece_layers() {
        let mut t = sample();
        let root = *t.piece_layers.keys().next().unwrap();
        t.piece_layers.get_mut(&root).unwrap()[0] ^= 1;
        assert!(matches!(Torrent::parse(&t.encode(), Limits::default()), Err(TorrentError::PieceLayers(_))));
        let mut t = sample();
        t.piece_layers.clear();
        assert!(matches!(Torrent::parse(&t.encode(), Limits::default()), Err(TorrentError::PieceLayers(_))));
    }

    #[test]
    fn infohash_hex() {
        let h = sample().infohash();
        assert_eq!(h.to_string().parse::<Infohash>().unwrap(), h);
        assert!(h.to_string().to_uppercase().parse::<Infohash>().is_err());
    }
}
