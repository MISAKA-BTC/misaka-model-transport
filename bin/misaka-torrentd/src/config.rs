//! Configuration (§6.4).
//!
//! - The file is `~/.misaka/torrent.toml` on a desktop and `/etc/misaka/torrent.toml` on a
//!   server, parsed with unknown fields refused. It is its own file: misakas parses
//!   `~/.misaka/config.toml` with `deny_unknown_fields`, and a new section there would break every
//!   older `misaka` on the host.
//! - `MISAKA_TORRENT_*` environment variables set paths and the profile only.
//! - Precedence: the command line, then the environment, then the file, then the default.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use misaka_transport_engine::Limits;

const MIB: u64 = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    Desktop,
    Server,
}

impl Profile {
    pub fn as_str(self) -> &'static str {
        match self {
            Profile::Desktop => "desktop",
            Profile::Server => "server",
        }
    }
}

impl std::str::FromStr for Profile {
    type Err = ConfigError;
    fn from_str(s: &str) -> Result<Self, ConfigError> {
        match s {
            "desktop" => Ok(Profile::Desktop),
            "server" => Ok(Profile::Server),
            _ => Err(ConfigError::Invalid(format!("profile {s:?} is not desktop or server"))),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    profile: Option<Profile>,
    /// The root of `incomplete/`, `store/` and `state/`.
    home: Option<PathBuf>,
    socket: Option<PathBuf>,
    listen_port: Option<u16>,
    #[serde(default)]
    seeding: SeedingFile,
    #[serde(default)]
    network: NetworkFile,
    #[serde(default)]
    i2p: I2pFile,
    #[serde(default)]
    store: StoreFile,
    #[serde(default)]
    ipc: IpcFile,
    #[serde(default)]
    adopt_roots: Vec<AdoptRootFile>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeedingFile {
    /// Seed after download and accept `Seed` / `Adopt`. Off by default on a server (MT-12).
    enabled: Option<bool>,
    upload_bytes_per_sec: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct NetworkFile {
    /// RFC-0002: "direct" (the default), "private" or "anonymous".
    mode: Option<NetworkMode>,
    connections: Option<u32>,
    download_bytes_per_sec: Option<u64>,
    upnp_natpmp: Option<bool>,
    lsd: Option<bool>,
    dht: Option<bool>,
    dht_bootstrap: Option<Vec<String>>,
}

/// RFC-0002 §2: the transport of one daemon instance, fixed for its lifetime (MP-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkMode {
    #[default]
    Direct,
    /// I2P, one hop each way: hides the address from peers and trackers (low-latency privacy).
    Private,
    /// I2P, three hops each way.
    Anonymous,
}

impl NetworkMode {
    pub fn as_str(self) -> &'static str {
        match self {
            NetworkMode::Direct => "direct",
            NetworkMode::Private => "private",
            NetworkMode::Anonymous => "anonymous",
        }
    }
    pub fn is_i2p(self) -> bool {
        self != NetworkMode::Direct
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct I2pFile {
    /// The router's SAM bridge, `host:port` on loopback (default `127.0.0.1:7656`).
    sam: Option<String>,
    /// `.i2p` HTTP trackers, announced to all (RFC-0002 §4.1).
    #[serde(default)]
    trackers: Vec<String>,
    /// Tunnels per direction, 1–16 (default 4 in Private mode, 3 in Anonymous).
    tunnels: Option<u8>,
}

/// The resolved I2P settings of an instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I2pConfig {
    pub sam_host: String,
    pub sam_port: u16,
    pub hops: u8,
    pub tunnels: u8,
    pub trackers: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreFile {
    quota_bytes: Option<u64>,
    incomplete_gc_days: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct IpcFile {
    /// Uids besides the daemon's own that may control it (a server's operator).
    #[serde(default)]
    allowed_uids: Vec<u32>,
    /// A group whose members may only read status.
    read_only_gid: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdoptRootFile {
    id: String,
    path: PathBuf,
}

/// An adopt root: a directory the daemon may read, named by id over IPC (§4.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptRoot {
    pub id: String,
    pub path: PathBuf,
}

/// The resolved configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub profile: Profile,
    pub config_file: Option<PathBuf>,
    pub home: PathBuf,
    pub socket: PathBuf,
    pub listen_port: Option<u16>,
    pub seeding_enabled: bool,
    pub limits: Limits,
    pub upnp_natpmp: bool,
    pub lsd: bool,
    pub dht: bool,
    pub dht_bootstrap: Vec<String>,
    pub quota_bytes: Option<u64>,
    pub incomplete_gc: Duration,
    pub allowed_uids: Vec<u32>,
    pub read_only_gid: Option<u32>,
    pub adopt_roots: Vec<AdoptRoot>,
    pub mode: NetworkMode,
    /// Present exactly when `mode` is an I2P mode.
    pub i2p: Option<I2pConfig>,
}

/// What the command line sets.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub profile: Option<Profile>,
    pub config: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub socket: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("{path}: {source}")]
    Parse { path: PathBuf, source: Box<toml::de::Error> },
    #[error("{0}")]
    Invalid(String),
}

/// The public DHT routers, as the fallback of Open question 11. MISAKA-operated bootstrap nodes
/// go first in `network.dht_bootstrap` once they exist; the list is the operator's.
pub const DEFAULT_DHT_BOOTSTRAP: &[&str] = &[
    "dht.libtorrent.org:25401",
    "router.bittorrent.com:6881",
    "dht.transmissionbt.com:6881",
    "router.utorrent.com:6881",
];

fn home_dir() -> Result<PathBuf, ConfigError> {
    std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| ConfigError::Invalid("HOME is not set".into()))
}

impl Config {
    /// Resolves the configuration from the command line, the environment (`env`) and the file.
    pub fn resolve(cli: &Overrides, env: &dyn Fn(&str) -> Option<String>) -> Result<Config, ConfigError> {
        let env_path = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let env_profile = env("MISAKA_TORRENT_PROFILE").map(|p| p.parse::<Profile>()).transpose()?;
        // The profile picks the default file, so it is read first from what is not the file.
        let pre_profile = cli.profile.or(env_profile);
        let config_file = match cli.config.clone().or_else(|| env_path("MISAKA_TORRENT_CONFIG")) {
            Some(p) => Some(p),
            None => {
                let p = match pre_profile.unwrap_or(Profile::Desktop) {
                    Profile::Desktop => home_dir()?.join(".misaka/torrent.toml"),
                    Profile::Server => PathBuf::from("/etc/misaka/torrent.toml"),
                };
                p.exists().then_some(p)
            }
        };
        let file = match &config_file {
            Some(p) => {
                let text =
                    std::fs::read_to_string(p).map_err(|source| ConfigError::Read { path: p.clone(), source })?;
                toml::from_str::<FileConfig>(&text)
                    .map_err(|source| ConfigError::Parse { path: p.clone(), source: Box::new(source) })?
            }
            None => FileConfig::default(),
        };
        let profile = pre_profile.or(file.profile).unwrap_or(Profile::Desktop);
        let desktop = profile == Profile::Desktop;

        let home = match cli.home.clone().or_else(|| env_path("MISAKA_TORRENT_HOME")).or(file.home) {
            Some(h) => h,
            None if desktop => home_dir()?.join(".misaka/torrent"),
            None => PathBuf::from("/var/lib/misaka-torrent"),
        };
        let socket = match cli.socket.clone().or_else(|| env_path("MISAKA_TORRENT_SOCKET")).or(file.socket) {
            Some(s) => s,
            None => default_socket(profile, &home, env),
        };

        let mut adopt_roots = Vec::new();
        for r in file.adopt_roots {
            if r.id.is_empty() || !r.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
                return Err(ConfigError::Invalid(format!("adopt root id {:?} is not [A-Za-z0-9_-]+", r.id)));
            }
            if !r.path.is_absolute() {
                return Err(ConfigError::Invalid(format!("adopt root {:?} is not an absolute path", r.path)));
            }
            if adopt_roots.iter().any(|x: &AdoptRoot| x.id == r.id) {
                return Err(ConfigError::Invalid(format!("adopt root id {:?} repeats", r.id)));
            }
            adopt_roots.push(AdoptRoot { id: r.id, path: r.path });
        }

        let seeding_enabled = file.seeding.enabled.unwrap_or(desktop);
        let limits = Limits {
            upload_bytes_per_sec: file.seeding.upload_bytes_per_sec.unwrap_or(if desktop { 20 * MIB } else { 0 }),
            download_bytes_per_sec: file.network.download_bytes_per_sec.unwrap_or(0),
            connections: file.network.connections.unwrap_or(if desktop { 200 } else { 100 }),
        };
        let mode = file.network.mode.unwrap_or_default();
        let i2p = if mode.is_i2p() {
            let sam = file.i2p.sam.unwrap_or_else(|| "127.0.0.1:7656".into());
            let (host, port) = sam
                .rsplit_once(':')
                .and_then(|(h, p)| Some((h.to_owned(), p.parse::<u16>().ok()?)))
                .ok_or_else(|| ConfigError::Invalid(format!("i2p.sam {sam:?} is not host:port")))?;
            if !matches!(host.as_str(), "127.0.0.1" | "::1" | "[::1]" | "localhost") {
                return Err(ConfigError::Invalid("i2p.sam must be on loopback (RFC-0002 §7)".into()));
            }
            if file.i2p.trackers.is_empty() {
                return Err(ConfigError::Invalid(
                    "an I2P mode needs at least one i2p.trackers entry (RFC-0002 §4.1)".into(),
                ));
            }
            for t in &file.i2p.trackers {
                let host = t.strip_prefix("http://").and_then(|r| r.split('/').next()).unwrap_or("");
                if !host.ends_with(".i2p") || t.contains(',') {
                    return Err(ConfigError::Invalid(format!("tracker {t:?} is not an http://….i2p URL")));
                }
            }
            if file.network.dht == Some(true)
                || file.network.lsd == Some(true)
                || file.network.upnp_natpmp == Some(true)
            {
                return Err(ConfigError::Invalid("an I2P mode uses no DHT, LSD or UPnP (RFC-0002 MP-3)".into()));
            }
            let tunnels = file.i2p.tunnels.unwrap_or(if mode == NetworkMode::Private { 4 } else { 3 });
            if !(1..=16).contains(&tunnels) {
                return Err(ConfigError::Invalid("i2p.tunnels must be 1–16".into()));
            }
            Some(I2pConfig {
                sam_host: host.trim_matches(['[', ']']).to_owned(),
                sam_port: port,
                hops: if mode == NetworkMode::Private { 1 } else { 3 },
                tunnels,
                trackers: file.i2p.trackers,
            })
        } else {
            if !file.i2p.trackers.is_empty() || file.i2p.sam.is_some() {
                return Err(ConfigError::Invalid("[i2p] is set but network.mode is direct".into()));
            }
            None
        };
        if let Some(0) = file.store.incomplete_gc_days {
            return Err(ConfigError::Invalid("store.incomplete_gc_days must be at least 1".into()));
        }
        Ok(Config {
            profile,
            config_file,
            home,
            socket,
            listen_port: file.listen_port,
            seeding_enabled,
            limits,
            upnp_natpmp: !mode.is_i2p() && file.network.upnp_natpmp.unwrap_or(desktop),
            lsd: !mode.is_i2p() && file.network.lsd.unwrap_or(false),
            dht: !mode.is_i2p() && file.network.dht.unwrap_or(true),
            dht_bootstrap: file
                .network
                .dht_bootstrap
                .unwrap_or_else(|| DEFAULT_DHT_BOOTSTRAP.iter().map(|s| s.to_string()).collect()),
            quota_bytes: file.store.quota_bytes,
            incomplete_gc: Duration::from_secs(u64::from(file.store.incomplete_gc_days.unwrap_or(14)) * 86_400),
            allowed_uids: file.ipc.allowed_uids,
            read_only_gid: file.ipc.read_only_gid,
            adopt_roots,
            mode,
            i2p,
        })
    }

    pub fn adopt_root(&self, id: &str) -> Option<&AdoptRoot> {
        self.adopt_roots.iter().find(|r| r.id == id)
    }
}

/// `$XDG_RUNTIME_DIR/misaka-torrent/torrentd.sock` on a desktop that has one, else
/// `<home>/run/torrentd.sock`; `/run/misaka-torrent/torrentd.sock` on a server.
pub fn default_socket(profile: Profile, home: &Path, env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    match profile {
        Profile::Server => PathBuf::from("/run/misaka-torrent/torrentd.sock"),
        Profile::Desktop => match env("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
            Some(x) => PathBuf::from(x).join("misaka-torrent/torrentd.sock"),
            None => home.join("run/torrentd.sock"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| m.get(k).cloned()
    }

    fn write_tmp(text: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("mt-cfg-{}-{n}.toml", std::process::id()));
        std::fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn profiles_and_precedence() {
        let file = write_tmp("profile = \"server\"\nhome = \"/srv/file\"\n[seeding]\nupload_bytes_per_sec = 5\n");
        let cli = Overrides { config: Some(file.clone()), ..Default::default() };
        let c = Config::resolve(&cli, &env_of(&[])).unwrap();
        assert_eq!(c.profile, Profile::Server);
        assert!(!c.seeding_enabled, "MT-12: a server does not seed unless enabled");
        assert_eq!(c.home, PathBuf::from("/srv/file"));
        assert_eq!(c.limits.upload_bytes_per_sec, 5);
        assert_eq!(c.socket, PathBuf::from("/run/misaka-torrent/torrentd.sock"));
        // The environment beats the file; the command line beats both.
        let c = Config::resolve(
            &cli,
            &env_of(&[("MISAKA_TORRENT_HOME", "/srv/env"), ("MISAKA_TORRENT_PROFILE", "desktop")]),
        )
        .unwrap();
        assert_eq!(c.home, PathBuf::from("/srv/env"));
        assert_eq!(c.profile, Profile::Desktop);
        assert!(c.seeding_enabled);
        let cli2 = Overrides { home: Some("/srv/cli".into()), ..cli };
        let c = Config::resolve(&cli2, &env_of(&[("MISAKA_TORRENT_HOME", "/srv/env")])).unwrap();
        assert_eq!(c.home, PathBuf::from("/srv/cli"));
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn refuses_unknown_fields() {
        let file = write_tmp("profile = \"desktop\"\n[seeding]\nenabled = true\nrewards = true\n");
        let cli = Overrides { config: Some(file.clone()), ..Default::default() };
        assert!(matches!(Config::resolve(&cli, &env_of(&[])), Err(ConfigError::Parse { .. })));
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn i2p_modes() {
        let file = write_tmp("[network]\nmode = \"anonymous\"\n[i2p]\ntrackers = [\"http://abcd.b32.i2p/announce\"]\n");
        let cli = Overrides { config: Some(file.clone()), home: Some("/tmp/x".into()), ..Default::default() };
        let c = Config::resolve(&cli, &env_of(&[])).unwrap();
        assert_eq!(c.mode, NetworkMode::Anonymous);
        let i = c.i2p.unwrap();
        assert_eq!((i.sam_host.as_str(), i.sam_port, i.hops, i.tunnels), ("127.0.0.1", 7656, 3, 3));
        assert!(!c.dht && !c.lsd && !c.upnp_natpmp, "MP-3");
        std::fs::remove_file(file).unwrap();
        for bad in [
            "[network]\nmode = \"private\"\n",
            "[network]\nmode = \"private\"\n[i2p]\ntrackers = [\"http://tracker.example.com/announce\"]\n",
            "[network]\nmode = \"private\"\ndht = true\n[i2p]\ntrackers = [\"http://a.b32.i2p/a\"]\n",
            "[network]\nmode = \"private\"\n[i2p]\nsam = \"10.0.0.1:7656\"\ntrackers = [\"http://a.b32.i2p/a\"]\n",
            "[i2p]\ntrackers = [\"http://a.b32.i2p/a\"]\n",
        ] {
            let file = write_tmp(bad);
            let cli = Overrides { config: Some(file.clone()), home: Some("/tmp/x".into()), ..Default::default() };
            assert!(matches!(Config::resolve(&cli, &env_of(&[])), Err(ConfigError::Invalid(_))), "{bad}");
            std::fs::remove_file(file).unwrap();
        }
    }

    #[test]
    fn adopt_roots_are_checked() {
        let file = write_tmp("[[adopt_roots]]\nid = \"hf\"\npath = \"relative\"\n");
        let cli = Overrides { config: Some(file.clone()), home: Some("/tmp/x".into()), ..Default::default() };
        assert!(matches!(Config::resolve(&cli, &env_of(&[])), Err(ConfigError::Invalid(_))));
        std::fs::remove_file(file).unwrap();
    }
}
