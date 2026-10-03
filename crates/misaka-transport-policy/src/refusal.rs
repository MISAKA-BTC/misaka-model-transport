//! Why a bundle was refused before admission.

use misaka_btv2::torrent::TorrentError;
use misaka_bundle::BundleKind;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("the info dictionary is {0} bytes; at most {max}", max = crate::limits::MAX_INFO_BYTES)]
    InfoTooLarge(usize),
    #[error("SHA-256 of the info dictionary is not the infohash")]
    InfohashMismatch,
    #[error("the info dictionary breaks canonical torrent rule v1: {0}")]
    Rule(#[from] TorrentError),
    #[error("the torrent name {0:?} is not a valid bundle title")]
    Title(String),
    #[error("{0} files; at most {max}", max = crate::limits::MAX_FILES)]
    TooManyFiles(usize),
    #[error("file name {name:?}: {why}")]
    Name { name: String, why: &'static str },
    #[error("file names {0:?} and another differ only in case")]
    CaseCollision(String),
    #[error("{name:?} is refused: {why}")]
    RefusedFormat { name: String, why: &'static str },
    #[error("{0:?} is not in the Safe Model Profile")]
    NotAllowed(String),
    #[error("{name:?} is {size} bytes; its class allows at most {max}")]
    FileTooLarge { name: String, size: u64, max: u64 },
    #[error("the bundle has no misaka-bundle.json")]
    NoDescriptor,
    #[error("a {kind} bundle: {why}")]
    Kind { kind: BundleKind, why: String },
    #[error("expected kind {expected}, the bundle holds {found}")]
    KindMismatch { expected: BundleKind, found: BundleKind },
    #[error("kind {0} is reserved")]
    ReservedKind(BundleKind),
    #[error("the bundle is {0} bytes; at most 4 TiB")]
    TotalTooLarge(u64),
    #[error("the bundle is {found} bytes; the declaration says {declared}")]
    TotalMismatch { declared: u64, found: u64 },
    #[error("the bundle needs {needed} bytes of free space; {free} are free")]
    NoSpace { needed: u64, free: u64 },
    #[error("the bundle is {total} bytes; {remaining} remain under the store quota")]
    Quota { total: u64, remaining: u64 },
    #[error("piece layers are {0} bytes; at most 1 MiB")]
    PieceLayersTooLarge(usize),
}
