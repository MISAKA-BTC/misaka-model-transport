//! # misaka-btv2
//!
//! BitTorrent v2 (BEP 52) as RFC-0001 §2.3 uses it, with no engine:
//! - [`bencode`] — a strict decoder (canonical input only, bounded) and the encoder;
//! - [`merkle`] — per-file SHA-256 Merkle trees over 16 KiB blocks, pieces roots and piece layers;
//! - [`torrent`] — canonical torrent rule v1: the info dictionary, the `.torrent` file, the
//!   infohash, and [`torrent::piece_length_for`];
//! - [`magnet`] — `magnet:?xt=urn:btmh:1220<hex>&dn=<title>`.
//!
//! The infohash is computed here, in pure Rust, so that a declared infohash is reproducible from
//! the files alone by any packager.

pub mod bencode;
pub mod magnet;
pub mod merkle;
pub mod torrent;

pub use merkle::{BLOCK_SIZE, FileDigest, FileHasher};
pub use torrent::{InfoDict, InfoFile, Infohash, Torrent, TorrentError, infohash, piece_length_for};
