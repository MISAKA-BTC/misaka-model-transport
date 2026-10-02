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

Design. Nothing is implemented yet; this repository is built from its RFC:

- [RFC-0001: MISAKA Torrent](docs/rfc/0001-misaka-torrent.md) — the design, with a Japanese summary
  (概要) at the top.
- [RFC index](docs/rfc/README.md).

The chain side (a declaration object and its fence) is proposed to
[MISAKA-BTC/misakas](https://github.com/MISAKA-BTC/misakas). It becomes normative there only through a
misakas RFC.

## License

[Apache-2.0](LICENSE).
