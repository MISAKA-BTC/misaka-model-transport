//! `misaka-torrent` — this repository's chain-agnostic tool (RFC-0001 §7.2).
//!
//! | command | does |
//! | --- | --- |
//! | `create <dir> --kind <kind> --title <title> --license <spdx>` | write `misaka-bundle.json` and `<title>.torrent`; print the commitment, infohash and magnet |
//! | `inspect <dir\|file.torrent>` | check the descriptor and the info dictionary against the rule and the profile |
//! | `share <dir>` | `create`, then seed in place; an unanchored share |
//! | `pull <magnet> [--expect <commitment>]` | fetch and L2 |
//! | `seed start\|stop\|list\|adopt` | seeding control (§4.5) |
//! | `status [--json]` | §6.6 |
//!
//! Anything that touches the chain — resolving `line@version`, L3, installing, declaring — is
//! misakas's `misaka model …`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use misaka_btv2::magnet;
use misaka_btv2::torrent::{Infohash, Torrent};
use misaka_bundle::{BundleKind, DESCRIPTOR_FILE_NAME, Hex64, PalwRoot};
use misaka_torrentd::config::{Config, Overrides, Profile};
use misaka_transport_ipc::client::Client;
use misaka_transport_ipc::{BundleReport, BundleState, Expect, Request, SetLimits};
use misaka_transport_policy::limits::{MAX_TORRENT_FILE_BYTES, TORRENT_DECODE_LIMITS};
use misaka_transport_policy::pack::{self, CreateOptions, Packed};
use misaka_transport_policy::{AdmissionEnv, Expectation, admit, check_torrent_limits};

#[derive(Parser)]
#[command(name = "misaka-torrent", version, about = "MISAKA Torrent: BitTorrent v2 transport for MISAKA model bundles")]
struct Cli {
    /// The daemon's socket (default: from MISAKA_TORRENT_SOCKET or the profile).
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    #[arg(long, global = true, value_enum)]
    profile: Option<Profile>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write misaka-bundle.json and <title>.torrent for the files in <dir>.
    Create(CreateArgs),
    /// Check a bundle directory or a .torrent against rule v1 and the Safe Model Profile.
    Inspect { path: PathBuf },
    /// Create, then seed <dir> in place (it must be under an adopt root). An unanchored share.
    Share(CreateArgs),
    /// Fetch a bundle by magnet, check it at L2, and seal it in the store.
    Pull(PullArgs),
    /// Seeding control.
    #[command(subcommand)]
    Seed(SeedCmd),
    /// Show bundles: state, anchor label, progress, peers.
    Status {
        infohash: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Remove a bundle from the daemon; with --delete-files, its sealed copy too.
    Remove {
        infohash: String,
        #[arg(long)]
        delete_files: bool,
    },
    /// Set upload/download caps (bytes/s, 0 = unlimited) and the connection limit.
    Limits {
        #[arg(long)]
        upload: Option<u64>,
        #[arg(long)]
        download: Option<u64>,
        #[arg(long)]
        connections: Option<u32>,
    },
}

#[derive(Args, Clone)]
struct CreateArgs {
    dir: PathBuf,
    #[arg(long)]
    kind: BundleKind,
    /// [a-z0-9][a-z0-9._-]{0,63}; the directory's name by default.
    #[arg(long)]
    title: Option<String>,
    /// An SPDX expression, or LicenseRef-… with the text in the license file.
    #[arg(long)]
    license: String,
    #[arg(long, default_value = "LICENSE")]
    license_file: String,
    /// <class_id>:<inventory_root>, 128 hex each, for a PALW container. Repeatable.
    #[arg(long = "palw-root")]
    palw_roots: Vec<String>,
    /// The container's own digest (128 hex), when it defines one.
    #[arg(long)]
    artifact_digest: Option<Hex64>,
    /// Trackers written outside `info` (never part of the identity). Repeatable.
    #[arg(long)]
    announce: Vec<String>,
    /// BEP 19 web seeds written outside `info`. Repeatable.
    #[arg(long = "web-seed")]
    web_seeds: Vec<String>,
    /// Where to write the .torrent (default: <dir>/../<title>.torrent).
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Args)]
struct PullArgs {
    magnet: String,
    /// The bundle_commitment the anchor declares (128 hex): L2 checks it.
    #[arg(long)]
    expect: Option<Hex64>,
    #[arg(long)]
    total_bytes: Option<u64>,
    #[arg(long)]
    kind: Option<BundleKind>,
    /// Required without --expect: the bytes will be anchored to nothing (MT-8).
    #[arg(long)]
    unanchored: bool,
    #[arg(long)]
    no_seed: bool,
    /// Return once the daemon has the request instead of following it.
    #[arg(long)]
    no_wait: bool,
}

#[derive(Subcommand)]
enum SeedCmd {
    /// Seed a sealed bundle from the store.
    Start { infohash: String },
    /// Stop seeding (pause) a bundle; its files stay.
    Stop { infohash: String },
    /// List bundles and whether they seed.
    List,
    /// Seed files obtained elsewhere, in place, if their canonical torrent has this infohash.
    Adopt {
        dir: PathBuf,
        #[arg(long)]
        expect: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("misaka-torrent: {e}");
            ExitCode::FAILURE
        }
    }
}

type R<T> = Result<T, String>;

fn run(cli: Cli) -> R<()> {
    let conn = Conn { socket: cli.socket.clone(), profile: cli.profile };
    match cli.cmd {
        Cmd::Create(a) => {
            let (p, out) = create(&a)?;
            print_packed(&p, Some(&out));
            Ok(())
        }
        Cmd::Inspect { path } => inspect(&path),
        Cmd::Share(a) => {
            let (p, out) = create(&a)?;
            print_packed(&p, Some(&out));
            adopt(&conn, &a.dir, p.infohash())?;
            println!("sharing  unanchored — anyone with the magnet can fetch it");
            Ok(())
        }
        Cmd::Pull(a) => pull(&conn, a),
        Cmd::Seed(SeedCmd::Start { infohash }) => {
            conn.client()?.call(&Request::Seed { infohash: parse_ih(&infohash)?.0 }).map_err(|e| e.to_string())?;
            println!("seeding {infohash}");
            Ok(())
        }
        Cmd::Seed(SeedCmd::Stop { infohash }) => {
            conn.client()?.call(&Request::Pause { infohash: parse_ih(&infohash)?.0 }).map_err(|e| e.to_string())?;
            println!("stopped {infohash}");
            Ok(())
        }
        Cmd::Seed(SeedCmd::List) => {
            let mut c = conn.client()?;
            for r in c.status(None).map_err(|e| e.to_string())? {
                let seeding = r.state == BundleState::Seeding;
                println!(
                    "{}  {:<9} {:<14} {}",
                    Infohash(r.infohash),
                    if seeding { "seeding" } else { "-" },
                    r.label.as_str(),
                    r.title.unwrap_or_default()
                );
            }
            Ok(())
        }
        Cmd::Seed(SeedCmd::Adopt { dir, expect }) => adopt(&conn, &dir, parse_ih(&expect)?),
        Cmd::Status { infohash, json } => {
            let ih = infohash.as_deref().map(parse_ih).transpose()?;
            let mut c = conn.client()?;
            let reports = c.status(ih.map(|h| h.0)).map_err(|e| e.to_string())?;
            if json {
                let v: Vec<serde_json::Value> = reports.iter().map(report_json).collect();
                println!("{}", serde_json::to_string_pretty(&v).expect("json"));
            } else {
                println!(
                    "daemon   {} profile, engine {}, peer port {}, confinement: {}",
                    c.info.profile, c.info.engine, c.info.listen_port, c.info.confinement
                );
                for r in &reports {
                    print_report(r);
                }
                if reports.is_empty() {
                    println!("no bundles");
                }
            }
            Ok(())
        }
        Cmd::Remove { infohash, delete_files } => {
            conn.client()?
                .call(&Request::Remove { infohash: parse_ih(&infohash)?.0, delete_files })
                .map_err(|e| e.to_string())?;
            println!("removed {infohash}");
            Ok(())
        }
        Cmd::Limits { upload, download, connections } => {
            conn.client()?
                .call(&Request::SetLimits(SetLimits {
                    upload_bytes_per_sec: upload,
                    download_bytes_per_sec: download,
                    connections,
                }))
                .map_err(|e| e.to_string())?;
            println!("limits set");
            Ok(())
        }
    }
}

struct Conn {
    socket: Option<PathBuf>,
    profile: Option<Profile>,
}

impl Conn {
    fn client(&self) -> R<Client> {
        let socket = match &self.socket {
            Some(s) => s.clone(),
            None => {
                let o = Overrides { profile: self.profile, ..Default::default() };
                Config::resolve(&o, &|k| std::env::var(k).ok()).map_err(|e| e.to_string())?.socket
            }
        };
        Client::connect(&socket, concat!("misaka-torrent/", env!("CARGO_PKG_VERSION"))).map_err(|e| e.to_string())
    }
}

fn parse_ih(s: &str) -> R<Infohash> {
    s.parse().map_err(|_| format!("{s:?} is not an infohash (64 lowercase hex)"))
}

fn create(a: &CreateArgs) -> R<(Packed, PathBuf)> {
    let dir = &a.dir;
    let title = match &a.title {
        Some(t) => t.clone(),
        None => dir
            .canonicalize()
            .ok()
            .and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
            .ok_or("give --title")?,
    };
    let mut roots = Vec::new();
    for r in &a.palw_roots {
        let (c, i) =
            r.split_once(':').ok_or_else(|| format!("--palw-root {r:?} is not <class_id>:<inventory_root>"))?;
        roots.push(PalwRoot {
            class_id: c.parse().map_err(|e| format!("class_id: {e}"))?,
            inventory_root: i.parse().map_err(|e| format!("inventory_root: {e}"))?,
        });
    }
    let opts = CreateOptions {
        kind: a.kind,
        title: title.clone(),
        spdx: a.license.clone(),
        license_file: a.license_file.clone(),
        palw_roots: roots,
        artifact_digest: a.artifact_digest,
    };
    eprintln!("hashing  {} …", dir.display());
    let mut p = pack::create(dir, &opts).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(DESCRIPTOR_FILE_NAME), &p.descriptor_bytes).map_err(|e| e.to_string())?;
    if a.announce.len() == 1 {
        p.torrent.announce = Some(a.announce[0].clone());
    } else if !a.announce.is_empty() {
        p.torrent.announce = Some(a.announce[0].clone());
        p.torrent.announce_list = a.announce.iter().map(|u| vec![u.clone()]).collect();
    }
    p.torrent.url_list = a.web_seeds.clone();
    let out = match &a.out {
        Some(o) => o.clone(),
        None => {
            let parent =
                dir.canonicalize().map_err(|e| e.to_string())?.parent().map(Path::to_owned).ok_or("no parent")?;
            parent.join(format!("{title}.torrent"))
        }
    };
    std::fs::write(&out, p.torrent.encode()).map_err(|e| format!("{}: {e}", out.display()))?;
    if dir.canonicalize().ok().and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned())).as_deref()
        != Some(&title)
    {
        eprintln!("note     the directory is not named {title:?}; rename it before `seed adopt`");
    }
    Ok((p, out))
}

fn print_packed(p: &Packed, torrent: Option<&Path>) {
    println!("title       {}", p.descriptor.title);
    println!("kind        {}", p.descriptor.kind);
    println!("license     {} ({})", p.descriptor.license.spdx, p.descriptor.license.file);
    println!("files       {}", p.bundle.info().files.len());
    println!("total       {} ({} bytes)", human(p.total_bytes()), p.total_bytes());
    println!("piece       {}", human(p.bundle.info().piece_length));
    println!("commitment  {}", p.commitment());
    println!("infohash    {}", p.infohash());
    println!("magnet      {}", p.magnet());
    if let Some(t) = torrent {
        println!("torrent     {}", t.display());
    }
}

fn inspect(path: &Path) -> R<()> {
    if path.is_dir() {
        let p = pack::rebuild(path).map_err(|e| e.to_string())?;
        print_packed(&p, None);
        println!("check       rule v1 ok · Safe Model Profile ok · descriptor ↔ files ok (every byte re-hashed)");
        return Ok(());
    }
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.len() > MAX_TORRENT_FILE_BYTES as u64 {
        return Err(format!("{} is larger than {MAX_TORRENT_FILE_BYTES} bytes", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let t = Torrent::parse(&bytes, TORRENT_DECODE_LIMITS).map_err(|e| format!("not a rule-v1 torrent: {e}"))?;
    check_torrent_limits(&t).map_err(|e| e.to_string())?;
    let info = t.info.encode();
    let ih = misaka_btv2::infohash(&info);
    let b = admit(&info, &ih, &Expectation::default(), &AdmissionEnv::default()).map_err(|e| e.to_string())?;
    println!("title       {}", b.title());
    println!("kind        {}", b.kind());
    println!("total       {} ({} bytes)", human(b.total_bytes()), b.total_bytes());
    println!("piece       {}", human(b.info().piece_length));
    println!("infohash    {ih}");
    println!("magnet      {}", magnet::format(&ih, b.title()));
    for (f, _) in b.files() {
        println!("  {:>12}  {}  {}", f.length, hex::encode(f.pieces_root), f.name);
    }
    if let Some(a) = &t.announce {
        println!("announce    {a}");
    }
    for w in &t.url_list {
        println!("web seed    {w}");
    }
    println!("check       rule v1 ok · piece layers ok · Safe Model Profile ok (unanchored: no descriptor read)");
    Ok(())
}

fn adopt(conn: &Conn, dir: &Path, ih: Infohash) -> R<()> {
    let mut c = conn.client()?;
    let dir = dir.canonicalize().map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut found = None;
    for r in &c.info.adopt_roots {
        let root = Path::new(&r.path).canonicalize().unwrap_or_else(|_| PathBuf::from(&r.path));
        if let Ok(rel) = dir.strip_prefix(&root) {
            found = Some((r.id.clone(), rel.to_string_lossy().replace('\\', "/")));
            break;
        }
    }
    let (root_id, rel) = found.ok_or_else(|| {
        format!(
            "{} is under no adopt root; add one to torrent.toml ([[adopt_roots]] id = …, path = …). Roots: {:?}",
            dir.display(),
            c.info.adopt_roots.iter().map(|r| &r.path).collect::<Vec<_>>()
        )
    })?;
    c.call(&Request::Adopt { infohash: ih.0, adopt_root_id: root_id, rel_dir: rel }).map_err(|e| e.to_string())?;
    println!("adopting {ih} — the daemon re-hashes the files, then seeds them in place, read-only");
    Ok(())
}

fn pull(conn: &Conn, a: PullArgs) -> R<()> {
    let m = magnet::parse(&a.magnet).map_err(|e| e.to_string())?;
    if a.expect.is_none() && !a.unanchored {
        return Err("a bare magnet anchors the bytes to nothing: pass --expect <bundle_commitment>, or --unanchored to accept that (MT-8)".into());
    }
    let expect = Expect {
        bundle_commitment: a.expect.map(|h| h.0),
        total_bytes: a.total_bytes,
        kind: a.kind.map(BundleKind::id),
    };
    let mut c = conn.client()?;
    c.call(&Request::Fetch { infohash: m.infohash.0, expect, seed_after: !a.no_seed }).map_err(|e| e.to_string())?;
    println!("fetching {} {}", m.infohash, m.display_name.unwrap_or_default());
    if a.no_wait {
        return Ok(());
    }
    let mut last = String::new();
    loop {
        let r = c.status(Some(m.infohash.0)).map_err(|e| e.to_string())?.pop().ok_or("the bundle disappeared")?;
        let line = progress_line(&r);
        if line != last {
            println!("{line}");
            last = line;
        }
        match &r.state {
            BundleState::Seeding | BundleState::Idle => {
                println!("sealed   {}", r.path.unwrap_or_default());
                println!("label    {}", r.label.as_str());
                return Ok(());
            }
            s if s.is_terminal_failure() => return Err(format!("{}: {}", s.name(), failure_reason(s))),
            _ => std::thread::sleep(Duration::from_millis(500)),
        }
    }
}

fn failure_reason(s: &BundleState) -> String {
    match s {
        BundleState::Refused { reason } | BundleState::Failed { reason } => reason.clone(),
        BundleState::Mismatch { level, reason } => format!("L{level}: {reason}"),
        _ => String::new(),
    }
}

fn progress_line(r: &BundleReport) -> String {
    match (&r.state, r.total_bytes) {
        (BundleState::Downloading | BundleState::Stalled, Some(t)) if t > 0 => format!(
            "{:<9} {:>3}%  {}/s  peers {} ({} seeds)",
            r.state.name(),
            r.done_bytes * 100 / t,
            human(r.download_rate),
            r.peers,
            r.seeds
        ),
        _ => r.state.name().to_owned(),
    }
}

fn print_report(r: &BundleReport) {
    let ih = Infohash(r.infohash);
    println!(
        "{}  {}  {}  {}",
        ih,
        r.title.as_deref().unwrap_or("(metadata pending)"),
        r.state.name(),
        r.label.as_str()
    );
    if let Some(t) = r.total_bytes {
        println!(
            "    {} of {}  ↓ {} ({}/s)  ↑ {} ({}/s)  peers {} seeds {}{}",
            human(r.done_bytes),
            human(t),
            human(r.downloaded),
            human(r.download_rate),
            human(r.uploaded),
            human(r.upload_rate),
            r.peers,
            r.seeds,
            if r.seeding_enabled { "" } else { "  (not seeding)" }
        );
    }
    if r.state.is_terminal_failure() {
        println!("    {}", failure_reason(&r.state));
    }
    if let Some(p) = &r.path {
        println!("    {p}");
    }
}

fn report_json(r: &BundleReport) -> serde_json::Value {
    serde_json::json!({
        "infohash": Infohash(r.infohash).to_string(),
        "title": r.title,
        "kind": r.kind.and_then(BundleKind::from_id).map(|k| k.as_str()),
        "state": r.state.name(),
        "reason": if r.state.is_terminal_failure() { Some(failure_reason(&r.state)) } else { None },
        "label": r.label.as_str(),
        "total_bytes": r.total_bytes,
        "done_bytes": r.done_bytes,
        "uploaded": r.uploaded,
        "downloaded": r.downloaded,
        "upload_rate": r.upload_rate,
        "download_rate": r.download_rate,
        "peers": r.peers,
        "seeds": r.seeds,
        "seeding": r.seeding_enabled,
        "path": r.path,
        "bundle_commitment": r.bundle_commitment.map(hex::encode),
    })
}

fn human(b: u64) -> String {
    const U: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < U.len() {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.2} {}", U[i]) }
}
