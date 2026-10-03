//! Every daemon state of RFC-0001 §4.2, driven over the fake swarm (phase A3's exit gate), and
//! the client drills that need no real network: D-T6 (hostile peers) and D-T7 (hostile torrents).

mod common;

use std::collections::BTreeMap;

use common::*;
use misaka_btv2::bencode::{self, Value};
use misaka_bundle::DESCRIPTOR_FILE_NAME;
use misaka_torrentd::config::{AdoptRoot, Profile};
use misaka_torrentd::core::PeerRole;
use misaka_transport_engine::fake::{FakeEngine, FakeSwarm, PeerBehaviour};
use misaka_transport_ipc::{AnchorLabel, BundleState, ErrorCode, Expect, Request, Response};

fn fetch(ih: [u8; 32], expect: Expect, seed_after: bool) -> Request {
    Request::Fetch { infohash: ih, expect, seed_after }
}

#[test]
fn fetch_verify_seal_seed() {
    let root = tempdir("happy");
    let fx = palw_fixture(&root.join("src"), "qwen-test", 1);
    let swarm = FakeSwarm::new();
    swarm.publish(fx.torrent(), fx.files.clone(), PeerBehaviour::Honest);
    let mut d = daemon(config(&root.join("home"), Profile::Desktop, vec![]), FakeEngine::new(swarm));
    let events = d.subscribe();

    assert_eq!(d.handle(fetch(fx.ih(), fx.expect(), true), PeerRole::Control), Response::Ok);
    assert_eq!(status(&d, fx.ih()).state, BundleState::Metadata);
    // No file exists before admission.
    assert!(!d.store().incomplete_dir(&fx.packed.infohash()).exists());

    let r = run_until(&mut d, fx.ih(), |s| *s == BundleState::Seeding);
    assert_eq!(r.label, AnchorLabel::Declared, "an expected commitment matched at L2");
    assert_eq!(r.bundle_commitment, Some(fx.packed.commitment().0));
    assert_eq!(r.total_bytes, Some(fx.packed.total_bytes()));

    let seen: Vec<String> = events.try_iter().map(|e| e.state.name().to_owned()).collect();
    let mut order = seen.clone();
    order.dedup();
    for s in ["metadata", "admitted", "downloading", "verifying", "sealed", "seeding"] {
        assert!(order.iter().any(|x| x == s), "{s} missing from {order:?}");
    }

    // Sealed: files 0444, directories 0555, under store/<infohash>/<title>/, staging gone.
    let path = std::path::PathBuf::from(r.path.unwrap());
    assert_eq!(path, d.store().sealed_dir(&fx.packed.infohash()).join("qwen-test"));
    for (name, bytes) in &fx.files {
        assert_eq!(&std::fs::read(path.join(name)).unwrap(), bytes);
        #[cfg(unix)]
        assert_eq!(mode(&path.join(name)), 0o444);
    }
    #[cfg(unix)]
    assert_eq!(mode(&path), 0o555);
    assert!(!d.store().incomplete_dir(&fx.packed.infohash()).exists());

    // Remove with files: the sealed copy and the record go.
    assert_eq!(d.handle(Request::Remove { infohash: fx.ih(), delete_files: true }, PeerRole::Control), Response::Ok);
    assert!(!d.store().sealed_dir(&fx.packed.infohash()).exists());
    assert!(d.report(&fx.packed.infohash()).is_none());
    cleanup(&root);
}

#[test]
fn server_profile_does_not_seed() {
    let root = tempdir("server");
    let fx = palw_fixture(&root.join("src"), "srv", 2);
    let swarm = FakeSwarm::new();
    swarm.publish(fx.torrent(), fx.files.clone(), PeerBehaviour::Honest);
    let mut d = daemon(config(&root.join("home"), Profile::Server, vec![]), FakeEngine::new(swarm));
    d.handle(fetch(fx.ih(), fx.expect(), true), PeerRole::Control);
    let r = run_until(&mut d, fx.ih(), |s| *s == BundleState::Idle);
    assert!(!r.seeding_enabled, "MT-12");
    match d.handle(Request::Seed { infohash: fx.ih() }, PeerRole::Control) {
        Response::Error { code: ErrorCode::Forbidden, .. } => {}
        other => panic!("{other:?}"),
    }
    #[cfg(unix)]
    assert_eq!(mode(&d.store().sealed), 0o750, "kaspad's group reads the store");
    cleanup(&root);
}

/// D-T7: torrents the profile refuses never create a file.
#[test]
fn hostile_torrents_are_refused_before_any_file() {
    let root = tempdir("hostile");
    let swarm = FakeSwarm::new();
    let mut d = daemon(config(&root.join("home"), Profile::Desktop, vec![]), FakeEngine::new(swarm.clone()));

    let leaf = |len: i64| {
        Value::dict([("", Value::dict([("length", Value::Int(len)), ("pieces root", Value::bytes(vec![1; 32]))]))])
    };
    let mk = |tree: Vec<(&str, Value)>| {
        let total: i64 = tree.len() as i64;
        let v = Value::dict([
            ("file tree", Value::dict(tree)),
            ("meta version", Value::Int(2)),
            ("name", Value::bytes("evil")),
            ("piece length", Value::Int(misaka_btv2::piece_length_for(total as u64) as i64)),
        ]);
        bencode::encode(&v)
    };
    let symlink = {
        let l = Value::dict([(
            "",
            Value::dict([
                ("length", Value::Int(1)),
                ("pieces root", Value::bytes(vec![1; 32])),
                ("symlink path", Value::List(vec![Value::bytes("etc"), Value::bytes("passwd")])),
            ]),
        )]);
        mk(vec![(DESCRIPTOR_FILE_NAME, leaf(1)), ("LICENSE", leaf(1)), ("x.gguf", l)])
    };
    let attr = {
        let l = Value::dict([(
            "",
            Value::dict([
                ("attr", Value::bytes("x")),
                ("length", Value::Int(1)),
                ("pieces root", Value::bytes(vec![1; 32])),
            ]),
        )]);
        mk(vec![(DESCRIPTOR_FILE_NAME, leaf(1)), ("LICENSE", leaf(1)), ("x.gguf", l)])
    };
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("pickle", mk(vec![(DESCRIPTOR_FILE_NAME, leaf(1)), ("LICENSE", leaf(1)), ("pytorch_model.bin", leaf(1))])),
        (
            "archive",
            mk(vec![(DESCRIPTOR_FILE_NAME, leaf(1)), ("LICENSE", leaf(1)), ("w.gguf", leaf(1)), ("x.zip", leaf(1))]),
        ),
        ("dotdot", mk(vec![(DESCRIPTOR_FILE_NAME, leaf(1)), ("..", leaf(1)), ("w.gguf", leaf(1))])),
        ("device", mk(vec![(DESCRIPTOR_FILE_NAME, leaf(1)), ("NUL.json", leaf(1)), ("w.gguf", leaf(1))])),
        (
            "case",
            mk(vec![(DESCRIPTOR_FILE_NAME, leaf(1)), ("LICENSE", leaf(1)), ("License", leaf(1)), ("w.gguf", leaf(1))]),
        ),
        ("directory", mk(vec![(DESCRIPTOR_FILE_NAME, leaf(1)), ("sub", Value::dict([("w.gguf", leaf(1))]))])),
        ("symlink", symlink),
        ("attr", attr),
        ("many files", {
            let names: Vec<String> = (0..100_000).map(|i| format!("f{i}.gguf")).collect();
            mk(names.iter().map(|n| (n.as_str(), leaf(1))).collect())
        }),
        ("huge info", {
            let mut v = vec![b'd'];
            v.extend(b"4:name");
            v.extend(format!("{}:", 2 << 20).as_bytes());
            v.extend(std::iter::repeat_n(b'a', 2 << 20));
            v.push(b'e');
            v
        }),
    ];
    for (what, info) in cases {
        let ih = swarm.publish_raw(info, BTreeMap::new(), BTreeMap::new(), PeerBehaviour::Honest);
        d.handle(fetch(ih.0, Expect::default(), false), PeerRole::Control);
        let r = run_until(&mut d, ih.0, |s| s.is_terminal_failure());
        assert!(matches!(r.state, BundleState::Refused { .. }), "{what}: {:?}", r.state);
        assert!(!d.store().incomplete_dir(&ih).exists(), "{what}: a file was created");
    }

    // A declared size that lies.
    let fx = palw_fixture(&root.join("src"), "liar", 3);
    swarm.publish(fx.torrent(), fx.files.clone(), PeerBehaviour::Honest);
    let mut lie = fx.expect();
    lie.total_bytes = Some(lie.total_bytes.unwrap() + 1);
    d.handle(fetch(fx.ih(), lie, false), PeerRole::Control);
    let r = run_until(&mut d, fx.ih(), |s| s.is_terminal_failure());
    match r.state {
        BundleState::Refused { reason } => assert!(reason.contains("declaration"), "{reason}"),
        s => panic!("{s:?}"),
    }
    assert!(!d.store().incomplete_dir(&fx.packed.infohash()).exists());
    cleanup(&root);
}

/// L2: the bytes are the torrent's but the descriptor is not the declared one. The staging copy
/// is deleted and a new Fetch is the only retry.
#[test]
fn l2_mismatch_deletes_staging() {
    let root = tempdir("l2");
    let fx = palw_fixture(&root.join("src"), "decl", 4);
    let swarm = FakeSwarm::new();
    swarm.publish(fx.torrent(), fx.files.clone(), PeerBehaviour::Honest);
    let mut d = daemon(config(&root.join("home"), Profile::Desktop, vec![]), FakeEngine::new(swarm));
    let mut wrong = fx.expect();
    wrong.bundle_commitment = Some([9; 64]);
    d.handle(fetch(fx.ih(), wrong, true), PeerRole::Control);
    let r = run_until(&mut d, fx.ih(), |s| s.is_terminal_failure());
    match &r.state {
        BundleState::Mismatch { level: 2, reason } => assert!(reason.contains("bundle_commitment"), "{reason}"),
        s => panic!("{s:?}"),
    }
    assert!(!d.store().incomplete_dir(&fx.packed.infohash()).exists());
    assert!(!d.store().sealed_dir(&fx.packed.infohash()).exists());
    // Never retried silently: more ticks change nothing.
    for _ in 0..10 {
        d.tick();
    }
    assert!(matches!(status(&d, fx.ih()).state, BundleState::Mismatch { .. }));
    // A new request with the right anchor succeeds.
    d.handle(fetch(fx.ih(), fx.expect(), false), PeerRole::Control);
    run_until(&mut d, fx.ih(), |s| *s == BundleState::Idle);
    cleanup(&root);
}

/// D-T6: peers that send bad blocks or bad metadata are banned; the download completes from
/// honest peers.
#[test]
fn hostile_peers_are_banned() {
    let root = tempdir("peers");
    let fx = palw_fixture(&root.join("src"), "peers", 5);
    let swarm = FakeSwarm::new();
    swarm.publish(fx.torrent(), fx.files.clone(), PeerBehaviour::BadMetadata);
    swarm.publish(fx.torrent(), fx.files.clone(), PeerBehaviour::CorruptPieces);
    swarm.publish(fx.torrent(), fx.files.clone(), PeerBehaviour::Honest);
    let mut d = daemon(config(&root.join("home"), Profile::Desktop, vec![]), FakeEngine::new(swarm));
    d.handle(fetch(fx.ih(), fx.expect(), false), PeerRole::Control);
    let r = run_until(&mut d, fx.ih(), |s| *s == BundleState::Idle);
    assert_eq!(r.peers, 0, "no engine handle once idle");
    let path = std::path::PathBuf::from(r.path.unwrap());
    assert_eq!(std::fs::read(path.join("model.palwart")).unwrap(), fx.files["model.palwart"]);
    cleanup(&root);
}

#[test]
fn stalled_then_recovers_and_pause_resume() {
    let root = tempdir("stall");
    let fx = palw_fixture(&root.join("src"), "stall", 6);
    let swarm = FakeSwarm::new();
    // A peer with the metadata but no file bytes.
    swarm.publish(fx.torrent(), BTreeMap::new(), PeerBehaviour::Honest);
    let mut engine = FakeEngine::new(swarm.clone());
    engine.bytes_per_poll = 1 << 20;
    let mut d = daemon(config(&root.join("home"), Profile::Desktop, vec![]), engine);
    d.handle(fetch(fx.ih(), fx.expect(), false), PeerRole::Control);
    run_until(&mut d, fx.ih(), |s| *s == BundleState::Stalled);

    swarm.withdraw(&fx.packed.infohash());
    swarm.publish(fx.torrent(), fx.files.clone(), PeerBehaviour::Honest);
    run_until(&mut d, fx.ih(), |s| *s == BundleState::Downloading);

    assert_eq!(d.handle(Request::Pause { infohash: fx.ih() }, PeerRole::Control), Response::Ok);
    assert_eq!(status(&d, fx.ih()).state, BundleState::Paused);
    let before = status(&d, fx.ih()).done_bytes;
    for _ in 0..5 {
        d.tick();
    }
    assert_eq!(status(&d, fx.ih()).done_bytes, before, "paused means no progress");
    assert_eq!(d.handle(Request::Resume { infohash: fx.ih() }, PeerRole::Control), Response::Ok);
    run_until(&mut d, fx.ih(), |s| *s == BundleState::Idle);
    cleanup(&root);
}

#[test]
fn restart_restores_sealed_and_partial_bundles() {
    let root = tempdir("restart");
    let home = root.join("home");
    let sealed = palw_fixture(&root.join("src"), "sealed", 7);
    let partial = palw_fixture(&root.join("src"), "partial", 8);
    let swarm = FakeSwarm::new();
    swarm.publish(sealed.torrent(), sealed.files.clone(), PeerBehaviour::Honest);
    swarm.publish(partial.torrent(), partial.files.clone(), PeerBehaviour::Honest);
    {
        let mut engine = FakeEngine::new(swarm.clone());
        engine.bytes_per_poll = 1 << 20;
        let mut d = daemon(config(&home, Profile::Desktop, vec![]), engine);
        d.handle(fetch(sealed.ih(), sealed.expect(), true), PeerRole::Control);
        run_until(&mut d, sealed.ih(), |s| *s == BundleState::Seeding);
        d.handle(Request::SetLabel { infohash: sealed.ih(), label: AnchorLabel::ChainVerified }, PeerRole::Control);
        d.handle(fetch(partial.ih(), partial.expect(), false), PeerRole::Control);
        run_until(&mut d, partial.ih(), |s| *s == BundleState::Downloading);
        d.tick();
    }
    let mut d = daemon(config(&home, Profile::Desktop, vec![]), FakeEngine::new(swarm));
    let r = status(&d, sealed.ih());
    assert_eq!(r.state, BundleState::Seeding);
    assert_eq!(r.label, AnchorLabel::ChainVerified);
    assert_eq!(status(&d, partial.ih()).state, BundleState::Downloading);
    run_until(&mut d, partial.ih(), |s| *s == BundleState::Idle);
    assert_eq!(d.listen_port(), 50000);
    cleanup(&root);
}

#[test]
fn adopt_in_place() {
    let root = tempdir("adopt");
    let roots = root.join("roots");
    let fx = palw_fixture(&roots.join("hf"), "adopted", 9);
    let other = palw_fixture(&root.join("outside"), "outside", 10);
    let cfg =
        config(&root.join("home"), Profile::Desktop, vec![AdoptRoot { id: "models".into(), path: roots.clone() }]);
    let swarm = FakeSwarm::new();
    let mut d = daemon(cfg, FakeEngine::new(swarm.clone()));

    let adopt = |ih, rel: &str| Request::Adopt { infohash: ih, adopt_root_id: "models".into(), rel_dir: rel.into() };
    assert!(matches!(
        d.handle(adopt(fx.ih(), "../outside/outside"), PeerRole::Control),
        Response::Error { code: ErrorCode::Invalid, .. }
    ));
    assert!(matches!(
        d.handle(
            Request::Adopt { infohash: fx.ih(), adopt_root_id: "nope".into(), rel_dir: "x".into() },
            PeerRole::Control
        ),
        Response::Error { code: ErrorCode::NotFound, .. }
    ));

    assert_eq!(d.handle(adopt(fx.ih(), "hf/adopted"), PeerRole::Control), Response::Ok);
    let r = run_until(&mut d, fx.ih(), |s| *s == BundleState::Seeding);
    assert_eq!(r.path.as_deref(), Some(fx.dir.canonicalize().unwrap().to_str().unwrap()));
    assert_eq!(r.bundle_commitment, Some(fx.packed.commitment().0));

    // Another daemon fetches it from the adopted seeder.
    let mut d2 = daemon(config(&root.join("home2"), Profile::Desktop, vec![]), FakeEngine::new(swarm));
    d2.handle(fetch(fx.ih(), fx.expect(), false), PeerRole::Control);
    run_until(&mut d2, fx.ih(), |s| *s == BundleState::Idle);

    // A wrong expected infohash.
    #[cfg(unix)]
    {
        // A link out of the root is not followed.
        std::os::unix::fs::symlink(&other.dir, roots.join("escape")).unwrap();
        d.handle(adopt(other.ih(), "escape"), PeerRole::Control);
        let r = run_until(&mut d, other.ih(), |s| s.is_terminal_failure());
        assert!(matches!(r.state, BundleState::Mismatch { .. }), "{:?}", r.state);
    }
    let wrong = palw_fixture(&roots.join("w"), "wrong", 11);
    d.handle(adopt(fx.ih().map(|b| b ^ 1), "w/wrong"), PeerRole::Control);
    let r = run_until(&mut d, fx.ih().map(|b| b ^ 1), |s| s.is_terminal_failure());
    match r.state {
        BundleState::Mismatch { reason, .. } => {
            assert!(reason.contains(&wrong.packed.infohash().to_string()), "{reason}")
        }
        s => panic!("{s:?}"),
    }
    // Adopted files are never deleted by Remove.
    d.handle(Request::Remove { infohash: fx.ih(), delete_files: true }, PeerRole::Control);
    assert!(fx.dir.join("model.palwart").exists());
    cleanup(&root);
}

#[test]
fn read_only_peers_and_quota() {
    let root = tempdir("ro");
    let fx = palw_fixture(&root.join("src"), "ro", 12);
    let swarm = FakeSwarm::new();
    swarm.publish(fx.torrent(), fx.files.clone(), PeerBehaviour::Honest);
    let mut cfg = config(&root.join("home"), Profile::Desktop, vec![]);
    cfg.quota_bytes = Some(1000);
    let mut d = daemon(cfg, FakeEngine::new(swarm));
    assert!(matches!(
        d.handle(fetch(fx.ih(), fx.expect(), false), PeerRole::ReadOnly),
        Response::Error { code: ErrorCode::Forbidden, .. }
    ));
    assert!(matches!(d.handle(Request::Status { infohash: None }, PeerRole::ReadOnly), Response::Status(_)));
    d.handle(fetch(fx.ih(), fx.expect(), false), PeerRole::Control);
    let r = run_until(&mut d, fx.ih(), |s| s.is_terminal_failure());
    match r.state {
        BundleState::Refused { reason } => assert!(reason.contains("quota"), "{reason}"),
        s => panic!("{s:?}"),
    }
    cleanup(&root);
}

#[test]
fn incomplete_garbage_is_collected() {
    let root = tempdir("gc");
    let home = root.join("home");
    let mut cfg = config(&home, Profile::Desktop, vec![]);
    cfg.incomplete_gc = std::time::Duration::from_secs(0);
    let mut d = daemon(cfg, FakeEngine::new(FakeSwarm::new()));
    let orphan = d.store().incomplete.join("ab".repeat(32));
    std::fs::create_dir_all(orphan.join("t")).unwrap();
    std::fs::write(orphan.join("t/x"), "x").unwrap();
    // An active fetch is kept even though its directory is "old".
    let active = misaka_btv2::Infohash([0xcd; 32]);
    d.handle(fetch(active.0, Expect::default(), false), PeerRole::Control);
    std::fs::create_dir_all(d.store().incomplete_dir(&active)).unwrap();
    d.collect_garbage();
    assert!(!orphan.exists());
    assert!(d.store().incomplete_dir(&active).exists());
    cleanup(&root);
}

/// An engine that reports "finished" before admission (libtorrent does, for a torrent whose files
/// are all at priority 0) must not make L2 read a partial bundle.
#[test]
fn early_finished_is_ignored() {
    let root = tempdir("early");
    let fx = palw_fixture(&root.join("src"), "early", 13);
    let swarm = FakeSwarm::new();
    swarm.publish(fx.torrent(), fx.files.clone(), PeerBehaviour::Honest);
    let mut engine = FakeEngine::new(swarm);
    engine.finish_early = true;
    engine.bytes_per_poll = 1 << 20;
    let mut d = daemon(config(&root.join("home"), Profile::Desktop, vec![]), engine);
    d.handle(fetch(fx.ih(), fx.expect(), false), PeerRole::Control);
    let r = run_until(&mut d, fx.ih(), |s| *s == BundleState::Idle || s.is_terminal_failure());
    assert_eq!(r.state, BundleState::Idle, "{:?}", r.state);
    cleanup(&root);
}
