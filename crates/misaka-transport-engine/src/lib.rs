//! # misaka-transport-engine
//!
//! The [`ModelTransport`] trait (RFC-0001 §6.1): the only way the daemon drives a BitTorrent
//! engine. It takes an [`AdmittedBundle`], which only `misaka-transport-policy` can construct, so
//! nothing reaches an engine without passing the Safe Model Profile.
//!
//! - [`fake`]: an in-memory swarm with honest and hostile peers, for tests of every state of §4.2.
//! - `libtorrent` (feature `libtorrent`): libtorrent-rasterbar 2.0.x through the C ABI of
//!   `shim/libtorrent`. No C++ type crosses it.
//!
//! Engine settings are fixed by the daemon at construction ([`EngineSettings`]), not by IPC
//! callers. The DHT is used for peers only: no BEP 44 puts or gets, no BEP 46.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub use misaka_btv2::torrent::Infohash;
pub use misaka_transport_policy::AdmittedBundle;

pub mod fake;
#[cfg(feature = "libtorrent")]
pub mod libtorrent;

/// An engine's handle on one torrent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BundleHandle(pub u64);

/// How a bundle is added (§6.1): `Fetch` downloads into `save_dir`; `Seed` serves a sealed bundle;
/// `Adopt` serves files obtained elsewhere, in place and read-only (§4.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddMode {
    Fetch,
    Seed,
    Adopt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveFiles {
    Keep,
    Delete,
}

/// `pieces root → concatenated piece-layer hashes`, as in a `.torrent`.
pub type PieceLayers = BTreeMap<[u8; 32], Vec<u8>>;

/// What [`ModelTransport::add`] needs.
#[derive(Debug, Clone)]
pub struct AddRequest<'a> {
    pub bundle: &'a AdmittedBundle,
    pub mode: AddMode,
    /// The directory that holds (or will hold) `<title>/<files>`.
    pub save_dir: &'a Path,
    /// Piece layers when the caller has them; an engine may fetch them from peers otherwise.
    pub piece_layers: Option<&'a PieceLayers>,
}

/// Rates and connection limits. A rate of 0 is unlimited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub upload_bytes_per_sec: u64,
    pub download_bytes_per_sec: u64,
    pub connections: u32,
}

/// Fixed by the daemon at start (§6.3, §4.6); no IPC caller changes these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineSettings {
    pub listen_port: u16,
    pub limits: Limits,
    pub upnp_natpmp: bool,
    pub lsd: bool,
    pub dht: bool,
    /// `host:port` entries.
    pub dht_bootstrap: Vec<String>,
    /// Where the engine keeps its own session state (DHT node id, resume data).
    pub state_dir: PathBuf,
    /// RFC-0002: an I2P mode. The swarm is reached only through this SAM bridge; no clearnet
    /// discovery or peer is used, and every torrent is announced to `trackers`.
    pub i2p: Option<I2pSettings>,
    /// Trackers added to every torrent (outside `info`). In an I2P mode, `.i2p` trackers only.
    pub trackers: Vec<String>,
}

/// The SAM bridge and tunnel shape of an I2P mode (RFC-0002 §2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I2pSettings {
    pub sam_host: String,
    pub sam_port: u16,
    pub inbound_length: u8,
    pub outbound_length: u8,
    pub inbound_quantity: u8,
    pub outbound_quantity: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineState {
    /// Fetching the info dictionary (BEP 9). No file exists.
    Metadata,
    /// Metadata received and handed up; waiting for admission.
    AwaitingAdmission,
    Checking,
    Downloading,
    Finished,
    Seeding,
    Paused,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleStatus {
    pub state: EngineState,
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub uploaded: u64,
    pub downloaded: u64,
    pub upload_rate: u64,
    pub download_rate: u64,
    pub peers: u32,
    pub seeds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineEvent {
    /// The info dictionary arrived and its SHA-256 equals the infohash. The engine has created
    /// nothing; the daemon admits it or removes the handle.
    Metadata {
        handle: BundleHandle,
        info_bytes: Vec<u8>,
    },
    /// Every piece verified (L1) and written.
    Finished {
        handle: BundleHandle,
    },
    /// A piece failed its hash; the peer that sent it was banned and the piece is fetched again.
    HashFailed {
        handle: BundleHandle,
        piece: u64,
    },
    /// A peer was banned (bad blocks, bad metadata, bad hashes).
    PeerBanned {
        handle: BundleHandle,
        reason: String,
    },
    /// No progress and no usable peers.
    Stalled {
        handle: BundleHandle,
    },
    Error {
        handle: BundleHandle,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    #[error("unknown handle")]
    UnknownHandle,
    #[error("the handle is not waiting for admission")]
    NotAwaitingAdmission,
    #[error("the bundle is already in the engine")]
    Duplicate,
    #[error("{0}")]
    Io(String),
    #[error("engine: {0}")]
    Engine(String),
}

/// The engine boundary.
pub trait ModelTransport: Send {
    /// A short name for status output (`fake`, `libtorrent-2.0.x`).
    fn name(&self) -> &str;

    /// Starts fetching the info dictionary of `infohash`. No file is created; a
    /// [`EngineEvent::Metadata`] follows when it arrives.
    fn fetch_metadata(&mut self, infohash: Infohash, save_dir: &Path) -> Result<BundleHandle, EngineError>;

    /// Starts downloading a bundle whose metadata arrived on `handle` and was admitted.
    fn start_download(&mut self, handle: BundleHandle, bundle: &AdmittedBundle) -> Result<(), EngineError>;

    /// Adds a bundle whose info dictionary is already admitted: a resumed fetch, a sealed bundle
    /// to seed, or an adopted one.
    fn add(&mut self, req: AddRequest<'_>) -> Result<BundleHandle, EngineError>;

    fn pause(&mut self, h: BundleHandle) -> Result<(), EngineError>;
    fn resume(&mut self, h: BundleHandle) -> Result<(), EngineError>;
    fn remove(&mut self, h: BundleHandle, files: RemoveFiles) -> Result<(), EngineError>;
    fn set_limits(&mut self, limits: Limits) -> Result<(), EngineError>;
    fn status(&self, h: BundleHandle) -> Result<BundleStatus, EngineError>;

    /// The piece layers the engine holds for a finished bundle, so the daemon can keep the full
    /// `.torrent` and seed it later without re-hashing.
    fn piece_layers(&self, h: BundleHandle) -> Option<PieceLayers>;

    fn poll_events(&mut self, max: usize) -> Vec<EngineEvent>;
}
