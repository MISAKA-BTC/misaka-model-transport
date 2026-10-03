//! # misaka-transport-policy
//!
//! The Safe Model Profile v1 (RFC-0001 §5): an allowlist applied to the info dictionary **before**
//! the engine sees it ([`admit`]) and again to the bytes after download ([`l2`]).
//!
//! [`AdmittedBundle`] can only be constructed here. The engine takes nothing else, so nothing
//! reaches it without passing the profile, and the type system says so (§6.1).
//!
//! [`pack`] builds a bundle's descriptor and canonical torrent from a directory (`create`,
//! `seed --adopt`), and checks its result against the same profile.

mod admission;
pub mod l2;
pub mod limits;
pub mod names;
pub mod pack;
pub mod profile;
mod refusal;

pub use admission::{AdmissionEnv, AdmittedBundle, Expectation, admit, check_torrent_limits};
pub use refusal::Refusal;
