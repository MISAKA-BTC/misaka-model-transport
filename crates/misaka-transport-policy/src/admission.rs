//! Admission: the info dictionary against the infohash, then against the profile, before the
//! engine sees it and before any file is created (MT-3, §4.2 `Admitted`).

use misaka_btv2::torrent::{InfoDict, Infohash, Torrent, infohash};
use misaka_bundle::{BundleCommitment, BundleKind, is_valid_title};

use crate::Refusal;
use crate::limits::{self, INFO_DECODE_LIMITS, MAX_FILES, MAX_INFO_BYTES, MAX_PIECE_LAYERS_BYTES, MAX_TOTAL_BYTES};
use crate::names::check_names;
use crate::profile::{FileClass, check_kind, classify, infer_kind};

/// What the anchor (a declaration, a manifest row, `--expect`) says about the bundle. Every field
/// is optional: an unanchored fetch knows none of them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expectation {
    pub bundle_commitment: Option<BundleCommitment>,
    pub total_bytes: Option<u64>,
    pub kind: Option<BundleKind>,
}

/// The host's side of admission. `None` skips a check (`inspect` has no store).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AdmissionEnv {
    pub free_space: Option<u64>,
    pub quota_remaining: Option<u64>,
}

/// An info dictionary that passed the Safe Model Profile. Only this crate constructs one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedBundle {
    infohash: Infohash,
    info_bytes: Vec<u8>,
    info: InfoDict,
    kind: BundleKind,
    classes: Vec<FileClass>,
}

impl AdmittedBundle {
    pub fn infohash(&self) -> Infohash {
        self.infohash
    }

    /// The exact bencoded info dictionary, whose SHA-256 is the infohash.
    pub fn info_bytes(&self) -> &[u8] {
        &self.info_bytes
    }

    pub fn info(&self) -> &InfoDict {
        &self.info
    }

    /// The torrent's name, the descriptor's title.
    pub fn title(&self) -> &str {
        &self.info.name
    }

    pub fn kind(&self) -> BundleKind {
        self.kind
    }

    pub fn total_bytes(&self) -> u64 {
        self.info.total_bytes()
    }

    /// `(file, class)`, in the info dictionary's order.
    pub fn files(&self) -> impl Iterator<Item = (&misaka_btv2::InfoFile, FileClass)> {
        self.info.files.iter().zip(self.classes.iter().copied())
    }
}

/// Admits `info_bytes` fetched by `infohash`, under `expect` and `env`.
pub fn admit(
    info_bytes: &[u8],
    expected_infohash: &Infohash,
    expect: &Expectation,
    env: &AdmissionEnv,
) -> Result<AdmittedBundle, Refusal> {
    if info_bytes.len() > MAX_INFO_BYTES {
        return Err(Refusal::InfoTooLarge(info_bytes.len()));
    }
    if infohash(info_bytes) != *expected_infohash {
        return Err(Refusal::InfohashMismatch);
    }
    let info = InfoDict::parse_canonical(info_bytes, INFO_DECODE_LIMITS).map_err(|e| match e {
        // A dictionary too large for the node budget is a dictionary with too many files.
        misaka_btv2::TorrentError::Bencode(misaka_btv2::bencode::DecodeError::TooManyNodes(_)) => {
            Refusal::TooManyFiles(MAX_FILES + 1)
        }
        e => Refusal::Rule(e),
    })?;
    if !is_valid_title(&info.name) {
        return Err(Refusal::Title(info.name.clone()));
    }
    if info.files.len() > MAX_FILES {
        return Err(Refusal::TooManyFiles(info.files.len()));
    }
    check_names(info.files.iter().map(|f| f.name.as_str()))?;
    let mut classes = Vec::with_capacity(info.files.len());
    for f in &info.files {
        let class = classify(&f.name)?;
        if let Some(max) = class.max_size().filter(|&m| f.length > m) {
            return Err(Refusal::FileTooLarge { name: f.name.clone(), size: f.length, max });
        }
        classes.push(class);
    }
    let found = infer_kind(&classes);
    let kind = match expect.kind {
        Some(k) if !k.is_active() => return Err(Refusal::ReservedKind(k)),
        Some(k) if k != found => return Err(Refusal::KindMismatch { expected: k, found }),
        _ => found,
    };
    let named: Vec<(&str, FileClass)> =
        info.files.iter().map(|f| f.name.as_str()).zip(classes.iter().copied()).collect();
    check_kind(kind, &named)?;

    let total = info.total_bytes();
    if total > MAX_TOTAL_BYTES {
        return Err(Refusal::TotalTooLarge(total));
    }
    if let Some(declared) = expect.total_bytes.filter(|&d| d != total) {
        return Err(Refusal::TotalMismatch { declared, found: total });
    }
    if let Some(remaining) = env.quota_remaining.filter(|&r| total > r) {
        return Err(Refusal::Quota { total, remaining });
    }
    if let Some(free) = env.free_space {
        let needed = limits::required_free_space(total);
        if free < needed {
            return Err(Refusal::NoSpace { needed, free });
        }
    }
    Ok(AdmittedBundle { infohash: *expected_infohash, info_bytes: info_bytes.to_vec(), info, kind, classes })
}

/// The `.torrent`-level limit: piece layers ≤ 1 MiB (§5.3).
pub fn check_torrent_limits(t: &Torrent) -> Result<(), Refusal> {
    let layers: usize = t.piece_layers.values().map(Vec::len).sum();
    if layers > MAX_PIECE_LAYERS_BYTES {
        return Err(Refusal::PieceLayersTooLarge(layers));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use misaka_btv2::bencode::{self, Value};
    use misaka_btv2::{InfoDict, InfoFile};

    fn info(names: &[(&str, u64)]) -> InfoDict {
        InfoDict::new(
            "bundle",
            names.iter().map(|(n, l)| InfoFile { name: (*n).into(), length: *l, pieces_root: [9; 32] }).collect(),
        )
    }

    fn try_admit(i: &InfoDict, expect: &Expectation, env: &AdmissionEnv) -> Result<AdmittedBundle, Refusal> {
        let bytes = i.encode();
        admit(&bytes, &misaka_btv2::infohash(&bytes), expect, env)
    }

    fn ok_files() -> Vec<(&'static str, u64)> {
        vec![("misaka-bundle.json", 900), ("LICENSE", 100), ("m.gguf", 5 << 30)]
    }

    #[test]
    fn admits_a_source_bundle() {
        let b = try_admit(&info(&ok_files()), &Expectation::default(), &AdmissionEnv::default()).unwrap();
        assert_eq!(b.kind(), BundleKind::SourceWeights);
        assert_eq!(b.total_bytes(), 900 + 100 + (5 << 30));
    }

    #[test]
    fn checks_the_infohash_first() {
        let bytes = info(&ok_files()).encode();
        let r = admit(&bytes, &misaka_btv2::Infohash([0; 32]), &Expectation::default(), &AdmissionEnv::default());
        assert_eq!(r, Err(Refusal::InfohashMismatch));
    }

    #[test]
    fn hostile_dictionaries() {
        let none = Expectation::default();
        let env = AdmissionEnv::default();
        let with = |extra: (&'static str, u64)| {
            let mut f = ok_files();
            f.push(extra);
            info(&f)
        };
        assert!(matches!(try_admit(&with(("pytorch_model.bin", 10)), &none, &env), Err(Refusal::RefusedFormat { .. })));
        assert!(matches!(try_admit(&with(("run.sh", 10)), &none, &env), Err(Refusal::RefusedFormat { .. })));
        assert!(matches!(try_admit(&with(("w.zip", 10)), &none, &env), Err(Refusal::RefusedFormat { .. })));
        assert!(matches!(try_admit(&with(("CON.json", 10)), &none, &env), Err(Refusal::Name { .. })));
        assert!(matches!(try_admit(&with(("license", 10)), &none, &env), Err(Refusal::CaseCollision(_))));
        assert!(matches!(try_admit(&with(("README.md", 2 << 20)), &none, &env), Err(Refusal::FileTooLarge { .. })));
        assert!(matches!(try_admit(&info(&[("LICENSE", 1), ("m.gguf", 1)]), &none, &env), Err(Refusal::NoDescriptor)));

        // `..` and a path separator inside a name.
        let dotdot = InfoDict { name: "bundle".into(), piece_length: 1 << 20, files: vec![] };
        let mut v = dotdot.to_value();
        let leaf = Value::dict([("length", Value::Int(1)), ("pieces root", Value::bytes(vec![0; 32]))]);
        if let Value::Dict(d) = &mut v {
            d.insert(b"file tree".to_vec(), Value::dict([("../evil", Value::dict([("", leaf)]))]));
        }
        let bytes = bencode::encode(&v);
        assert!(admit(&bytes, &misaka_btv2::infohash(&bytes), &none, &env).is_err());

        // 10⁵ files fail on the node budget, before a vector of them is built.
        let many: Vec<(String, u64)> = (0..100_000).map(|i| (format!("f{i}.json"), 1)).collect();
        let big = InfoDict::new(
            "bundle",
            many.iter().map(|(n, l)| InfoFile { name: n.clone(), length: *l, pieces_root: [0; 32] }).collect(),
        );
        let bytes = big.encode();
        let r = admit(&bytes, &misaka_btv2::infohash(&bytes), &none, &env);
        assert!(matches!(r, Err(Refusal::InfoTooLarge(_)) | Err(Refusal::TooManyFiles(_))), "{r:?}");
    }

    #[test]
    fn declaration_and_host() {
        let i = info(&ok_files());
        let total = i.total_bytes();
        let lie = Expectation { total_bytes: Some(total - 1), ..Default::default() };
        assert!(matches!(try_admit(&i, &lie, &AdmissionEnv::default()), Err(Refusal::TotalMismatch { .. })));
        let wrong_kind = Expectation { kind: Some(BundleKind::PalwArtifact), ..Default::default() };
        assert!(matches!(try_admit(&i, &wrong_kind, &AdmissionEnv::default()), Err(Refusal::KindMismatch { .. })));
        let tight = AdmissionEnv { free_space: Some(total + (1 << 30)), quota_remaining: None };
        assert!(matches!(try_admit(&i, &Expectation::default(), &tight), Err(Refusal::NoSpace { .. })));
        let quota = AdmissionEnv { free_space: None, quota_remaining: Some(total - 1) };
        assert!(matches!(try_admit(&i, &Expectation::default(), &quota), Err(Refusal::Quota { .. })));
        let huge = info(&[("misaka-bundle.json", 900), ("LICENSE", 100), ("m.gguf", (1 << 42) + 1)]);
        assert!(matches!(
            try_admit(&huge, &Expectation::default(), &AdmissionEnv::default()),
            Err(Refusal::TotalTooLarge(_))
        ));
    }
}
