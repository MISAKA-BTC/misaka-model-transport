//! `bundle_commitment = BLAKE2b-512(key = "misaka-torrent/bundle/v1", msg = u64_le(len) ‖ bytes)`
//! (RFC-0001 §2.2).

use crate::hexfmt::Hex64;

/// The BLAKE2b key of the commitment.
pub const BUNDLE_COMMITMENT_KEY: &[u8] = b"misaka-torrent/bundle/v1";

/// A descriptor's commitment: 64 bytes, the chain's hash width.
pub type BundleCommitment = Hex64;

/// misakas's `keyed64` (`palw_model_lines_v1.rs:47-55`), reproduced: BLAKE2b with a 64-byte output,
/// keyed by the domain, over the concatenation of `parts`.
fn keyed64(domain: &[u8], parts: &[&[u8]]) -> [u8; 64] {
    let mut state = blake2b_simd::Params::new().hash_length(64).key(domain).to_state();
    for part in parts {
        state.update(part);
    }
    let mut out = [0u8; 64];
    out.copy_from_slice(state.finalize().as_bytes());
    out
}

/// The commitment over the exact descriptor bytes. A reader never re-serializes before hashing.
pub fn bundle_commitment(descriptor_bytes: &[u8]) -> BundleCommitment {
    let len = (descriptor_bytes.len() as u64).to_le_bytes();
    Hex64(keyed64(BUNDLE_COMMITMENT_KEY, &[&len, descriptor_bytes]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cross-implementation vectors, computed independently with Python's
    /// `hashlib.blake2b(u64_le(len) + msg, key=b"misaka-torrent/bundle/v1", digest_size=64)`.
    #[test]
    fn golden_vectors() {
        assert_eq!(bundle_commitment(b"").to_string(), GOLDEN_EMPTY);
        assert_eq!(bundle_commitment(b"{}\n").to_string(), GOLDEN_BRACES);
    }

    const GOLDEN_EMPTY: &str = "48a9b43ccaf07ab182d65d58be6edb4e6cc36f8919bd550675fb91cd2c9a3e68252e276a38bd16790e1be7a197d0d6f6590afb6d6654efcfe606b8be2ee73d43";
    const GOLDEN_BRACES: &str = "fa1fad724d12d22a3bd5195e1bc7d487fe79bd07f1641b8081a01acc7e47f72ee1d49d5a6b4cbd4fda195510f3257f8f2b1b5c9e739e9992ea0d53577611fda0";

    #[test]
    fn length_prefix_separates_messages() {
        assert_ne!(bundle_commitment(b"a"), bundle_commitment(b"a\0"));
    }
}
