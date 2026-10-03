#!/usr/bin/env python3
"""Phase A1's exit gate (RFC-0001 §9): the canonical infohash of a bundle equals libtorrent's.

Usage: golden_libtorrent.py <bundle-dir>

<bundle-dir> holds misaka-bundle.json and the bundle's files (as `misaka-torrent create` leaves
it) and is named after the bundle's title. The script builds a v2-only torrent of the same files
with libtorrent's python bindings, at the piece length rule v1 picks, and prints both infohashes.
It exits 1 if they differ. Needs `import libtorrent` (2.0.x) and the misaka-torrent binary.
"""

import hashlib
import os
import re
import subprocess
import sys

import libtorrent as lt


def piece_length(total: int) -> int:
    p = 1
    n = -(-total // 4096)
    while p < n:
        p <<= 1
    return min(max(p, 1 << 20), 16 << 20)


def main() -> int:
    d = os.path.abspath(sys.argv[1])
    fs = lt.file_storage()
    lt.add_files(fs, d)
    total = sum(fs.file_size(i) for i in range(fs.num_files()))
    flags = getattr(lt.create_torrent, "v2_only", None)
    if flags is None:
        flags = lt.create_torrent_flags_t.v2_only
    ct = lt.create_torrent(fs, piece_length(total), flags)
    lt.set_piece_hashes(ct, os.path.dirname(d))
    info = lt.bencode(ct.generate()[b"info"])
    theirs = hashlib.sha256(info).hexdigest()

    exe = os.environ.get("MISAKA_TORRENT", "misaka-torrent")
    out = subprocess.run([exe, "inspect", d], check=True, capture_output=True, text=True).stdout
    ours = re.search(r"^infohash\s+([0-9a-f]{64})$", out, re.M).group(1)
    print(f"libtorrent {lt.__version__}  {theirs}")
    print(f"misaka-torrent      {ours}")
    if theirs != ours:
        print("MISMATCH", file=sys.stderr)
        return 1
    print("equal")
    return 0


if __name__ == "__main__":
    sys.exit(main())
