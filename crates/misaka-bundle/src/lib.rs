//! # misaka-bundle
//!
//! The bundle descriptor of RFC-0001 §2: `misaka-bundle.json`, schema `misaka/torrent-bundle/v1`,
//! its canonical form, and the `bundle_commitment` a declaration names.
//!
//! This crate knows no chain and no BitTorrent. It is a function of bytes:
//! - [`Descriptor::parse_canonical`] accepts only canonical bytes and refuses unknown keys;
//! - [`Descriptor::to_canonical_bytes`] is the one serialization;
//! - [`bundle_commitment`] is keyed BLAKE2b-512 over `u64_le(len) ‖ bytes`, the same primitive as
//!   misakas's `keyed64` (`consensus/core/src/palw_model_lines_v1.rs:47-55`).
//!
//! The `misaka-model://` deep link of §8.2 is parsed strictly by [`link`].

mod commitment;
mod descriptor;

pub mod hexfmt;
mod kind;
pub mod link;

pub use commitment::{BUNDLE_COMMITMENT_KEY, BundleCommitment, bundle_commitment};
pub use descriptor::{
    DESCRIPTOR_FILE_NAME, DESCRIPTOR_SCHEMA, Descriptor, DescriptorError, FileEntry, License, MAX_DESCRIPTOR_BYTES,
    PalwInfo, PalwMagic, PalwRoot, Role, is_valid_spdx, is_valid_title,
};
pub use hexfmt::{Hex32, Hex64};
pub use kind::{BundleKind, UnknownKind};
