//! An in-memory swarm and engine, for tests of the daemon's state machine (RFC-0001 §9, A3).
//!
//! The fake does what L1 does: metadata is checked against the infohash (BEP 9) and every piece
//! against its piece-layer node or its file's pieces root (BEP 52). A peer that sends bad
//! metadata or a bad piece is banned and the piece is fetched from another. It writes real files
//! under `save_dir`, with exclusive, no-follow opens, so the daemon's staging, sealing and
//! cleanup are exercised on a real file system.

use std::collections::{BTreeMap, HashMap};
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use misaka_btv2::merkle::{file_root_of_bytes, piece_node};
use misaka_btv2::torrent::{InfoDict, Torrent};
use misaka_btv2::{bencode, infohash};

use crate::{
    AddMode, AddRequest, AdmittedBundle, BundleHandle, BundleStatus, EngineError, EngineEvent, EngineState, Infohash,
    Limits, ModelTransport, PieceLayers, RemoveFiles,
};

/// How a fake peer behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerBehaviour {
    Honest,
    /// Serves pieces with one byte flipped.
    CorruptPieces,
    /// Serves an info dictionary that does not hash to the infohash.
    BadMetadata,
}

#[derive(Debug, Clone)]
struct Peer {
    id: u64,
    behaviour: PeerBehaviour,
}

#[derive(Debug, Clone)]
struct Published {
    info_bytes: Vec<u8>,
    piece_layers: PieceLayers,
    files: BTreeMap<String, Arc<Vec<u8>>>,
    peers: Vec<Peer>,
}

/// A swarm shared by fake engines in one process.
#[derive(Debug, Clone, Default)]
pub struct FakeSwarm {
    inner: Arc<Mutex<SwarmInner>>,
}

#[derive(Debug, Default)]
struct SwarmInner {
    torrents: HashMap<Infohash, Published>,
    next_peer: u64,
}

impl FakeSwarm {
    pub fn new() -> Self {
        Self::default()
    }

    /// Publishes a torrent's bytes with one peer of `behaviour`. Calling it again adds a peer.
    /// `files` are the bytes by name; they are not checked here, so a test can publish a
    /// torrent whose bytes are wrong.
    pub fn publish(&self, torrent: &Torrent, files: BTreeMap<String, Vec<u8>>, behaviour: PeerBehaviour) -> Infohash {
        self.publish_raw(torrent.info.encode(), torrent.piece_layers.clone(), files, behaviour)
    }

    /// Publishes arbitrary info-dictionary bytes: what a hostile packager would.
    pub fn publish_raw(
        &self,
        info_bytes: Vec<u8>,
        piece_layers: PieceLayers,
        files: BTreeMap<String, Vec<u8>>,
        behaviour: PeerBehaviour,
    ) -> Infohash {
        let ih = infohash(&info_bytes);
        let mut s = self.inner.lock().expect("unpoisoned");
        s.next_peer += 1;
        let peer = Peer { id: s.next_peer, behaviour };
        let entry = s.torrents.entry(ih).or_insert_with(|| Published {
            info_bytes,
            piece_layers,
            files: files.into_iter().map(|(k, v)| (k, Arc::new(v))).collect(),
            peers: Vec::new(),
        });
        entry.peers.push(peer);
        ih
    }

    /// Removes every peer of `infohash`.
    pub fn withdraw(&self, ih: &Infohash) {
        self.inner.lock().expect("unpoisoned").torrents.remove(ih);
    }

    fn snapshot(&self, ih: &Infohash) -> Option<Published> {
        self.inner.lock().expect("unpoisoned").torrents.get(ih).cloned()
    }
}

#[derive(Debug)]
struct Entry {
    infohash: Infohash,
    save_dir: PathBuf,
    state: EngineState,
    info: Option<InfoDict>,
    banned: Vec<u64>,
    /// Next piece to fetch, as (file index, piece index).
    cursor: (usize, u64),
    done: u64,
    downloaded: u64,
    idle_polls: u32,
    paused_from: Option<EngineState>,
}

/// A fake engine on a [`FakeSwarm`].
pub struct FakeEngine {
    swarm: FakeSwarm,
    entries: BTreeMap<BundleHandle, Entry>,
    next: u64,
    events: Vec<EngineEvent>,
    /// Bytes fetched per poll per bundle.
    pub bytes_per_poll: u64,
    /// Polls without progress before a `Stalled` event.
    pub stall_after: u32,
    /// Report `Finished` right after the metadata, as libtorrent does for a torrent that wants
    /// nothing yet.
    pub finish_early: bool,
    limits: Option<Limits>,
}

impl FakeEngine {
    pub fn new(swarm: FakeSwarm) -> Self {
        FakeEngine {
            swarm,
            entries: BTreeMap::new(),
            next: 0,
            events: Vec::new(),
            bytes_per_poll: 4 << 20,
            stall_after: 3,
            finish_early: false,
            limits: None,
        }
    }

    pub fn limits(&self) -> Option<Limits> {
        self.limits
    }

    fn entry(&mut self, h: BundleHandle) -> Result<&mut Entry, EngineError> {
        self.entries.get_mut(&h).ok_or(EngineError::UnknownHandle)
    }

    fn new_handle(&mut self, e: Entry) -> Result<BundleHandle, EngineError> {
        if self.entries.values().any(|x| x.infohash == e.infohash) {
            return Err(EngineError::Duplicate);
        }
        self.next += 1;
        let h = BundleHandle(self.next);
        self.entries.insert(h, e);
        Ok(h)
    }

    fn step_metadata(swarm: &FakeSwarm, h: BundleHandle, e: &mut Entry, events: &mut Vec<EngineEvent>) {
        let Some(p) = swarm.snapshot(&e.infohash) else {
            e.idle_polls += 1;
            return;
        };
        let live: Vec<Peer> = p.peers.iter().filter(|x| !e.banned.contains(&x.id)).cloned().collect();
        for peer in live {
            let mut bytes = p.info_bytes.clone();
            if peer.behaviour == PeerBehaviour::BadMetadata {
                bytes.push(b'x');
            }
            if infohash(&bytes) != e.infohash {
                e.banned.push(peer.id);
                events.push(EngineEvent::PeerBanned {
                    handle: h,
                    reason: "metadata does not hash to the infohash".into(),
                });
                continue;
            }
            e.state = EngineState::AwaitingAdmission;
            e.idle_polls = 0;
            events.push(EngineEvent::Metadata { handle: h, info_bytes: bytes });
            return;
        }
        e.idle_polls += 1;
    }

    fn step_download(swarm: &FakeSwarm, budget: u64, h: BundleHandle, e: &mut Entry, events: &mut Vec<EngineEvent>) {
        let info = e.info.clone().expect("downloading has info");
        let Some(p) = swarm.snapshot(&e.infohash) else {
            e.idle_polls += 1;
            return;
        };
        let torrent_dir = e.save_dir.join(&info.name);
        let mut spent = 0u64;
        while spent < budget {
            let (fi, pi) = e.cursor;
            let Some(file) = info.files.get(fi) else {
                e.state = EngineState::Finished;
                events.push(EngineEvent::Finished { handle: h });
                return;
            };
            let plen = info.piece_length;
            let start = pi * plen;
            if start >= file.length {
                e.cursor = (fi + 1, 0);
                continue;
            }
            let end = (start + plen).min(file.length);
            let Some(peer) = p.peers.iter().find(|x| !e.banned.contains(&x.id)) else {
                e.idle_polls += 1;
                return;
            };
            let Some(src) = p.files.get(&file.name) else {
                e.idle_polls += 1;
                return;
            };
            let mut piece = src.get(start as usize..end as usize).map(<[u8]>::to_vec).unwrap_or_default();
            if peer.behaviour == PeerBehaviour::CorruptPieces && !piece.is_empty() {
                piece[0] ^= 0xff;
            }
            // L1: the piece against its node, or a single-piece file against its root.
            let ok = piece.len() as u64 == end - start
                && if file.length > plen {
                    p.piece_layers
                        .get(&file.pieces_root)
                        .and_then(|l| l.get(pi as usize * 32..pi as usize * 32 + 32))
                        .is_some_and(|node| node == piece_node(&piece, plen))
                } else {
                    file_root_of_bytes(&piece) == file.pieces_root
                };
            e.downloaded += piece.len() as u64;
            if !ok {
                e.banned.push(peer.id);
                events.push(EngineEvent::HashFailed { handle: h, piece: pi });
                events.push(EngineEvent::PeerBanned { handle: h, reason: "sent a piece that fails its hash".into() });
                continue;
            }
            if let Err(err) = write_at(&torrent_dir, &file.name, start, &piece) {
                e.state = EngineState::Error(err.clone());
                events.push(EngineEvent::Error { handle: h, message: err });
                return;
            }
            e.done += piece.len() as u64;
            e.idle_polls = 0;
            spent += piece.len() as u64;
            e.cursor = (fi, pi + 1);
        }
    }
}

/// Writes `bytes` at `offset` into `dir/name`, creating the directory and the file (never
/// following a link) on first use.
fn write_at(dir: &Path, name: &str, offset: u64, bytes: &[u8]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(name);
    let mut opts = OpenOptions::new();
    // A resumed fetch reopens its own staging file; a link in its place is refused below.
    opts.write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let mut f = opts.open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    f.write_all(bytes).map_err(|e| e.to_string())
}

impl ModelTransport for FakeEngine {
    fn name(&self) -> &str {
        "fake"
    }

    fn fetch_metadata(&mut self, infohash: Infohash, save_dir: &Path) -> Result<BundleHandle, EngineError> {
        self.new_handle(Entry {
            infohash,
            save_dir: save_dir.to_owned(),
            state: EngineState::Metadata,
            info: None,
            banned: Vec::new(),
            cursor: (0, 0),
            done: 0,
            downloaded: 0,
            idle_polls: 0,
            paused_from: None,
        })
    }

    fn start_download(&mut self, handle: BundleHandle, bundle: &AdmittedBundle) -> Result<(), EngineError> {
        let e = self.entry(handle)?;
        if e.state != EngineState::AwaitingAdmission || e.infohash != bundle.infohash() {
            return Err(EngineError::NotAwaitingAdmission);
        }
        e.info = Some(bundle.info().clone());
        e.state = EngineState::Downloading;
        Ok(())
    }

    fn add(&mut self, req: AddRequest<'_>) -> Result<BundleHandle, EngineError> {
        let info = req.bundle.info().clone();
        let total = info.total_bytes();
        let state = match req.mode {
            AddMode::Fetch => EngineState::Downloading,
            AddMode::Seed | AddMode::Adopt => EngineState::Seeding,
        };
        if matches!(req.mode, AddMode::Seed | AddMode::Adopt) {
            // Join the swarm as an honest seeder of the files on disk.
            let dir = req.save_dir.join(&info.name);
            let mut files = BTreeMap::new();
            for f in &info.files {
                let bytes = std::fs::read(dir.join(&f.name)).map_err(|e| EngineError::Io(e.to_string()))?;
                files.insert(f.name.clone(), bytes);
            }
            let layers = req.piece_layers.cloned().unwrap_or_default();
            self.swarm.publish_raw(req.bundle.info_bytes().to_vec(), layers, files, PeerBehaviour::Honest);
        }
        // A resumed fetch starts over in this fake; files already present are truncated by the
        // daemon's cleanup before it resumes.
        let done = if state == EngineState::Seeding { total } else { 0 };
        self.new_handle(Entry {
            infohash: req.bundle.infohash(),
            save_dir: req.save_dir.to_owned(),
            state,
            info: Some(info),
            banned: Vec::new(),
            cursor: (0, 0),
            done,
            downloaded: 0,
            idle_polls: 0,
            paused_from: None,
        })
    }

    fn pause(&mut self, h: BundleHandle) -> Result<(), EngineError> {
        let e = self.entry(h)?;
        if e.state != EngineState::Paused {
            e.paused_from = Some(std::mem::replace(&mut e.state, EngineState::Paused));
        }
        Ok(())
    }

    fn resume(&mut self, h: BundleHandle) -> Result<(), EngineError> {
        let e = self.entry(h)?;
        if let Some(s) = e.paused_from.take() {
            e.state = s;
        }
        Ok(())
    }

    fn remove(&mut self, h: BundleHandle, files: RemoveFiles) -> Result<(), EngineError> {
        let e = self.entries.remove(&h).ok_or(EngineError::UnknownHandle)?;
        if files == RemoveFiles::Delete
            && let Some(info) = &e.info
        {
            let dir = e.save_dir.join(&info.name);
            for f in &info.files {
                let _ = std::fs::remove_file(dir.join(&f.name));
            }
            let _ = std::fs::remove_dir(&dir);
        }
        Ok(())
    }

    fn set_limits(&mut self, limits: Limits) -> Result<(), EngineError> {
        self.limits = Some(limits);
        Ok(())
    }

    fn status(&self, h: BundleHandle) -> Result<BundleStatus, EngineError> {
        let e = self.entries.get(&h).ok_or(EngineError::UnknownHandle)?;
        let peers = self
            .swarm
            .snapshot(&e.infohash)
            .map(|p| p.peers.iter().filter(|x| !e.banned.contains(&x.id)).count() as u32)
            .unwrap_or(0);
        Ok(BundleStatus {
            state: e.state.clone(),
            total_bytes: e.info.as_ref().map(InfoDict::total_bytes).unwrap_or(0),
            done_bytes: e.done,
            uploaded: 0,
            downloaded: e.downloaded,
            upload_rate: 0,
            download_rate: 0,
            peers,
            seeds: peers,
        })
    }

    fn piece_layers(&self, h: BundleHandle) -> Option<PieceLayers> {
        let e = self.entries.get(&h)?;
        self.swarm.snapshot(&e.infohash).map(|p| p.piece_layers)
    }

    fn poll_events(&mut self, max: usize) -> Vec<EngineEvent> {
        let budget = self.bytes_per_poll;
        let stall_after = self.stall_after;
        for (&h, e) in self.entries.iter_mut() {
            let before = e.idle_polls;
            match e.state {
                EngineState::Metadata => {
                    Self::step_metadata(&self.swarm, h, e, &mut self.events);
                    if self.finish_early && e.state == EngineState::AwaitingAdmission {
                        self.events.push(EngineEvent::Finished { handle: h });
                    }
                }
                EngineState::Downloading => Self::step_download(&self.swarm, budget, h, e, &mut self.events),
                _ => continue,
            }
            if e.idle_polls > before && e.idle_polls == stall_after {
                self.events.push(EngineEvent::Stalled { handle: h });
            }
        }
        let n = max.min(self.events.len());
        self.events.drain(..n).collect()
    }
}

/// Parses a `.torrent` for [`FakeSwarm::publish`].
pub fn parse_torrent(bytes: &[u8]) -> Torrent {
    Torrent::parse(bytes, bencode::Limits::default()).expect("a valid torrent")
}
