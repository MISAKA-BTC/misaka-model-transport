# MISAKA Model Transport

Peer-to-peer distribution of MISAKA model artifacts over BitTorrent v2 — "MISAKA Torrent".

The MISAKA chain pins what a model version is: an inventory root over its canonical rows. It
deliberately says nothing about where the bytes live. This repository is the transport for those bytes:
- PALW containers (`.palwart`, `.palwq36`, `.palwtir`);
- the public weights they are converted from (GGUF, safetensors).

It moves them between any hosts that hold them. Every byte is verified against BitTorrent v2's Merkle
trees, against a canonical bundle descriptor, and — for PALW containers — against the version root on
the chain.

- **Transport only.** The daemon knows no chain, holds no key, exposes no network API and executes
  nothing it receives.
- **Safe formats only.** Weights and metadata formats are allowlisted. Pickle, code and archives are
  refused.
- **Every downloader can seed.**

## Status

This repository's part of the design, phases A1–A5 of the RFC, is implemented; the RFC itself is still
a draft:

- [RFC-0001: MISAKA Torrent](docs/rfc/0001-misaka-torrent.md) — the design, with a Japanese summary
  (概要) at the top.
- [RFC index](docs/rfc/README.md).

| phase | what | state |
| --- | --- | --- |
| A1 | `misaka-bundle`, `misaka-btv2` | done. The canonical infohash equals libtorrent 2.0.15's (`scripts/golden_libtorrent.cpp`) for a real 1.8 GB bundle and for synthetic bundles at the piece rule's boundaries. The commitment equals an independent BLAKE2b |
| A2 | `misaka-transport-policy`, `fuzz/` | the D-T7 corpus is refused. All six fuzz targets ran clean (smoke runs). The 24 h run started 2026-10-03 04:55 CEST on a test server |
| A3 | `misaka-transport-ipc`, the fake engine, the daemon's state machine | done. Every state of §4.2 is driven by tests over the fake swarm, D-T6 included |
| A4 | `shim/libtorrent` and its backend | done against libtorrent 2.0.15 on Linux. A 1.8 GB bundle went between two daemons over the DHT, byte-identical and sealed 0444 |
| A5 | `misaka-torrentd`, `misaka-torrent`, confinement | running as `misaka-torrentd.service` on the server behind misakaoptions.com. D-T9 (`misaka-torrentd drill-confinement`) passes on Linux: Landlock fully enforced, seccomp, no exec. The macOS profile was checked with `sandbox-exec`. D-T8 needs a kaspad mapping the seeded file |

The chain side (a declaration object and its fence) is proposed to
[MISAKA-BTC/misakas](https://github.com/MISAKA-BTC/misakas). It becomes normative there only through a
misakas RFC. `misaka model …` (A6), Part B, misakaoptions.com and Studio (C) live there, not here.

## Layout

```text
crates/misaka-bundle/            the descriptor (misaka/torrent-bundle/v1), bundle_commitment, misaka-model:// links
crates/misaka-btv2/              strict bencode, BEP 52 Merkle trees, canonical torrent rule v1, magnets
crates/misaka-transport-policy/  the Safe Model Profile, admission (AdmittedBundle), L2, packing a directory
crates/misaka-transport-ipc/     misaka-torrent-borsh/v1 and a blocking client
crates/misaka-transport-engine/  the ModelTransport trait, a fake swarm, the libtorrent backend (feature)
shim/libtorrent/                 the C ABI over libtorrent-rasterbar 2.0.x
bin/misaka-torrentd/             the daemon: config, store, state machine, socket, confinement
bin/misaka-torrent/              the tool CLI
app/                             MISAKA Model Transport, the desktop app (Tauri 2) over misaka-torrentd
deploy/                          systemd unit, sysusers, example config; launchd agent and sandbox profile
fuzz/                            cargo-fuzz targets
scripts/golden_libtorrent.cpp    A1's cross-check against libtorrent (a .py twin for the python bindings)
integrations/misaka-options/     misakaoptions.com's download panel: patch, index builder, deployment notes
```

## Build and test

Everything but the libtorrent backend builds and tests with no C++ toolchain:

```bash
cargo test --workspace
```

The daemon needs the engine. Build libtorrent-rasterbar 2.0.15 with the patches listed in
`third_party.toml` (`patch -p1 < third_party/patches/<name>.patch` in its source tree), install it
where pkg-config finds it, then:

```bash
cargo build --release -p misaka-torrentd --features libtorrent
```

Fuzzing (nightly):

```bash
cargo +nightly fuzz run policy
```

## The desktop app

`app/` is **MISAKA Model Transport**: a window and a tray icon over the bundled `misaka-torrentd`.
- **Links**: `misaka-model://<network>/<line_id>/<version>?kind=<kind>` opens the app. It looks the link
  up in misakaoptions.com's index and shows the size and destination; nothing downloads until you
  confirm.
- **Seeding**: completed downloads seed while the app runs, and the app starts at login (in the tray).
- **Add existing model**: hard-links files obtained elsewhere into the app's adopt directory, then seeds
  them after the daemon re-hashes every byte. Nothing is copied.
- **Confinement**: the daemon runs under `deploy/launchd/misaka-torrentd.sb` on macOS, and applies
  Landlock and seccomp to itself on Linux.

Builds are served from https://misakaoptions.com/#/download: a macOS (Apple silicon) `.dmg`, a Linux
`.deb` and `.AppImage`, and CLI archives. They are not code-signed yet.

```bash
# macOS: static libtorrent 2.0.15 visible to pkg-config, then
MISAKA_LT_STATIC=1 cargo build --release -p misaka-torrentd --features libtorrent
cp target/release/misaka-torrentd app/src-tauri/binaries/misaka-torrentd-$(rustc -vV | sed -n 's/host: //p')
cd app/src-tauri && cargo tauri build
```

## Use

```bash
# Package a directory: writes misaka-bundle.json into it and <title>.torrent beside it.
misaka-torrent create ./qwen25-1.5b-a16 --kind palw-artifact --license Apache-2.0 \
    --palw-root <class_id>:<inventory_root>

# Check a directory (re-hashes every byte) or a .torrent against rule v1 and the profile.
misaka-torrent inspect ./qwen25-1.5b-a16

# Fetch by magnet. Without --expect the bytes are anchored to nothing, so say so (MT-8).
misaka-torrent pull 'magnet:?xt=urn:btmh:1220…' --expect <bundle_commitment>

# Seed files you already hold, in place and read-only (the directory must be under an adopt root).
misaka-torrent seed adopt /srv/models/qwen25-1.5b-a16 --expect <infohash>

misaka-torrent status --json
```

`misaka-torrentd run --profile desktop|server` reads `~/.misaka/torrent.toml` or
`/etc/misaka/torrent.toml` (see `deploy/systemd/torrent.toml.example`). The server profile never seeds
unless `[seeding] enabled = true`.

## License

[Apache-2.0](LICENSE).
