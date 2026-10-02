# RFC-0001: MISAKA Torrent — BitTorrent v2 transport for MISAKA model artifacts: the chain pins what a version is, a swarm carries its bytes, every downloader can seed, and nothing in the client executes

| Field | Value |
| --- | --- |
| Status | Draft, 2026-10-02 — design; nothing is implemented. This repository implements it from this text |
| Author(s) | MISAKA core (drafted with Claude) |
| Created | 2026-10-02 |
| Normative dependencies | misakas ADR-0088 (model lines and versions: the rows a declaration names) and ADR-0067 Decisions 4 and 6 (the chain pins *what*; distribution stays off-chain; registration and possession are never coupled). External: BEP 3, 5, 9, 10, 11 and **BEP 52** (BitTorrent v2). BEP 47's file attributes are refused, not used |
| Affects | **this repository** (every crate of §1.3; nothing exists yet) · **misakas**, proposed only and adopted through a misakas RFC that cites this one: a lifecycle object, a fence and an RPC method (Part B, §3), the `misaka model …` commands (§7.1), `web/misaka-options` (§8.1), `docs/components-manifest.md` v2 (§8.4) · **MISAKA Studio** (separate repository): the deep-link handler and a download source (§8.2–§8.3) · all networks: Part B is dormant until armed; Phases A, C and D change no consensus rule |
| Branch | `main` (text only) |
| Related | misakas ADR-0133 (option C, "chunked P2P distribution … later, as B's transport"), ADR-0100 Decision 7 item 6 (an out-of-band content-addressed artifact store and resolver), ADR-0108 Decision 9 (publication is off-chain and the id makes it safe), ADR-0136 (an artifact is mapped, not read; a host holds one copy), ADR-0135 (`artifact_prefetch_spans`), ADR-0143 (an artifact root has one owner), ADR-0079 and `docs/palw-registry-map.md` §3–§4 (the refusals; adding a field), ADR-0095 (a declaration collection gets its own fence), ADR-0101 (off-chain signed descriptors), ADR-0144 (non-goals), ADR-0096 (MISAKA Studio, the components manifest), misakas RFC-0006 (seats that hold only their layers) |

**Sources.** A path written `misakas:<path>:<lines>` is in MISAKA-BTC/misakas at commit `7e99f72`
(`main`, 2026-10-02). The BEP facts were checked against the BEP texts (BEP 9, 44, 47 and 52), the
libtorrent facts against `include/libtorrent/create_torrent.hpp` on its `RC_2_0` branch, and rqbit's
against its README, all on 2026-10-02. RFC numbers in this repository are its own; an RFC of misakas is
cited as "misakas RFC-NNNN".

## 概要(日本語)

- **問題。** MISAKA のモデル重みは、いまプロトコルの外で運ばれている。
  - misakas ADR-0133 の実測表には「artifact 転送: プロトコル内なし。24 GB のファイルをオペレーターが
    手でコピー」とある。hybrid class は抽選後に全量をダウンロードする設計だったため、一度も licence
    されなかった(1 claim あたり panel が 41 時間止まる)。
  - fleet への配布は手作業の rsync over SSH だった。Mac の上り回線は約 7 KB/s で使い物にならず、ホスト間の
    直接転送に切り替えた。
  - 公開元は Hugging Face の 1 リポジトリだけ。
  - モデルの大きさは 1.8 GB(Qwen2.5 1.5B A16)から 38.6 GB(Qwen3.6 35B、2M context)まで。70B 級で
    65.7 GiB、Kimi 級の想定で 300 GiB。
- **既に決まっていること。** misakas の doctrine は次のとおり。
  - 「チェーンは WHAT(class id と artifact root)を固定する。WHERE(HF、mirror、torrent)は誰の問題でも
    あり、誰の選択肢でもある」(ADR-0067 D4)。
  - 「登録と保有は別の行為で、結び付けてはならない」(D6)。
  - 「チェーン側のモデル配布ネットワーク」は却下されている(registry-map §3)。
  - その一方で、ADR-0133 は「chunked P2P 配布」を「後で、content-addressed cache の transport として」
    作ると予告し、ADR-0100 D7 は「local / repository / mirror / peer を引く out-of-band の resolver」を
    予告している。**本 RFC はその transport と resolver を作る。** チェーン側に置くのは、どの規則も読まない
    ポインタだけである。
- **何を作るか。** 「MISAKA Torrent」= BitTorrent v2(BEP 52)の上でモデル artifact を運ぶ仕組み。
  - **`misaka-torrentd`**: libtorrent 2.0.x を内包するサイドカー daemon。持つのはネットワークと自分の
    store だけ。チェーンを知らず、鍵を持たず、何も実行しない。作業名の `misaka-p2p` は misakas の
    node P2P と紛らわしいため使わない。
  - **bundle**: 1 つの (line, version, kind) に対応する、変更されないファイルの集合。1 bundle が 1 swarm に
    なる。種類は 2 つ。
    - `palw-artifact`: PALW コンテナ `.palwart` / `.palwq36` / `.palwtir`、`.palwmanifest`、
      LICENSE、README。
    - `source-weights`: GGUF または safetensors、config、tokenizer。
    量子化や n_ctx が違うモデルは MISAKA では既に別の class か別の version なので、「variant ごとに swarm を
    分ける」は構造上ひとりでに成り立つ。
  - **descriptor `misaka-bundle.json`**: 各ファイルのパス、サイズ、SHA-256、BTv2 の pieces root を並べる。
    PALW コンテナについては、class_id ごとの inventory root も持つ。`bundle_commitment` は keyed
    BLAKE2b-512(H64)で取る。
  - **canonical torrent v1**: v2 専用。piece 長は総サイズから決定的に決まる(1〜16 MiB)。symlink、
    ファイル属性、private フラグは持たない。ファイルを持っている人なら誰でも、同じ infohash を再計算できる。
- **オンチェーン(Part B、misakas への提案)。**
  - オブジェクトは `ModelDistributionDeclared { line_id, version, kind, bundle_commitment, btv2_infohash,
    total_bytes, by, signature }`。line の developer か maintainer が ML-DSA-87 で署名する。
  - (line, version, kind) ごとに 1 行を持つ。行は置き換えられる。rent は 1 MSK。version が履歴から
    外れると一緒に evict される。
  - **どの規則もこの行を読まない、宣言のための行である**(ADR-0088 の `notes_hash` と同じ扱い)。priced
    bytes の外にあり、URL を持たず、どのノードにも何も取得させない。
  - 専用の休眠 fence `palw_model_distribution` を立てる。ADR-0095 の `palw_model_benefits` と同じ形である。
  - misakas 側で別の RFC として採択されて、初めて規範になる。
  - genesis の line には所有者がいないので、誰も宣言できない。そうした line のポインタは、リリースの
    components manifest v2 が持つ。
- **3 段階の検証。**
  - **L1(BitTorrent)**: info dict の SHA-256 が infohash と一致するか。16 KiB ブロックごとに Merkle で
    検証する。
  - **L2(宣言)**: descriptor の H64 が宣言の `bundle_commitment` と一致するか。descriptor と info dict が
    一致するか。安全プロファイルを満たすか。
  - **L3(チェーン)**: PALW コンテナの inventory root を再計算し、オンチェーンの version root と一致するか。
    これはネットワークを持たない別プロセスで行う(2M context で約 135 秒)。
  - 表示ラベルは chain-verified / declared / unanchored の 3 つ。
- **インストール。**
  - 流れは staging → 封印(0444)→ atomic rename → `~/.misaka/<net>/models/` へのハードリンク。
  - ハードリンクなので inode は 1 つで済み、ADR-0136 の「1 ホスト 1 コピー」を守れる。misakas の
    `artifacts_here` も、そのまま見つける。
  - kaspad が mmap しているファイルには決して書き込まない。修復は新しいファイルとして落とし、rename で
    置き換える。
- **安全プロファイル(Safe Model Profile v1)。**
  - 許可するもの: PALW コンテナ、`.palwmanifest`、`.gguf`、`.safetensors`、JSON、テキスト、tokenizer
    ファイル。
  - 拒否するもの: pickle 系(`.bin` `.pt` `.pth` `.ckpt` `.pkl`)、コード、実行ファイル、スクリプト、
    アーカイブ。
  - ファイル名は `[A-Za-z0-9._-]` に限る。symlink、ファイル属性、`..`、Windows の予約名、大文字・小文字
    だけが違う名前の衝突は、engine に渡す前に拒否する。
  - daemon は実行、展開、プラグイン、Web UI、ネットワーク API を一切持たない。
- **セキュリティ境界。**
  - daemon は鍵を持たない。kaspad は鍵をプロセス内に持っているので、server では daemon を別アカウントで
    動かす。
  - サンドボックスは Linux で Landlock + seccomp、macOS で sandbox-exec、server では専用アカウント +
    systemd hardening。
  - IPC は 0700 ディレクトリの中の UDS で、SO_PEERCRED で相手を確かめる。
  - **seat の IP アドレスは swarm から見える。** どの IP がどの class を持っているかが分かり、panel への DoS
    につながりうる。そのため server プロファイルの既定は「取得するだけで seed しない」にする。
- **報酬なし。** seed への支払いはしない。
  - announce の `uploaded` は自己申告で、peer どうしの receipt は Sybil で水増しできる。
  - misakas の ADR の 4 原則にある「チェーンはホストの言葉を信じない」に従う。ADR-0144 の non-goal とも
    整合する。
- **段階。**
  - **A**: このリポに crate 群、daemon、ツール CLI を作る。misakas には `misaka model pull/seed/library/verify`
    を足す。合意の変更はない。
  - **B**: misakas に fence、オブジェクト、RPC、`distribute` を足す。misakas 側の RFC の後で行う。
  - **C**: misakaoptions.com の「Download with MISAKA」ボタン、deep link `misaka-model://`、Studio。
  - **D**: 自動 seed のポリシー、容量管理、web seed、RFC-0006 の層ごとの取得。
- **作業設計からの主な変更**(全対応は付録 A)。
  - Ed25519 の publisher 鍵 → line の developer / maintainer の bond(ML-DSA-87)。チェーンが PQ 専用だから。
  - SHA-256 の manifest hash → H64 の `bundle_commitment`。チェーンのハッシュ規約に合わせる。BitTorrent
    部分の SHA-256 は BEP 52 が決めているので、そのまま使う。
  - BEP 44 → 使わない。Ed25519 専用で、値は 1000 バイトまで、2 時間で失効しうる。チェーンが既に署名付き・
    順序付きのポインタになっている。
  - version 構造体に infohash を足す案 → 別オブジェクトにする。配布は publish の後に来るし、置き換えも
    要る。borsh の行も壊さずに済む。
  - `misaka://` → `misaka-model://`。`misaka:` は既にアドレスの接頭辞である。
  - hybrid v1+v2 → v2 専用。SHA-1 の経路を持たない。
  - misaka ポイント、private tracker、Hub → non-goal、または却下。

## Summary

MISAKA pins a model version on its chain by an inventory root over the model's canonical rows, and
deliberately says nothing about where the bytes live. Today they live on one Hugging Face repository, on
operators' disks, and in hand-run `rsync` between hosts. This RFC builds the missing transport. It is
**BitTorrent v2**, driven by a sidecar daemon (`misaka-torrentd`, libtorrent 2.0.x). It moves **bundles**:
immutable sets of files for one (line, version, kind), described by a canonical descriptor and packed into
a canonical, reproducible v2 torrent.

The transport knows no chain. A resolver in the `misaka` CLI and in MISAKA Studio reads the chain, asks
the daemon for the bytes, verifies them in three levels (BitTorrent's Merkle proofs, the bundle
descriptor, and the PALW inventory root recomputed against the version root) and installs them by hard
link where the node already looks.

Every downloader can seed. The desktop profile seeds by default; the server profile does not, because a
seat's address in a swarm says which classes it holds.

The client is **transport only**:
- a strict allowlist of weight and metadata formats;
- no pickle, code, archives, symlinks or attributes;
- no execution, plugins, keys or network API.

The chain's part is optional and comes second (**Part B**, proposed to misakas). It is a
`ModelDistributionDeclared` object, signed by the line's developer or maintainer, naming a version's
bundle by its descriptor commitment and its v2 infohash. It is a declaration that no rule reads, outside
every priced byte, behind its own dormant fence. It carries no URL, obliges no node to fetch, hold or
seed anything, and changes no validity rule beyond its own admission.

There are no seeding rewards. There is no tracker the network depends on, no DHT search, no BEP 44, no
DRM and no browser BitTorrent. Phases A, C and D change no consensus rule.

## Motivation

### 1. How artifacts move today

- **There is no in-protocol transfer.** misakas ADR-0133's measured table records "artifact transfer —
  none in-protocol; a 24 GB file copied by the operator — out of band"
  (`misakas:docs/adr/0133-verification-is-its-own-clock-a-class-verifies-over-spans-and-a-starved-class-stops-only-itself.md:59`).
  Its option A, a full download after the draw, is "why a 24 GB class never licensed — 41 h of a dead
  panel per claim" (same file, :169).
- **The fleet copies by hand.** testnet-12's deploy kit pushes artifacts with `rsync -a --partial` over
  SSH and checks SHA-256 afterwards (`misakas:contrib/t12-deploy-kit/distribute-from-mac.sh:3,34`). Its
  plan records that the operator's upstream was "about 7 KB/s" and unusable
  (`misakas:contrib/t12-deploy-kit/PLAN.md:403`).
- **There is one public origin.** The hybrid's runtime is published on one Hugging Face repository
  (`misakas:README.md:195`; the 2M-context upload is at
  `misakas:docs/palw-add-a-model-runbook.md:322-323`). The release's components manifest allows exactly
  `https://` and `hf://` sources (`misakas:docs/components-manifest.md:43`), and MISAKA Studio's
  download manager follows it (:78-83).
- **The sizes:**

  | artifact | bytes | source |
  | --- | --- | --- |
  | Qwen2.5-1.5B A16 `.palwart` | 1,795,427,276 | `misakas:docs/components-manifest.md:194` |
  | Qwen3.6-35B-A3B source GGUF (Q4_K_M) | 23,938,321,728 | `misakas:consensus/core/src/palw_qwen36_profile.rs:95` |
  | Qwen3.6-35B-A3B `.palwq36` at 512 | 36,492,831,232 | `misakas:docs/components-manifest.md:208` |
  | Qwen3.6-35B-A3B at a 2M context | 38,639,790,592 | `misakas:docs/palw-add-a-model-runbook.md:105` |
  | a Llama-3-70B-shaped IR model | ≈ 65.7 GiB | `misakas:docs/rfc/0006-palw-layer-sharded-panels.md:581` |
  | a Kimi-class stand-in | 300 GiB | ADR-0133, :77 |

- **The protocol already budgets the fetch, not the transfer.** ADR-0135 derives
  `artifact_prefetch_spans = ⌈io_safety × artifact_bytes / reference_bytes_per_span⌉`
  (`misakas:docs/adr/0135-a-model-is-data-the-permissionless-registry-derives-its-profile-proves-its-panel-and-walks-its-lifecycle.md:60`)
  from a disk reference rate. Network transfer is outside every window the chain keeps. The seat that
  will be drawn must already hold the bytes, and nothing helps it get them.

### 2. What the doctrine already decided

- **ADR-0067 Decision 4** (`misakas:docs/adr/0067-classes-are-chain-data-kernels-are-the-build.md:144-155`):
  "`ClassRegistered` continues to carry no URL. The chain pins WHAT (class id, artifact root); WHERE is
  Hugging Face, a mirror, a torrent — anyone's problem and everyone's option, made safe by the
  bit-reproducible conversion". Also: "making it HAPPEN is the registrant's distribution work".
- **ADR-0067 Decision 6** (same file, :180-201): four storage tiers, and "Registration and possession
  are different acts, and nothing may couple them."
- **The registry map refuses "a chain-side model distribution network"**: "The registration carries
  kilobytes and no URL, precisely so a fleet's disk does not grow with strangers' decisions. Model bytes
  are obtained out of band and admitted only by digest" (`misakas:docs/palw-registry-map.md:66`).
- **ADR-0133** chose "B. content-addressed artifact cache" as the substrate. It deferred "C. chunked P2P
  distribution — a 300 GiB file reaches many seats without one server — a transport to build and secure —
  later, as B's transport" (ADR-0133, :170-171; :152 calls C and F "transports for B").
- **ADR-0100 Decision 7 item 6** lists "an out-of-band content-addressed artifact store and resolver
  (local, repository, mirror, peer), fetching only what an executor chose — registration still carries
  kilobytes and no URL"
  (`misakas:docs/adr/0100-a-model-is-data-and-the-court-the-measure-and-the-licence-are-built-for-a-shard.md:226-229`).
- **ADR-0108 Decision 9**: publication and discovery are off-chain, "and the id is what makes that safe"
  (`misakas:docs/adr/0108-an-extension-is-a-manifest-the-verifier-recomputes-and-a-receipt-is-evidence-not-a-vote.md:286-290`).

So the gap is not a rule. It is the transport C and the resolver of ADR-0100, both promised and never
built. Whatever the chain says about them must be a pointer that no rule reads (§3.3).

### 3. Why BitTorrent v2, and why libtorrent

**What BEP 52 gives.**
- Every file has its own SHA-256 Merkle tree over 16 KiB blocks, and its `pieces root` sits in the
  info dictionary.
- Every non-empty file starts on a piece boundary.
- The infohash is the SHA-256 of the bencoded info dictionary, truncated to 20 bytes where the wire
  needs 20.
- A receiver can therefore verify any 16 KiB block of any file against a root it already trusts, and a
  file is the same bytes in every torrent that contains it.

**What the swarm gives.** Every downloader becomes a source for the pieces it holds, while it is still
downloading. Peers are found without a server through the DHT (BEP 5) and peer exchange (BEP 11), and
a magnet link (BEP 9) is enough to start.

**The engine.**
- **libtorrent-rasterbar 2.0.x** implements BEP 52 and v2-only creation (`create_torrent`'s `v2_only`
  flag). Its piece sizes are powers of two from 16 KiB, and above 128 MiB they are "considered
  unreasonable and will be rejected". It is BSD-3-Clause, compatible with this repository's Apache-2.0
  and misakas's ISC.
- **qBittorrent** is GPL. Forking it would bind every redistribution of the client to the GPL, and its UI
  concepts are a torrent client's, not a model library's.
- **rqbit** is Rust-native. Its README lists BEPs 3, 5, 7, 9, 10, 11, 12, 14, 15, 20, 23, 27, 29, 32,
  47 and 53 — no BEP 52 (checked 2026-10-02). It stays a candidate behind the engine trait (§6.1).

## Goals and non-goals

**Goals.**

- G1. **Many hosts without one server.** A version's bytes reach any number of hosts from any holder.
  Every downloader can serve what it holds.
- G2. **Integrity independent of the source.** Bytes are accepted only if they verify against the
  declaration, and a PALW container only if its recomputed inventory root equals the version root on
  the chain.
- G3. **One click, one command.** A button on misakaoptions.com or one `misaka model pull` reaches a
  verified artifact installed where the node and the CLI already look.
- G4. **Transport only.** The client never executes, imports or renders received bytes. It holds no
  key and exposes no network API.
- G5. **No consensus change to use it.** Phase A works on every network as it is. The on-chain pointer
  (Part B) is a fenced declaration that no rule reads.
- G6. **Registration and possession stay decoupled.** No node is obliged to fetch, hold or seed anything.

**Non-goals.**

- A chain-side distribution network. kaspad never links the engine; the node's P2P never carries
  bundle bytes (PALW-DIST-8).
- Payments, points or rewards for seeding (§Security, "free-riding").
- Discovery inside the DHT: no keyword search, no BEP 44 or BEP 46 feeds. Discovery is the chain and
  misakaoptions.com.
- Running models. No inference, no runtime integration, no "open with".
- Access control. There is no DRM, no private torrents and no passkeys. Every bundle is public.
- Anonymity. Peers see each other's addresses (§Security).
- An on-chain registry for models that are not PALW lines. Unanchored bundles can be shared, labelled
  as such, without the chain.
- Replacing Hugging Face or mirrors. They stay sources: web seeds (§8.4) and `seed --adopt` (§4.5).

## 1. Architecture

### 1.1 Components and trust boundaries

```text
  misakaoptions.com  ── misaka-model://<net>/<line>/<version> ──┐   static site; reads the chain (wRPC)
                                                                ▼
  ┌────────────────────── resolver · verifier · installer ──────────────────────┐
  │  misakas `misaka model …`           MISAKA Studio (`misaka-studiod`)         │
  │  reads the chain · runs L2 and L3 · installs by hard link · never talks to   │
  │  a peer · holds the wallet / bond keys it already holds, and passes none     │
  └───────────┬──────────────────────────────────────────────┬───────────────────┘
              │ gRPC / wRPC, read-only                       │ UDS, misaka-torrent-borsh/v1
              ▼                                              ▼
   kaspad — unchanged but for Part B             misaka-torrentd — this repository
   never links the engine; its P2P never         libtorrent 2.0.x; writes only its store;
   carries bundle bytes                          knows no chain, holds no key, executes nothing
                                                             │ BitTorrent v2: TCP/µTP, DHT, PEX
                                                             ▼
                                   swarm: registrants, seats, users, mirrors as web seeds
```

| component | holds | may | may not |
| --- | --- | --- | --- |
| `misaka-torrentd` | its store, its resume state, sockets | download, seed, verify pieces (L1), refuse by policy | read keys, read outside its store and adopt roots, execute, listen on anything but its peer port and its UDS |
| verifier (`misaka model verify`, Studio's equivalent) | read-only access to a sealed bundle | parse descriptors and container headers; recompute inventory roots (L3) | open a socket, write the bundle |
| resolver / installer (`misaka`, Studio) | what it holds today | read the chain; ask the daemon for bytes; seal and install; sign Part B objects with keys it already holds | hand a key, a seed file or a key path to the daemon |
| kaspad | unchanged | load an installed artifact (`--palw-class-artifact`, digest-checked on load: `misakas:kaspad/src/args.rs:1830-1841`) | fetch, hold or seed anything because of a Part B row |

### 1.2 Two repositories

**This repository** (MISAKA-BTC/misaka-model-transport, Apache-2.0) holds everything that knows no
chain:
- the descriptor and its commitment;
- the canonical torrent rule;
- the policy;
- the IPC protocol;
- the engine binding;
- the daemon;
- a chain-agnostic tool CLI.

It depends on no misakas crate. It reproduces the one misakas primitive it needs, keyed BLAKE2b-512 with
a 64-byte output. A cross-repository golden vector pins that this implementation equals misakas's
`keyed64` (`misakas:consensus/core/src/palw_model_lines_v1.rs:47-55`).

**misakas** reads the chain:
- it resolves `line@version`;
- runs L3 with its own SDK;
- submits Part B objects;
- installs;
- depends on this repository's `misaka-bundle`, `misaka-btv2` and `misaka-transport-ipc` crates.

**MISAKA Studio** gains the deep-link handler and a download source, and supervises the daemon.

### 1.3 Repository layout

```text
misaka-model-transport/
├── Cargo.toml                      workspace; Rust edition 2024
├── crates/
│   ├── misaka-bundle/              descriptor schema, canonical JSON, bundle_commitment (§2.2)
│   ├── misaka-btv2/                bencode, BEP 52 Merkle, canonical torrent build/parse (§2.3);
│   │                               no engine — the infohash is computed in pure Rust
│   ├── misaka-transport-policy/    the Safe Model Profile, names, limits (§5); pure, fuzzed
│   ├── misaka-transport-ipc/       misaka-torrent-borsh/v1 messages and framing (§6.2)
│   └── misaka-transport-engine/    the ModelTransport trait, a fake in-memory engine for tests,
│                                   the libtorrent backend behind the `libtorrent` feature (§6.1)
├── shim/libtorrent/                a C ABI over a pinned libtorrent-rasterbar 2.0.x
├── bin/
│   ├── misaka-torrentd/            the daemon (§6)
│   └── misaka-torrent/             the tool CLI (§7.2)
├── deploy/{systemd,launchd}/       units and confinement profiles (§6.5)
├── fuzz/                           descriptor, bencode, info dict, policy
└── docs/rfc/                       this text
```

The libtorrent backend is not in the default build, as misakas keeps the worker that links its pinned
llama.cpp out of `default-members` (`misakas:Cargo.toml:108-121`). Every crate but the backend and the
daemon builds and tests with no C++ toolchain.

## 2. Bundles

### 2.1 What a bundle is

A **bundle** is an immutable set of files for one (line, version, kind). One bundle is one torrent and
one swarm. A new version is a new bundle; a bundle is never edited, only replaced (§3.2).

| kind | id | holds | anchored by |
| --- | --- | --- | --- |
| `palw-artifact` | 0 | exactly one PALW container (`.palwart`, magic `PALWB0A2` or `PALWB0A1`; `.palwq36`, `PALWQ361`; `.palwtir`, `PALWTIR1`), its optional `.palwmanifest` sidecar, `LICENSE`, `README.md` | the version root on the chain (L3) |
| `source-weights` | 1 | the public weights a container is converted from: one `.gguf`, or `.safetensors` shards with `model.safetensors.index.json`; `config.json`, `generation_config.json`, the tokenizer files; `LICENSE`, `README.md` | the declaration (L2); optionally a reproducible conversion (L3′) |
| `adapter` | 2 | reserved: misakas RFC-0004's unmerged adapters | — |
| `shard` | 3 | reserved: a layer range for misakas RFC-0006's seats | — |

The magics are misakas's own:
- `misakas:misaka-palw-base0/src/artifact.rs:967,970`;
- `misakas:misaka-palw-base0/src/qwen36.rs:1510`;
- `misakas:misaka-palw-tir-artifact/src/lib.rs:36`.

`PALWK3V1` (`misakas:consensus/core/src/palw_kimi_k3_artifact_v1.rs:14`) joins the list when that
container gets a file extension.

**Variants need no new concept.** A different quantization map or context length is already a different
class or a different version root (`misakas:docs/palw-registry-map.md:29`). "One swarm per variant", which
the working design asked for, is what one swarm per (line, version, kind) gives by construction.

**The source bundle is not optional for every line.** The hybrid's `.palwq36` "deliberately carries no
tokenizer". Its FP worker reads the tokenizer from the GGUF header
(`misakas:misaka-palw-base0/src/bin/palw-qwen36-fp-worker.rs:18-21`). A producer on that line needs both
bundles.

### 2.2 The descriptor

Every bundle contains `misaka-bundle.json` at its root.

```jsonc
{
  "files": [
    {
      "btv2_pieces_root": "<64 hex: BEP 52 Merkle root of the file's 16 KiB blocks>",
      "path": "LICENSE",
      "role": "license",
      "sha256": "<64 hex: SHA-256 of the file's own bytes>",
      "size": 11357
    },
    {
      "btv2_pieces_root": "<64 hex>",
      "palw": {
        "artifact_digest": "<128 hex: the container's own digest (PalwArtifactDigestV1), when it defines one>",
        "magic": "PALWB0A2",
        "roots": [
          { "class_id": "4277d84f…f35f1b7", "inventory_root": "1a7457f1…17b72708" }
        ]
      },
      "path": "qwen25-1.5b-a16.palwart",
      "role": "palw-container",
      "sha256": "a8c4e53e5b30dd0d4dc6ef791e0513890a07a2b3a22d045e612536bba1240b1f",
      "size": 1795427276
    }
  ],
  "kind": "palw-artifact",
  "license": { "file": "LICENSE", "spdx": "Apache-2.0" },
  "schema": "misaka/torrent-bundle/v1",
  "title": "qwen25-1.5b-a16-graph-v5-512"
}
```

(Values from the testnet-11 components manifest, `misakas:docs/components-manifest.md:184-197`;
"`…`" elides hex in this text only.)

- **Canonical form.** UTF-8. Keys sorted, two-space indent, one trailing newline, and `files` sorted by
  `path`. This is the same canonical form as misakas's components manifest
  (`misakas:docs/components-manifest.md:31-33`).
- **Unknown keys are refused at every level.** A new key is a new schema string.
- **The commitment:**

  ```text
  bundle_commitment = BLAKE2b-512(key = "misaka-torrent/bundle/v1", msg = u64_le(len) ‖ bytes)
  ```

  It is computed over the exact bytes, so a reader never re-serializes before hashing.
- **The descriptor does not list itself.** The torrent's file set is exactly
  `{misaka-bundle.json} ∪ files[].path`.
- **`roots` are the registry's values, copied.** Each one is a `PalwInventoryRootV1` at a named class
  (`misakas:consensus/core/src/palw_class_identity_v1.rs:41-48`). `artifact_digest` is the container's
  `PalwArtifactDigestV1` (:34-39).
  - A reader MUST NOT treat a digest as a root. That substitution caused two outages, and misakas made
    them distinct types because of it (:14-25).
  - The version's root must appear in `roots` at the version's class; L3 checks it by recomputation, not
    by reading this field.
- **`sha256` is the same value a components-manifest row carries** "of the component's OWN bytes"
  (`misakas:docs/components-manifest.md:44`). It is not a second spelling: it binds a bundle to a
  release's row (§3.5), and it lets a plain HTTPS download be checked by Studio's existing download
  manager.
- **The descriptor carries no URL.** Where bytes may also be found is client configuration (§8.4).
- `title` is `[a-z0-9][a-z0-9._-]{0,63}`. `license.spdx` is an SPDX identifier, or `LicenseRef-…` with the
  text in `license.file`.

### 2.3 The canonical torrent (rule v1)

A bundle's torrent is a function of its files. Two honest packagers holding the same files produce the
same info dictionary, byte for byte, and so the same infohash.

1. **`meta version` is 2.** The torrent is v2-only (libtorrent's `v2_only`): there are no v1 `pieces`
   and no padding files.
2. **`name` is the descriptor's `title`.**
3. **`piece length` is `P(T)`**, where `T` is the sum of the bundle's file sizes:

   ```text
   P(T) = clamp( 2^⌈log2 ⌈T / 4096⌉⌉ , 1 MiB , 16 MiB )
   ```

   The target is about 4,096 pieces. A file's `pieces root` does not depend on `P`: the piece layer is
   a layer of the same 16 KiB tree.
4. **`file tree`** holds the bundle's files at depth 1, with no subdirectories. Every file is non-empty.
   There are no `attr` or `symlink path` keys.
5. **The info dictionary holds no keys but these four.** In particular there is no `private`, `source`
   or `similar`.
6. **Outside `info`:**
   - `piece layers` is present, as BEP 52 requires, for every file longer than `P`;
   - `announce`, `announce-list` and `url-list` (BEP 19 web seeds) are optional, chosen by whoever
     writes the `.torrent`, and never part of the identity;
   - `creation date`, `created by` and `comment` are omitted.
7. **`btv2_infohash = SHA-256(bencode(info))`.**

| bundle | T (bytes) | P | pieces of the large file | its piece layer |
| --- | --- | --- | --- | --- |
| Qwen2.5-1.5B A16 `.palwart` | 1,795,427,276 | 1 MiB | 1,713 | 54,816 B |
| Qwen3.6 source GGUF | 23,938,321,728 | 8 MiB | 2,854 | 91,328 B |
| Qwen3.6 `.palwq36` at 512 | 36,492,831,232 | 16 MiB | 2,176 | 69,632 B |
| Qwen3.6 at a 2M context | 38,639,790,592 | 16 MiB | 2,304 | 73,728 B |
| 70B-shaped IR model | ≈ 70.5 × 10⁹ | 16 MiB | ≈ 4,205 | ≈ 134,560 B |
| Kimi-class stand-in | 322,122,547,200 | 16 MiB | 19,200 | 614,400 B |

The rule is ours, not libtorrent's `piece_size = 0`. A library's automatic choice can change between
releases, and a declared infohash must be reproducible from the files alone by any packager, today and
later. A different rule is a new descriptor schema; an existing declaration keeps its infohash.

### 2.4 Links

- **magnet:** `magnet:?xt=urn:btmh:1220<64 hex infohash>&dn=<title>` (BEP 9). `1220` is the multihash
  prefix for a 32-byte SHA-256.
  - Any v2-capable client can fetch and seed with it, and more seeders are welcome. Integrity is the
    protocol's, so it does not depend on which client is used.
  - A bare magnet is **unanchored**: the client knows the bytes are the torrent's, nothing more.
- **`misaka-model://<network>/<line_id>/<version>?kind=<kind>`** is the chain-anchored link (§8.2).

## 3. Part B — declaring a distribution on the chain (proposed to misakas)

This part is what the transport asks of the chain. It is written here so this repository can build
against it. It becomes normative only through a misakas RFC that adopts it and cites this one, and
nothing in Phases A, C or D depends on it.

### 3.1 The object and the row

```rust
// misakas consensus/core/src/palw_state_v2.rs — appended LAST to PalwConsensusObjectV2.
ModelDistributionDeclared {
    line_id: Hash64,
    version: u32,
    /// 0 = palw-artifact, 1 = source-weights (§2.1); any other value is refused.
    kind: u8,
    /// BLAKE2b-512 keyed "misaka-torrent/bundle/v1" over the descriptor's bytes (§2.2).
    bundle_commitment: Hash64,
    /// SHA-256 of the bencoded v2 info dictionary (BEP 52). An external identifier: no rule
    /// compares it with any chain hash.
    btv2_infohash: [u8; 32],
    /// The bundle's size, its descriptor included.
    total_bytes: u64,
    /// The line's developer or maintainer bond.
    by: PalwBondKeyV2,
    /// ML-DSA-87, context b"misaka-palw-model-distribution-v1".
    signature: Vec<u8>,
},

// misakas consensus/core/src/palw_model_lines_v1.rs — one row per (line_id, version, kind).
pub struct PalwModelDistributionV1 {
    pub bundle_commitment: Hash64,
    pub btv2_infohash: [u8; 32],
    pub total_bytes: u64,
    pub declared_daa: u64,
    pub declared_by: PalwBondKeyV2,
    /// Declarations this row replaced, saturating.
    pub replaced: u16,
}

// The signed message, in the style of palw_model_version_message_v1 (palw_model_lines_v1.rs:319-351).
pub fn palw_model_distribution_message_v1(
    network_domain: Hash64, line_id: &Hash64, version: u32, kind: u8, bundle_commitment: &Hash64,
    btv2_infohash: &[u8; 32], total_bytes: u64, by: &PalwBondKeyV2,
) -> Hash64 {
    let mut s = begin(b"misaka-palw/model-distribution/declared/v1", network_domain, line_id);
    s.update(&version.to_le_bytes());
    s.update(&[kind]);
    s.update(bundle_commitment.as_byte_slice());
    s.update(btv2_infohash);
    s.update(&total_bytes.to_le_bytes());
    s.update(&borsh::to_vec(by).expect("a bond key is borsh-serializable"));
    finish(s)
}
```

The message carries `by`, so a developer's signature cannot be replayed as the maintainer's. It carries
the network domain, as every line message does.

### 3.2 Admission

Past the fence, a `ModelDistributionDeclared` is accepted when:

1. **The line exists and is `Active`.** Past `palw_model_lines` a founding line exists by synthesis
   (`founding_line_v1`, `misakas:consensus/core/src/palw_model_lines_v1.rs:123-141`).
2. **The version is valid.** It is inside the kept history (`PALW_MODEL_VERSION_HISTORY_V1 = 64`, :33)
   and not `Withdrawn` (:146-156).
3. **`kind` is 0 or 1.**
4. **The signer is the line's own word.** `by` is the line's `developer_bond()` or `maintainer_bond()`
   (:109-117), the bond is Active, and the signature verifies.
5. **`1 ≤ total_bytes ≤ 2^42`** (4 TiB).
6. **It pays rent.** It burns `PALW_MODEL_OBJECT_RENT_SOMPI_V1` (1 MSK, :42-43), as a founding, a
   proposal and an evaluation do.
7. **It is not a duplicate.** A declaration identical to the live row (same commitment, same infohash,
   same size) is refused as a no-op.

The fold then writes the row for (line_id, version, kind), replacing any live row and incrementing
`replaced`. A version's rows leave state when the version leaves the kept history.

**No rule reads a distribution row.** No admission, court, draw, readiness, licence, fee or reward rule
reads it, and no field of it enters a priced, committed or certified byte. It is a declaration in the
exact sense of the version row's "Recorded, labelled, never read by a rule" (:199-203). Constants are
code constants, not bundle fields (:19-21).

### 3.3 Why this is not a chain-side distribution network

The refusal (`misakas:docs/palw-registry-map.md:66`) is of a network that makes a fleet's disk grow with
strangers' decisions. The registry map's own checklist for adding a field (:69-81) is answered rule by
rule:

| rule | answer |
| --- | --- |
| 1. find the row that already carries the meaning | The meaning "which bytes" is the version root, and it stays the only identity: nothing here re-spells it, and L3 checks bytes against it, not against the row. The row carries a transport handle, a meaning no row carries today (the components manifest is off-chain and release-scoped; `notes_hash` is release notes) |
| 2. priced or committed bytes must be pinned by an equation | It is outside all of them. "A free field is a free draw" (`misakas:docs/adr/README.md:381-382`) concerns values an accused party chooses inside what prices a lottery; nobody is accused by this row and it prices nothing |
| 3. a change to what the state root hashes rides a fence | It writes a new collection into the state root, so it gets its own fence (§10), exactly as ADR-0095's declarations did (`misakas:consensus/core/src/config/params.rs:2405-2412`) |
| 4. host matters belong to the host | The daemon's posture (confinement, listeners) is reported by `misaka node security-report` and committed nowhere (§6.6) |

And against the decisions it must not reopen:

- **ADR-0067 D4, "carries no URL".** The row names bytes by content: an infohash and a commitment.
  There is no host, path, domain or tracker in it, and the descriptor carries none either (§2.2).
- **ADR-0067 D6, possession.** No node fetches, holds, seeds or checks anything because of a row. kaspad
  does not link the engine, and the node's P2P never carries bundle bytes (PALW-DIST-8).
- **"The chain never takes the host's word"** (`misakas:docs/adr/README.md:388-390`). The row is the
  line's word, labelled as such. Every consumer re-checks the bytes (L1–L3). A wrong row costs its
  signer the rent and its name, never anyone's bond and never a block's validity.
- **ADR-0143, one owner per root.** The row is keyed by (line, version): it is the owning line speaking
  about its own root.

### 3.4 Why a separate object, not a field on `ModelVersionPublished`

- **Timing.** A bundle exists after packaging and a first seed. Publication should not wait for it, and
  a source bundle may come later still.
- **Correction.** A packaging mistake (a missing `LICENSE`, a wrong title) is fixed by redeclaring. A
  field on the version could only be fixed by a new version, and a new version is a new root.
- **Layout.** `PalwModelVersionV1` is a borsh row in live state (:192-208). A separate map is additive.
- **Activation.** The version object is under `palw_model_lines`, which testnet-11 and testnet-12 have
  armed. Changing it would move a live state root; a new collection behind its own fence moves nothing
  until armed.

### 3.5 Unowned lines

A genesis class's founding line has no owner: "unowned, one version, nobody publishes"
(`misakas:consensus/core/src/palw_model_lines_v1.rs:87`). Nobody can speak for it, so it gets no row.

Its pointer is release-scoped instead. A components-manifest v2 artifact row carries a `torrent` object
(§8.4), published by the same release that already carries that artifact's `sha256` and `artifact_root`.
L3 still binds the bytes to the chain's root. Whether unowned lines should accept declarations from any
bond is Open question 2.

### 3.6 RPC

`getPalwModelDistribution { lineId, version }` is served on gRPC and wRPC like the other `getPalwModel*`
methods (`misakas:web/misaka-options/README.md:229-235`). The response starts with its `u16` version, as
misakas RPC messages do:

```text
{ lineId, version, root, versionStatus, inForce,
  declarations: [ { kind, bundleCommitment, btv2Infohash, totalBytes, declaredDaa, declaredBy, replaced } ] }
```

The magnet is not a chain fact; a client composes it (§2.4).

### 3.7 Where it lands in misakas

| what | where | precedent |
| --- | --- | --- |
| the variant, appended **last** | `PalwConsensusObjectV2`, after the registry objects at `consensus/core/src/palw_state_v2.rs:6776-6890` | the tag is pinned by `consensus_object_discriminants_are_the_ones_the_chain_carries`. A mid-enum insert once stopped every fresh node at DAA 1,945 (`consensus/core/src/palw_lifecycle_objects_v2.rs:1495-1501`) |
| carriage | the signed-registry-object arm of `palw_lifecycle_object_may_ride_v2` (`palw_lifecycle_objects_v2.rs:99`, :206-218) | may-ride is stateless; the fence is checked at acceptance (:138-140) |
| rent | the rent list, `palw_state_v2.rs:7813` | founding, proposal, evaluation |
| the fold | beside the `apply_model_*` arms, `palw_state_v2.rs:31255-31310` | `ModelLineBenefitsDeclared` (:6884-6890, :31289-31291) |
| the message and context | `palw_model_lines_v1.rs`, beside `palw_model_version_message_v1` (:319-351) and `PALW_MODEL_VERSION_MLDSA87_CONTEXT` (:260) | — |
| the fence | `Params::palw_model_distribution` and `palw_model_distribution_fence()` with the lines dependency folded in | `palw_model_benefits_fence()` (`consensus/core/src/config/params.rs:7703-7716`) |

## 4. Fetch, verify, install

### 4.1 Resolution

| input | resolves through | anchor | label |
| --- | --- | --- | --- |
| `<line_id>[@<version>]`, or a class id (its founding line) | the node: `getPalwModelLine` → `getPalwModelVersion` → `getPalwModelDistribution` | the Part B row | chain-verified (`palw-artifact`), declared (`source-weights`) |
| `misaka-model://…` | the same, after a confirmation (§8.2) | the Part B row | the same |
| a components-manifest v2 row | the release's manifest | the row's `sha256` and `artifact_root` | chain-verified |
| `magnet:?xt=urn:btmh:…`, optionally with `--expect <bundle_commitment>` | nothing | none, or the expected commitment | unanchored |

The chain is read through the operator's node (`--rpc`, `MISAKA_RPC`, or the `endpoints.json` hint). A
public endpoint can lie about the chain. L3 then binds the bytes to whatever root that endpoint served,
so a seat SHOULD resolve through its own node.

### 4.2 States

```text
Resolving → Metadata → Admitted → Downloading → Verifying → Sealed → Installed → Seeding | Idle
                └─ Refused(policy)        └─ Stalled           └─ Mismatch(L2 | L3)
```

- **Metadata** is the info dictionary, fetched by the infohash (BEP 9) and checked against it.
- **Admitted** means the dictionary passed the policy (§5) and the declaration's `total_bytes`. No file
  exists before this state.
- **Downloading** already serves the verified pieces to other peers. That is BitTorrent's ordinary
  behaviour and needs no feature.

### 4.3 Verification levels

| level | checks | where | cost |
| --- | --- | --- | --- |
| **L1 transport** | `SHA-256(info) = infohash`; every 16 KiB block against its file's `pieces root` (BEP 52); peers that send bad data are banned | the daemon, during download | streaming |
| **L2 declaration** | `bundle_commitment` of `misaka-bundle.json` equals the anchor's; descriptor ↔ info dictionary (paths, sizes, pieces roots) exactly equal; `total_bytes` equal; each file's leading magic matches the descriptor; the Safe Model Profile | the verifier, read-only, no network | seconds |
| **L3 chain** | for the PALW container: the inventory root recomputed at the version's class equals the version root | the verifier, via misakas's SDK, read-only, no network | about 135 s at a 2M context (`misakas:misaka-palw-sdk/src/class_manifest.rs:19`) |
| L3′ reproduction (opt-in) | a `source-weights` bundle converted by the bit-reproducible converter lands on the version root | the operator's converter | the conversion |

A `palw-artifact` bundle is installed only after L3. A mismatch deletes the staging copy and reports
which level failed, naming the declaration. It is never retried silently against the same declaration.
kaspad would refuse the same bytes at load anyway ("Digest-checked on load",
`misakas:kaspad/src/args.rs:1838`). L3 exists so that the operator learns it before the draw, not after.

### 4.4 Staging, sealing, installing

```text
desktop: ~/.misaka/torrent/{incomplete,store,state}/     server: /var/lib/misaka-torrent/{incomplete,store,state}/
         $XDG_RUNTIME_DIR/misaka-torrent/torrentd.sock (0700)        /run/misaka-torrent/torrentd.sock
```

1. **Download** into `incomplete/<infohash>/<title>/`.
2. **Seal** after L2. Files become `0444` and directories `0555`. Then the bundle moves to
   `store/<infohash>/<title>/` by one atomic rename on the same filesystem.
3. **Install** after L3, for an anchored PALW container: a **hard link** at `~/.misaka/<network>/models/<file>`.
   - That is a directory misakas's `artifacts_here` already scans, non-recursively
     (`misakas:misaka-cli/src/operator/wizard.rs:543-571`).
   - A hard link is one inode, so the host holds one copy (ADR-0136).
   - A symbolic link is the fallback across filesystems. A copy is never made.
   - On a server, kaspad's `--palw-class-artifact` names the store path directly.
4. **Never write into a sealed file.** Workers map artifacts `MAP_PRIVATE` read-only
   (`misakas:docs/adr/0136-an-artifact-is-mapped-not-read-and-a-host-holds-one-copy-of-it.md:41`), and a
   write under a live mapping is a hazard the mapping cannot see.
   - A sealed file that fails a recheck stops seeding and is reported.
   - Repair downloads a fresh copy into `incomplete/` and renames it over the old one. Existing mappings
     keep the old inode until they are dropped.

### 4.5 Adopting a file you already hold

`misaka-torrent seed --adopt <dir>` serves a bundle whose files were obtained elsewhere: Hugging Face, a
mirror, a converter. It builds the canonical torrent from the local files. It seeds in place, read-only,
with no copy, if the result equals the declared infohash (or the one given with `--expect`).

The directory must be under one of the configured `adopt_roots`; the daemon can read nothing else.
Adoption is how a host that converted locally joins the same swarm. ADR-0067 D4 rests on exactly this:
"two honest converters land on the same root".

### 4.6 Seeding policy

| setting | desktop profile | server profile |
| --- | --- | --- |
| seed after download | on | **off** |
| upload cap | 20 MiB/s | operator-set |
| on battery / metered network | pause | — |
| peer connections | 200 | 100 |
| UPnP / NAT-PMP | on | off |
| LSD (BEP 14) | off | off (on for a fleet LAN by choice) |
| version `Withdrawn` | stop seeding, keep the files | same |
| version `Superseded` | keep seeding | same |
| free space before admission | `total_bytes × 1.05 + 1 GiB` | same |
| store quota | 50 % of the volume at first start | operator-set |
| `incomplete/` garbage collection | after 14 idle days | same |

### 4.7 Partial and layer-range fetch (Phase D)

- **Whole-file selection** is the engine's file priority, for a reader who wants one file of a source
  bundle.
- **Layer ranges** for misakas RFC-0006's seats are piece priorities, computed from the container's
  header. The header is at the file's start: a TIR header is at most 64 MiB
  (`misakas:misaka-palw-tir-artifact/src/lib.rs:42`), so its first pieces are fetched first.
  - The layout is param-major, so a layer range is many byte ranges, not one.
  - Readiness proofs that draw leaves from the whole inventory are RFC-0006's question, not this
    transport's.
- **Identical files across bundles** have equal pieces roots and are hard-linked, not stored twice.

## 5. Transport only — the Safe Model Profile v1

The profile is an allowlist. It is applied to the info dictionary **before** the engine sees it, and
again to the bytes after download (L2).

### 5.1 Files

| class | names | before admission | at L2 |
| --- | --- | --- | --- |
| PALW container | `*.palwart`, `*.palwq36`, `*.palwtir` | kind `palw-artifact`; exactly one | the first 8 bytes are `PALWB0A2`, `PALWB0A1`, `PALWQ361` or `PALWTIR1`, and equal the descriptor's `magic` |
| PALW sidecar | `*.palwmanifest` | ≤ 16 MiB | UTF-8 JSON |
| weights | `*.gguf`, `*.safetensors` | kind `source-weights` | GGUF magic; a safetensors header is an 8-byte length ≤ 100 MiB followed by JSON. Read with bounds; nothing evaluated |
| metadata | `misaka-bundle.json`, `config.json`, `generation_config.json`, `model.safetensors.index.json`, `tokenizer.json`, `tokenizer_config.json`, `special_tokens_map.json`, `vocab.json` | ≤ 64 MiB | UTF-8 JSON |
| tokenizer | `tokenizer.model`, `*.tiktoken`, `merges.txt` | ≤ 64 MiB | — |
| text | `LICENSE`, `LICENSE.txt`, `NOTICE`, `README.md` | ≤ 1 MiB | UTF-8 |
| anything else | **refused** | | |

Refused by name, so that a reader does not have to infer it:
- the pickle family: `.bin`, `.pt`, `.pth`, `.ckpt`, `.pkl`, `.pickle`, `.joblib`, `.npy`, `.npz`;
- other formats that execute on load: `.onnx` custom-op carriers, `.h5` / `.keras` (lambda layers);
- code: `.py`, `.pyc`, `.ipynb`, `.js`, `.sh`, `.bat`, `.cmd`, `.ps1`;
- binaries: `.exe`, `.dll`, `.so`, `.dylib`;
- archives: `.zip`, `.tar`, `.gz`, `.tgz`, `.7z`, `.rar`, `.zst`.

Hugging Face's own guidance is that loading a pickle can run code, and safetensors exists to avoid it.

### 5.2 Names and paths

- **Depth 1.** A path is one element.
- **Allowed bytes.** `[A-Za-z0-9._-]`, 1–128 bytes long, not starting with `.` or `-`, not ending with
  `.`, and containing no `..`.
- **Case-folded uniqueness.** Two names that differ only in case are refused, so a bundle cannot clobber
  itself on macOS or Windows.
- **Windows device names** (`CON`, `PRN`, `AUX`, `NUL`, `COM0`–`COM9`, `LPT0`–`LPT9`, with any
  extension) are refused.
- **No attributes.** No BEP 47 `attr` at all (`p`, `x`, `h`, `l`), no `symlink path`, and no empty file.
- **No following links.** The daemon creates files with no-follow, exclusive opens where the platform
  has them. It never follows a link inside its store; the store's directories are its own and `0700`.

### 5.3 Limits

- At most 64 files.
- The info dictionary is ≤ 1 MiB, which is ≤ 64 metadata blocks of 16 KiB (BEP 9).
- `piece layers` total ≤ 1 MiB, and the descriptor is ≤ 1 MiB.
- `T` ≤ 4 TiB, ≤ the store quota, and ≤ the free space (§4.6).
- The declared `total_bytes` must equal `T` before a byte is downloaded.

### 5.4 What the client never does

- It never executes, imports, unpickles, `torch.load`s, or renders received bytes. A chat template in
  `tokenizer_config.json` is Jinja and is never rendered.
- It never extracts an archive.
- It never follows a URL found in a file, opens a browser or a socket on the strength of received data,
  or spawns a process because of it.
- It never loads plugins, starts inference, or installs software.
- It never serves HTTP.
- Peers can request pieces, metadata and hashes (BEP 3, 9, 52) and exchange peers (BEP 11). There is no
  custom extension message and no remote command of any kind. The extension handshake (BEP 10)
  advertises standard extensions only.

### 5.5 What the profile does not promise

**Weights can be malicious in ways no format check sees.** A backdoored behaviour, or a model trained
to follow injected instructions, passes every check here.

**Parsers have bugs.** A GGUF or safetensors parser bug in a downstream runtime is that runtime's attack
surface. The profile shrinks that surface (no code formats). Provenance labels it (chain-verified,
declared, unanchored). Neither removes it.

PALW's own consumption is protected separately: the node admits bytes only by digest
(`misakas:docs/palw-registry-map.md:30-31`).

## 6. `misaka-torrentd`

### 6.1 Engine

- **libtorrent-rasterbar 2.0.x**, pinned by version and source digest in `third_party.toml`. It is built
  outside the default build (§1.3).
- **A C ABI shim** (`shim/libtorrent/`) of about twenty functions:
  - session create and destroy;
  - add a torrent from info-dictionary bytes plus resume data;
  - file and piece priorities;
  - pause, resume, remove;
  - status, pop alerts, apply settings.
  No C++ type crosses it.
- **The Rust side** implements:

```rust
pub trait ModelTransport {
    fn add(&mut self, bundle: AdmittedBundle, mode: AddMode) -> Result<BundleHandle, EngineError>;
    fn pause(&mut self, h: BundleHandle) -> Result<(), EngineError>;
    fn resume(&mut self, h: BundleHandle) -> Result<(), EngineError>;
    fn remove(&mut self, h: BundleHandle, files: RemoveFiles) -> Result<(), EngineError>;
    fn set_limits(&mut self, limits: Limits) -> Result<(), EngineError>;
    fn status(&self, h: BundleHandle) -> Result<BundleStatus, EngineError>;
    fn poll_events(&mut self, max: usize) -> Vec<EngineEvent>;
}
```

`AddMode` is `Fetch`, `Seed` (a sealed bundle) or `Adopt` (§4.5). This covers the working design's
`publish`, `download`, `seed` and `remove`.

`AdmittedBundle` can only be constructed by `misaka-transport-policy`, because its constructor is
private to that crate. Nothing reaches the engine without passing the profile, and the type system says
so.

- **Engine settings are fixed by the daemon, not by IPC callers.** The DHT is used for peers only: no
  BEP 44 puts or gets, no BEP 46.

### 6.2 IPC

- **Protocol** `misaka-torrent-borsh/v1`: a `u32` little-endian length (≤ 1 MiB), then a borsh message.
  misakas's remote signer (ADR-0015) and `palw-agent` (`misaka-palw-agent-borsh/v1`,
  `misakas:misaka-palw-agent/src/agent.rs:4-5`) already speak framed borsh over a socket; this
  protocol fixes its own prefix.
- **Transport.** A Unix domain socket in a `0700` directory, with the peer checked by `SO_PEERCRED` or
  `getpeereid`: the same uid on a desktop, and on a server the operator's uid plus a read-only group.
  On Windows, a named pipe with an owner-only DACL.
- **Requests:**
  - `Hello`;
  - `Fetch { infohash, expect { bundle_commitment?, total_bytes?, kind? }, seed_after }`;
  - `Seed { infohash }`;
  - `Adopt { infohash, adopt_root_id, rel_dir }`;
  - `Pause`, `Resume`, `Remove { infohash, delete_files }`;
  - `Status { infohash? }`, `SetLimits { … }`, `Subscribe`.
- **Responses** never carry file contents.
- **No request names an absolute path.** Writes go to the store; reads come from the store and from the
  configured adopt roots, by id.

### 6.3 Network defaults

- **Port.** One listen port, chosen at random in 49152–65535 at first start and persisted. That avoids
  misakas's node ports:
  - P2P 26111 / 26311 etc. (`misakas:consensus/core/src/network.rs:267-291`);
  - RPC 26110–28610;
  - 8545, 8787–8791, 3030 and 11434.
- **Transports.** TCP and µTP over IPv4 and IPv6. The DHT and PEX are on.
- **Discovery per profile.** LSD and UPnP/NAT-PMP are set by profile (§4.6). There are no default
  trackers, and the DHT bootstrap list is configurable (Open question 11).
- **Encryption** is libtorrent's "enabled": obfuscation against throttling, not confidentiality. The
  content is public anyway.
- **No identity.** The peer id, the extension handshake and every announce carry no MISAKA identity: no
  address, no bond and no node id.

### 6.4 Configuration

- **The file** is `~/.misaka/torrent.toml` on a desktop and `/etc/misaka/torrent.toml` on a server. It
  is parsed with unknown fields refused.
- **It is its own file**, because misakas parses `~/.misaka/config.toml` with `deny_unknown_fields`. A
  new section there would break every older `misaka` on the host
  (`misakas:misaka-cli/src/operator/profile.rs:8-10`, ADR-0122 §14).
- **Environment variables** `MISAKA_TORRENT_*` set paths and the profile only.
- **Precedence** is the CLI, then the environment, then the file, then the default.

### 6.5 Confinement

**Desktop, Linux.**
- **Landlock.** Read and write only the store, the state directory and the runtime directory. Read the
  configuration and the adopt roots. Nothing else.
- **seccomp.** A filter that denies `execve`, `execveat`, `ptrace`, `process_vm_readv` /
  `process_vm_writev`, `mount`, `bpf`, `perf_event_open`, module loading, `kexec_*`, `keyctl` and
  `userfaultfd`.
- **The precedent is misakas's confinement backends** (`misaka-palw/src/host_security.rs`). The worker's
  filter denies `socket` and `connect`, which the daemon needs, so the daemon carries its own profile.

**Desktop, macOS and Windows.** On macOS, a `sandbox-exec` profile denies by default and allows the
network and the store. Windows confinement (an AppContainer or a restricted token) is decided with Open
question 8, before Windows ships.

**Server.**
- **A dedicated account**, `misaka_torrent`, created as misakas's setup creates `misaka_user`
  (`useradd --system … --shell /usr/sbin/nologin`, `misakas:misaka-cli/src/setup/mod.rs:1032`).
- **Never kaspad's account.** kaspad holds hot keys in-process (`--palw-producer-key`, `--validator-key`).
- **kaspad reads the store through a group, never writes it.** The store is group-readable (`misaka_models`),
  and kaspad's user is in that group.
- **A starting unit**, adapted from misakas's OTA fetcher sandbox
  (`misakas:docs/misaka-palw-ota-secure-update-design-v0.1-ja.md:652-664`). Drill D-T9 confirms it:

```ini
[Service]
User=misaka_torrent
Group=misaka_models
ExecStart=/usr/local/bin/misaka-torrentd run --profile server
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictNamespaces=yes
LockPersonality=yes
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK
SystemCallFilter=@system-service
SystemCallFilter=~@privileged
CapabilityBoundingSet=
ReadWritePaths=/var/lib/misaka-torrent /run/misaka-torrent
UMask=0027
MemoryMax=2G
CPUWeight=50
IOWeight=50
```

### 6.6 Observability

- **`misaka-torrent status [--json]`** reports, per bundle: the state, its peers and seeds, rates, bytes
  up and down, and the anchor label.
- **Metrics leave only through the IPC `Status`.** There is no metrics port.
- **misakas's `misaka node security-report`** gains a transport section: the daemon's listener, the
  confinement in force, the store path and the bundles seeding. It is printed, signed by nobody, and
  committed nowhere, as the rest of that report is (ADR-0079 Decision 2).

## 7. Command lines

### 7.1 misakas `misaka model …`

| command | does |
| --- | --- |
| `misaka model pull <line_id>[@<version>] [--kind palw-artifact\|source-weights] [--no-seed] [--max-up <MiB/s>]` | resolve, fetch, L2, L3, install; seed per profile |
| `misaka model pull 'misaka-model://…'` | the same after a confirmation |
| `misaka model pull 'magnet:…' --unanchored [--expect <commitment>]` | unanchored fetch and L2 (with `--expect`); never auto-installed as a class artifact |
| `misaka model seed list\|start\|stop <bundle>` and `misaka model seed adopt <dir> --expect <infohash>` | seeding control (§4.5–§4.6) |
| `misaka model library` | installed bundles: label, line and version, size, up/down bytes, state |
| `misaka model verify <bundle\|path>` | run L2 and L3 again, read-only |
| `misaka model distribute <line_id>@<version> --kind <kind> --dir <bundle-dir> [--dry-run] [--yes]` | write the descriptor, build the canonical torrent, print the commitment and the infohash, sign and submit `ModelDistributionDeclared` (Part B), and start seeding |
| `misaka model add … --distribute <bundle-dir>` | after the class registration folds, declare the founding line's version 1 and seed |
| `misaka palw version-publish … --distribute <bundle-dir>` | the same for a new version |

Conversion stays where it is (`qwen25-convert`, `qwen36-convert`, `palw-class manifest`). A bundle
directory is what those commands already produce, plus `LICENSE` and `README.md`.

### 7.2 This repository's `misaka-torrent`

| command | does |
| --- | --- |
| `misaka-torrent create <dir> --kind <kind> --title <title> --license <spdx>` | write `misaka-bundle.json` and `<title>.torrent`; print the commitment, infohash and magnet |
| `misaka-torrent inspect <dir\|file.torrent>` | print and check the descriptor and the info dictionary against the rule and the profile |
| `misaka-torrent share <dir>` | `create`, then seed; an unanchored share |
| `misaka-torrent pull <magnet> [--expect <commitment>]` | fetch and L2 |
| `misaka-torrent seed start\|stop\|list\|adopt` | as §4.5 |
| `misaka-torrent status [--json]` | §6.6 |
| `misaka-torrentd run --profile desktop\|server` | the daemon |

### 7.3 What a pull looks like

```text
$ misaka model pull 5bd9ae3d…99ef8e@3
Qwen3.6-35B-A3B · line 5bd9ae3d…99ef8e · version 3 (Current)
bundle   palw-artifact · 36.49 GB · declared by the line's developer at DAA 1,204,511
license  Apache-2.0

metadata  ok (infohash 9f2c…e1)        policy ok (4 files)
download  ████████████████░░░░  81%   63.2 MiB/s   peers 27 (8 seeds)   ETA 01:52
          uploaded 4.3 GiB to 11 peers while downloading
verify    L2 ok · L3 inventory root = version root (131 s)
install   ~/.misaka/testnet-12/models/qwen36.palwq36  (hard link)
seeding   on (desktop profile, ≤ 20 MiB/s) — `misaka model seed stop` to stop
```

The numbers are illustrative.

## 8. misakaoptions.com, the deep link and MISAKA Studio

### 8.1 The model page

- **Where.** `#/store/<lineId>` and `#/line/<lineId>` read `getPalwModelDistribution` and show, per
  kind:
  - the size;
  - who declared it and when;
  - the infohash and a copyable magnet;
  - a **Download with MISAKA** button that opens `misaka-model://…`.
- **A static site cannot speak BitTorrent.** Seeder counts are shown only from an optional off-chain
  source (a BEP 33 DHT scrape, or a tracker scrape), labelled as such. They are never a chain fact.
- **No WebTorrent.** Its WebRTC swarm is a different swarm, and a browser is no place for 38 GB.
- The site already serves the read side of the registry over wRPC JSON
  (`misakas:web/misaka-options/README.md:229-236`). Today it builds no deep links (:402).

### 8.2 The deep link

The link is `misaka-model://<network>/<line_id>/<version>?kind=<kind>`.

- **The scheme is not `misaka:`.** That is the bech32 prefix of mainnet addresses (`crypto/addresses` in
  misakas), and a wallet may own it.
- **Parsing is strict:**
  - `network` is a known network name;
  - `line_id` is 128 lowercase hex;
  - `version` is a decimal `u32` without leading zeros;
  - `kind` is one of the kinds;
  - there are no other parameters;
  - the whole link is ≤ 256 bytes.
- **The link is an identifier, not an instruction.** The handler resolves everything from the chain,
  through the operator's own node. It shows a confirmation built only from what it resolved: the line
  name, the version and its status, the size, the license, the declarant, and the free space after the
  download.
- **Nothing is fetched before the confirmation.**

### 8.3 Studio

- **Download source.** Studio's artifact source enum gains
  `Torrent { btv2_infohash, bundle_commitment, total_bytes }` beside `Download { sha256, size, hf_repo }`.
- **The daemon is a component.** Studio supervises `misaka-torrentd` like its other components, through
  the components manifest.
- **The handler** registers the `misaka-model` scheme on install and follows §8.2.

### 8.4 Components manifest v2

`misaka/components/v2` adds an optional object to `artifact` rows:

```json
"torrent": { "btv2_infohash": "<64 hex>", "bundle_commitment": "<128 hex>", "total_bytes": 36492831232 }
```

The row's `sha256` must equal the bundle descriptor's entry for the container. Its `url` (https or hf)
stays, and becomes a BEP 19 web seed when the engine supports web seeds for v2 torrents. The hybrid's
Hugging Face repository then serves the swarm's first copy.

v1 refuses unknown keys (`misakas:docs/components-manifest.md:54-56`). So v2 is a new schema string, and
v1 consumers are untouched. Like v1, v2 signs nothing (:252-256): what binds it is the release that
publishes it.

## 9. Phases

The working design's seven steps, mapped to repositories, each phase with an exit gate:

| phase | repository | content | exit gate |
| --- | --- | --- | --- |
| A1 | this | `misaka-bundle`, `misaka-btv2` | golden vectors: the canonical infohash of a fixed bundle equals libtorrent's for the same files; `bundle_commitment` equals misakas's `keyed64` for the same bytes |
| A2 | this | `misaka-transport-policy`, `fuzz/` | the D-T7 corpus refused; 24 h of fuzzing with no panic |
| A3 | this | `misaka-transport-ipc`, the fake engine, the daemon's state machine | every state of §4.2 driven by tests over the fake engine |
| A4 | this | the libtorrent shim and backend | a 38.6 GB file between two hosts; kill and resume; recheck |
| A5 | this | `misaka-torrentd`, `misaka-torrent`, confinement | D-T6, D-T8, D-T9 |
| A6 | misakas | `misaka model pull\|seed\|library\|verify` (manifest-anchored and unanchored), components v2 | Qwen2.5 A16 installed from a v2 row; `misaka model add` finds it; kaspad loads it digest-checked |
| B | misakas | its RFC, then the fence, object, RPC, `distribute`, `--distribute` | D-T1 to D-T5 on a devnet that crosses the fence |
| C | misakas web, Studio | the button, the deep link, the Studio source | button → confirmation → installed, on a fresh machine |
| D | both | profiles, web seeds, layer ranges, dedupe | D-T10 measured and published |

Inside this repository the order is A1 → A6. The first code after this text is A1, because everything
else consumes the descriptor and the infohash.

## 10. Activation (Part B)

**One dormant fence, `palw_model_distribution`**, named like `palw_model_lines` and
`palw_model_benefits`. Past it:
- `ModelDistributionDeclared` may be accepted (§3.2);
- the fold keeps the distribution collection, and the state root hashes it;
- `getPalwModelDistribution` serves rows.

Below it the object is refused by name at acceptance, and the collection does not exist.

`palw_model_distribution_fence()` is `Some` only where `palw_model_lines_fence()` is, as
`palw_model_benefits_fence()` folds its dependency
(`misakas:consensus/core/src/config/params.rs:7703-7716`). `validate_palw_v2` (:3767) requires the lines
fence at or below it. The signature-context set gains `b"misaka-palw-model-distribution-v1"`.

**Fingerprinting**, in the four places every fence has. The `palw_model_lines` sites are the pattern:
1. the field (:2404);
2. its entry in `for_each_fence` (:9971);
3. Some-only writes in `consensus_params_id` (:11797-11798) and the schedule list (:9352), so a dormant
   network fingerprints as if the fence did not exist;
4. the `never()` collapse (:5970-5971).

Its height must be one no other fence uses. Whether testnet-12's arm-every-rule function (:20382) arms
it is the misakas RFC's decision. The recommendation is that it does not, until D-T1 to D-T5 pass.

**Drills.** D-T1 to D-T5 cross the fence on the shipping binary. The rest are client drills on real
hosts.

| drill | what it shows |
| --- | --- |
| D-T1 cross | a declaration is refused before the fence and accepted after it; the RPC serves the row; the fingerprint moves only where the fence is set |
| D-T2 pull and L3 | two seeders and three downloaders on a devnet; all three install and pass L3; kaspad loads the artifact |
| D-T3 mismatch | the declared bundle carries a different container: L1 and L2 pass, L3 fails, nothing is installed, the report names the declaration |
| D-T4 replacement | a redeclaration replaces the row and increments `replaced`; clients follow the new bundle; the old swarm stops by default |
| D-T5 withdrawn | the version is withdrawn: declarations are refused, clients stop seeding, the RPC shows the status |
| D-T6 hostile peers | peers that send bad blocks, bad metadata or wrong hashes are banned; the download completes from honest peers |
| D-T7 hostile torrents | symlinks, attributes, `..`, device names, case collisions, pickles, archives, 10⁵ files, a 10 GiB info dictionary, a declared size that lies — all refused before a file is created |
| D-T8 mmap safety | kaspad maps the artifact while the daemon seeds it, rechecks it and repairs it; the mapped file is never written |
| D-T9 confinement | the daemon cannot read `~/.misaka/*.seed`, `~/.ssh` or kaspad's datadir, cannot exec, and binds only its port and its socket |
| D-T10 the hybrid prefetch | the 38.6 GB container from N seeders before activation, timed against the deploy kit's `rsync`, published |

## Proposed Spec text (sketch)

### Consensus (for the misakas RFC that adopts Part B; requirements PALW-DIST-n)

Past `palw_model_distribution`:

- **PALW-DIST-1 (the line's word).** A `ModelDistributionDeclared` MUST be refused unless `by` is the
  line's developer or maintainer bond and is Active, and its ML-DSA-87 signature verifies over
  `palw_model_distribution_message_v1` under `b"misaka-palw-model-distribution-v1"`.
- **PALW-DIST-2 (what it names).** It MUST name a version of an Active line, inside the kept history and
  not Withdrawn, and a kind of 0 or 1.
- **PALW-DIST-3 (bounds).** `total_bytes` MUST be in `[1, 2^42]`. A declaration identical to the live
  row MUST be refused.
- **PALW-DIST-4 (one live row).** There is at most one row per (line, version, kind). An accepted
  declaration replaces it and increments `replaced`, saturating.
- **PALW-DIST-5 (rent).** An accepted declaration burns `PALW_MODEL_OBJECT_RENT_SOMPI_V1`.
- **PALW-DIST-6 (no rule reads it).** No admission, court, draw, readiness, licence, fee or reward rule
  MAY read a distribution row. No field of one MAY enter a priced, committed or certified byte.
- **PALW-DIST-7 (eviction).** A version's rows leave state with the version.
- **PALW-DIST-8 (no transport in the node).** A node MUST NOT fetch, hold, seed or verify bundle bytes
  because of a row. The node's P2P MUST NOT carry bundle bytes, and kaspad MUST NOT link a torrent
  engine.

### Client conformance (this repository; requirements MT-n)

- **MT-1 (canonical torrent).** A bundle's torrent MUST be the output of rule v1 (§2.3) for its files.
  A client MUST refuse to share or declare one that is not.
- **MT-2 (descriptor).** A descriptor MUST be canonical, schema `misaka/torrent-bundle/v1`, with unknown
  keys refused. Its files plus itself MUST equal the info dictionary's file tree, with equal sizes and
  pieces roots.
- **MT-3 (policy first).** Metadata from peers MUST be checked against the infohash, and then against
  the Safe Model Profile, before the engine sees it and before any file is created.
- **MT-4 (verify before install).** Nothing is sealed before L2. An anchored `palw-artifact` bundle is
  installed only after L3. A mismatch MUST delete the staging copy and be reported.
- **MT-5 (no execution).** The daemon and the verifier MUST NOT execute, import, unpickle, render or
  extract received bytes. They MUST NOT spawn a process, open a URL or open a connection on the
  strength of received data.
- **MT-6 (sealed files).** The daemon MUST NOT open a sealed file for writing. Repair replaces it by
  rename.
- **MT-7 (no keys, no network API).** The daemon holds no signing key and reads no key file. Its only
  control surface is the local IPC socket, with peer credentials checked.
- **MT-8 (labels).** Every bundle MUST be shown with its anchor label. An unanchored fetch requires an
  explicit flag or confirmation.
- **MT-9 (links are identifiers).** A `misaka-model://` link MUST be parsed strictly. Nothing is fetched
  before a confirmation built from chain-resolved data.
- **MT-10 (withdrawn).** A client MUST stop seeding a bundle whose version is Withdrawn, unless the
  operator overrides it for that bundle.
- **MT-11 (no identity).** No MISAKA identity MAY appear in a peer id, an extension handshake or an
  announce.
- **MT-12 (server default).** The server profile MUST NOT seed unless the operator enables it.

## Alternatives

| alternative | why not |
| --- | --- |
| A central hub (Hub API, PostgreSQL, S3 metadata, accounts, a point ledger — the first working draft) | the chain already is the registry; a hub would be a trusted party, a second spelling of every row, and a host to keep up; the transport needs none of it |
| The infohash inside `ClassRegistered` | "`ClassRegistered` continues to carry no URL" (ADR-0067 D4); couples distribution to registration (D6); priced admission bytes would carry a free field |
| The infohash as a field of `ModelVersionPublished` | §3.4: timing, correction, layout, activation |
| Off-chain only (a signed descriptor in the style of ADR-0101) | works and is Phase A's fallback, but needs a publication channel — a host, i.e. a WHERE — and the chain is the only permissionless, signed, ordered pointer the project has |
| BEP 44 / BEP 46 mutable items as the publisher's feed | Ed25519 only (a second, non-PQ identity), values ≤ 1000 bytes (an ML-DSA-87 signature is 4,627 bytes, `misakas:consensus/core/src/mldsa87_primitives.rs:18-19`), items may expire in two hours without republishing; the chain already orders versions |
| Hybrid v1 + v2 torrents | the v1 half verifies by SHA-1 pieces and needs padding files; the chain anchors the v2 infohash; v2 clients exist (Open question 1) |
| rqbit as the engine | no BEP 52 per its README (2026-10-02); kept behind `ModelTransport` |
| A qBittorrent fork | GPL; a torrent client's concepts, not a model library's |
| Hugging Face / HTTPS / IPFS only | one origin's bandwidth and policy; no swarm; HTTPS sources stay as web seeds and adoption |
| Model bytes over the node's P2P (`PalwMaterialRequest`-style) | refused by the registry map (`misakas:docs/palw-registry-map.md:66`); PALW material is capped at 16 MiB a message (`misakas:protocol/flows/src/palw_gossip.rs:45`), and those links carry consensus traffic a 38 GB transfer would starve |
| Private torrents, passkeys and paid downloads | not enforceable against a public DHT once a magnet leaks; needs a passkey issuer; DRM is not a goal |
| Seeding rewards from announce statistics or peer receipts | announce `uploaded` is self-reported, and receipts between peers are mintable by two colluding identities; "the chain never takes the host's word" |
| WebTorrent in the browser | a different (WebRTC) swarm, and a browser tab for 38 GB |
| `misaka://` as the link scheme | `misaka:` is the address prefix (Open question 5) |

## Security and economic analysis

| failure | what happens |
| --- | --- |
| a peer sends corrupt blocks or a corrupt piece layer | BEP 52 verification rejects the block against the file's pieces root; the engine bans the peer; the piece is fetched elsewhere (D-T6) |
| a peer sends metadata that is not the info dictionary | `SHA-256(info) ≠ infohash` (BEP 9); refused before the policy sees it |
| a declaration names the wrong bundle, by mistake or malice | L1 and L2 pass on the declared bytes; L3 fails; nothing is installed; the report names the declaration; kaspad would refuse the bytes at load anyway (D-T3) |
| a developer or maintainer key is compromised | the attacker can redeclare, at 1 MSK a time; every consumer still runs L3; the owner rotates roles (`ModelLineRolesSet`) |
| declaration spam | only the line's own roles may declare; each costs rent; one live row per key |
| a DHT Sybil or eclipse | can delay or deny peers, never corrupt bytes; mitigated by multiple bootstrap sources, optional trackers, web seeds, and LSD on a fleet LAN |
| path traversal, symlinks, device files, case collisions | refused from the info dictionary before any file exists (§5.2, D-T7) |
| disk exhaustion | `total_bytes` checked against the declaration, the quota and the free space before admission; bundle and dictionary limits (§5.3) |
| a remote-code bug in the engine | the daemon holds no key, cannot exec, sees only its store (Landlock / seccomp / sandbox-exec / systemd); on a server it is another account than kaspad's (D-T9) |
| malicious weights; a parser bug in another runtime | outside the transport's reach (§5.5): code formats are refused, provenance is labelled, PALW consumption is digest-checked |
| a drive-by deep link (a page that opens a 300 GB pull) | nothing is fetched before a confirmation built from chain data (MT-9) |
| a seat's address in a swarm reveals which classes it holds, inviting DoS on panels | the server profile does not seed by default (MT-12); a seat that wants to seed can do so from another host or address; this is the price of P2P, stated, not hidden |
| contention with the node's P2P and consensus traffic | separate process, port and protocol; caps, connection limits, `CPUWeight` / `IOWeight`; not run on validators by default (Open question 10) |
| a file changed after verification | sealed `0444`, never written by the daemon (MT-6); kaspad digest-checks at load; `misaka model verify` re-runs L2 and L3 |
| a public RPC endpoint lies about the chain | L3 binds bytes to the root that endpoint served; seats resolve through their own node (§4.1) |
| a withdrawn version (a takedown, a licence problem, a defect) | clients stop seeding by default (MT-10); the network is permissionless, so a determined holder can still seed — the chain does not hold bytes and cannot delete them |
| free-riding, with no rewards | availability rests on the registrant (D4: "the registrant's distribution work"), on the hosts that need the artifact anyway, and on web seeds; there is no rewards path to game |
| a tracker lies | trackers are optional and can only lie about peers |
| SHA-256 and post-quantum security | the transport's identifiers are SHA-256 (BEP 52); the chain's row is signed with ML-DSA-87 and commits the descriptor with BLAKE2b-512; v2-only means no SHA-1 anywhere |

## Compatibility and migration

- **Before the fence** nothing changes on any chain: no object, row, RPC field or fingerprint. Phases A,
  C and D are client software.
- **The new variant is appended last** (§3.7), and its tag is pinned by the discriminant test.
- **RPC.** `getPalwModelDistribution` is a new method; no existing response changes.
- **Components manifest.** v1 is untouched; v2 is a new schema (§8.4).
- **`~/.misaka/config.toml`** is untouched; the transport has its own file (§6.4).
- **Existing artifacts** on Hugging Face or on operators' disks join by `seed --adopt` (§4.5). Their
  owners declare them with `misaka model distribute --dir`.
- **`palw_public_model_source_v1`** in misakas (a `uri` and a `revision`; https, ipfs or hf) is not
  declared in `consensus/core/src/lib.rs` and is not wired. This RFC does not revive it: it names a
  location, and this RFC names content.

## Open questions

Each question carries the **recommended default** this text assumes until it is decided.

1. **v2-only or hybrid?**
   *Recommended default:* **v2-only.** The chain anchors the v2 infohash, and a v1 half adds SHA-1
   pieces and padding files for clients that do not verify per file.
2. **Declarations for unowned (genesis) lines.** Should any Active bond be able to declare, bounded and
   labelled?
   *Recommended default:* **no.** Unowned lines get their pointer from the release's components
   manifest v2 (§3.5).
3. **Who may declare?**
   *Recommended default:* **the developer or the maintainer**, both "the line's own word" as
   evaluations already are.
4. **One live row or an append-only history?**
   *Recommended default:* **one live row plus `replaced`.** The chain does not need a history of
   packaging; an explorer can keep one.
5. **The link scheme.**
   *Recommended default:* **`misaka-model://`.** `misaka:` belongs to addresses.
6. **Seeding on servers.**
   *Recommended default:* **off unless enabled** (MT-12). Seats are the hosts whose addresses matter
   most.
7. **The piece rule's constants** (about 4,096 pieces, 1–16 MiB).
   *Recommended default:* **adopt them now.** Re-tune after D-T10 under a new descriptor schema;
   declarations made under v1 keep their infohash.
8. **The engine boundary.**
   *Recommended default:* **a Rust daemon over a C ABI shim** (§6.1). Policy, IPC and state stay in
   Rust; a C++ daemon would move them into C++. Windows confinement (AppContainer or a restricted token)
   is decided with it.
9. **Per-file BLAKE2b digests in the descriptor, beside SHA-256 and the pieces root.**
   *Recommended default:* **no.** The container's own digest and root already cover PALW files; a third
   hash per file is a third spelling.
10. **The daemon on validator hosts.**
    *Recommended default:* **not by default.** It is supported with caps and the server profile, and the
    operator's guide says why.
11. **DHT bootstrap.**
    *Recommended default:* **MISAKA-operated bootstrap nodes first, the public routers as a fallback,
    both configurable.**
12. **Seeder statistics on misakaoptions.com.**
    *Recommended default:* **later, from a labelled off-chain source** (§8.1). Never a chain fact.

## Decision

<Open.>

## Appendix A — from the working design to this text

| the working design said | this text | why |
| --- | --- | --- |
| a central Hub, PostgreSQL, S3 metadata, accounts, a misaka point ledger, Chihaya, Next.js | the chain (Part B) + misakaoptions.com; trackers optional; no ledger | the chain is the registry; doctrine (§3.3); no rewards (§Security) |
| a permissionless P2P network with no server | Phase A, unanchored sharing | kept, and labelled |
| `misaka-p2p` as the sidecar | `misaka-torrentd` | "P2P" is the node's protocol in misakas |
| libtorrent 2.x, not a qBittorrent fork | kept | BSD-3 vs GPL (Motivation §3) |
| BitTorrent v2, hybrid for the MVP | v2-only | the chain anchors the v2 infohash; no SHA-1 (Open question 1) |
| one torrent per variant, never one giant file, files kept as they are | one bundle per (line, version, kind); files kept; depth 1 | variants are already classes or versions (§2.1) |
| `misaka-manifest.json` / `misaka-model.json`, SHA-256 | `misaka-bundle.json`, schema `misaka/torrent-bundle/v1`; per-file SHA-256 and pieces root; BLAKE2b-512 commitment | "manifest" already names four things in misakas; the chain's hash convention |
| a piece-size table by model size | a deterministic rule (§2.3), close to the table | a declared infohash must be reproducible from files alone |
| an Ed25519 publisher key | the line's developer or maintainer bond, ML-DSA-87 | the chain is post-quantum only; the identity already exists |
| BEP 44 mutable items for the publisher's latest manifest | not used | Ed25519, 1000 bytes, expiry; the chain already orders versions (Alternatives) |
| `distribution_infohash_v2` and `manifest_hash` on the registry version | a separate `ModelDistributionDeclared` row | §3.4 |
| `artifact_root` and the distribution bytes kept apart | kept: the version root stays the identity; L3 binds them | §3.3, §4.3 |
| `misaka model add --model ./dir` doing everything | `misaka model distribute` and `--distribute` | conversion stays its own step; a bundle needs its root first |
| download → verified → seeding, with caps, battery and metered networks | kept for the desktop profile; the server profile does not seed by default | seat addresses (§Security) |
| `~/Misaka/models/` | `~/.misaka/torrent/store` + a hard link into `~/.misaka/<net>/models/` | where misakas already looks; one copy per host |
| a Safe Model Profile: safetensors, GGUF, JSON, tokenizer, README; no pickle, code or archives | kept, plus the PALW containers and `.palwmanifest` | §5 |
| no execution, no Web UI, IPC only, sandboxing, no remote commands | kept: MT-5, MT-7, §6.2, §6.5 | — |
| `misaka://model/<id>/<version>` from misakaoptions.com to the desktop | `misaka-model://<network>/<line_id>/<version>` with a confirmation | the address prefix; a link is an identifier |
| misaka points, receipts, private trackers, passkeys | not built | Alternatives; Security |
| peers see IP addresses | stated, with the seat consequence and a default | Security |
