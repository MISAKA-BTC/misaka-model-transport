//! `misaka-torrentd run --profile desktop|server` (RFC-0001 §7.2).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use misaka_torrentd::config::{Config, Overrides, Profile};

#[derive(Parser)]
#[command(
    name = "misaka-torrentd",
    version,
    about = "MISAKA Torrent daemon: transport only — no chain, no keys, no execution"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the daemon in the foreground.
    Run(Common),
    /// Print the resolved configuration and exit.
    Config(Common),
    /// Drill D-T9: apply the daemon's confinement to this process, then check that the store is
    /// writable and that the given paths cannot be read and nothing can be executed.
    DrillConfinement {
        #[command(flatten)]
        common: Common,
        /// Paths the daemon must not be able to read (key seeds, ~/.ssh, kaspad's datadir).
        #[arg(long = "forbidden", required = true)]
        forbidden: Vec<PathBuf>,
    },
}

#[derive(clap::Args)]
struct Common {
    #[arg(long, value_enum)]
    profile: Option<Profile>,
    /// The configuration file (default ~/.misaka/torrent.toml or /etc/misaka/torrent.toml).
    #[arg(long)]
    config: Option<PathBuf>,
    /// The store root: incomplete/, store/, state/.
    #[arg(long)]
    home: Option<PathBuf>,
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Start an I2P mode even though clearnet packets are not blocked below the engine
    /// (RFC-0002 MP-4). For testing only.
    #[arg(long)]
    accept_unconfined_network: bool,
}

impl Common {
    fn resolve(&self) -> Result<Config, String> {
        let o = Overrides {
            profile: self.profile,
            config: self.config.clone(),
            home: self.home.clone(),
            socket: self.socket.clone(),
        };
        Config::resolve(&o, &|k| std::env::var(k).ok()).map_err(|e| e.to_string())
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let r = match cli.cmd {
        Cmd::Config(c) => c.resolve().map(|cfg| println!("{cfg:#?}")),
        Cmd::Run(c) => {
            let accept = c.accept_unconfined_network;
            c.resolve().and_then(|cfg| run(cfg, accept))
        }
        Cmd::DrillConfinement { common, forbidden } => common.resolve().and_then(|cfg| drill(cfg, &forbidden)),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("misaka-torrentd: {e}");
            ExitCode::FAILURE
        }
    }
}

fn drill(cfg: Config, forbidden: &[PathBuf]) -> Result<(), String> {
    use misaka_torrentd::confine::{self, Paths};
    let store = misaka_torrentd::store::Store::open(&cfg.home, cfg.profile).map_err(|e| e.to_string())?;
    let sock_dir = cfg.socket.parent().ok_or("socket path has no directory")?.to_owned();
    std::fs::create_dir_all(&sock_dir).map_err(|e| e.to_string())?;
    let mut read_only: Vec<PathBuf> = cfg.adopt_roots.iter().map(|r| r.path.clone()).collect();
    read_only.extend(cfg.config_file.clone());
    let confinement = confine::apply(&Paths { read_write: vec![cfg.home.clone(), sock_dir], read_only })?;
    println!("confinement  {confinement}");
    let mut failed = 0;
    let mut check = |ok: bool, what: String| {
        println!("{}  {what}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            failed += 1;
        }
    };
    let probe = store.state.join("drill-probe");
    check(std::fs::write(&probe, b"x").is_ok() && std::fs::remove_file(&probe).is_ok(), "the store is writable".into());
    for root in &cfg.adopt_roots {
        check(std::fs::read_dir(&root.path).is_ok(), format!("adopt root {} is readable", root.path.display()));
        let w = root.path.join(".misaka-drill-probe");
        check(std::fs::write(&w, b"x").is_err(), format!("adopt root {} is not writable", root.path.display()));
    }
    for p in forbidden {
        let readable = std::fs::read(p).is_ok() || std::fs::read_dir(p).is_ok();
        check(!readable, format!("{} cannot be read", p.display()));
    }
    let exec = std::process::Command::new("/bin/true").status();
    check(
        exec.is_err(),
        format!("exec is refused ({})", exec.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string())),
    );
    let tmp = std::env::temp_dir().join("misaka-drill-escape");
    check(std::fs::write(&tmp, b"x").is_err(), format!("{} is not writable", tmp.display()));
    if failed == 0 { Ok(()) } else { Err(format!("{failed} check(s) failed")) }
}

#[cfg(all(unix, feature = "libtorrent"))]
fn run(cfg: Config, accept_unconfined: bool) -> Result<(), String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use misaka_torrentd::confine::{self, Paths};
    use misaka_torrentd::core::{Daemon, listen_port_for};
    use misaka_torrentd::server::{self, Access};
    use misaka_torrentd::store::Store;
    use misaka_transport_engine::EngineSettings;
    use misaka_transport_engine::libtorrent::LibtorrentEngine;

    static STOP: AtomicBool = AtomicBool::new(false);
    extern "C" fn on_signal(_: libc::c_int) {
        STOP.store(true, Ordering::Relaxed);
    }
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
    }

    // The store and the socket directory exist before confinement narrows what can be created.
    let store = Store::open(&cfg.home, cfg.profile).map_err(|e| format!("store {}: {e}", cfg.home.display()))?;
    let sock_dir = cfg.socket.parent().ok_or("socket path has no directory")?.to_owned();
    std::fs::create_dir_all(&sock_dir).map_err(|e| format!("{}: {e}", sock_dir.display()))?;
    let listen_port = listen_port_for(&cfg, &store);

    // Confinement before the engine starts a thread: Landlock binds the threads created after it.
    let mut read_only: Vec<PathBuf> = cfg.adopt_roots.iter().map(|r| r.path.clone()).collect();
    read_only.extend(cfg.config_file.clone());
    let mut confinement = confine::apply(&Paths { read_write: vec![cfg.home.clone(), sock_dir], read_only })?;
    if let Some(i2p) = &cfg.i2p {
        // RFC-0002 MP-4: an I2P mode runs only where clearnet packets cannot leave this process.
        match confine::clearnet_blocked() {
            Ok(()) => confinement.push_str(&format!(
                "; network {} (I2P, {} hop{} each way, {} tunnels) — clearnet blocked below the engine",
                cfg.mode.as_str(),
                i2p.hops,
                if i2p.hops == 1 { "" } else { "s" },
                i2p.tunnels
            )),
            Err(e) if accept_unconfined => {
                eprintln!("misaka-torrentd: WARNING: {e}; continuing because --accept-unconfined-network was given");
                confinement.push_str(&format!("; network {} (I2P) — CLEARNET NOT BLOCKED", cfg.mode.as_str()));
            }
            Err(e) => {
                return Err(format!(
                    "network mode {} needs clearnet blocked below the engine (RFC-0002 MP-4): {e}. Run it in a \
                     loopback-only network namespace or a systemd unit with IPAddressDeny=any / IPAddressAllow=localhost",
                    cfg.mode.as_str()
                ));
            }
        }
    }
    eprintln!("misaka-torrentd: confinement: {confinement}");

    let engine = LibtorrentEngine::new(&EngineSettings {
        listen_port,
        limits: cfg.limits,
        upnp_natpmp: cfg.upnp_natpmp,
        lsd: cfg.lsd,
        dht: cfg.dht,
        dht_bootstrap: cfg.dht_bootstrap.clone(),
        state_dir: store.state.clone(),
        i2p: cfg.i2p.as_ref().map(|i| misaka_transport_engine::I2pSettings {
            sam_host: i.sam_host.clone(),
            sam_port: i.sam_port,
            inbound_length: i.hops,
            outbound_length: i.hops,
            inbound_quantity: i.tunnels,
            outbound_quantity: i.tunnels,
        }),
        trackers: cfg.i2p.as_ref().map(|i| i.trackers.clone()).unwrap_or_default(),
    })
    .map_err(|e| e.to_string())?;
    let socket = cfg.socket.clone();
    let profile = cfg.profile;
    let access = Access::from_config(&cfg);
    let daemon = Daemon::new(cfg, engine, confinement).map_err(|e| e.to_string())?;
    let listener = server::bind(&socket, profile).map_err(|e| format!("{}: {e}", socket.display()))?;
    eprintln!(
        "misaka-torrentd: {} profile, peer port {}, socket {}",
        profile.as_str(),
        daemon.listen_port(),
        socket.display()
    );
    let stop = Arc::new(AtomicBool::new(false));
    let watcher = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            while !STOP.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            stop.store(true, Ordering::Relaxed);
        })
    };
    let daemon = Arc::new(Mutex::new(daemon));
    server::serve(listener, socket, daemon, access, stop).map_err(|e| e.to_string())?;
    let _ = watcher.join();
    Ok(())
}

#[cfg(not(all(unix, feature = "libtorrent")))]
fn run(_: Config, _: bool) -> Result<(), String> {
    Err("this build has no BitTorrent engine: rebuild with `--features libtorrent` \
         (libtorrent-rasterbar 2.0.x, RFC-0001 §6.1). Windows is not supported yet (Open question 8)."
        .into())
}
