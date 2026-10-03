#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use misaka_btv2::torrent::Torrent;
use misaka_bundle::{BundleKind, DESCRIPTOR_FILE_NAME, Hex64, PalwRoot};
use misaka_torrentd::config::{AdoptRoot, Config, Profile};
use misaka_torrentd::core::Daemon;
use misaka_transport_engine::Limits;
use misaka_transport_engine::fake::FakeEngine;
use misaka_transport_ipc::{BundleReport, BundleState, Expect};
use misaka_transport_policy::pack::{self, CreateOptions, Packed};

pub fn tempdir(tag: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("mtd-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A bundle on disk and in memory.
pub struct Fixture {
    pub dir: PathBuf,
    pub packed: Packed,
    pub files: BTreeMap<String, Vec<u8>>,
}

impl Fixture {
    pub fn torrent(&self) -> &Torrent {
        &self.packed.torrent
    }

    pub fn expect(&self) -> Expect {
        Expect {
            bundle_commitment: Some(self.packed.commitment().0),
            total_bytes: Some(self.packed.total_bytes()),
            kind: Some(self.packed.descriptor.kind.id()),
        }
    }

    pub fn ih(&self) -> [u8; 32] {
        self.packed.infohash().0
    }
}

fn noise(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

/// A palw-artifact bundle of ~3 MiB in `<parent>/<title>/`.
pub fn palw_fixture(parent: &Path, title: &str, seed: u32) -> Fixture {
    let dir = parent.join(title);
    std::fs::create_dir_all(&dir).unwrap();
    let mut art = b"PALWB0A2".to_vec();
    art.extend(noise(3 << 20, seed));
    std::fs::write(dir.join("model.palwart"), &art).unwrap();
    std::fs::write(dir.join("LICENSE"), "Apache License 2.0").unwrap();
    std::fs::write(dir.join("README.md"), "# model\n").unwrap();
    let packed = pack::create(
        &dir,
        &CreateOptions {
            kind: BundleKind::PalwArtifact,
            title: title.into(),
            spdx: "Apache-2.0".into(),
            license_file: "LICENSE".into(),
            palw_roots: vec![PalwRoot { class_id: Hex64([1; 64]), inventory_root: Hex64([2; 64]) }],
            artifact_digest: None,
        },
    )
    .unwrap();
    std::fs::write(dir.join(DESCRIPTOR_FILE_NAME), &packed.descriptor_bytes).unwrap();
    let files = ["model.palwart", "LICENSE", "README.md", DESCRIPTOR_FILE_NAME]
        .iter()
        .map(|n| (n.to_string(), std::fs::read(dir.join(n)).unwrap()))
        .collect();
    Fixture { dir, packed, files }
}

pub fn config(home: &Path, profile: Profile, adopt_roots: Vec<AdoptRoot>) -> Config {
    Config {
        profile,
        config_file: None,
        home: home.to_owned(),
        socket: home.join("run/torrentd.sock"),
        listen_port: Some(50000),
        seeding_enabled: profile == Profile::Desktop,
        limits: Limits { upload_bytes_per_sec: 0, download_bytes_per_sec: 0, connections: 10 },
        upnp_natpmp: false,
        lsd: false,
        dht: false,
        dht_bootstrap: vec![],
        quota_bytes: Some(1 << 40),
        incomplete_gc: Duration::from_secs(14 * 86_400),
        allowed_uids: vec![],
        read_only_gid: None,
        adopt_roots,
        mode: misaka_torrentd::config::NetworkMode::Direct,
        i2p: None,
    }
}

pub fn daemon(cfg: Config, engine: FakeEngine) -> Daemon<FakeEngine> {
    Daemon::with_free_space(cfg, engine, "test".into(), Box::new(|_| Some(1 << 40))).unwrap()
}

pub fn status(d: &Daemon<FakeEngine>, ih: [u8; 32]) -> BundleReport {
    d.report(&misaka_btv2::Infohash(ih)).expect("bundle present")
}

/// Ticks until `pred` holds or about five seconds pass; returns the last report.
pub fn run_until(d: &mut Daemon<FakeEngine>, ih: [u8; 32], pred: impl Fn(&BundleState) -> bool) -> BundleReport {
    for _ in 0..5000 {
        d.tick();
        std::thread::sleep(Duration::from_millis(1));
        let r = status(d, ih);
        if pred(&r.state) {
            return r;
        }
    }
    panic!("state never reached; last {:?}", status(d, ih).state);
}

#[cfg(unix)]
pub fn mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// Removes a test tree, sealed directories included.
pub fn cleanup(root: &Path) {
    #[cfg(unix)]
    fn open_up(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(m) = std::fs::symlink_metadata(p)
            && m.is_dir()
        {
            let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700));
            for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
                open_up(&e.path());
            }
        }
    }
    #[cfg(unix)]
    open_up(root);
    std::fs::remove_dir_all(root).unwrap();
}
