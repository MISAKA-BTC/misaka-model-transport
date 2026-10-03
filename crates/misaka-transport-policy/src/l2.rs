//! L2, the declaration level (§4.3): read-only, no network.
//!
//! - `bundle_commitment` of `misaka-bundle.json` equals the anchor's;
//! - descriptor ↔ info dictionary: paths, sizes and pieces roots exactly equal; title = name;
//! - `total_bytes` equal; roles and kind as the profile derives them;
//! - each file's leading bytes match its class and, for the container, the descriptor's magic;
//! - with [`Depth::Full`], every file's SHA-256 and pieces root recomputed from its bytes.
//!
//! Nothing here executes, renders or extracts anything. Files are opened without following
//! links and read with bounds.

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use misaka_btv2::FileHasher;
use misaka_bundle::{BundleCommitment, DESCRIPTOR_FILE_NAME, Descriptor, DescriptorError, bundle_commitment};

use crate::admission::{AdmittedBundle, Expectation};
use crate::profile::{ContentCheck, FileClass};

/// A safetensors header is at most 100 MiB (§5.1).
pub const MAX_SAFETENSORS_HEADER: u64 = 100 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// Headers, sizes and the descriptor: seconds. L1 has already verified every block.
    Headers,
    /// Also re-hash every byte: SHA-256 and the pieces root of each file.
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L2Report {
    pub commitment: BundleCommitment,
    pub descriptor: Descriptor,
    pub total_bytes: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum L2Mismatch {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{0:?} is not a regular file")]
    NotRegular(PathBuf),
    #[error("{name:?} is {found} bytes; the info dictionary says {expected}")]
    Size { name: String, expected: u64, found: u64 },
    #[error("bundle_commitment is {found}; the anchor says {expected}")]
    Commitment { expected: Box<BundleCommitment>, found: Box<BundleCommitment> },
    #[error("descriptor: {0}")]
    Descriptor(#[from] DescriptorError),
    #[error("descriptor ↔ info dictionary: {0}")]
    Mismatch(String),
    #[error("{name:?}: {why}")]
    Content { name: String, why: String },
    #[error("{name:?}: {what} recomputed from the bytes differs from the descriptor")]
    Digest { name: String, what: &'static str },
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> L2Mismatch + '_ {
    move |source| L2Mismatch::Io { path: path.to_owned(), source }
}

/// Opens a regular file for reading without following a link at its last component.
pub fn open_regular(path: &Path) -> io::Result<File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let f = opts.open(path)?;
    if !f.metadata()?.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"));
    }
    Ok(f)
}

fn read_bounded(path: &Path, max: u64) -> Result<Vec<u8>, L2Mismatch> {
    let f = open_regular(path).map_err(io_err(path))?;
    let mut buf = Vec::new();
    f.take(max + 1).read_to_end(&mut buf).map_err(io_err(path))?;
    if buf.len() as u64 > max {
        return Err(L2Mismatch::Content { name: display(path), why: format!("longer than {max} bytes") });
    }
    Ok(buf)
}

fn display(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Runs L2 on the bundle in `dir` (the directory holding its files, `<…>/<title>/`).
pub fn verify(dir: &Path, bundle: &AdmittedBundle, expect: &Expectation, depth: Depth) -> Result<L2Report, L2Mismatch> {
    // Every file of the info dictionary is a regular file of its size.
    for (f, _) in bundle.files() {
        let p = dir.join(&f.name);
        let meta = std::fs::symlink_metadata(&p).map_err(io_err(&p))?;
        if !meta.file_type().is_file() {
            return Err(L2Mismatch::NotRegular(p));
        }
        if meta.len() != f.length {
            return Err(L2Mismatch::Size { name: f.name.clone(), expected: f.length, found: meta.len() });
        }
    }

    // The descriptor and its commitment.
    let dpath = dir.join(DESCRIPTOR_FILE_NAME);
    let dbytes = read_bounded(&dpath, misaka_bundle::MAX_DESCRIPTOR_BYTES as u64)?;
    let commitment = bundle_commitment(&dbytes);
    if let Some(expected) = expect.bundle_commitment.filter(|e| *e != commitment) {
        return Err(L2Mismatch::Commitment { expected: Box::new(expected), found: Box::new(commitment) });
    }
    let descriptor = Descriptor::parse_canonical(&dbytes)?;
    check_against_info(&descriptor, bundle)?;
    if let Some(k) = expect.kind.filter(|k| *k != descriptor.kind) {
        return Err(L2Mismatch::Mismatch(format!("descriptor kind {} ≠ anchor kind {k}", descriptor.kind)));
    }
    let total = bundle.total_bytes();
    if let Some(t) = expect.total_bytes.filter(|t| *t != total) {
        return Err(L2Mismatch::Mismatch(format!("total_bytes {total} ≠ anchor {t}")));
    }

    // Leading bytes and formats, read with bounds.
    for (f, class) in bundle.files() {
        check_content(&dir.join(&f.name), &f.name, f.length, class, &descriptor)?;
    }

    if depth == Depth::Full {
        for (f, _) in bundle.files() {
            let p = dir.join(&f.name);
            let d = hash_file(&p)?;
            if d.pieces_root != f.pieces_root {
                return Err(L2Mismatch::Digest { name: f.name.clone(), what: "the pieces root" });
            }
            if let Some(e) = descriptor.file(&f.name).filter(|e| e.sha256.0 != d.sha256) {
                return Err(L2Mismatch::Digest { name: e.path.clone(), what: "SHA-256" });
            }
        }
    }
    Ok(L2Report { commitment, descriptor, total_bytes: total })
}

/// Hashes a file in one pass: SHA-256 and its BEP 52 tree.
pub fn hash_file(path: &Path) -> Result<misaka_btv2::FileDigest, L2Mismatch> {
    let mut f = open_regular(path).map_err(io_err(path))?;
    let mut h = FileHasher::new();
    let mut buf = vec![0u8; 4 << 20];
    loop {
        let n = f.read(&mut buf).map_err(io_err(path))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finish())
}

/// Descriptor ↔ info dictionary (MT-2): the file set plus the descriptor itself, sizes, pieces
/// roots, title, kind and roles.
pub fn check_against_info(d: &Descriptor, bundle: &AdmittedBundle) -> Result<(), L2Mismatch> {
    let info = bundle.info();
    if d.title != info.name {
        return Err(L2Mismatch::Mismatch(format!("title {:?} ≠ torrent name {:?}", d.title, info.name)));
    }
    if d.kind != bundle.kind() {
        return Err(L2Mismatch::Mismatch(format!("descriptor kind {} ≠ the file set's {}", d.kind, bundle.kind())));
    }
    let listed = d.torrent_file_names();
    let torrent: std::collections::BTreeSet<&str> = info.files.iter().map(|f| f.name.as_str()).collect();
    if listed != torrent {
        let extra: Vec<_> = listed.symmetric_difference(&torrent).collect();
        return Err(L2Mismatch::Mismatch(format!("file sets differ at {extra:?}")));
    }
    for (f, class) in bundle.files() {
        if f.name == DESCRIPTOR_FILE_NAME {
            continue;
        }
        let e = d.file(&f.name).expect("same set");
        if e.size != f.length {
            return Err(L2Mismatch::Mismatch(format!("{:?}: size {} ≠ {}", f.name, e.size, f.length)));
        }
        if e.btv2_pieces_root.0 != f.pieces_root {
            return Err(L2Mismatch::Mismatch(format!("{:?}: pieces root differs", f.name)));
        }
        if Some(e.role) != class.role() {
            return Err(L2Mismatch::Mismatch(format!("{:?}: role {} is not the profile's", f.name, e.role)));
        }
        if let (FileClass::PalwContainer(allowed), Some(p)) = (class, &e.palw)
            && !allowed.contains(&p.magic)
        {
            return Err(L2Mismatch::Mismatch(format!(
                "{:?}: magic {} does not belong to its extension",
                f.name, p.magic
            )));
        }
    }
    Ok(())
}

fn check_content(path: &Path, name: &str, len: u64, class: FileClass, d: &Descriptor) -> Result<(), L2Mismatch> {
    let bad = |why: String| L2Mismatch::Content { name: name.to_owned(), why };
    let head = |n: usize| -> Result<Vec<u8>, L2Mismatch> {
        let mut f = open_regular(path).map_err(io_err(path))?;
        let mut b = vec![0u8; n];
        f.read_exact(&mut b).map_err(|_| bad(format!("shorter than {n} bytes")))?;
        Ok(b)
    };
    match class.content_check() {
        ContentCheck::PalwMagic => {
            let magic = head(8)?;
            let declared = d.file(name).and_then(|e| e.palw.as_ref()).map(|p| p.magic);
            match declared {
                Some(m) if m.bytes().as_slice() == magic.as_slice() => {}
                _ => {
                    return Err(bad(format!(
                        "leading bytes {:?} are not the descriptor's magic",
                        String::from_utf8_lossy(&magic)
                    )));
                }
            }
        }
        ContentCheck::Json => {
            let max = class.max_size().unwrap_or(64 << 20);
            let bytes = read_bounded(path, max)?;
            serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|e| bad(format!("not UTF-8 JSON: {e}")))?;
        }
        ContentCheck::Gguf => {
            if head(4)? != b"GGUF" {
                return Err(bad("no GGUF magic".into()));
            }
        }
        ContentCheck::Safetensors => {
            let n = u64::from_le_bytes(head(8)?.try_into().expect("8"));
            if n > MAX_SAFETENSORS_HEADER || n.checked_add(8).is_none_or(|e| e > len) {
                return Err(bad(format!("header length {n} is out of bounds")));
            }
            let mut f = open_regular(path).map_err(io_err(path))?;
            let mut buf = vec![0u8; 8 + n as usize];
            f.read_exact(&mut buf).map_err(io_err(path))?;
            let v: serde_json::Value =
                serde_json::from_slice(&buf[8..]).map_err(|e| bad(format!("header is not JSON: {e}")))?;
            if !v.is_object() {
                return Err(bad("header is not a JSON object".into()));
            }
        }
        ContentCheck::Utf8 => {
            let bytes = read_bounded(path, class.max_size().unwrap_or(1 << 20))?;
            std::str::from_utf8(&bytes).map_err(|_| bad("not UTF-8".into()))?;
        }
        ContentCheck::None => {}
    }
    Ok(())
}
