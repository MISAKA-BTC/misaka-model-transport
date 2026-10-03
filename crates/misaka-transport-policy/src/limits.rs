//! The limits of §5.3 and §4.6.

use misaka_btv2::bencode::Limits;

/// At most 64 files, the descriptor included.
pub const MAX_FILES: usize = 64;
/// The info dictionary is ≤ 1 MiB: ≤ 64 metadata blocks of 16 KiB (BEP 9).
pub const MAX_INFO_BYTES: usize = 1 << 20;
/// `piece layers` total ≤ 1 MiB.
pub const MAX_PIECE_LAYERS_BYTES: usize = 1 << 20;
/// A `.torrent` file read from disk: its info, its piece layers and some trackers.
pub const MAX_TORRENT_FILE_BYTES: usize = 3 << 20;
/// `T ≤ 4 TiB`, the same bound as a declaration's `total_bytes` (PALW-DIST-3).
pub const MAX_TOTAL_BYTES: u64 = 1 << 42;
pub const MIB: u64 = 1 << 20;
pub const GIB: u64 = 1 << 30;

/// Bencode bounds for an info dictionary: four values per file (entry, leaf, length, root) and a
/// handful at the top. A dictionary of 10⁵ files fails here, before it is built.
pub const INFO_DECODE_LIMITS: Limits = Limits { max_depth: 5, max_nodes: 4 * MAX_FILES + 8 };
/// Bencode bounds for a `.torrent`.
pub const TORRENT_DECODE_LIMITS: Limits = Limits { max_depth: 6, max_nodes: 4 * MAX_FILES + 2 * MAX_FILES + 4096 };

/// Free space required before admission: `total_bytes × 1.05 + 1 GiB` (§4.6).
pub fn required_free_space(total_bytes: u64) -> u64 {
    total_bytes.saturating_add(total_bytes / 20).saturating_add(GIB)
}
