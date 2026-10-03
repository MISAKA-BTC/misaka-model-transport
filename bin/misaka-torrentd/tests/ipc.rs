//! The daemon over its real socket: hello, peer checks, requests, events (§6.2).

#![cfg(unix)]

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use common::*;
use misaka_torrentd::config::Profile;
use misaka_torrentd::server::{self, Access};
use misaka_transport_engine::fake::{FakeEngine, FakeSwarm, PeerBehaviour};
use misaka_transport_ipc::client::{Client, ClientError};
use misaka_transport_ipc::{BundleState, ErrorCode, PROTOCOL, Request, Response, read_frame, write_frame};

#[test]
fn over_the_socket() {
    let root = tempdir("ipc");
    let fx = palw_fixture(&root.join("src"), "ipc", 21);
    let swarm = FakeSwarm::new();
    swarm.publish(fx.torrent(), fx.files.clone(), PeerBehaviour::Honest);
    // macOS limits socket paths to 104 bytes; keep it short.
    let sock = std::path::PathBuf::from(format!("/tmp/mtd-{}.d/s.sock", std::process::id()));
    let mut cfg = config(&root.join("home"), Profile::Desktop, vec![]);
    cfg.socket = sock.clone();
    let access = Access::from_config(&cfg);
    let listener = server::bind(&sock, Profile::Desktop).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let m = std::fs::metadata(sock.parent().unwrap()).unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o700);
    }
    // A second daemon on the same socket is refused.
    assert!(server::bind(&sock, Profile::Desktop).is_err());

    let d = Arc::new(Mutex::new(daemon(cfg, FakeEngine::new(swarm))));
    let stop = Arc::new(AtomicBool::new(false));
    let t = {
        let (d, stop, sock) = (d.clone(), stop.clone(), sock.clone());
        std::thread::spawn(move || server::serve(listener, sock, d, access, stop).unwrap())
    };

    // Hello first, with the protocol string.
    let mut raw = std::os::unix::net::UnixStream::connect(&sock).unwrap();
    write_frame(&mut raw, &Request::Status { infohash: None }).unwrap();
    assert!(matches!(read_frame::<Response, _>(&mut raw).unwrap(), Response::Error { code: ErrorCode::Protocol, .. }));
    let mut raw = std::os::unix::net::UnixStream::connect(&sock).unwrap();
    write_frame(&mut raw, &Request::Hello { protocol: "misaka-torrent-borsh/v0".into(), client: "t".into() }).unwrap();
    assert!(matches!(read_frame::<Response, _>(&mut raw).unwrap(), Response::Error { code: ErrorCode::Protocol, .. }));

    let c = Client::connect(&sock, "test").unwrap();
    assert_eq!(c.info.protocol, PROTOCOL);
    assert!(!c.info.read_only);
    let events = c.subscribe().unwrap();

    let mut c = Client::connect(&sock, "test").unwrap();
    c.call(&Request::Fetch { infohash: fx.ih(), expect: fx.expect(), seed_after: true }).unwrap();
    let mut seen = Vec::new();
    for e in events {
        seen.push(e.state.name());
        if e.state == BundleState::Seeding {
            break;
        }
    }
    assert!(seen.contains(&"sealed"), "{seen:?}");
    let st = c.status(Some(fx.ih())).unwrap();
    assert_eq!(st[0].state, BundleState::Seeding);
    match c.call(&Request::Seed { infohash: [0; 32] }) {
        Err(ClientError::Daemon { code: ErrorCode::NotFound, .. }) => {}
        other => panic!("{other:?}"),
    }

    stop.store(true, Ordering::Relaxed);
    t.join().unwrap();
    assert!(!sock.exists(), "the socket is removed on shutdown");
    let _ = std::fs::remove_dir_all(sock.parent().unwrap());
    cleanup(&root);
}
