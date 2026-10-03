//! The IPC server: one Unix socket in a private directory, the peer checked on every connection
//! (§6.2). The daemon listens on nothing else but its peer port (MT-7).

use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use misaka_transport_engine::ModelTransport;
use misaka_transport_ipc::{ErrorCode, FrameError, PROTOCOL, Request, Response, read_frame, write_frame};

use crate::config::{Config, Profile};
use crate::core::{Daemon, PeerRole};
use crate::peercred::{own_uid, peer_cred};

/// Who may connect.
#[derive(Debug, Clone)]
pub struct Access {
    pub control_uids: Vec<u32>,
    pub read_only_gid: Option<u32>,
}

impl Access {
    pub fn from_config(cfg: &Config) -> Self {
        let mut control_uids = vec![own_uid()];
        control_uids.extend(&cfg.allowed_uids);
        Access { control_uids, read_only_gid: cfg.read_only_gid }
    }

    fn role(&self, s: &UnixStream) -> Option<PeerRole> {
        let c = peer_cred(s).ok()?;
        if self.control_uids.contains(&c.uid) {
            Some(PeerRole::Control)
        } else if self.read_only_gid == Some(c.gid) {
            Some(PeerRole::ReadOnly)
        } else {
            None
        }
    }
}

fn set_mode(p: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))
}

/// Binds the socket. Its directory is created `0700` (`0750` on a server, for the read-only
/// group); a stale socket file is replaced, anything else at the path is refused.
pub fn bind(socket: &Path, profile: Profile) -> io::Result<UnixListener> {
    let dir =
        socket.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "socket path has no directory"))?;
    std::fs::create_dir_all(dir)?;
    let (dir_mode, sock_mode) = if profile == Profile::Server { (0o750, 0o660) } else { (0o700, 0o600) };
    set_mode(dir, dir_mode)?;
    match std::fs::symlink_metadata(socket) {
        Ok(m) => {
            use std::os::unix::fs::FileTypeExt;
            if !m.file_type().is_socket() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{} exists and is not a socket", socket.display()),
                ));
            }
            if UnixStream::connect(socket).is_ok() {
                return Err(io::Error::new(io::ErrorKind::AddrInUse, "another misaka-torrentd is running"));
            }
            std::fs::remove_file(socket)?;
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let l = UnixListener::bind(socket)?;
    set_mode(socket, sock_mode)?;
    Ok(l)
}

/// Serves until `stop` is set: a ticker thread drives the state machine, one thread per
/// connection handles requests.
pub fn serve<E: ModelTransport + 'static>(
    listener: UnixListener,
    socket: PathBuf,
    daemon: Arc<Mutex<Daemon<E>>>,
    access: Access,
    stop: Arc<AtomicBool>,
) -> io::Result<()> {
    let ticker = {
        let daemon = daemon.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                daemon.lock().expect("unpoisoned").tick();
                std::thread::sleep(Duration::from_millis(200));
            }
        })
    };
    listener.set_nonblocking(true)?;
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((s, _)) => {
                let daemon = daemon.clone();
                let access = access.clone();
                std::thread::spawn(move || connection(s, daemon, access));
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => eprintln!("misaka-torrentd: accept: {e}"),
        }
    }
    let _ = ticker.join();
    let _ = std::fs::remove_file(socket);
    Ok(())
}

fn connection<E: ModelTransport>(mut s: UnixStream, daemon: Arc<Mutex<Daemon<E>>>, access: Access) {
    let _ = s.set_nonblocking(false);
    let Some(role) = access.role(&s) else {
        let _ =
            write_frame(&mut s, &Response::Error { code: ErrorCode::Forbidden, message: "peer not allowed".into() });
        return;
    };
    // The first message is Hello with our protocol.
    match read_frame::<Request, _>(&mut s) {
        Ok(Request::Hello { protocol, .. }) if protocol == PROTOCOL => {
            let info = daemon.lock().expect("unpoisoned").hello(role);
            if write_frame(&mut s, &Response::Hello(info)).is_err() {
                return;
            }
        }
        Ok(_) => {
            let _ = write_frame(
                &mut s,
                &Response::Error {
                    code: ErrorCode::Protocol,
                    message: format!("expected Hello {{ protocol: {PROTOCOL:?} }}"),
                },
            );
            return;
        }
        Err(_) => return,
    }
    loop {
        let req = match read_frame::<Request, _>(&mut s) {
            Ok(r) => r,
            Err(FrameError::Closed) => return,
            Err(e) => {
                let _ = write_frame(&mut s, &Response::Error { code: ErrorCode::Protocol, message: e.to_string() });
                return;
            }
        };
        if req == Request::Subscribe {
            let rx = daemon.lock().expect("unpoisoned").subscribe();
            if write_frame(&mut s, &Response::Ok).is_err() {
                return;
            }
            for r in rx {
                if write_frame(&mut s, &Response::Event(r)).is_err() {
                    return;
                }
            }
            return;
        }
        let resp = daemon.lock().expect("unpoisoned").handle(req, role);
        if write_frame(&mut s, &resp).is_err() {
            return;
        }
    }
}
