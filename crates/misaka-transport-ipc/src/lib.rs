//! # misaka-transport-ipc
//!
//! `misaka-torrent-borsh/v1` (RFC-0001 §6.2): a `u32` little-endian length (≤ 1 MiB), then a
//! borsh message, over a Unix domain socket in a `0700` directory whose peer is checked by
//! `SO_PEERCRED` / `getpeereid`.
//!
//! - Responses never carry file contents.
//! - No request names an absolute path. Writes go to the store; reads come from the store and
//!   from the configured adopt roots, named by id.
//! - The first request of a connection is [`Request::Hello`] with [`PROTOCOL`].

use std::io::{self, Read, Write};

use borsh::{BorshDeserialize, BorshSerialize};

/// The protocol string a [`Request::Hello`] carries and the daemon answers with.
pub const PROTOCOL: &str = "misaka-torrent-borsh/v1";
/// A frame's body is at most 1 MiB.
pub const MAX_FRAME_BYTES: usize = 1 << 20;

pub type Hash32 = [u8; 32];
pub type Hash64 = [u8; 64];

/// What the anchor says; every field optional (an unanchored fetch knows none).
#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Expect {
    pub bundle_commitment: Option<Hash64>,
    pub total_bytes: Option<u64>,
    /// 0 = palw-artifact, 1 = source-weights.
    pub kind: Option<u8>,
}

/// Limits a caller may set. `None` leaves a value as it is; `Some(0)` means unlimited for rates.
#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SetLimits {
    pub upload_bytes_per_sec: Option<u64>,
    pub download_bytes_per_sec: Option<u64>,
    pub connections: Option<u32>,
}

/// The provenance a bundle is shown with (MT-8). The daemon knows only what it was told: it sets
/// `Declared` when an expected commitment matched at L2; the resolver, which runs L3, sets
/// `ChainVerified` with [`Request::SetLabel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
pub enum AnchorLabel {
    Unanchored = 0,
    Declared = 1,
    ChainVerified = 2,
}

impl AnchorLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            AnchorLabel::Unanchored => "unanchored",
            AnchorLabel::Declared => "declared",
            AnchorLabel::ChainVerified => "chain-verified",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Request {
    Hello {
        protocol: String,
        client: String,
    },
    /// Fetch by infohash: metadata, admission, download, L2, seal; then seed if `seed_after` and
    /// the profile allows it.
    Fetch {
        infohash: Hash32,
        expect: Expect,
        seed_after: bool,
    },
    /// Seed a sealed bundle in the store.
    Seed {
        infohash: Hash32,
    },
    /// Seed, in place and read-only, the bundle at `<adopt root id>/<rel_dir>` if its canonical
    /// torrent has this infohash (§4.5). `rel_dir` is relative and contains no `..`.
    Adopt {
        infohash: Hash32,
        adopt_root_id: String,
        rel_dir: String,
    },
    Pause {
        infohash: Hash32,
    },
    Resume {
        infohash: Hash32,
    },
    Remove {
        infohash: Hash32,
        delete_files: bool,
    },
    Status {
        infohash: Option<Hash32>,
    },
    SetLimits(SetLimits),
    SetLabel {
        infohash: Hash32,
        label: AnchorLabel,
    },
    /// Turn this connection into a stream of [`Response::Event`]s.
    Subscribe,
}

impl Request {
    /// Whether a peer admitted read-only (the server's read-only group) may send it.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Request::Hello { .. } | Request::Status { .. } | Request::Subscribe)
    }
}

/// The states of §4.2 that the daemon reports. `Resolving`, `Installed` and `Mismatch(L3)` are
/// the resolver's, and are listed so that one enum names every state a caller shows.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum BundleState {
    Resolving,
    Metadata,
    Admitted,
    Downloading,
    Stalled,
    Verifying,
    Sealed,
    Installed,
    Seeding,
    Idle,
    Paused,
    Refused { reason: String },
    Mismatch { level: u8, reason: String },
    Failed { reason: String },
}

impl BundleState {
    pub fn name(&self) -> &'static str {
        match self {
            BundleState::Resolving => "resolving",
            BundleState::Metadata => "metadata",
            BundleState::Admitted => "admitted",
            BundleState::Downloading => "downloading",
            BundleState::Stalled => "stalled",
            BundleState::Verifying => "verifying",
            BundleState::Sealed => "sealed",
            BundleState::Installed => "installed",
            BundleState::Seeding => "seeding",
            BundleState::Idle => "idle",
            BundleState::Paused => "paused",
            BundleState::Refused { .. } => "refused",
            BundleState::Mismatch { .. } => "mismatch",
            BundleState::Failed { .. } => "failed",
        }
    }

    /// No further progress will happen without a new request.
    pub fn is_terminal_failure(&self) -> bool {
        matches!(self, BundleState::Refused { .. } | BundleState::Mismatch { .. } | BundleState::Failed { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BundleReport {
    pub infohash: Hash32,
    /// Known once the metadata is.
    pub title: Option<String>,
    pub kind: Option<u8>,
    pub state: BundleState,
    pub label: AnchorLabel,
    pub total_bytes: Option<u64>,
    pub done_bytes: u64,
    pub uploaded: u64,
    pub downloaded: u64,
    pub upload_rate: u64,
    pub download_rate: u64,
    pub peers: u32,
    pub seeds: u32,
    pub seeding_enabled: bool,
    /// The sealed bundle's directory (`store/<infohash>/<title>/`) or the adopted one, for the
    /// installer and the verifier. A response may name a path; a request never does.
    pub path: Option<String>,
    pub bundle_commitment: Option<Hash64>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AdoptRootInfo {
    pub id: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct DaemonInfo {
    pub protocol: String,
    pub version: String,
    /// `desktop` or `server`.
    pub profile: String,
    pub seeding_enabled: bool,
    pub read_only: bool,
    pub adopt_roots: Vec<AdoptRootInfo>,
    pub listen_port: u16,
    pub engine: String,
    /// The confinement in force, for `misaka node security-report` (§6.6).
    pub confinement: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
pub enum ErrorCode {
    Protocol = 0,
    Forbidden = 1,
    NotFound = 2,
    Invalid = 3,
    Refused = 4,
    Busy = 5,
    Internal = 6,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Response {
    Hello(DaemonInfo),
    Ok,
    Status(Vec<BundleReport>),
    Event(BundleReport),
    Error { code: ErrorCode, message: String },
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("frame of {0} bytes exceeds {MAX_FRAME_BYTES}")]
    TooLarge(usize),
    #[error("malformed message: {0}")]
    Decode(io::Error),
    #[error("connection closed")]
    Closed,
}

/// Writes one frame.
pub fn write_frame<T: BorshSerialize, W: Write>(w: &mut W, msg: &T) -> Result<(), FrameError> {
    let body = borsh::to_vec(msg)?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(body.len()));
    }
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
    buf.extend_from_slice(&body);
    w.write_all(&buf)?;
    w.flush()?;
    Ok(())
}

/// Reads one frame. The length is checked before the body is allocated; a body that does not
/// decode to exactly one message is refused.
pub fn read_frame<T: BorshDeserialize, R: Read>(r: &mut R) -> Result<T, FrameError> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(FrameError::Closed),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(len));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    borsh::from_slice(&body).map_err(FrameError::Decode)
}

/// Checks a `rel_dir` of [`Request::Adopt`]: relative, `/`-separated, each component a plain
/// name (no `.`, `..` or empty component), at most 1024 bytes.
pub fn is_safe_rel_dir(rel: &str) -> bool {
    !rel.is_empty()
        && rel.len() <= 1024
        && !rel.starts_with('/')
        && !rel.contains('\\')
        && !rel.contains('\0')
        && rel.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

/// A blocking client over the daemon's socket, for `misaka-torrent`, `misaka model …` and
/// Studio.
#[cfg(unix)]
pub mod client {
    use std::os::unix::net::UnixStream;
    use std::path::Path;

    use super::*;

    #[derive(Debug, thiserror::Error)]
    pub enum ClientError {
        #[error("cannot reach misaka-torrentd at {path}: {source}")]
        Connect { path: String, source: io::Error },
        #[error("{0}")]
        Frame(#[from] FrameError),
        #[error("the daemon answered {0:?}")]
        Unexpected(Box<Response>),
        #[error("{code:?}: {message}")]
        Daemon { code: ErrorCode, message: String },
    }

    pub struct Client {
        stream: UnixStream,
        pub info: DaemonInfo,
    }

    impl Client {
        /// Connects and says hello.
        pub fn connect(socket: &Path, client_name: &str) -> Result<Client, ClientError> {
            let mut stream = UnixStream::connect(socket)
                .map_err(|source| ClientError::Connect { path: socket.display().to_string(), source })?;
            write_frame(&mut stream, &Request::Hello { protocol: PROTOCOL.into(), client: client_name.into() })?;
            match read_frame::<Response, _>(&mut stream)? {
                Response::Hello(info) => Ok(Client { stream, info }),
                Response::Error { code, message } => Err(ClientError::Daemon { code, message }),
                other => Err(ClientError::Unexpected(Box::new(other))),
            }
        }

        /// Sends a request and returns the response; a daemon error becomes `Err`.
        pub fn call(&mut self, req: &Request) -> Result<Response, ClientError> {
            write_frame(&mut self.stream, req)?;
            match read_frame::<Response, _>(&mut self.stream)? {
                Response::Error { code, message } => Err(ClientError::Daemon { code, message }),
                r => Ok(r),
            }
        }

        pub fn status(&mut self, infohash: Option<Hash32>) -> Result<Vec<BundleReport>, ClientError> {
            match self.call(&Request::Status { infohash })? {
                Response::Status(v) => Ok(v),
                other => Err(ClientError::Unexpected(Box::new(other))),
            }
        }

        /// Turns the connection into an event stream.
        pub fn subscribe(mut self) -> Result<impl Iterator<Item = BundleReport>, ClientError> {
            self.call(&Request::Subscribe)?;
            let mut s = self.stream;
            Ok(std::iter::from_fn(move || match read_frame::<Response, _>(&mut s) {
                Ok(Response::Event(r)) => Some(r),
                _ => None,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let req = Request::Fetch {
            infohash: [7; 32],
            expect: Expect { bundle_commitment: Some([1; 64]), total_bytes: Some(5), kind: Some(0) },
            seed_after: true,
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &req).unwrap();
        assert_eq!(u32::from_le_bytes(buf[..4].try_into().unwrap()) as usize, buf.len() - 4);
        let back: Request = read_frame(&mut buf.as_slice()).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn refuses_oversized_and_trailing() {
        let mut big = Vec::new();
        big.extend_from_slice(&((MAX_FRAME_BYTES + 1) as u32).to_le_bytes());
        assert!(matches!(read_frame::<Request, _>(&mut big.as_slice()), Err(FrameError::TooLarge(_))));
        let mut buf = Vec::new();
        write_frame(&mut buf, &Request::Subscribe).unwrap();
        // One trailing byte inside the frame.
        buf[0] += 1;
        buf.push(0);
        assert!(matches!(read_frame::<Request, _>(&mut buf.as_slice()), Err(FrameError::Decode(_))));
        assert!(matches!(read_frame::<Request, _>(&mut [].as_slice()), Err(FrameError::Closed)));
    }

    #[test]
    fn rel_dirs() {
        for ok in ["qwen", "a/b", "hf/Qwen3.6-35B"] {
            assert!(is_safe_rel_dir(ok), "{ok}");
        }
        for bad in ["", "/etc", "../x", "a/../b", "a//b", "./a", "a/", "a\\b"] {
            assert!(!is_safe_rel_dir(bad), "{bad}");
        }
    }
}
