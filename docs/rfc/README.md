# RFC index

An RFC is a proposal under discussion. Numbers are local to this repository; an RFC of
[MISAKA-BTC/misakas](https://github.com/MISAKA-BTC/misakas) is cited as "misakas RFC-NNNN". Take the next
number from this table, not from `ls`: an RFC can live on a branch before it reaches `main`.

A part of an RFC here that changes misakas's consensus (a lifecycle object, a fence, an RPC) is a
proposal *to* misakas. It becomes normative only through a misakas RFC that adopts it and cites the text
here.

| RFC | Title | Status | Where |
| --- | --- | --- | --- |
| 0001 | MISAKA Torrent: BitTorrent v2 transport for MISAKA model artifacts — bundles (one immutable file set per line, version and kind: `palw-artifact` for PALW containers, `source-weights` for GGUF/safetensors) with a canonical descriptor and a canonical, reproducible v2-only torrent; a sidecar daemon `misaka-torrentd` on libtorrent 2.0.x that knows no chain, holds no key and executes nothing; three verification levels (BEP 52 Merkle, the descriptor commitment, the PALW inventory root recomputed against the version root); a Safe Model Profile allowlist; installation by hard link where misakas already looks; every downloader can seed (desktop by default, servers by choice); the misaka CLI, misakaoptions.com's "Download with MISAKA" and a `misaka-model://` deep link; Part B, proposed to misakas: `ModelDistributionDeclared`, the line's own declaration of a version's bundle, read by no rule, behind its own dormant fence `palw_model_distribution`; no seeding rewards | Draft, 2026-10-02 | [0001-misaka-torrent.md](0001-misaka-torrent.md) |
| 0002 | Private transport: Direct, Private and Anonymous network modes for MISAKA Torrent — the same bundles, infohashes and L1/L2/L3 verification, carried over I2P (libtorrent's SAM v3 support, a bundled i2pd) when a user does not want peers to see their address; hidden seeding by construction and no clearnet exit; no clearnet discovery and a single loopback exit in I2P modes, enforced by confinement; one daemon instance per mode with transient destinations; I2P trackers (`misaka-i2p-tracker`) listed in the index; whole-index resolution; honest labels; throughput measured before it is claimed; a MISAKA onion overlay deferred to its own RFC; no consensus change | Draft, 2026-10-03 | [0002-private-transport.md](0002-private-transport.md) |

**Next free number: RFC-0003.**
