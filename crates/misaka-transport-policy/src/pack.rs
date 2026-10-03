//! Packing a directory into a bundle: the descriptor and the canonical torrent (§2, §4.5).
//!
//! [`create`] writes nothing; it returns the descriptor's bytes and the torrent for the caller
//! to write. [`rebuild`] recomputes the torrent of a directory that already holds its
//! descriptor, which is what `seed --adopt` and `inspect <dir>` do. Both check their result
//! against the same profile a downloader applies, so a packager cannot produce what a downloader
//! would refuse.

use std::path::Path;

use misaka_btv2::magnet;
use misaka_btv2::torrent::{Infohash, Torrent};
use misaka_btv2::{FileDigest, merkle};
use misaka_bundle::{
    BundleCommitment, BundleKind, DESCRIPTOR_FILE_NAME, DESCRIPTOR_SCHEMA, Descriptor, DescriptorError, FileEntry,
    Hex32, Hex64, License, PalwInfo, PalwMagic, PalwRoot, bundle_commitment,
};

use crate::admission::{AdmissionEnv, AdmittedBundle, Expectation, admit};
use crate::l2::{self, L2Mismatch};
use crate::limits::MAX_FILES;
use crate::names::check_names;
use crate::profile::{FileClass, check_kind, classify};
use crate::{Refusal, check_torrent_limits};

#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Refused(#[from] Refusal),
    #[error("descriptor: {0}")]
    Descriptor(#[from] DescriptorError),
    #[error("{0}")]
    L2(#[from] L2Mismatch),
    #[error("{0:?} is not a regular file (no directories, links or devices in a bundle)")]
    NotRegular(String),
    #[error("{0}")]
    Usage(String),
}

/// What `misaka-torrent create` is told.
#[derive(Debug, Clone)]
pub struct CreateOptions {
    pub kind: BundleKind,
    pub title: String,
    pub spdx: String,
    /// The license file's name; `LICENSE` by default.
    pub license_file: String,
    /// The registry's roots for the PALW container, copied (`palw-artifact` only).
    pub palw_roots: Vec<PalwRoot>,
    /// The container's own digest, when it defines one.
    pub artifact_digest: Option<Hex64>,
}

/// A packed bundle.
#[derive(Debug, Clone)]
pub struct Packed {
    pub descriptor: Descriptor,
    pub descriptor_bytes: Vec<u8>,
    pub torrent: Torrent,
    pub bundle: AdmittedBundle,
}

impl Packed {
    pub fn commitment(&self) -> BundleCommitment {
        bundle_commitment(&self.descriptor_bytes)
    }

    pub fn infohash(&self) -> Infohash {
        self.bundle.infohash()
    }

    pub fn magnet(&self) -> String {
        magnet::format(&self.infohash(), &self.descriptor.title)
    }

    pub fn total_bytes(&self) -> u64 {
        self.bundle.total_bytes()
    }
}

/// The regular files directly in `dir`, sorted by name. A directory, a link or anything else
/// that is not a regular file is refused, as is a name that is not UTF-8.
pub fn scan(dir: &Path) -> Result<Vec<(String, u64)>, PackError> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name =
            entry.file_name().into_string().map_err(|n| PackError::NotRegular(n.to_string_lossy().into_owned()))?;
        let ft = entry.file_type()?;
        if !ft.is_file() {
            return Err(PackError::NotRegular(name));
        }
        out.push((name.clone(), entry.metadata()?.len()));
        if out.len() > MAX_FILES {
            return Err(Refusal::TooManyFiles(out.len()).into());
        }
    }
    out.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    Ok(out)
}

/// Hashes files in parallel, one thread per file up to the machine's parallelism.
fn hash_all(dir: &Path, names: &[String]) -> Result<Vec<FileDigest>, PackError> {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let slots: Vec<std::sync::Mutex<Option<Result<FileDigest, L2Mismatch>>>> =
        names.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|s| {
        for _ in 0..threads.min(names.len()) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= names.len() {
                        break;
                    }
                    *slots[i].lock().expect("unpoisoned") = Some(l2::hash_file(&dir.join(&names[i])));
                }
            });
        }
    });
    slots
        .into_iter()
        .map(|m| m.into_inner().expect("unpoisoned").expect("every slot filled").map_err(Into::into))
        .collect()
}

fn read_magic(path: &Path) -> Result<[u8; 8], PackError> {
    use std::io::Read;
    let mut f = l2::open_regular(path)?;
    let mut m = [0u8; 8];
    f.read_exact(&mut m)?;
    Ok(m)
}

/// Builds the descriptor and the canonical torrent of the files in `dir`. An existing
/// `misaka-bundle.json` is ignored and replaced in the result.
pub fn create(dir: &Path, opts: &CreateOptions) -> Result<Packed, PackError> {
    let files: Vec<(String, u64)> = scan(dir)?.into_iter().filter(|(n, _)| n != DESCRIPTOR_FILE_NAME).collect();
    let mut all_names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    all_names.push(DESCRIPTOR_FILE_NAME);
    check_names(all_names.iter().copied())?;
    let mut classes = Vec::new();
    for n in &all_names {
        classes.push((*n, classify(n)?));
    }
    check_kind(opts.kind, &classes)?;
    if !opts.palw_roots.is_empty() && opts.kind != BundleKind::PalwArtifact {
        return Err(PackError::Usage("PALW roots belong to a palw-artifact bundle".into()));
    }

    let names: Vec<String> = files.iter().map(|(n, _)| n.clone()).collect();
    let digests = hash_all(dir, &names)?;
    let mut entries = Vec::with_capacity(files.len());
    for ((name, _), d) in files.iter().zip(&digests) {
        let class = classify(name)?;
        if d.size == 0 {
            return Err(DescriptorError::EmptyFile(name.clone()).into());
        }
        let palw = match class {
            FileClass::PalwContainer(allowed) => {
                let raw = read_magic(&dir.join(name))?;
                let magic = PalwMagic::from_bytes(&raw).filter(|m| allowed.contains(m)).ok_or_else(|| {
                    PackError::Usage(format!("{name:?} does not start with a PALW magic its extension allows"))
                })?;
                let mut roots = opts.palw_roots.clone();
                roots.sort_by_key(|r| r.class_id);
                roots.dedup_by(|a, b| a.class_id == b.class_id);
                if roots.len() != opts.palw_roots.len() {
                    return Err(PackError::Usage("a class id is given twice".into()));
                }
                if roots.is_empty() {
                    return Err(PackError::Usage(
                        "a PALW container needs its registry root: --palw-root <class_id>:<inventory_root>".into(),
                    ));
                }
                Some(PalwInfo { artifact_digest: opts.artifact_digest, magic, roots })
            }
            _ => None,
        };
        entries.push(FileEntry {
            btv2_pieces_root: Hex32(d.pieces_root),
            palw,
            path: name.clone(),
            role: class.role().expect("not the descriptor"),
            sha256: Hex32(d.sha256),
            size: d.size,
        });
    }
    let descriptor = Descriptor {
        files: entries,
        kind: opts.kind,
        license: License { file: opts.license_file.clone(), spdx: opts.spdx.clone() },
        schema: DESCRIPTOR_SCHEMA.to_owned(),
        title: opts.title.clone(),
    };
    descriptor.validate()?;
    let descriptor_bytes = descriptor.to_canonical_bytes();
    let mut dh = misaka_btv2::FileHasher::new();
    dh.update(&descriptor_bytes);

    let mut named: Vec<(String, FileDigest)> = names.into_iter().zip(digests).collect();
    named.push((DESCRIPTOR_FILE_NAME.to_owned(), dh.finish()));
    finish(descriptor, descriptor_bytes, &named)
}

/// Recomputes the bundle in `dir` from its files, its descriptor included, and checks the
/// descriptor against them: the torrent `seed --adopt` serves.
pub fn rebuild(dir: &Path) -> Result<Packed, PackError> {
    let files = scan(dir)?;
    let names: Vec<String> = files.iter().map(|(n, _)| n.clone()).collect();
    check_names(names.iter().map(String::as_str))?;
    for n in &names {
        classify(n)?;
    }
    let descriptor_bytes = l2_read_descriptor(dir)?;
    let descriptor = Descriptor::parse_canonical(&descriptor_bytes)?;
    let digests = hash_all(dir, &names)?;
    for (n, d) in names.iter().zip(&digests) {
        if let Some(e) = descriptor.file(n)
            && (e.sha256.0 != d.sha256 || e.btv2_pieces_root.0 != d.pieces_root || e.size != d.size)
        {
            return Err(L2Mismatch::Digest { name: n.clone(), what: "a digest or the size" }.into());
        }
    }
    let named: Vec<(String, FileDigest)> = names.into_iter().zip(digests).collect();
    let packed = finish(descriptor, descriptor_bytes, &named)?;
    l2::verify(dir, &packed.bundle, &Expectation::default(), l2::Depth::Headers)?;
    Ok(packed)
}

fn l2_read_descriptor(dir: &Path) -> Result<Vec<u8>, PackError> {
    use std::io::Read;
    let f = l2::open_regular(&dir.join(DESCRIPTOR_FILE_NAME))
        .map_err(|e| PackError::Usage(format!("{}: {e}", dir.join(DESCRIPTOR_FILE_NAME).display())))?;
    let mut buf = Vec::new();
    f.take(misaka_bundle::MAX_DESCRIPTOR_BYTES as u64 + 1).read_to_end(&mut buf)?;
    Ok(buf)
}

fn finish(
    descriptor: Descriptor,
    descriptor_bytes: Vec<u8>,
    named: &[(String, FileDigest)],
) -> Result<Packed, PackError> {
    let torrent = Torrent::build(descriptor.title.clone(), named);
    check_torrent_limits(&torrent)?;
    let info_bytes = torrent.info.encode();
    let ih = misaka_btv2::infohash(&info_bytes);
    let expect = Expectation { kind: Some(descriptor.kind), ..Default::default() };
    let bundle = admit(&info_bytes, &ih, &expect, &AdmissionEnv::default())?;
    l2::check_against_info(&descriptor, &bundle)?;
    debug_assert_eq!(
        merkle::file_root_of_bytes(&descriptor_bytes),
        bundle.info().file(DESCRIPTOR_FILE_NAME).unwrap().pieces_root
    );
    Ok(Packed { descriptor, descriptor_bytes, torrent, bundle })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    pub(crate) fn tempdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("mt-pack-{tag}-{}-{}", std::process::id(), rand_suffix()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn rand_suffix() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64
    }

    fn opts(kind: BundleKind) -> CreateOptions {
        CreateOptions {
            kind,
            title: "t".into(),
            spdx: "Apache-2.0".into(),
            license_file: "LICENSE".into(),
            palw_roots: vec![],
            artifact_digest: None,
        }
    }

    #[test]
    fn create_then_rebuild_is_the_same_bundle() {
        let dir = tempdir("gguf");
        fs::write(dir.join("LICENSE"), "Apache License").unwrap();
        let mut gguf = b"GGUF".to_vec();
        gguf.extend((0..3_000_000u32).map(|i| (i % 251) as u8));
        fs::write(dir.join("m.gguf"), &gguf).unwrap();
        let p = create(&dir, &opts(BundleKind::SourceWeights)).unwrap();
        fs::write(dir.join(DESCRIPTOR_FILE_NAME), &p.descriptor_bytes).unwrap();
        let r = rebuild(&dir).unwrap();
        assert_eq!(r.infohash(), p.infohash());
        assert_eq!(r.commitment(), p.commitment());
        // Creating again gives the same bytes: the rule is a function of the files.
        let again = create(&dir, &opts(BundleKind::SourceWeights)).unwrap();
        assert_eq!(again.descriptor_bytes, p.descriptor_bytes);
        assert_eq!(again.torrent.encode(), p.torrent.encode());
        // L2 at full depth agrees.
        l2::verify(
            &dir,
            &p.bundle,
            &Expectation { bundle_commitment: Some(p.commitment()), ..Default::default() },
            l2::Depth::Full,
        )
        .unwrap();
        // A changed byte is caught by rebuild.
        gguf[100] ^= 1;
        fs::write(dir.join("m.gguf"), &gguf).unwrap();
        assert!(rebuild(&dir).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn palw_container() {
        let dir = tempdir("palw");
        fs::write(dir.join("LICENSE"), "Apache License").unwrap();
        let mut art = b"PALWB0A2".to_vec();
        art.extend([0u8; 5000]);
        fs::write(dir.join("m.palwart"), &art).unwrap();
        let mut o = opts(BundleKind::PalwArtifact);
        assert!(matches!(create(&dir, &o), Err(PackError::Usage(_))));
        o.palw_roots = vec![PalwRoot { class_id: Hex64([1; 64]), inventory_root: Hex64([2; 64]) }];
        let p = create(&dir, &o).unwrap();
        assert_eq!(p.descriptor.palw_container().unwrap().palw.as_ref().unwrap().magic, PalwMagic::PalwB0A2);
        // A wrong magic for the extension.
        fs::write(dir.join("m.palwart"), b"PALWQ361xxxxxxxx").unwrap();
        assert!(create(&dir, &o).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn refuses_hostile_directories() {
        let dir = tempdir("hostile");
        fs::write(dir.join("LICENSE"), "x").unwrap();
        fs::write(dir.join("m.gguf"), "GGUFxxxx").unwrap();
        fs::write(dir.join("pytorch_model.bin"), "x").unwrap();
        assert!(matches!(
            create(&dir, &opts(BundleKind::SourceWeights)),
            Err(PackError::Refused(Refusal::RefusedFormat { .. }))
        ));
        fs::remove_file(dir.join("pytorch_model.bin")).unwrap();
        fs::create_dir(dir.join("sub")).unwrap();
        assert!(matches!(create(&dir, &opts(BundleKind::SourceWeights)), Err(PackError::NotRegular(_))));
        fs::remove_dir(dir.join("sub")).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", dir.join("README.md")).unwrap();
            assert!(matches!(create(&dir, &opts(BundleKind::SourceWeights)), Err(PackError::NotRegular(_))));
            fs::remove_file(dir.join("README.md")).unwrap();
        }
        fs::write(dir.join("README.md"), [0xff, 0xfe]).unwrap();
        let p = create(&dir, &opts(BundleKind::SourceWeights)).unwrap();
        fs::write(dir.join(DESCRIPTOR_FILE_NAME), &p.descriptor_bytes).unwrap();
        let r = l2::verify(&dir, &p.bundle, &Expectation::default(), l2::Depth::Headers);
        assert!(matches!(r, Err(L2Mismatch::Content { .. })), "{r:?}");
        fs::remove_dir_all(dir).unwrap();
    }
}
