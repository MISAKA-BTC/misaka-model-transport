//! MISAKA Model Transport, the desktop app (RFC-0001 §7.3).
//!
//! The app is a window and a tray icon over `misaka-torrentd`, which it bundles and starts as its
//! own child process. The daemon confines itself on Linux and runs under `sandbox-exec` on macOS.
//! The app holds no torrent state: it talks to the daemon over the daemon's local socket
//! (misaka-torrent-borsh/v1), like `misaka-torrent` does.
//!
//! - **Links**: `misaka-model://<network>/<line_id>/<version>?kind=<kind>` opens the app. The link
//!   is resolved through the hub's release index (`torrents/index.json`) to an infohash, a
//!   commitment and a size, and shown for confirmation. A link never starts a download by itself.
//! - **Seeding**: every completed download seeds while the app runs; the app starts at login,
//!   minimized to the tray, so a library keeps seeding after a reboot.
//! - **Adopt**: files obtained elsewhere (Hugging Face, a USB disk) are matched against the index,
//!   hard-linked into the app's adopt directory with the bundle's descriptor, re-hashed by the
//!   daemon, and seeded in place.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use misaka_btv2::torrent::Infohash;
use misaka_bundle::link::ModelLink;
use misaka_bundle::{BundleKind, Hex64};
use misaka_transport_ipc::client::Client;
use misaka_transport_ipc::{BundleReport, BundleState, Expect, Request, SetLimits};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

const CLIENT: &str = concat!("misaka-model-transport/", env!("CARGO_PKG_VERSION"));
const DEFAULT_INDEX: &str = "https://misakaoptions.com/torrents/index.json";
/// The adopt root the app owns, under the store: `<home>/adopt/<title>/`.
const ADOPT_ROOT_ID: &str = "app";

/// Where things live. Everything is under `~/.misaka`, beside misakas's own files.
#[derive(Debug, Clone)]
struct Paths {
    /// `~/.misaka/torrent`: incomplete/, store/, state/, adopt/, run/.
    home: PathBuf,
    /// `~/.misaka/torrent.toml`: the daemon's configuration.
    config: PathBuf,
    socket: PathBuf,
    /// `~/.misaka/torrent-app.json`: the app's own settings.
    app_settings: PathBuf,
}

impl Paths {
    fn new() -> Result<Paths, String> {
        let user = std::env::var_os("HOME").map(PathBuf::from).ok_or("HOME is not set")?;
        let misaka = user.join(".misaka");
        let home = misaka.join("torrent");
        // A Unix socket path is limited to ~104 bytes; a very long home falls back to the per-user
        // temporary directory (private on macOS).
        let mut socket = home.join("run/torrentd.sock");
        if socket.as_os_str().len() > 100 {
            socket = std::env::temp_dir().join("misaka-torrent/torrentd.sock");
        }
        Ok(Paths { socket, config: misaka.join("torrent.toml"), app_settings: misaka.join("torrent-app.json"), home })
    }
    fn adopt_dir(&self) -> PathBuf {
        self.home.join("adopt")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AppSettings {
    index_url: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings { index_url: DEFAULT_INDEX.into() }
    }
}

struct AppState {
    paths: Paths,
    daemon: Mutex<Option<Child>>,
    pending_link: Mutex<Option<String>>,
    settings: Mutex<AppSettings>,
}

type R<T> = Result<T, String>;

// ---- the daemon's configuration -------------------------------------------------------------

/// Writes `~/.misaka/torrent.toml` on first run: the desktop profile, seeding on with a 10 MiB/s
/// upload cap, port mapping on, and the app's adopt root.
fn ensure_config(p: &Paths) -> R<()> {
    std::fs::create_dir_all(p.adopt_dir()).map_err(|e| format!("{}: {e}", p.adopt_dir().display()))?;
    std::fs::create_dir_all(p.socket.parent().expect("has a parent")).map_err(|e| e.to_string())?;
    if p.config.exists() {
        // Keep the user's file; make sure the app's adopt root is in it.
        let mut v = read_config(p)?;
        if ensure_adopt_root(&mut v, p) {
            write_config(p, &v)?;
        }
        return Ok(());
    }
    let mut v: toml::Table = toml::from_str(
        "profile = \"desktop\"\n[seeding]\nenabled = true\nupload_bytes_per_sec = 10485760\n[network]\nupnp_natpmp = true\n",
    )
    .expect("valid");
    ensure_adopt_root(&mut v, p);
    write_config(p, &v)
}

fn read_config(p: &Paths) -> R<toml::Table> {
    let text = std::fs::read_to_string(&p.config).map_err(|e| format!("{}: {e}", p.config.display()))?;
    text.parse::<toml::Table>().map_err(|e| format!("{}: {e}", p.config.display()))
}

fn write_config(p: &Paths, v: &toml::Table) -> R<()> {
    let text = format!(
        "# misaka-torrentd, desktop profile. Written by MISAKA Model Transport; edits are kept.\n{}",
        toml::to_string_pretty(v).map_err(|e| e.to_string())?
    );
    let tmp = p.config.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p.config).map_err(|e| e.to_string())
}

fn ensure_adopt_root(v: &mut toml::Table, p: &Paths) -> bool {
    let roots = v.entry("adopt_roots").or_insert_with(|| toml::Value::Array(vec![]));
    let Some(arr) = roots.as_array_mut() else { return false };
    if arr.iter().any(|r| r.get("id").and_then(|i| i.as_str()) == Some(ADOPT_ROOT_ID)) {
        return false;
    }
    let mut t = toml::Table::new();
    t.insert("id".into(), ADOPT_ROOT_ID.into());
    t.insert("path".into(), p.adopt_dir().display().to_string().into());
    arr.push(toml::Value::Table(t));
    true
}

fn section<'a>(v: &'a mut toml::Table, name: &str) -> &'a mut toml::Table {
    v.entry(name).or_insert_with(|| toml::Value::Table(toml::Table::new())).as_table_mut().expect("a table")
}

fn load_app_settings(p: &Paths) -> AppSettings {
    std::fs::read(&p.app_settings).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

// ---- the daemon process ---------------------------------------------------------------------

fn connect(p: &Paths) -> R<Client> {
    Client::connect(&p.socket, CLIENT).map_err(|e| e.to_string())
}

/// Connects to a running daemon, or starts the bundled one and waits for its socket.
fn ensure_daemon(app: &AppHandle) -> R<()> {
    let st = app.state::<AppState>();
    if connect(&st.paths).is_ok() {
        return Ok(());
    }
    let mut guard = st.daemon.lock().expect("unpoisoned");
    if let Some(child) = guard.as_mut()
        && child.try_wait().ok().flatten().is_none()
    {
        drop(guard);
        return wait_for_socket(&st.paths, Duration::from_secs(10));
    }
    ensure_config(&st.paths)?;
    let exe_dir =
        std::env::current_exe().map_err(|e| e.to_string())?.parent().map(Path::to_owned).ok_or("no exe dir")?;
    let bin = exe_dir.join(if cfg!(windows) { "misaka-torrentd.exe" } else { "misaka-torrentd" });
    if !bin.exists() {
        return Err(format!("the bundled daemon is missing: {}", bin.display()));
    }
    let p = &st.paths;
    let args = [
        "run".to_string(),
        "--profile".into(),
        "desktop".into(),
        "--config".into(),
        p.config.display().to_string(),
        "--home".into(),
        p.home.display().to_string(),
        "--socket".into(),
        p.socket.display().to_string(),
    ];
    let mut cmd = daemon_command(app, &bin, p);
    cmd.args(&args);
    std::fs::create_dir_all(p.home.join("state")).map_err(|e| e.to_string())?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p.home.join("state/torrentd.log"))
        .map_err(|e| e.to_string())?;
    let _ = writeln!(&log, "--- started by {CLIENT}");
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::from(log));
    let child = cmd.spawn().map_err(|e| format!("cannot start {}: {e}", bin.display()))?;
    *guard = Some(child);
    drop(guard);
    wait_for_socket(&st.paths, Duration::from_secs(15))
}

/// On macOS the daemon runs under the sandbox profile bundled with the app (§6.5): the store, the
/// configuration and the network; no exec, no fork. On Linux it applies Landlock and seccomp to
/// itself.
fn daemon_command(app: &AppHandle, bin: &Path, p: &Paths) -> Command {
    #[cfg(target_os = "macos")]
    if let Ok(sb) = app.path().resolve("misaka-torrentd.sb", tauri::path::BaseDirectory::Resource)
        && sb.exists()
    {
        // The sandbox matches real paths: /var is /private/var, /tmp is /private/tmp.
        let real = |x: &Path| std::fs::canonicalize(x).unwrap_or_else(|_| x.to_owned());
        let sockdir = p.socket.parent().unwrap_or(&p.home);
        let _ = std::fs::create_dir_all(sockdir);
        let mut c = Command::new("/usr/bin/sandbox-exec");
        c.arg("-D").arg(format!("BIN={}", real(bin).display()));
        c.arg("-D").arg(format!("STORE={}", real(&p.home).display()));
        c.arg("-D").arg(format!("CONFIG={}", real(&p.config).display()));
        c.arg("-D").arg(format!("ADOPT={}", real(&p.adopt_dir()).display()));
        c.arg("-D").arg(format!("SOCKDIR={}", real(sockdir).display()));
        c.arg("-f").arg(sb).arg(bin);
        c.env("MISAKA_TORRENT_SANDBOX", "sandbox-exec");
        return c;
    }
    let _ = (app, p);
    Command::new(bin)
}

fn wait_for_socket(p: &Paths, max: Duration) -> R<()> {
    let start = Instant::now();
    loop {
        if connect(p).is_ok() {
            return Ok(());
        }
        if start.elapsed() > max {
            return Err(format!(
                "misaka-torrentd did not come up; see {}",
                p.home.join("state/torrentd.log").display()
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Stops the daemon this app started, gracefully (it saves its DHT state on SIGTERM).
fn stop_daemon(app: &AppHandle) {
    let st = app.state::<AppState>();
    let mut guard = st.daemon.lock().expect("unpoisoned");
    if let Some(mut child) = guard.take() {
        #[cfg(unix)]
        // SAFETY: a pid this process spawned and still owns.
        unsafe {
            libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
        }
        let start = Instant::now();
        while child.try_wait().ok().flatten().is_none() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn restart_daemon(app: &AppHandle) -> R<()> {
    stop_daemon(app);
    let st = app.state::<AppState>();
    let start = Instant::now();
    while connect(&st.paths).is_ok() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(100));
    }
    ensure_daemon(app)
}

// ---- reports --------------------------------------------------------------------------------

fn failure_reason(s: &BundleState) -> Option<String> {
    match s {
        BundleState::Refused { reason } | BundleState::Failed { reason } => Some(reason.clone()),
        BundleState::Mismatch { level, reason } => Some(format!("L{level}: {reason}")),
        _ => None,
    }
}

fn report_json(r: &BundleReport) -> Value {
    json!({
        "infohash": Infohash(r.infohash).to_string(),
        "title": r.title,
        "kind": r.kind.and_then(BundleKind::from_id).map(|k| k.as_str()),
        "state": r.state.name(),
        "reason": failure_reason(&r.state),
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

fn parse_ih(s: &str) -> R<Infohash> {
    s.parse().map_err(|_| format!("{s:?} is not an infohash"))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> R<T> + Send + 'static) -> R<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

// ---- the release index ----------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct Index {
    schema: String,
    network: String,
    bundles: Vec<IndexBundle>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexFile {
    path: String,
    size: u64,
    role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexBundle {
    line_id: String,
    repo: Option<String>,
    version: Option<u32>,
    kind: String,
    title: String,
    btv2_infohash: String,
    bundle_commitment: String,
    total_bytes: u64,
    license: Option<String>,
    files: Vec<IndexFile>,
    base_model: Option<String>,
    params: Option<String>,
    /// The canonical misaka-bundle.json, for adopting files obtained elsewhere.
    descriptor: Option<String>,
}

async fn fetch_index(url: &str) -> R<Index> {
    if !url.starts_with("https://") && !url.starts_with("http://127.0.0.1") && !url.starts_with("http://localhost") {
        return Err("the index URL must be https".into());
    }
    let resp = reqwest::Client::builder()
        .user_agent(CLIENT)
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?
        .get(url)
        .send()
        .await
        .map_err(|e| format!("cannot read the model index: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("the model index answered {}", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() > 16 << 20 {
        return Err("the model index is larger than 16 MiB".into());
    }
    let idx: Index = serde_json::from_slice(&bytes).map_err(|e| format!("the model index is not valid: {e}"))?;
    if idx.schema != "misaka/torrent-index/v1" {
        return Err(format!("unknown index schema {:?}", idx.schema));
    }
    Ok(idx)
}

fn index_url(st: &AppState) -> String {
    st.settings.lock().expect("unpoisoned").index_url.clone()
}

// ---- commands -------------------------------------------------------------------------------

#[tauri::command]
async fn status(app: AppHandle) -> R<Value> {
    blocking(move || {
        ensure_daemon(&app)?;
        let st = app.state::<AppState>();
        let mut c = connect(&st.paths)?;
        let reports = c.status(None).map_err(|e| e.to_string())?;
        Ok(json!({
            "daemon": {
                "version": c.info.version,
                "engine": c.info.engine,
                "listen_port": c.info.listen_port,
                "seeding_enabled": c.info.seeding_enabled,
                "confinement": c.info.confinement,
            },
            "store": st.paths.home.join("store").display().to_string(),
            "bundles": reports.iter().map(report_json).collect::<Vec<_>>(),
        }))
    })
    .await
}

/// Resolves a `misaka-model://` link through the index. Starts nothing.
#[tauri::command]
async fn resolve(app: AppHandle, link: String) -> R<Value> {
    let l: ModelLink = link.trim().parse().map_err(|e| format!("not a MISAKA model link: {e}"))?;
    let url = index_url(&app.state::<AppState>());
    let idx = fetch_index(&url).await?;
    if idx.network != l.network.to_string() {
        return Err(format!("the link is for {}, but the model index lists {}", l.network, idx.network));
    }
    let line = l.line_id.to_string();
    let b = idx
        .bundles
        .iter()
        .find(|b| b.line_id == line && b.version.unwrap_or(1) == l.version && b.kind == l.kind.as_str())
        .ok_or_else(|| format!("no {} bundle of version {} is listed for this model", l.kind, l.version))?;
    let have = {
        let app = app.clone();
        let ih = b.btv2_infohash.clone();
        blocking(move || {
            ensure_daemon(&app)?;
            let mut c = connect(&app.state::<AppState>().paths)?;
            Ok(c.status(Some(parse_ih(&ih)?.0)).ok().and_then(|v| v.first().map(report_json)))
        })
        .await?
    };
    Ok(json!({
        "link": l.to_string(),
        "network": idx.network,
        "line_id": b.line_id,
        "version": l.version,
        "repo": b.repo,
        "title": b.title,
        "kind": b.kind,
        "license": b.license,
        "total_bytes": b.total_bytes,
        "files": b.files,
        "base_model": b.base_model,
        "params": b.params,
        "infohash": b.btv2_infohash,
        "bundle_commitment": b.bundle_commitment,
        "destination": app.state::<AppState>().paths.home.join("store").join(&b.btv2_infohash).join(&b.title).display().to_string(),
        "index": url,
        "existing": have,
    }))
}

/// Starts a download the user confirmed. The commitment, size and kind come from the index: the
/// daemon refuses a bundle that does not match them (L2) and never seals it.
#[tauri::command]
async fn fetch(app: AppHandle, infohash: String, bundle_commitment: String, total_bytes: u64, kind: String) -> R<()> {
    blocking(move || {
        let ih = parse_ih(&infohash)?;
        let commitment: Hex64 = bundle_commitment.parse().map_err(|e| format!("bundle commitment: {e}"))?;
        let kind: BundleKind = kind.parse().map_err(|_| format!("unknown kind {kind:?}"))?;
        ensure_daemon(&app)?;
        let mut c = connect(&app.state::<AppState>().paths)?;
        c.call(&Request::Fetch {
            infohash: ih.0,
            expect: Expect {
                bundle_commitment: Some(commitment.0),
                total_bytes: Some(total_bytes),
                kind: Some(kind.id()),
            },
            seed_after: true,
        })
        .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
}

#[tauri::command]
async fn control(app: AppHandle, action: String, infohash: String) -> R<()> {
    blocking(move || {
        let ih = parse_ih(&infohash)?.0;
        let req = match action.as_str() {
            "pause" => Request::Pause { infohash: ih },
            "resume" => Request::Resume { infohash: ih },
            "seed" => Request::Seed { infohash: ih },
            "remove" => Request::Remove { infohash: ih, delete_files: false },
            "delete" => Request::Remove { infohash: ih, delete_files: true },
            _ => return Err(format!("unknown action {action:?}")),
        };
        ensure_daemon(&app)?;
        connect(&app.state::<AppState>().paths)?.call(&req).map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
}

#[tauri::command]
async fn open_folder(app: AppHandle, infohash: String) -> R<()> {
    let path = {
        let app = app.clone();
        blocking(move || {
            let mut c = connect(&app.state::<AppState>().paths)?;
            let r = c.status(Some(parse_ih(&infohash)?.0)).map_err(|e| e.to_string())?;
            r.first().and_then(|r| r.path.clone()).ok_or_else(|| "this model has no folder yet".to_string())
        })
        .await?
    };
    app.opener().reveal_item_in_dir(&path).map_err(|e| e.to_string())
}

/// "Add existing model": pick a folder, find the listed bundle whose files it holds (same names
/// and sizes), hard-link them into the app's adopt directory with the bundle's descriptor, and ask
/// the daemon to adopt it. The daemon re-hashes every byte and refuses unless the files' canonical
/// torrent has the listed infohash.
#[tauri::command]
async fn adopt(app: AppHandle) -> R<Value> {
    let Some(folder) =
        app.dialog().file().set_title("Choose the folder that holds the model's files").blocking_pick_folder()
    else {
        return Ok(json!(null));
    };
    let folder = folder.into_path().map_err(|e| e.to_string())?;
    let idx = fetch_index(&index_url(&app.state::<AppState>())).await?;
    blocking(move || {
        let mut present = BTreeMap::new();
        for e in std::fs::read_dir(&folder).map_err(|e| format!("{}: {e}", folder.display()))? {
            let e = e.map_err(|e| e.to_string())?;
            let m = std::fs::symlink_metadata(e.path()).map_err(|e| e.to_string())?;
            if m.is_file() {
                present.insert(e.file_name().to_string_lossy().into_owned(), m.len());
            }
        }
        let b = idx
            .bundles
            .iter()
            .find(|b| b.files.iter().all(|f| present.get(&f.path) == Some(&f.size)))
            .ok_or("this folder does not hold every file of any listed model (names and sizes must match)")?;
        let descriptor = b.descriptor.as_ref().ok_or("the index carries no descriptor for this model yet")?;
        let p = app.state::<AppState>().paths.clone();
        let staging = p.adopt_dir().join(&b.title);
        if staging.exists() {
            std::fs::remove_dir_all(&staging).map_err(|e| e.to_string())?;
        }
        std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
        for f in &b.files {
            std::fs::hard_link(folder.join(&f.path), staging.join(&f.path)).map_err(|e| {
                format!(
                    "cannot link {}: {e}. The folder must be on the same disk as {} (no copy is made).",
                    f.path,
                    p.home.display()
                )
            })?;
        }
        std::fs::write(staging.join("misaka-bundle.json"), descriptor).map_err(|e| e.to_string())?;
        ensure_config(&p)?;
        ensure_daemon(&app)?;
        let mut c = connect(&p)?;
        if !c.info.adopt_roots.iter().any(|r| r.id == ADOPT_ROOT_ID) {
            drop(c);
            restart_daemon(&app)?;
            c = connect(&p)?;
        }
        c.call(&Request::Adopt {
            infohash: parse_ih(&b.btv2_infohash)?.0,
            adopt_root_id: ADOPT_ROOT_ID.into(),
            rel_dir: b.title.clone(),
        })
        .map_err(|e| e.to_string())?;
        Ok(json!({ "title": b.title, "repo": b.repo, "infohash": b.btv2_infohash }))
    })
    .await
}

#[derive(Debug, Serialize, Deserialize)]
struct Settings {
    upload_mib: f64,
    download_mib: f64,
    connections: u32,
    seeding: bool,
    upnp: bool,
    autostart: bool,
    index_url: String,
}

#[tauri::command]
async fn settings_get(app: AppHandle) -> R<Settings> {
    blocking(move || {
        let st = app.state::<AppState>();
        ensure_config(&st.paths)?;
        let v = read_config(&st.paths)?;
        let get = |s: &str, k: &str| v.get(s).and_then(|t| t.get(k)).cloned();
        let mib = |x: Option<toml::Value>, dflt: i64| {
            x.and_then(|v| v.as_integer()).unwrap_or(dflt) as f64 / (1 << 20) as f64
        };
        Ok(Settings {
            upload_mib: mib(get("seeding", "upload_bytes_per_sec"), 20 << 20),
            download_mib: mib(get("network", "download_bytes_per_sec"), 0),
            connections: get("network", "connections").and_then(|v| v.as_integer()).unwrap_or(200) as u32,
            seeding: get("seeding", "enabled").and_then(|v| v.as_bool()).unwrap_or(true),
            upnp: get("network", "upnp_natpmp").and_then(|v| v.as_bool()).unwrap_or(true),
            autostart: app.autolaunch().is_enabled().unwrap_or(false),
            index_url: index_url(&st),
        })
    })
    .await
}

#[tauri::command]
async fn settings_set(app: AppHandle, s: Settings) -> R<()> {
    blocking(move || {
        let st = app.state::<AppState>();
        let mut v = read_config(&st.paths)?;
        let bytes = |m: f64| (m.max(0.0) * (1 << 20) as f64).round() as i64;
        let before = (
            v.get("seeding").and_then(|t| t.get("enabled")).and_then(|x| x.as_bool()).unwrap_or(true),
            v.get("network").and_then(|t| t.get("upnp_natpmp")).and_then(|x| x.as_bool()).unwrap_or(true),
        );
        section(&mut v, "seeding").insert("enabled".into(), s.seeding.into());
        section(&mut v, "seeding").insert("upload_bytes_per_sec".into(), bytes(s.upload_mib).into());
        section(&mut v, "network").insert("download_bytes_per_sec".into(), bytes(s.download_mib).into());
        section(&mut v, "network").insert("connections".into(), i64::from(s.connections.max(1)).into());
        section(&mut v, "network").insert("upnp_natpmp".into(), s.upnp.into());
        write_config(&st.paths, &v)?;
        if s.index_url != index_url(&st) {
            if !s.index_url.starts_with("https://") {
                return Err("the index URL must be https".into());
            }
            let mut g = st.settings.lock().expect("unpoisoned");
            g.index_url = s.index_url.clone();
            std::fs::write(&st.paths.app_settings, serde_json::to_vec_pretty(&*g).expect("json"))
                .map_err(|e| e.to_string())?;
        }
        let al = app.autolaunch();
        let _ = if s.autostart { al.enable() } else { al.disable() };
        if before != (s.seeding, s.upnp) {
            restart_daemon(&app)?;
        } else {
            ensure_daemon(&app)?;
            connect(&st.paths)?
                .call(&Request::SetLimits(SetLimits {
                    upload_bytes_per_sec: Some(bytes(s.upload_mib) as u64),
                    download_bytes_per_sec: Some(bytes(s.download_mib) as u64),
                    connections: Some(s.connections.max(1)),
                }))
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    })
    .await
}

/// A link that arrived before the window was ready.
#[tauri::command]
fn take_pending_link(state: State<'_, AppState>) -> Option<String> {
    state.pending_link.lock().expect("unpoisoned").take()
}

// ---- links, window, tray --------------------------------------------------------------------

fn handle_link(app: &AppHandle, url: &str) {
    if !url.starts_with("misaka-model://") {
        return;
    }
    *app.state::<AppState>().pending_link.lock().expect("unpoisoned") = Some(url.to_string());
    let _ = app.emit("deep-link", url.to_string());
    show_window(app);
}

fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Follows the daemon's event stream and forwards each report to the window.
fn spawn_event_pump(app: AppHandle) {
    std::thread::spawn(move || {
        loop {
            let paths = app.state::<AppState>().paths.clone();
            if let Ok(c) = connect(&paths)
                && let Ok(events) = c.subscribe()
            {
                for r in events {
                    let _ = app.emit("bundle", report_json(&r));
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

pub fn run() {
    let paths = Paths::new().expect("HOME is set");
    let settings = load_app_settings(&paths);
    let minimized = std::env::args().any(|a| a == "--minimized");
    tauri::Builder::default()
        // A second launch (a clicked link on Linux) hands its arguments to this instance.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _| {
            match argv.iter().find(|a| a.starts_with("misaka-model://")) {
                Some(url) => handle_link(app, url),
                None => show_window(app),
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--minimized"])))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            paths,
            daemon: Mutex::new(None),
            pending_link: Mutex::new(None),
            settings: Mutex::new(settings),
        })
        .invoke_handler(tauri::generate_handler![
            status,
            resolve,
            fetch,
            control,
            open_folder,
            adopt,
            settings_get,
            settings_set,
            take_pending_link
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            // Links: the scheme is registered by the installer (Info.plist, .desktop); an AppImage
            // or a development build registers it at run time.
            #[cfg(any(target_os = "linux", all(debug_assertions, windows)))]
            let _ = app.deep_link().register_all();
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                for u in urls {
                    handle_link(&handle, u.as_str());
                }
            }
            {
                let h = handle.clone();
                app.deep_link().on_open_url(move |ev| {
                    for u in ev.urls() {
                        handle_link(&h, u.as_str());
                    }
                });
            }
            // First run: start at login so a library keeps seeding after a reboot. The user can
            // turn it off in Settings; the app never turns it back on.
            let marker = app.state::<AppState>().paths.home.join("state/app-first-run");
            if !marker.exists() {
                let _ = app.autolaunch().enable();
                let _ = std::fs::create_dir_all(marker.parent().expect("parent"));
                let _ = std::fs::write(&marker, b"");
            }
            // The daemon, off the UI thread.
            {
                let h = handle.clone();
                std::thread::spawn(move || {
                    if let Err(e) = ensure_daemon(&h) {
                        let _ = h.emit("daemon-error", e);
                    }
                });
            }
            spawn_event_pump(handle.clone());
            // The tray keeps the app (and so the seeding) alive when the window is closed.
            let open = MenuItem::with_id(app, "open", "Open MISAKA Model Transport", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit (stops seeding)", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().cloned().expect("an app icon"))
                .tooltip("MISAKA Model Transport")
                .menu(&menu)
                .on_menu_event(|app, ev| match ev.id().as_ref() {
                    "open" => show_window(app),
                    "quit" => {
                        stop_daemon(app);
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;
            if !minimized {
                show_window(&handle);
            }
            Ok(())
        })
        .on_window_event(|w, ev| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = ev {
                // Closing the window keeps seeding; Quit in the tray stops it.
                api.prevent_close();
                let _ = w.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("the app starts")
        .run(|app, ev| {
            if let tauri::RunEvent::Exit = ev {
                stop_daemon(app);
            }
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = ev {
                show_window(app);
            }
        });
}
