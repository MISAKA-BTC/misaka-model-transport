//! The daemon's state machine (§4.2), generic over the engine so that tests drive every state
//! over the fake swarm (§9, A3).
//!
//! ```text
//! Metadata → Admitted → Downloading → Verifying → Sealed → Seeding | Idle
//!    └─ Refused(policy)      └─ Stalled         └─ Mismatch(L2)
//! ```
//!
//! `Resolving`, `Installed` and L3 belong to the resolver (misakas `misaka model …`, Studio). The
//! daemon knows no chain: it is told what to expect, checks what it can (L1 in the engine, the
//! profile before admission, L2 before sealing) and reports.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use misaka_btv2::bencode::{self, Value};
use misaka_btv2::torrent::Infohash;
use misaka_bundle::{BundleKind, Hex64};
use misaka_transport_engine::{
    AddMode, AddRequest, BundleHandle, EngineEvent, EngineState, Limits, ModelTransport, PieceLayers, RemoveFiles,
};
use misaka_transport_ipc::{
    AdoptRootInfo, AnchorLabel, BundleReport, BundleState, DaemonInfo, ErrorCode, Expect, PROTOCOL, Request, Response,
    is_safe_rel_dir,
};
use misaka_transport_policy::pack::{self, Packed};
use misaka_transport_policy::{AdmissionEnv, AdmittedBundle, Expectation, admit, l2};

use crate::config::Config;
use crate::store::{Phase, Record, Store};
use crate::sys;

/// What a connected peer may do (§6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerRole {
    /// The daemon's own uid or an allowed operator uid.
    Control,
    /// The server's read-only group: `Hello`, `Status`, `Subscribe`.
    ReadOnly,
}

struct Bundle {
    ih: Infohash,
    expect: Expect,
    seed: bool,
    paused: bool,
    label: AnchorLabel,
    state: BundleState,
    phase: Phase,
    admitted: Option<AdmittedBundle>,
    handle: Option<BundleHandle>,
    adopted: Option<(String, String)>,
    adopted_dir: Option<PathBuf>,
    commitment: Option<[u8; 64]>,
    created_unix: u64,
    last_done: u64,
}

impl Bundle {
    fn new(ih: Infohash, expect: Expect, seed: bool, phase: Phase) -> Self {
        Bundle {
            ih,
            expect,
            seed,
            paused: false,
            label: AnchorLabel::Unanchored,
            state: BundleState::Metadata,
            phase,
            admitted: None,
            handle: None,
            adopted: None,
            adopted_dir: None,
            commitment: None,
            created_unix: sys::unix_now(),
            last_done: 0,
        }
    }

    fn record(&self) -> Record {
        let failure = match &self.state {
            BundleState::Refused { reason } | BundleState::Failed { reason } => Some(reason.clone()),
            BundleState::Mismatch { level, reason } => Some(format!("L{level}: {reason}")),
            _ => None,
        };
        Record {
            infohash: self.ih.0,
            expect: self.expect.clone(),
            seed: self.seed,
            paused: self.paused,
            label: self.label,
            phase: self.phase,
            failure,
            adopted: self.adopted.clone(),
            bundle_commitment: self.commitment,
            created_unix: self.created_unix,
        }
    }

    fn in_progress(&self) -> bool {
        matches!(
            self.state,
            BundleState::Metadata
                | BundleState::Admitted
                | BundleState::Downloading
                | BundleState::Stalled
                | BundleState::Verifying
        ) || (self.phase == Phase::Fetching && self.state == BundleState::Paused)
    }
}

enum Job {
    Adopted { ih: Infohash, result: Result<(Box<Packed>, PathBuf), String> },
}

fn persisted_u64(store: &Store, name: &str, init: impl FnOnce() -> u64) -> u64 {
    let p = store.state.join(name);
    if let Some(v) = std::fs::read_to_string(&p).ok().and_then(|s| s.trim().parse().ok()) {
        return v;
    }
    let v = init();
    let _ = crate::store::write_atomic(&p, format!("{v}\n").as_bytes());
    v
}

/// The configured port, or the one chosen at random at first start and kept (§6.3).
pub fn listen_port_for(cfg: &Config, store: &Store) -> u16 {
    cfg.listen_port.unwrap_or_else(|| persisted_u64(store, "port", || u64::from(sys::random_port())) as u16)
}

type FreeSpace = Box<dyn Fn(&Path) -> Option<u64> + Send>;

/// The daemon.
pub struct Daemon<E: ModelTransport> {
    cfg: Config,
    store: Store,
    engine: E,
    bundles: BTreeMap<Infohash, Bundle>,
    by_handle: HashMap<BundleHandle, Infohash>,
    subscribers: Vec<mpsc::Sender<BundleReport>>,
    jobs_tx: mpsc::Sender<Job>,
    jobs_rx: mpsc::Receiver<Job>,
    quota: u64,
    listen_port: u16,
    limits: Limits,
    free_space: FreeSpace,
    last_gc: Option<Instant>,
    confinement: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("store: {0}")]
    Store(#[from] std::io::Error),
}

fn err(code: ErrorCode, message: impl Into<String>) -> Response {
    Response::Error { code, message: message.into() }
}

fn to_policy_expect(e: &Expect) -> Result<Expectation, String> {
    let kind = match e.kind {
        None => None,
        Some(id) => {
            Some(BundleKind::from_id(id).filter(|k| k.is_active()).ok_or_else(|| format!("kind {id} is not 0 or 1"))?)
        }
    };
    Ok(Expectation { bundle_commitment: e.bundle_commitment.map(Hex64), total_bytes: e.total_bytes, kind })
}

fn encode_layers(layers: &PieceLayers) -> Vec<u8> {
    bencode::encode(&Value::Dict(layers.iter().map(|(k, v)| (k.to_vec(), Value::Bytes(v.clone()))).collect()))
}

/// Only the layers BEP 52 defines: files longer than one piece. An engine may report a one-hash
/// "layer" for a smaller file; a torrent carrying it is refused when it is added again to seed.
fn bep52_layers(layers: &PieceLayers, info: &misaka_btv2::torrent::InfoDict) -> PieceLayers {
    let big: std::collections::HashSet<[u8; 32]> =
        info.files.iter().filter(|f| f.length > info.piece_length).map(|f| f.pieces_root).collect();
    layers.iter().filter(|(root, _)| big.contains(*root)).map(|(k, v)| (*k, v.clone())).collect()
}

fn decode_layers(bytes: &[u8]) -> Option<PieceLayers> {
    let v = bencode::decode(bytes, bencode::Limits { max_depth: 2, max_nodes: 200 }).ok()?;
    v.as_dict()?.iter().map(|(k, v)| Some((k.as_slice().try_into().ok()?, v.as_bytes()?.to_vec()))).collect()
}

impl<E: ModelTransport> Daemon<E> {
    /// Opens the store, picks the port and quota on first start, and restores every bundle.
    pub fn new(cfg: Config, engine: E, confinement: String) -> Result<Self, DaemonError> {
        Self::with_free_space(cfg, engine, confinement, Box::new(sys::free_space))
    }

    /// As [`Self::new`], with the free-space probe replaced (tests).
    pub fn with_free_space(
        cfg: Config,
        engine: E,
        confinement: String,
        free_space: FreeSpace,
    ) -> Result<Self, DaemonError> {
        let store = Store::open(&cfg.home, cfg.profile)?;
        let listen_port = listen_port_for(&cfg, &store);
        let quota = match cfg.quota_bytes {
            Some(q) => q,
            // 50 % of the volume at first start (§4.6), then kept.
            None => persisted_u64(&store, "quota", || sys::volume_size(&store.root).map(|v| v / 2).unwrap_or(u64::MAX)),
        };
        let (jobs_tx, jobs_rx) = mpsc::channel();
        let limits = cfg.limits;
        let mut d = Daemon {
            cfg,
            store,
            engine,
            bundles: BTreeMap::new(),
            by_handle: HashMap::new(),
            subscribers: Vec::new(),
            jobs_tx,
            jobs_rx,
            quota,
            listen_port,
            limits,
            free_space,
            last_gc: None,
            confinement,
        };
        for r in d.store.load_records() {
            d.restore(r);
        }
        Ok(d)
    }

    pub fn listen_port(&self) -> u16 {
        self.listen_port
    }

    pub fn engine(&self) -> &E {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut E {
        &mut self.engine
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    fn restore(&mut self, r: Record) {
        let ih = Infohash(r.infohash);
        let mut b = Bundle::new(ih, r.expect.clone(), r.seed, r.phase);
        b.paused = r.paused;
        b.label = r.label;
        b.adopted = r.adopted.clone();
        b.commitment = r.bundle_commitment;
        b.created_unix = r.created_unix;
        let expect = to_policy_expect(&r.expect).unwrap_or_default();
        let admitted =
            self.store.load_info(&ih).and_then(|info| admit(&info, &ih, &expect, &AdmissionEnv::default()).ok());
        match r.phase {
            Phase::Failed => {
                b.state = BundleState::Failed { reason: r.failure.unwrap_or_else(|| "failed".into()) };
                self.bundles.insert(ih, b);
            }
            Phase::Sealed => {
                let present = admitted.as_ref().is_some_and(|a| self.store.sealed_dir(&ih).join(a.title()).is_dir());
                b.admitted = admitted;
                b.state = if present {
                    BundleState::Idle
                } else {
                    b.phase = Phase::Failed;
                    BundleState::Failed { reason: "the sealed bundle is missing from the store".into() }
                };
                self.bundles.insert(ih, b);
                if present && r.seed && self.cfg.seeding_enabled && !r.paused {
                    let _ = self.start_seed(&ih);
                }
            }
            Phase::Fetching => {
                let res = match &admitted {
                    Some(a) => {
                        let layers = self
                            .store
                            .load_layers(&ih)
                            .and_then(|l| decode_layers(&l))
                            .map(|l| bep52_layers(&l, a.info()));
                        let dir = self.store.incomplete_dir(&ih);
                        self.engine
                            .add(AddRequest {
                                bundle: a,
                                mode: AddMode::Fetch,
                                save_dir: &dir,
                                piece_layers: layers.as_ref(),
                            })
                            .map(|h| (h, BundleState::Downloading))
                    }
                    None => self
                        .engine
                        .fetch_metadata(ih, &self.store.incomplete_dir(&ih))
                        .map(|h| (h, BundleState::Metadata)),
                };
                b.admitted = admitted;
                match res {
                    Ok((h, state)) => {
                        b.handle = Some(h);
                        b.state = state;
                        self.by_handle.insert(h, ih);
                        if r.paused {
                            let _ = self.engine.pause(h);
                            b.state = BundleState::Paused;
                        }
                    }
                    Err(e) => b.state = BundleState::Failed { reason: e.to_string() },
                }
                self.bundles.insert(ih, b);
            }
            Phase::Adopted => {
                self.bundles.insert(ih, b);
                if let Some((root, rel)) = r.adopted {
                    self.spawn_adopt(ih, &root, &rel);
                }
            }
        }
    }

    // ---- requests ------------------------------------------------------------------------

    pub fn hello(&self, role: PeerRole) -> DaemonInfo {
        DaemonInfo {
            protocol: PROTOCOL.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            profile: self.cfg.profile.as_str().to_owned(),
            seeding_enabled: self.cfg.seeding_enabled,
            read_only: role == PeerRole::ReadOnly,
            adopt_roots: self
                .cfg
                .adopt_roots
                .iter()
                .map(|r| AdoptRootInfo { id: r.id.clone(), path: r.path.display().to_string() })
                .collect(),
            listen_port: self.listen_port,
            engine: self.engine.name().to_owned(),
            confinement: self.confinement.clone(),
        }
    }

    pub fn subscribe(&mut self) -> mpsc::Receiver<BundleReport> {
        let (tx, rx) = mpsc::channel();
        self.subscribers.push(tx);
        rx
    }

    /// Handles one request. `Hello` and `Subscribe` are the server's.
    pub fn handle(&mut self, req: Request, role: PeerRole) -> Response {
        if role == PeerRole::ReadOnly && !req.is_read_only() {
            return err(ErrorCode::Forbidden, "this peer may only read status");
        }
        match req {
            Request::Hello { .. } => Response::Hello(self.hello(role)),
            Request::Subscribe => err(ErrorCode::Protocol, "Subscribe is handled by the connection"),
            Request::Fetch { infohash, expect, seed_after } => self.fetch(Infohash(infohash), expect, seed_after),
            Request::Seed { infohash } => self.seed(Infohash(infohash)),
            Request::Adopt { infohash, adopt_root_id, rel_dir } => {
                self.adopt(Infohash(infohash), &adopt_root_id, &rel_dir)
            }
            Request::Pause { infohash } => self.pause(Infohash(infohash)),
            Request::Resume { infohash } => self.resume(Infohash(infohash)),
            Request::Remove { infohash, delete_files } => self.remove(Infohash(infohash), delete_files),
            Request::Status { infohash } => match infohash {
                Some(ih) => match self.report(&Infohash(ih)) {
                    Some(r) => Response::Status(vec![r]),
                    None => err(ErrorCode::NotFound, "no such bundle"),
                },
                None => Response::Status(
                    self.bundles.keys().copied().collect::<Vec<_>>().iter().filter_map(|ih| self.report(ih)).collect(),
                ),
            },
            Request::SetLimits(l) => {
                let mut limits = self.limits;
                if let Some(v) = l.upload_bytes_per_sec {
                    limits.upload_bytes_per_sec = v;
                }
                if let Some(v) = l.download_bytes_per_sec {
                    limits.download_bytes_per_sec = v;
                }
                if let Some(v) = l.connections {
                    limits.connections = v.max(1);
                }
                match self.engine.set_limits(limits) {
                    Ok(()) => {
                        self.limits = limits;
                        Response::Ok
                    }
                    Err(e) => err(ErrorCode::Internal, e.to_string()),
                }
            }
            Request::SetLabel { infohash, label } => {
                let ih = Infohash(infohash);
                let Some(b) = self.bundles.get_mut(&ih) else { return err(ErrorCode::NotFound, "no such bundle") };
                if !matches!(b.phase, Phase::Sealed | Phase::Adopted) || b.state.is_terminal_failure() {
                    return err(ErrorCode::Invalid, "only a sealed or adopted bundle carries a label");
                }
                b.label = label;
                self.persist(&ih);
                self.emit(&ih);
                Response::Ok
            }
        }
    }

    fn fetch(&mut self, ih: Infohash, expect: Expect, seed_after: bool) -> Response {
        if let Err(e) = to_policy_expect(&expect) {
            return err(ErrorCode::Invalid, e);
        }
        // MT-12: a profile that does not seed records no wish to.
        let seed = seed_after && self.cfg.seeding_enabled;
        if let Some(b) = self.bundles.get(&ih) {
            if b.state.is_terminal_failure() {
                // An explicit new request is a retry; a failure is never retried silently.
                self.drop_bundle(&ih, true);
            } else if b.phase == Phase::Sealed {
                if seed && b.handle.is_none() && !b.paused {
                    if let Some(b) = self.bundles.get_mut(&ih) {
                        b.seed = true;
                    }
                    return match self.start_seed(&ih) {
                        Ok(()) => Response::Ok,
                        Err(e) => err(ErrorCode::Internal, e),
                    };
                }
                return Response::Ok;
            } else if b.phase == Phase::Adopted {
                return err(ErrorCode::Busy, "this bundle is adopted; remove it first");
            } else if b.expect != expect {
                return err(ErrorCode::Busy, "already fetching under a different anchor");
            } else {
                return Response::Ok;
            }
        }
        // A leftover with no record is not trusted.
        let _ = self.store.remove_incomplete(&ih);
        if self.store.sealed_dir(&ih).exists() {
            let _ = self.store.remove_sealed(&ih);
        }
        match self.engine.fetch_metadata(ih, &self.store.incomplete_dir(&ih)) {
            Ok(h) => {
                let mut b = Bundle::new(ih, expect, seed, Phase::Fetching);
                b.handle = Some(h);
                self.by_handle.insert(h, ih);
                self.bundles.insert(ih, b);
                self.persist(&ih);
                self.emit(&ih);
                Response::Ok
            }
            Err(e) => err(ErrorCode::Internal, e.to_string()),
        }
    }

    fn seed(&mut self, ih: Infohash) -> Response {
        if !self.cfg.seeding_enabled {
            return err(ErrorCode::Forbidden, "seeding is off in this profile; set seeding.enabled = true (MT-12)");
        }
        let Some(b) = self.bundles.get_mut(&ih) else { return err(ErrorCode::NotFound, "no such bundle") };
        if b.phase != Phase::Sealed {
            return err(ErrorCode::Invalid, "only a sealed bundle is seeded from the store");
        }
        b.seed = true;
        b.paused = false;
        if let Some(h) = b.handle {
            let _ = self.engine.resume(h);
            if let Some(b) = self.bundles.get_mut(&ih) {
                b.state = BundleState::Seeding;
            }
            self.persist(&ih);
            self.emit(&ih);
            return Response::Ok;
        }
        match self.start_seed(&ih) {
            Ok(()) => Response::Ok,
            Err(e) => err(ErrorCode::Internal, e),
        }
    }

    fn start_seed(&mut self, ih: &Infohash) -> Result<(), String> {
        let b = self.bundles.get(ih).ok_or("no such bundle")?;
        let a = b.admitted.as_ref().ok_or("no admitted metadata")?;
        let layers = self.store.load_layers(ih).and_then(|l| decode_layers(&l)).map(|l| bep52_layers(&l, a.info()));
        let dir = self.store.sealed_dir(ih);
        let h = self
            .engine
            .add(AddRequest { bundle: a, mode: AddMode::Seed, save_dir: &dir, piece_layers: layers.as_ref() })
            .map_err(|e| e.to_string())?;
        self.by_handle.insert(h, *ih);
        let b = self.bundles.get_mut(ih).expect("present");
        b.handle = Some(h);
        b.seed = true;
        b.state = BundleState::Seeding;
        self.persist(ih);
        self.emit(ih);
        Ok(())
    }

    fn adopt(&mut self, ih: Infohash, root_id: &str, rel: &str) -> Response {
        if !self.cfg.seeding_enabled {
            return err(ErrorCode::Forbidden, "seeding is off in this profile; set seeding.enabled = true (MT-12)");
        }
        if self.cfg.adopt_root(root_id).is_none() {
            return err(ErrorCode::NotFound, format!("no adopt root {root_id:?}"));
        }
        if !is_safe_rel_dir(rel) {
            return err(ErrorCode::Invalid, "rel_dir must be relative, with no `.` or `..` component");
        }
        if let Some(b) = self.bundles.get(&ih) {
            if b.state.is_terminal_failure() {
                self.drop_bundle(&ih, false);
            } else {
                return err(ErrorCode::Busy, "this infohash is already in the daemon");
            }
        }
        let mut b = Bundle::new(ih, Expect::default(), true, Phase::Adopted);
        b.state = BundleState::Verifying;
        b.adopted = Some((root_id.to_owned(), rel.to_owned()));
        self.bundles.insert(ih, b);
        self.persist(&ih);
        self.emit(&ih);
        self.spawn_adopt(ih, root_id, rel);
        Response::Ok
    }

    /// Hashes an adopt directory off the request path: a 38 GB file takes minutes.
    fn spawn_adopt(&mut self, ih: Infohash, root_id: &str, rel: &str) {
        let Some(root) = self.cfg.adopt_root(root_id).map(|r| r.path.clone()) else {
            let _ = self.jobs_tx.send(Job::Adopted { ih, result: Err(format!("no adopt root {root_id:?}")) });
            return;
        };
        let rel = rel.to_owned();
        let tx = self.jobs_tx.clone();
        if let Some(b) = self.bundles.get_mut(&ih) {
            b.state = BundleState::Verifying;
        }
        std::thread::spawn(move || {
            let result = (|| {
                let root = root.canonicalize().map_err(|e| format!("{}: {e}", root.display()))?;
                let dir = root.join(&rel).canonicalize().map_err(|e| format!("{rel}: {e}"))?;
                // No link may lead out of the root.
                if !dir.starts_with(&root) || !dir.is_dir() {
                    return Err("the directory is not inside its adopt root".to_owned());
                }
                let packed = pack::rebuild(&dir).map_err(|e| e.to_string())?;
                if packed.infohash() != ih {
                    return Err(format!("the files' canonical infohash is {}, not {ih}", packed.infohash()));
                }
                if dir.file_name().and_then(|n| n.to_str()) != Some(packed.descriptor.title.as_str()) {
                    return Err(format!("the directory must be named after the title, {:?}", packed.descriptor.title));
                }
                Ok((Box::new(packed), dir))
            })();
            let _ = tx.send(Job::Adopted { ih, result });
        });
    }

    fn pause(&mut self, ih: Infohash) -> Response {
        let Some(b) = self.bundles.get_mut(&ih) else { return err(ErrorCode::NotFound, "no such bundle") };
        if b.state.is_terminal_failure() {
            return err(ErrorCode::Invalid, "the bundle has failed; fetch it again or remove it");
        }
        if let Some(h) = b.handle
            && let Err(e) = self.engine.pause(h)
        {
            return err(ErrorCode::Internal, e.to_string());
        }
        b.paused = true;
        if b.handle.is_some() {
            b.state = BundleState::Paused;
        }
        self.persist(&ih);
        self.emit(&ih);
        Response::Ok
    }

    fn resume(&mut self, ih: Infohash) -> Response {
        let Some(b) = self.bundles.get_mut(&ih) else { return err(ErrorCode::NotFound, "no such bundle") };
        b.paused = false;
        let Some(h) = b.handle else {
            self.persist(&ih);
            return Response::Ok;
        };
        if let Err(e) = self.engine.resume(h) {
            return err(ErrorCode::Internal, e.to_string());
        }
        let st = self.engine.status(h).map(|s| s.state).unwrap_or(EngineState::Downloading);
        let b = self.bundles.get_mut(&ih).expect("present");
        b.state = match st {
            EngineState::Metadata => BundleState::Metadata,
            EngineState::Seeding | EngineState::Finished if b.phase != Phase::Fetching => BundleState::Seeding,
            _ => BundleState::Downloading,
        };
        self.persist(&ih);
        self.emit(&ih);
        Response::Ok
    }

    fn remove(&mut self, ih: Infohash, delete_files: bool) -> Response {
        if !self.bundles.contains_key(&ih) {
            return err(ErrorCode::NotFound, "no such bundle");
        }
        self.drop_bundle(&ih, delete_files);
        Response::Ok
    }

    /// Removes a bundle from the engine and the daemon. Adopted files are never deleted: they
    /// are not the daemon's.
    fn drop_bundle(&mut self, ih: &Infohash, delete_files: bool) {
        let Some(b) = self.bundles.remove(ih) else { return };
        if let Some(h) = b.handle {
            self.by_handle.remove(&h);
            let _ = self.engine.remove(h, RemoveFiles::Keep);
        }
        let _ = self.store.remove_incomplete(ih);
        // Kept sealed files stay where installs hard-linked them from; the record goes.
        if delete_files && b.phase == Phase::Sealed {
            let _ = self.store.remove_sealed(ih);
        }
        self.store.forget(ih);
    }

    // ---- the clock -----------------------------------------------------------------------

    /// Drains engine events and finished jobs, follows progress, and collects garbage.
    pub fn tick(&mut self) {
        for ev in self.engine.poll_events(256) {
            self.on_event(ev);
        }
        while let Ok(job) = self.jobs_rx.try_recv() {
            self.on_job(job);
        }
        let watch: Vec<(Infohash, BundleHandle)> = self
            .bundles
            .values()
            .filter(|b| matches!(b.state, BundleState::Downloading | BundleState::Stalled))
            .filter_map(|b| b.handle.map(|h| (b.ih, h)))
            .collect();
        for (ih, h) in watch {
            if let Ok(st) = self.engine.status(h) {
                let b = self.bundles.get_mut(&ih).expect("present");
                if st.done_bytes > b.last_done {
                    b.last_done = st.done_bytes;
                    if b.state == BundleState::Stalled {
                        b.state = BundleState::Downloading;
                        self.emit(&ih);
                    }
                }
            }
        }
        if self.last_gc.is_none_or(|t| t.elapsed() >= Duration::from_secs(600)) {
            self.last_gc = Some(Instant::now());
            self.collect_garbage();
        }
    }

    /// `incomplete/` garbage collection after the configured idle days (§4.6).
    pub fn collect_garbage(&mut self) {
        let active: Vec<Infohash> = self.bundles.values().filter(|b| b.in_progress()).map(|b| b.ih).collect();
        for ih in self.store.stale_incomplete(self.cfg.incomplete_gc, &|ih| active.contains(ih)) {
            let _ = self.store.remove_incomplete(&ih);
        }
    }

    fn on_event(&mut self, ev: EngineEvent) {
        let handle = match &ev {
            EngineEvent::Metadata { handle, .. }
            | EngineEvent::Finished { handle }
            | EngineEvent::HashFailed { handle, .. }
            | EngineEvent::PeerBanned { handle, .. }
            | EngineEvent::Stalled { handle }
            | EngineEvent::Error { handle, .. } => *handle,
        };
        let Some(ih) = self.by_handle.get(&handle).copied() else { return };
        match ev {
            EngineEvent::Metadata { info_bytes, .. } => self.on_metadata(ih, handle, &info_bytes),
            EngineEvent::Finished { .. } => self.on_finished(ih, handle),
            EngineEvent::HashFailed { piece, .. } => {
                eprintln!("misaka-torrentd: {} piece {piece} failed its hash; refetching", ih.short())
            }
            EngineEvent::PeerBanned { reason, .. } => {
                eprintln!("misaka-torrentd: {} banned a peer: {reason}", ih.short())
            }
            EngineEvent::Stalled { .. } => {
                if let Some(b) = self.bundles.get_mut(&ih).filter(|b| b.state == BundleState::Downloading) {
                    b.state = BundleState::Stalled;
                    self.emit(&ih);
                }
            }
            EngineEvent::Error { message, .. } => self.fail(ih, BundleState::Failed { reason: message }, false),
        }
    }

    fn admission_env(&self, except: &Infohash) -> AdmissionEnv {
        let used: u64 = self
            .bundles
            .values()
            .filter(|b| b.ih != *except && b.phase != Phase::Adopted && !b.state.is_terminal_failure())
            .filter_map(|b| b.admitted.as_ref().map(AdmittedBundle::total_bytes))
            .sum();
        AdmissionEnv {
            free_space: (self.free_space)(&self.store.root),
            quota_remaining: Some(self.quota.saturating_sub(used)),
        }
    }

    fn on_metadata(&mut self, ih: Infohash, h: BundleHandle, info_bytes: &[u8]) {
        let Some(b) = self.bundles.get(&ih) else { return };
        if b.state != BundleState::Metadata {
            return;
        }
        let expect = to_policy_expect(&b.expect).unwrap_or_default();
        // MT-3: the infohash, then the profile, before the engine downloads or creates a file.
        match admit(info_bytes, &ih, &expect, &self.admission_env(&ih)) {
            Err(r) => self.fail(ih, BundleState::Refused { reason: r.to_string() }, true),
            Ok(a) => {
                let _ = self.store.save_info(&ih, a.info_bytes());
                let b = self.bundles.get_mut(&ih).expect("present");
                b.state = BundleState::Admitted;
                self.emit(&ih);
                let res = self.engine.start_download(h, &a);
                let b = self.bundles.get_mut(&ih).expect("present");
                b.admitted = Some(a);
                match res {
                    Ok(()) => {
                        b.state = if b.paused { BundleState::Paused } else { BundleState::Downloading };
                        if b.paused {
                            let _ = self.engine.pause(h);
                        }
                        self.persist(&ih);
                        self.emit(&ih);
                    }
                    Err(e) => self.fail(ih, BundleState::Failed { reason: e.to_string() }, false),
                }
            }
        }
    }

    fn on_finished(&mut self, ih: Infohash, h: BundleHandle) {
        let Some(b) = self.bundles.get(&ih) else { return };
        // Only a download in progress finishes. An engine may report "finished" for a torrent
        // that wants nothing yet (every file at priority 0 before admission); that is not a
        // complete bundle, and L2 must never read a partial one.
        if b.phase != Phase::Fetching || !matches!(b.state, BundleState::Downloading | BundleState::Stalled) {
            return;
        }
        let total = b.admitted.as_ref().map(AdmittedBundle::total_bytes);
        match self.engine.status(h) {
            Ok(st) if Some(st.done_bytes) == total => {}
            _ => return,
        }
        let b = self.bundles.get_mut(&ih).expect("present");
        b.state = BundleState::Verifying;
        self.emit(&ih);
        // Release the files before sealing: the engine must not hold them open for writing.
        let layers = self.engine.piece_layers(h);
        let _ = self.engine.remove(h, RemoveFiles::Keep);
        self.by_handle.remove(&h);
        let b = self.bundles.get_mut(&ih).expect("present");
        b.handle = None;
        let Some(a) = b.admitted.clone() else {
            return self.fail(ih, BundleState::Failed { reason: "finished without admitted metadata".into() }, true);
        };
        let expect = to_policy_expect(&b.expect).unwrap_or_default();
        let staging = self.store.incomplete_dir(&ih).join(a.title());
        // MT-4: nothing is sealed before L2.
        match l2::verify(&staging, &a, &expect, l2::Depth::Headers) {
            Err(e) => self.fail(ih, BundleState::Mismatch { level: 2, reason: e.to_string() }, true),
            Ok(report) => {
                if let Some(l) = &layers {
                    let _ = self.store.save_layers(&ih, &encode_layers(&bep52_layers(l, a.info())));
                }
                if let Err(e) = self.store.seal(&ih, a.title()) {
                    return self.fail(ih, BundleState::Failed { reason: format!("sealing: {e}") }, true);
                }
                let b = self.bundles.get_mut(&ih).expect("present");
                b.phase = Phase::Sealed;
                b.commitment = Some(report.commitment.0);
                b.label =
                    if expect.bundle_commitment.is_some() { AnchorLabel::Declared } else { AnchorLabel::Unanchored };
                b.state = BundleState::Sealed;
                let seed = b.seed && self.cfg.seeding_enabled && !b.paused;
                self.persist(&ih);
                self.emit(&ih);
                if seed {
                    if let Err(e) = self.start_seed(&ih) {
                        eprintln!("misaka-torrentd: {} sealed but not seeding: {e}", ih.short());
                        self.set_state(&ih, BundleState::Idle);
                    }
                } else {
                    self.set_state(&ih, BundleState::Idle);
                }
            }
        }
    }

    fn on_job(&mut self, job: Job) {
        match job {
            Job::Adopted { ih, result } => {
                let Some(b) = self.bundles.get(&ih) else { return };
                if b.phase != Phase::Adopted {
                    return;
                }
                match result {
                    Err(e) => self.fail(ih, BundleState::Mismatch { level: 2, reason: e }, false),
                    Ok((packed, dir)) => {
                        let parent = dir.parent().map(Path::to_owned).unwrap_or_default();
                        let layers = packed.torrent.piece_layers.clone();
                        let _ = self.store.save_info(&ih, packed.bundle.info_bytes());
                        let _ = self.store.save_layers(&ih, &encode_layers(&layers));
                        let added = self.engine.add(AddRequest {
                            bundle: &packed.bundle,
                            mode: AddMode::Adopt,
                            save_dir: &parent,
                            piece_layers: Some(&layers),
                        });
                        let b = self.bundles.get_mut(&ih).expect("present");
                        b.admitted = Some(packed.bundle.clone());
                        b.adopted_dir = Some(dir);
                        b.commitment = Some(packed.commitment().0);
                        match added {
                            Ok(h) => {
                                b.handle = Some(h);
                                self.by_handle.insert(h, ih);
                                b.state = if b.paused { BundleState::Paused } else { BundleState::Seeding };
                                if b.paused {
                                    let _ = self.engine.pause(h);
                                }
                                self.persist(&ih);
                                self.emit(&ih);
                            }
                            Err(e) => self.fail(ih, BundleState::Failed { reason: e.to_string() }, false),
                        }
                    }
                }
            }
        }
    }

    /// Moves a bundle to a failure state. With `delete_staging`, the staging copy goes too (MT-4).
    fn fail(&mut self, ih: Infohash, state: BundleState, delete_staging: bool) {
        let Some(b) = self.bundles.get_mut(&ih) else { return };
        eprintln!("misaka-torrentd: {} {}: {state:?}", ih.short(), state.name());
        if let Some(h) = b.handle.take() {
            self.by_handle.remove(&h);
            let _ = self.engine.remove(h, RemoveFiles::Keep);
        }
        b.state = state;
        b.phase = Phase::Failed;
        if delete_staging {
            let _ = self.store.remove_incomplete(&ih);
        }
        self.persist(&ih);
        self.emit(&ih);
    }

    fn set_state(&mut self, ih: &Infohash, state: BundleState) {
        if let Some(b) = self.bundles.get_mut(ih) {
            b.state = state;
            self.emit(ih);
        }
    }

    fn persist(&self, ih: &Infohash) {
        if let Some(b) = self.bundles.get(ih)
            && let Err(e) = self.store.save_record(&b.record())
        {
            eprintln!("misaka-torrentd: cannot save the record of {}: {e}", ih.short());
        }
    }

    fn emit(&mut self, ih: &Infohash) {
        if self.subscribers.is_empty() {
            return;
        }
        if let Some(r) = self.report(ih) {
            self.subscribers.retain(|s| s.send(r.clone()).is_ok());
        }
    }

    /// The status of one bundle, with the engine's counters.
    pub fn report(&self, ih: &Infohash) -> Option<BundleReport> {
        let b = self.bundles.get(ih)?;
        let st = b.handle.and_then(|h| self.engine.status(h).ok());
        let admitted = b.admitted.as_ref();
        let path = match b.phase {
            Phase::Sealed => admitted.map(|a| self.store.sealed_dir(ih).join(a.title()).display().to_string()),
            Phase::Adopted => b.adopted_dir.as_ref().map(|d| d.display().to_string()),
            _ => None,
        };
        let total = admitted.map(AdmittedBundle::total_bytes);
        let done = match (&b.state, &st) {
            (_, Some(s)) => s.done_bytes,
            (BundleState::Idle | BundleState::Sealed, None) => total.unwrap_or(0),
            _ => 0,
        };
        Some(BundleReport {
            infohash: ih.0,
            title: admitted.map(|a| a.title().to_owned()),
            kind: admitted.map(|a| a.kind().id()),
            state: b.state.clone(),
            label: b.label,
            total_bytes: total,
            done_bytes: done,
            uploaded: st.as_ref().map_or(0, |s| s.uploaded),
            downloaded: st.as_ref().map_or(0, |s| s.downloaded),
            upload_rate: st.as_ref().map_or(0, |s| s.upload_rate),
            download_rate: st.as_ref().map_or(0, |s| s.download_rate),
            peers: st.as_ref().map_or(0, |s| s.peers),
            seeds: st.as_ref().map_or(0, |s| s.seeds),
            seeding_enabled: b.seed,
            path,
            bundle_commitment: b.commitment,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use misaka_btv2::torrent::{InfoDict, InfoFile};

    #[test]
    fn only_bep52_layers_are_kept() {
        let p = 1 << 20;
        let info = InfoDict::new(
            "t",
            vec![
                InfoFile { name: "LICENSE".into(), length: 100, pieces_root: [1; 32] },
                InfoFile { name: "edge.gguf".into(), length: p, pieces_root: [2; 32] },
                InfoFile { name: "big.gguf".into(), length: 3 * p, pieces_root: [3; 32] },
            ],
        );
        // What libtorrent reports: a one-hash "layer" for every file, three for the big one.
        let reported: PieceLayers =
            [([1; 32], vec![1; 32]), ([2; 32], vec![2; 32]), ([3; 32], vec![9; 96])].into_iter().collect();
        let kept = bep52_layers(&reported, &info);
        assert_eq!(kept.keys().copied().collect::<Vec<_>>(), vec![[3u8; 32]]);
        assert_eq!(kept[&[3u8; 32]].len(), 96);
    }
}
