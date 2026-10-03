//! The libtorrent-rasterbar 2.0.x backend, through `shim/libtorrent`'s C ABI (feature
//! `libtorrent`).

use std::ffi::{CStr, CString, c_char};
use std::path::Path;

use misaka_btv2::bencode::{self, Value};
use misaka_btv2::torrent::Torrent;
use misaka_transport_policy::limits::{MAX_INFO_BYTES, MAX_PIECE_LAYERS_BYTES};

use crate::{
    AddMode, AddRequest, AdmittedBundle, BundleHandle, BundleStatus, EngineError, EngineEvent, EngineSettings,
    EngineState, Infohash, Limits, ModelTransport, PieceLayers, RemoveFiles,
};

#[repr(C)]
struct MtSession {
    _private: [u8; 0],
}

#[repr(C)]
struct MtSettings {
    listen_port: u16,
    upload_rate: i64,
    download_rate: i64,
    connections: i32,
    upnp_natpmp: i32,
    lsd: i32,
    dht: i32,
    dht_bootstrap: *const c_char,
    state_dir: *const c_char,
    i2p_sam_host: *const c_char,
    i2p_sam_port: u16,
    i2p_inbound_length: i32,
    i2p_outbound_length: i32,
    i2p_inbound_quantity: i32,
    i2p_outbound_quantity: i32,
    trackers: *const c_char,
}

#[repr(C)]
struct MtStatus {
    state: i32,
    total: u64,
    done: u64,
    uploaded: u64,
    downloaded: u64,
    upload_rate: u64,
    download_rate: u64,
    peers: u32,
    seeds: u32,
    error: [c_char; 256],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MtEvent {
    kind: i32,
    handle: u64,
    piece: u64,
    message: [c_char; 256],
}

const ERR_LEN: usize = 512;

unsafe extern "C" {
    fn mt_session_create(s: *const MtSettings, err: *mut c_char, errlen: usize) -> *mut MtSession;
    fn mt_session_destroy(s: *mut MtSession);
    fn mt_apply_limits(s: *mut MtSession, up: i64, down: i64, conns: i32, err: *mut c_char, errlen: usize) -> i32;
    fn mt_add_magnet(
        s: *mut MtSession,
        ih: *const u8,
        save: *const c_char,
        out: *mut u64,
        err: *mut c_char,
        errlen: usize,
    ) -> i32;
    fn mt_get_metadata(
        s: *mut MtSession,
        h: u64,
        buf: *mut u8,
        cap: usize,
        len: *mut usize,
        err: *mut c_char,
        errlen: usize,
    ) -> i32;
    fn mt_admit(s: *mut MtSession, h: u64, err: *mut c_char, errlen: usize) -> i32;
    fn mt_add_torrent(
        s: *mut MtSession,
        t: *const u8,
        len: usize,
        save: *const c_char,
        mode: i32,
        out: *mut u64,
        err: *mut c_char,
        errlen: usize,
    ) -> i32;
    fn mt_pause(s: *mut MtSession, h: u64, err: *mut c_char, errlen: usize) -> i32;
    fn mt_resume(s: *mut MtSession, h: u64, err: *mut c_char, errlen: usize) -> i32;
    fn mt_remove(s: *mut MtSession, h: u64, del: i32, err: *mut c_char, errlen: usize) -> i32;
    fn mt_status(s: *mut MtSession, h: u64, out: *mut MtStatus, err: *mut c_char, errlen: usize) -> i32;
    fn mt_piece_layers(
        s: *mut MtSession,
        h: u64,
        buf: *mut u8,
        cap: usize,
        len: *mut usize,
        err: *mut c_char,
        errlen: usize,
    ) -> i32;
    fn mt_pop_events(s: *mut MtSession, out: *mut MtEvent, max: usize) -> usize;
}

fn cstr(buf: &[c_char]) -> String {
    // SAFETY: the shim always NUL-terminates within the buffer.
    unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
}

fn path_c(p: &Path) -> Result<CString, EngineError> {
    CString::new(p.to_str().ok_or_else(|| EngineError::Io("a non-UTF-8 path".into()))?)
        .map_err(|_| EngineError::Io("a path with NUL".into()))
}

/// A libtorrent session. One per daemon.
pub struct LibtorrentEngine {
    s: *mut MtSession,
}

// SAFETY: the shim serializes every call on its own mutex.
unsafe impl Send for LibtorrentEngine {}

impl LibtorrentEngine {
    pub fn new(settings: &EngineSettings) -> Result<Self, EngineError> {
        let boot = CString::new(settings.dht_bootstrap.join(","))
            .map_err(|_| EngineError::Engine("NUL in bootstrap".into()))?;
        let state = path_c(&settings.state_dir)?;
        let i2p = settings.i2p.clone();
        let sam = CString::new(i2p.as_ref().map(|i| i.sam_host.clone()).unwrap_or_default())
            .map_err(|_| EngineError::Engine("NUL in the SAM host".into()))?;
        let trackers =
            CString::new(settings.trackers.join(",")).map_err(|_| EngineError::Engine("NUL in a tracker".into()))?;
        let s = MtSettings {
            listen_port: settings.listen_port,
            upload_rate: settings.limits.upload_bytes_per_sec as i64,
            download_rate: settings.limits.download_bytes_per_sec as i64,
            connections: settings.limits.connections as i32,
            upnp_natpmp: settings.upnp_natpmp as i32,
            lsd: settings.lsd as i32,
            dht: settings.dht as i32,
            dht_bootstrap: boot.as_ptr(),
            state_dir: state.as_ptr(),
            i2p_sam_host: sam.as_ptr(),
            i2p_sam_port: i2p.as_ref().map_or(0, |i| i.sam_port),
            i2p_inbound_length: i2p.as_ref().map_or(3, |i| i32::from(i.inbound_length)),
            i2p_outbound_length: i2p.as_ref().map_or(3, |i| i32::from(i.outbound_length)),
            i2p_inbound_quantity: i2p.as_ref().map_or(3, |i| i32::from(i.inbound_quantity)),
            i2p_outbound_quantity: i2p.as_ref().map_or(3, |i| i32::from(i.outbound_quantity)),
            trackers: trackers.as_ptr(),
        };
        let mut err = [0 as c_char; ERR_LEN];
        // SAFETY: valid pointers for the call's duration.
        let p = unsafe { mt_session_create(&s, err.as_mut_ptr(), ERR_LEN) };
        if p.is_null() {
            return Err(EngineError::Engine(cstr(&err)));
        }
        Ok(LibtorrentEngine { s: p })
    }

    fn call(&self, f: impl FnOnce(*mut c_char, usize) -> i32) -> Result<(), EngineError> {
        let mut err = [0 as c_char; ERR_LEN];
        match f(err.as_mut_ptr(), ERR_LEN) {
            0 => Ok(()),
            _ => Err(EngineError::Engine(cstr(&err))),
        }
    }

    fn read_bytes(
        &self,
        cap: usize,
        f: impl Fn(*mut u8, usize, *mut usize, *mut c_char, usize) -> i32,
    ) -> Result<Vec<u8>, EngineError> {
        let mut buf = vec![0u8; cap];
        let mut len = 0usize;
        let mut err = [0 as c_char; ERR_LEN];
        match f(buf.as_mut_ptr(), cap, &mut len, err.as_mut_ptr(), ERR_LEN) {
            0 => {
                buf.truncate(len);
                Ok(buf)
            }
            -2 => Err(EngineError::Engine(format!("{len} bytes exceed the {cap}-byte bound"))),
            _ => Err(EngineError::Engine(cstr(&err))),
        }
    }
}

impl Drop for LibtorrentEngine {
    fn drop(&mut self) {
        // SAFETY: created by mt_session_create, destroyed once.
        unsafe { mt_session_destroy(self.s) }
    }
}

impl ModelTransport for LibtorrentEngine {
    fn name(&self) -> &str {
        "libtorrent-2.0"
    }

    fn fetch_metadata(&mut self, infohash: Infohash, save_dir: &Path) -> Result<BundleHandle, EngineError> {
        let save = path_c(save_dir)?;
        let mut out = 0u64;
        // SAFETY: a 32-byte hash and live C strings.
        self.call(|e, l| unsafe { mt_add_magnet(self.s, infohash.0.as_ptr(), save.as_ptr(), &mut out, e, l) })?;
        Ok(BundleHandle(out))
    }

    fn start_download(&mut self, h: BundleHandle, bundle: &AdmittedBundle) -> Result<(), EngineError> {
        // The engine's metadata must be the admitted bytes, exactly.
        // SAFETY: the buffer outlives the call.
        let info =
            self.read_bytes(MAX_INFO_BYTES, |b, c, n, e, l| unsafe { mt_get_metadata(self.s, h.0, b, c, n, e, l) })?;
        if info != bundle.info_bytes() {
            return Err(EngineError::NotAwaitingAdmission);
        }
        // SAFETY: plain call.
        self.call(|e, l| unsafe { mt_admit(self.s, h.0, e, l) })
    }

    fn add(&mut self, req: AddRequest<'_>) -> Result<BundleHandle, EngineError> {
        let t = Torrent {
            info: req.bundle.info().clone(),
            piece_layers: req.piece_layers.cloned().unwrap_or_default(),
            announce: None,
            announce_list: Vec::new(),
            url_list: Vec::new(),
        };
        let bytes = t.encode();
        let save = path_c(req.save_dir)?;
        let mode = match req.mode {
            AddMode::Fetch => 0,
            AddMode::Seed => 1,
            AddMode::Adopt => 2,
        };
        let mut out = 0u64;
        // SAFETY: live buffers for the call.
        self.call(|e, l| unsafe {
            mt_add_torrent(self.s, bytes.as_ptr(), bytes.len(), save.as_ptr(), mode, &mut out, e, l)
        })?;
        Ok(BundleHandle(out))
    }

    fn pause(&mut self, h: BundleHandle) -> Result<(), EngineError> {
        // SAFETY: plain call.
        self.call(|e, l| unsafe { mt_pause(self.s, h.0, e, l) })
    }

    fn resume(&mut self, h: BundleHandle) -> Result<(), EngineError> {
        // SAFETY: plain call.
        self.call(|e, l| unsafe { mt_resume(self.s, h.0, e, l) })
    }

    fn remove(&mut self, h: BundleHandle, files: RemoveFiles) -> Result<(), EngineError> {
        let del = (files == RemoveFiles::Delete) as i32;
        // SAFETY: plain call.
        self.call(|e, l| unsafe { mt_remove(self.s, h.0, del, e, l) })
    }

    fn set_limits(&mut self, limits: Limits) -> Result<(), EngineError> {
        // SAFETY: plain call.
        self.call(|e, l| unsafe {
            mt_apply_limits(
                self.s,
                limits.upload_bytes_per_sec as i64,
                limits.download_bytes_per_sec as i64,
                limits.connections as i32,
                e,
                l,
            )
        })
    }

    fn status(&self, h: BundleHandle) -> Result<BundleStatus, EngineError> {
        // SAFETY: MtStatus is plain data; zeroed is a valid value.
        let mut st: MtStatus = unsafe { std::mem::zeroed() };
        // SAFETY: a live out-pointer.
        self.call(|e, l| unsafe { mt_status(self.s, h.0, &mut st, e, l) })?;
        let state = match st.state {
            0 => EngineState::Metadata,
            1 => EngineState::Checking,
            2 => EngineState::Downloading,
            3 => EngineState::Finished,
            4 => EngineState::Seeding,
            5 => EngineState::Paused,
            _ => EngineState::Error(cstr(&st.error)),
        };
        Ok(BundleStatus {
            state,
            total_bytes: st.total,
            done_bytes: st.done,
            uploaded: st.uploaded,
            downloaded: st.downloaded,
            upload_rate: st.upload_rate,
            download_rate: st.download_rate,
            peers: st.peers,
            seeds: st.seeds,
        })
    }

    fn piece_layers(&self, h: BundleHandle) -> Option<PieceLayers> {
        // SAFETY: the buffer outlives the call.
        let bytes = self
            .read_bytes(MAX_PIECE_LAYERS_BYTES + 4096, |b, c, n, e, l| unsafe {
                mt_piece_layers(self.s, h.0, b, c, n, e, l)
            })
            .ok()?;
        let v = bencode::decode(&bytes, bencode::Limits { max_depth: 2, max_nodes: 200 }).ok()?;
        let mut out = PieceLayers::new();
        for (k, v) in v.as_dict()? {
            let root: [u8; 32] = k.as_slice().try_into().ok()?;
            let Value::Bytes(layer) = v else { return None };
            out.insert(root, layer.clone());
        }
        Some(out)
    }

    fn poll_events(&mut self, max: usize) -> Vec<EngineEvent> {
        let mut buf = vec![MtEvent { kind: 0, handle: 0, piece: 0, message: [0; 256] }; max.max(1)];
        // SAFETY: `buf` holds `max` events.
        let n = unsafe { mt_pop_events(self.s, buf.as_mut_ptr(), buf.len()) };
        let mut out = Vec::with_capacity(n);
        for ev in &buf[..n] {
            let handle = BundleHandle(ev.handle);
            out.push(match ev.kind {
                1 => {
                    // SAFETY: the buffer outlives the call.
                    match self.read_bytes(MAX_INFO_BYTES, |b, c, len, e, l| unsafe {
                        mt_get_metadata(self.s, ev.handle, b, c, len, e, l)
                    }) {
                        Ok(info_bytes) => EngineEvent::Metadata { handle, info_bytes },
                        Err(e) => EngineEvent::Error { handle, message: e.to_string() },
                    }
                }
                2 => EngineEvent::Finished { handle },
                3 => EngineEvent::HashFailed { handle, piece: ev.piece },
                4 => EngineEvent::PeerBanned { handle, reason: cstr(&ev.message) },
                _ => EngineEvent::Error { handle, message: cstr(&ev.message) },
            });
        }
        out
    }
}
