//! A stable-toolchain stand-in for the fuzz targets: many deterministic mutations of valid inputs
//! through every parser, asserting no panic and the round-trip properties.

use misaka_btv2::bencode::{Limits, decode, encode};
use misaka_btv2::torrent::{InfoDict, InfoFile, Torrent};
use misaka_bundle::{BundleKind, DESCRIPTOR_SCHEMA, Descriptor, FileEntry, Hex32, License, Role};
use misaka_transport_policy::limits::INFO_DECODE_LIMITS;
use misaka_transport_policy::{AdmissionEnv, Expectation, admit};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn mutate(rng: &mut Rng, input: &[u8]) -> Vec<u8> {
    let mut v = input.to_vec();
    for _ in 0..=rng.below(4) {
        match rng.below(5) {
            0 if !v.is_empty() => {
                let i = rng.below(v.len());
                v[i] = rng.next() as u8;
            }
            1 if !v.is_empty() => {
                let i = rng.below(v.len());
                v.remove(i);
            }
            2 => {
                let i = rng.below(v.len() + 1);
                v.insert(i, b"0123456789:ield-"[rng.below(16)]);
            }
            3 if v.len() > 2 => {
                let a = rng.below(v.len());
                let b = rng.below(v.len());
                v.swap(a, b);
            }
            _ => {
                let i = rng.below(v.len() + 1);
                v.truncate(i);
            }
        }
    }
    v
}

fn sample_info() -> Vec<u8> {
    InfoDict::new(
        "bundle",
        vec![
            InfoFile { name: "misaka-bundle.json".into(), length: 900, pieces_root: [1; 32] },
            InfoFile { name: "LICENSE".into(), length: 10, pieces_root: [2; 32] },
            InfoFile { name: "m.gguf".into(), length: 5 << 20, pieces_root: [3; 32] },
        ],
    )
    .encode()
}

#[test]
fn info_and_bencode_mutations() {
    let base = sample_info();
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    for _ in 0..30_000 {
        let m = mutate(&mut rng, &base);
        if let Ok(v) = decode(&m, Limits::default()) {
            assert_eq!(encode(&v), m);
        }
        if let Ok(i) = InfoDict::parse_canonical(&m, INFO_DECODE_LIMITS) {
            assert_eq!(i.encode(), m);
        }
        let ih = misaka_btv2::infohash(&m);
        if let Ok(b) = admit(&m, &ih, &Expectation::default(), &AdmissionEnv::default()) {
            assert!(b.info().files.len() <= 64);
        }
        let _ = Torrent::parse(&m, Limits::default());
    }
}

#[test]
fn descriptor_mutations() {
    let d = Descriptor {
        files: vec![FileEntry {
            btv2_pieces_root: Hex32([1; 32]),
            palw: None,
            path: "LICENSE".into(),
            role: Role::License,
            sha256: Hex32([2; 32]),
            size: 10,
        }],
        kind: BundleKind::SourceWeights,
        license: License { file: "LICENSE".into(), spdx: "MIT".into() },
        schema: DESCRIPTOR_SCHEMA.into(),
        title: "t".into(),
    };
    let base = d.to_canonical_bytes();
    let mut rng = Rng(0x0dd5_eed5_0000_0002);
    for _ in 0..30_000 {
        let m = mutate(&mut rng, &base);
        if let Ok(p) = Descriptor::parse_canonical(&m) {
            assert_eq!(p.to_canonical_bytes(), m);
        }
    }
}
