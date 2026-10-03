//! The daemon's store (§4.4): `incomplete/`, `store/` and `state/` under one root, on one file
//! system, so that sealing is one atomic rename.
//!
//! ```text
//! <home>/incomplete/<infohash>/<title>/…   downloading; the daemon's own, 0700
//! <home>/store/<infohash>/<title>/…        sealed: files 0444, directories 0555
//! <home>/state/bundles/<infohash>          a borsh record per bundle
//! <home>/state/info/<infohash>             the admitted info dictionary
//! <home>/state/layers/<infohash>           its piece layers, bencoded, once known
//! <home>/state/{port,quota}                chosen at first start, then kept
//! ```
//!
//! The daemon never opens a sealed file for writing (MT-6). Removing a sealed bundle makes its
//! directories writable again first; a mapped file keeps its inode until its mappings drop.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use borsh::{BorshDeserialize, BorshSerialize};
use misaka_btv2::torrent::Infohash;
use misaka_transport_ipc::{AnchorLabel, Expect};

use crate::config::Profile;

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Phase {
    Fetching,
    Sealed,
    Adopted,
    Failed,
}

/// What survives a restart.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Record {
    pub infohash: [u8; 32],
    pub expect: Expect,
    pub seed: bool,
    pub paused: bool,
    pub label: AnchorLabel,
    pub phase: Phase,
    pub failure: Option<String>,
    pub adopted: Option<(String, String)>,
    pub bundle_commitment: Option<[u8; 64]>,
    pub created_unix: u64,
}

#[derive(Debug, Clone)]
pub struct Store {
    pub root: PathBuf,
    pub incomplete: PathBuf,
    pub sealed: PathBuf,
    pub state: PathBuf,
}

fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let mut p = fs::metadata(path)?.permissions();
        p.set_readonly(mode & 0o200 == 0);
        fs::set_permissions(path, p)
    }
}

fn mkdir(path: &Path, mode: u32) -> io::Result<()> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            if !fs::symlink_metadata(path)?.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{} is not a directory", path.display()),
                ));
            }
        }
        Err(e) => return Err(e),
    }
    set_mode(path, mode)
}

/// Writes `bytes` to `path` through a temporary file and a rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

impl Store {
    /// Opens (creating) the store. On a server the root and `store/` are group-readable, so
    /// kaspad's group reads sealed bundles and never writes them; everything else is `0700`.
    pub fn open(root: &Path, profile: Profile) -> io::Result<Store> {
        fs::create_dir_all(root)?;
        let shared = if profile == Profile::Server { 0o750 } else { 0o700 };
        let s = Store {
            root: root.to_owned(),
            incomplete: root.join("incomplete"),
            sealed: root.join("store"),
            state: root.join("state"),
        };
        set_mode(root, shared)?;
        mkdir(&s.incomplete, 0o700)?;
        mkdir(&s.sealed, shared)?;
        mkdir(&s.state, 0o700)?;
        for sub in ["bundles", "info", "layers"] {
            mkdir(&s.state.join(sub), 0o700)?;
        }
        Ok(s)
    }

    pub fn incomplete_dir(&self, ih: &Infohash) -> PathBuf {
        self.incomplete.join(ih.to_string())
    }

    pub fn sealed_dir(&self, ih: &Infohash) -> PathBuf {
        self.sealed.join(ih.to_string())
    }

    fn state_file(&self, kind: &str, ih: &Infohash) -> PathBuf {
        self.state.join(kind).join(ih.to_string())
    }

    pub fn save_record(&self, r: &Record) -> io::Result<()> {
        write_atomic(&self.state_file("bundles", &Infohash(r.infohash)), &borsh::to_vec(r)?)
    }

    pub fn save_info(&self, ih: &Infohash, info: &[u8]) -> io::Result<()> {
        write_atomic(&self.state_file("info", ih), info)
    }

    pub fn load_info(&self, ih: &Infohash) -> Option<Vec<u8>> {
        fs::read(self.state_file("info", ih)).ok()
    }

    pub fn save_layers(&self, ih: &Infohash, layers: &[u8]) -> io::Result<()> {
        write_atomic(&self.state_file("layers", ih), layers)
    }

    pub fn load_layers(&self, ih: &Infohash) -> Option<Vec<u8>> {
        fs::read(self.state_file("layers", ih)).ok()
    }

    /// Every record; unreadable ones are reported and skipped.
    pub fn load_records(&self) -> Vec<Record> {
        let mut out = Vec::new();
        let Ok(rd) = fs::read_dir(self.state.join("bundles")) else { return out };
        for e in rd.flatten() {
            let name = e.file_name();
            if name.to_string_lossy().parse::<Infohash>().is_err() {
                continue;
            }
            match fs::read(e.path()).map(|b| borsh::from_slice::<Record>(&b)) {
                Ok(Ok(r)) => out.push(r),
                _ => eprintln!("misaka-torrentd: skipping an unreadable record {}", e.path().display()),
            }
        }
        out
    }

    pub fn forget(&self, ih: &Infohash) {
        for kind in ["bundles", "info", "layers"] {
            let _ = fs::remove_file(self.state_file(kind, ih));
        }
    }

    /// Seals `incomplete/<ih>/<title>` and moves it to `store/<ih>/<title>` by one rename.
    pub fn seal(&self, ih: &Infohash, title: &str) -> io::Result<PathBuf> {
        let src = self.incomplete_dir(ih);
        let files = src.join(title);
        for e in fs::read_dir(&files)? {
            let e = e?;
            if !e.file_type()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} is not a regular file", e.path().display()),
                ));
            }
            fs::File::open(e.path())?.sync_all()?;
            set_mode(&e.path(), 0o444)?;
        }
        let dst = self.sealed_dir(ih);
        if dst.exists() {
            self.remove_sealed(ih)?;
        }
        // Moving a directory to another parent rewrites its `..`, which needs write permission
        // on it: the directories are closed after the rename, the files before it.
        fs::rename(&src, &dst)?;
        set_mode(&dst.join(title), 0o555)?;
        set_mode(&dst, 0o555)?;
        Ok(dst.join(title))
    }

    /// Removes a sealed bundle. Mappings of its files keep their inodes until they drop.
    pub fn remove_sealed(&self, ih: &Infohash) -> io::Result<()> {
        make_removable(&self.sealed_dir(ih))?;
        fs::remove_dir_all(self.sealed_dir(ih))
    }

    pub fn remove_incomplete(&self, ih: &Infohash) -> io::Result<()> {
        let d = self.incomplete_dir(ih);
        if !d.exists() {
            return Ok(());
        }
        make_removable(&d)?;
        fs::remove_dir_all(d)
    }

    /// `incomplete/<ih>` entries older than `max_idle` that `keep` does not claim.
    pub fn stale_incomplete(&self, max_idle: std::time::Duration, keep: &dyn Fn(&Infohash) -> bool) -> Vec<Infohash> {
        let mut out = Vec::new();
        let Ok(rd) = fs::read_dir(&self.incomplete) else { return out };
        let now = std::time::SystemTime::now();
        for e in rd.flatten() {
            let Ok(ih) = e.file_name().to_string_lossy().parse::<Infohash>() else { continue };
            if keep(&ih) {
                continue;
            }
            let idle = newest_mtime(&e.path()).and_then(|m| now.duration_since(m).ok());
            if idle.is_some_and(|d| d >= max_idle) {
                out.push(ih);
            }
        }
        out
    }
}

fn newest_mtime(dir: &Path) -> Option<std::time::SystemTime> {
    let mut newest = fs::symlink_metadata(dir).ok()?.modified().ok()?;
    let mut stack = vec![dir.to_owned()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).ok()?.flatten() {
            let m = e.metadata().ok()?;
            newest = newest.max(m.modified().ok()?);
            if m.is_dir() {
                stack.push(e.path());
            }
        }
    }
    Some(newest)
}

/// Makes every directory under `dir` owner-writable so that its entries can be unlinked. Files
/// are left as they are: unlinking needs the directory's permission, not the file's.
fn make_removable(dir: &Path) -> io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    set_mode(dir, 0o700)?;
    for e in fs::read_dir(dir)? {
        let e = e?;
        if e.file_type()?.is_dir() {
            make_removable(&e.path())?;
        }
    }
    Ok(())
}
