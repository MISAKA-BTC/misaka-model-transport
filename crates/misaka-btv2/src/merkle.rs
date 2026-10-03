//! BEP 52 per-file Merkle trees.
//!
//! A file is cut into 16 KiB blocks; a leaf is the SHA-256 of a block (the last one hashed as it
//! is, unpadded). The tree has the next power of two of the block count as its leaves, and the
//! leaves beyond the end of the file are zero. The **pieces root** is the tree's root; it does
//! not depend on the piece length. The **piece layer** for a piece length `P` is the layer whose
//! nodes each cover `P / 16 KiB` leaves, truncated to the file's pieces, and exists only for
//! files longer than `P`.
//!
//! [`FileHasher`] streams a file once and keeps a node per MiB — 32 bytes per MiB, ≈ 9.4 MiB
//! for a 300 GiB file — from which the root and the piece layer for any `P` in 1–16 MiB follow.
//! It computes the file's SHA-256 in the same pass.

use sha2::{Digest, Sha256};

/// A BEP 52 leaf: 16 KiB.
pub const BLOCK_SIZE: u64 = 16 * 1024;
/// The granularity [`FileHasher`] keeps: 1 MiB, the smallest piece rule v1 allows.
pub const CHUNK_SIZE: u64 = 1 << 20;
const BLOCKS_PER_CHUNK: u64 = CHUNK_SIZE / BLOCK_SIZE;

pub type Hash = [u8; 32];

fn h2(a: &Hash, b: &Hash) -> Hash {
    let mut s = Sha256::new();
    s.update(a);
    s.update(b);
    s.finalize().into()
}

/// The root of a subtree of `2^height` zero leaves.
pub fn zero_root(height: u32) -> Hash {
    let mut z = [0u8; 32];
    for _ in 0..height {
        z = h2(&z, &z);
    }
    z
}

/// The root of `nodes` padded to `width` (a power of two ≥ `nodes.len()`) with `pad`, where `pad`
/// is the root of a zero subtree at the nodes' height.
pub fn root_of(nodes: &[Hash], width: usize, pad: Hash) -> Hash {
    assert!(width.is_power_of_two() && width >= nodes.len() && !nodes.is_empty());
    let mut layer: Vec<Hash> = nodes.to_vec();
    layer.resize(width, pad);
    while layer.len() > 1 {
        layer = layer.chunks(2).map(|p| h2(&p[0], &p[1])).collect();
    }
    layer[0]
}

/// Streams a file's bytes into its SHA-256 and its Merkle nodes.
pub struct FileHasher {
    sha: Sha256,
    size: u64,
    /// The block being filled.
    block: Vec<u8>,
    /// Leaves of the chunk being filled.
    leaves: Vec<Hash>,
    /// One node per complete MiB chunk.
    chunks: Vec<Hash>,
    /// The leaves of the first chunk, kept while the file is no longer than one chunk.
    first_leaves: Option<Vec<Hash>>,
}

impl Default for FileHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl FileHasher {
    pub fn new() -> Self {
        FileHasher {
            sha: Sha256::new(),
            size: 0,
            block: Vec::with_capacity(BLOCK_SIZE as usize),
            leaves: Vec::with_capacity(BLOCKS_PER_CHUNK as usize),
            chunks: Vec::new(),
            first_leaves: None,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.sha.update(data);
        self.size += data.len() as u64;
        while !data.is_empty() {
            let take = (BLOCK_SIZE as usize - self.block.len()).min(data.len());
            self.block.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.block.len() == BLOCK_SIZE as usize {
                self.push_block();
            }
        }
    }

    fn push_block(&mut self) {
        let leaf: Hash = Sha256::digest(&self.block).into();
        self.block.clear();
        self.leaves.push(leaf);
        if self.leaves.len() == BLOCKS_PER_CHUNK as usize {
            self.push_chunk();
        }
    }

    fn push_chunk(&mut self) {
        if self.chunks.is_empty() {
            self.first_leaves = Some(self.leaves.clone());
        } else {
            self.first_leaves = None;
        }
        self.chunks.push(root_of(&self.leaves, BLOCKS_PER_CHUNK as usize, [0; 32]));
        self.leaves.clear();
    }

    /// Finishes the file. An empty file has no tree; rule v1 refuses empty files anyway.
    pub fn finish(mut self) -> FileDigest {
        if !self.block.is_empty() {
            self.push_block();
        }
        let size = self.size;
        let sha256: Hash = std::mem::take(&mut self.sha).finalize().into();
        if size == 0 {
            return FileDigest { size, sha256, pieces_root: [0; 32], chunks: Vec::new() };
        }
        let small_leaves = if size <= CHUNK_SIZE {
            // The tree is narrower than a chunk: its root is over the leaves themselves.
            Some(if self.leaves.is_empty() {
                self.first_leaves.take().expect("one full chunk")
            } else {
                self.leaves.clone()
            })
        } else {
            None
        };
        if !self.leaves.is_empty() {
            self.push_chunk();
        }
        let pieces_root = match small_leaves {
            Some(leaves) => root_of(&leaves, leaves.len().next_power_of_two(), [0; 32]),
            None => root_of(&self.chunks, self.chunks.len().next_power_of_two(), zero_root(BLOCKS_PER_CHUNK.ilog2())),
        };
        FileDigest { size, sha256, pieces_root, chunks: self.chunks }
    }
}

/// A file's digests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDigest {
    pub size: u64,
    pub sha256: Hash,
    pub pieces_root: Hash,
    /// One node per MiB (the last one zero-padded).
    chunks: Vec<Hash>,
}

impl FileDigest {
    /// The piece layer for `piece_length` (a power of two ≥ 1 MiB): `None` when the file is no longer than one piece, as BEP 52 has it.
    pub fn piece_layer(&self, piece_length: u64) -> Option<Vec<Hash>> {
        assert!(piece_length.is_power_of_two() && piece_length >= CHUNK_SIZE);
        if self.size <= piece_length {
            return None;
        }
        let per_piece = (piece_length / CHUNK_SIZE) as usize;
        let pad = zero_root(BLOCKS_PER_CHUNK.ilog2());
        Some(self.chunks.chunks(per_piece).map(|c| root_of(c, per_piece, pad)).collect())
    }
}

/// The pieces root a piece layer implies, for checking a `.torrent`'s `piece layers` against a
/// file's `pieces root`.
pub fn root_from_piece_layer(layer: &[Hash], piece_length: u64) -> Hash {
    assert!(piece_length.is_power_of_two() && piece_length >= BLOCK_SIZE);
    let height = (piece_length / BLOCK_SIZE).ilog2();
    root_of(layer, layer.len().next_power_of_two(), zero_root(height))
}

/// Hashes one piece's bytes into its piece-layer node (a subtree of `piece_length / 16 KiB`
/// leaves, zero-padded). For a file no longer than one piece, compare with the pieces root via
/// [`file_root_of_bytes`] instead.
pub fn piece_node(bytes: &[u8], piece_length: u64) -> Hash {
    let leaves: Vec<Hash> = bytes.chunks(BLOCK_SIZE as usize).map(|b| Sha256::digest(b).into()).collect();
    root_of(&leaves, (piece_length / BLOCK_SIZE) as usize, [0; 32])
}

/// The pieces root of a whole file held in memory.
pub fn file_root_of_bytes(bytes: &[u8]) -> Hash {
    let mut h = FileHasher::new();
    h.update(bytes);
    h.finish().pieces_root
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The definition, written naively: every leaf, padded to a power of two with zeros.
    fn naive_root(bytes: &[u8]) -> Hash {
        let leaves: Vec<Hash> = bytes.chunks(BLOCK_SIZE as usize).map(|b| Sha256::digest(b).into()).collect();
        root_of(&leaves, leaves.len().next_power_of_two(), [0; 32])
    }

    fn data(len: usize) -> Vec<u8> {
        let mut x: u32 = 0x9e37_79b9;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect()
    }

    #[test]
    fn root_matches_definition_at_every_boundary() {
        let b = BLOCK_SIZE as usize;
        let m = CHUNK_SIZE as usize;
        for len in [1, b - 1, b, b + 1, 3 * b, m - 1, m, m + 1, 2 * m, 2 * m + b, 5 * m + 7] {
            let bytes = data(len);
            // Feed in odd sizes so block and chunk boundaries fall inside updates.
            let mut h = FileHasher::new();
            for part in bytes.chunks(7777) {
                h.update(part);
            }
            let d = h.finish();
            assert_eq!(d.pieces_root, naive_root(&bytes), "len {len}");
            assert_eq!(d.sha256, <[u8; 32]>::from(Sha256::digest(&bytes)));
            assert_eq!(d.size, len as u64);
        }
    }

    #[test]
    fn single_block_root_is_its_leaf() {
        let bytes = b"hello";
        assert_eq!(file_root_of_bytes(bytes), <[u8; 32]>::from(Sha256::digest(bytes)));
    }

    #[test]
    fn piece_layers_imply_the_root() {
        let m = CHUNK_SIZE as usize;
        let bytes = data(5 * m + 12345);
        let mut h = FileHasher::new();
        h.update(&bytes);
        let d = h.finish();
        for p in [1u64 << 20, 2 << 20, 4 << 20] {
            let layer = d.piece_layer(p).unwrap();
            assert_eq!(layer.len(), bytes.len().div_ceil(p as usize));
            assert_eq!(root_from_piece_layer(&layer, p), d.pieces_root, "P {p}");
            for (i, piece) in bytes.chunks(p as usize).enumerate() {
                assert_eq!(piece_node(piece, p), layer[i]);
            }
        }
        assert!(d.piece_layer(8 << 20).is_none());
    }
}
