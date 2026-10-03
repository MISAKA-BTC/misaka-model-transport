# RFC-0002: Private transport — Direct, Private and Anonymous network modes for MISAKA Torrent: the same bundles and the same verification, carried over I2P when a user does not want peers to see their address, with a MISAKA onion overlay left to a later RFC

| Field | Value |
| --- | --- |
| Status | Draft, 2026-10-03 — design; nothing is implemented |
| Author(s) | MISAKA core (drafted with Claude) |
| Created | 2026-10-03 |
| Normative dependencies | RFC-0001 (bundles, rule v1, the three verification levels, the Safe Model Profile, `misaka-torrentd`, MT-1…MT-12). External: I2P's SAM v3 bridge; libtorrent 2.0.x's I2P support (`i2p_hostname`, `i2p_port`, `allow_i2p_mixed`, `i2p_{in,out}bound_{quantity,length,length_variance}`, `torrent_flags::i2p_torrent`) |
| Affects | **this repository** only: `misaka-torrentd` (a network mode per instance), the engine settings, a small I2P tracker, the desktop app (a Network setting and a bundled I2P router), the release index (`i2p_trackers`), misakaoptions.com (labels). **No consensus change**: nothing here touches misakas or Part B of RFC-0001 |
| Branch | `main` (text only) |
| Related | RFC-0001 §6.3 (network defaults), §Security (a seat's address in a swarm), MT-11 (no identity), MT-12 (server default); the Tor Project's position on BitTorrent over Tor; Tribler's anonymous downloading (a dedicated Tor-like overlay with three proxy layers and hidden seeding) |

**Sources.** The libtorrent facts were checked against the libtorrent-rasterbar 2.0.15 source on
2026-10-03:
- `include/libtorrent/settings_pack.hpp`, `src/settings_pack.cpp`: the defaults are 3 tunnels of 3 hops
  each way, and `allow_i2p_mixed = false`.
- `src/torrent.cpp`: an I2P torrent is never announced on the DHT or LSD unless mixing is allowed, and
  one is any torrent flagged `i2p_torrent` or with a tracker under `.i2p`.
- `src/ut_pex.cpp`: no PEX for an I2P torrent unless mixing is allowed.
- `src/http_tracker_connection.cpp`: an I2P announce carries `ip=<destination>.i2p`, and the response's
  `peers` is a string of 32-byte destination hashes.
- `include/libtorrent/i2p_stream.hpp`: `SESSION CREATE STYLE=STREAM DESTINATION=TRANSIENT …`, so the
  destination is new for every SAM session.

The Tor Project's position is its support page "Can I use Tor with Torrent?". Tribler's design is its
page `tribler.org/anonymity.html`. i2pd's licence (BSD-3-Clause) and latest release (2.61.0, 2026-07-20)
come from its GitHub repository. All were read on 2026-10-03.

## 概要(日本語)

- **問題。** RFC-0001 の MISAKA Torrent は普通の BitTorrent なので、スウォームの全員に IP アドレスが見える。
  RFC-0001 自身も「seat のアドレスが、その seat がどの class を持つかを明かす」と書いている。
  - 「どのモデルを取得したか」を同じスウォームの他人やトラッカー運営者に知られたくない利用者がいる。
  - seat ではないホストからシードしたいが、アドレスは出したくない運営者もいる。
- **Tor は使わない。** Tor Project 自身が「Torrent over Tor は安全でなく推奨しない」と明言している。理由は、
  多くのクライアントが実 IP をピアに漏らすことと、ボランティアの exit relay に負荷と法的リスクを負わせる
  ことである。
- **何を作るか。** 同じ bundle・同じ infohash・同じ L1/L2/L3 検証のまま、**運ぶ経路だけ** をモードで
  切り替える。
  - **Direct**: RFC-0001 そのまま。最速で、ピアに IP が見える。既定。
  - **Private**: I2P 上で、トンネルを短くする(1 ホップ)。ピアとトラッカーから IP を隠す。ただし最初の
    ホップと相手が共謀すれば結び付けられる。
  - **Anonymous**: I2P の既定である 3 ホップのトンネル。より強いが遅い。
  - I2P では、相手も I2P の宛先(destination)の背後にいる。そのため **シーダーも自動的に隠れ**
    (hidden seeding)、**clearnet へ出る exit は存在しない**。
- **なぜ I2P か。** 次の理由で、最初の実装として最も堅い。
  - libtorrent 2.0.x が SAM v3 経由で I2P を内蔵している。
  - I2P は Tor と違い、ネットワーク内での P2P を想定している。
  - 外部ルーターに i2pd(C++、BSD-3-Clause)を使える。
  - 独自のオニオンネットワークを設計・審査するのは別の大仕事なので、**後の RFC(Phase P3)に回す**。
- **漏洩させない規則。** Private / Anonymous モードの daemon は次を守る。
  - clearnet の DHT・LSD・UPnP/NAT-PMP・PEX・clearnet トラッカー・Web シードを **一切使わない**。
  - clearnet へは一切接続できず、ループバック上の I2P ルーター(SAM)だけとつながる。この性質そのものを
    要件にし、パケットの層で強制する。Linux では loopback だけの network namespace か systemd のアドレス
    制限を使い、Landlock(ファイル)と seccomp(exec 禁止)を重ねる。Landlock のネットワーク規則はポート
    単位なので、それだけでは保証にならない。
  - ピアの発見は I2P 上のトラッカー(`misaka-i2p-tracker`)で行う。libtorrent は I2P torrent を DHT に載せない。
  - モデル索引は「全体を 1 回取る」ので、どのモデルを見たかは索引の運営者に伝わらない。
- **識別子を分ける。**
  - **プロセスを分ける**: モードごとに別の daemon プロセス、別の state、別のポート、別の I2P セッションにする。
  - **宛先**: libtorrent の I2P 宛先は SAM セッションごとに新しい(TRANSIENT)。
  - **同時シードの禁止**: 同じ bundle を Direct と Private で同時にシードすると、可用性やタイミングで結び付け
    られる。既定では拒否する。
- **レビューで決めたこと(2026-10-03)。**
  - Private(1 ホップ)は「低遅延のプライバシー」として残す。
  - I2P トラッカーは最低 3 つ、運営主体を分ける。最初は MISAKA 2 台とコミュニティ 1 台で、利用者が追加
    してもよい。
  - バリデータ / seat のホストは、Direct でも I2P でも既定でシードしない。I2P シードは専用のシードホストで
    行う。
  - 同じ bundle を 2 つのモードで同時にシードすることは、既定で拒否する。
- **速度について正直に書く。**
  - 匿名経路では、1 バイトが往路と復路のホップ数だけ中継され、ネットワーク全体の帯域を消費する。この
    トレードオフは消せない。
  - 対策は BitTorrent の並列性である。多数のピアとトンネルに piece を分散させる。
  - 数字はまだない。**Phase P1 で実測するまで、速度も匿名性も宣伝しない。**
  - 表示は「Private: ピアにあなたの IP は見えません」のように、何を隠すかを具体的に書く。「追跡不能」とは
    書かない。
- **対象外。**
  - 公開者の匿名性: Part B の宣言はチェーン上で署名されるので、誰が出したかは公開される。
  - 全体を見張る受動的攻撃者への耐性。
  - モデル内容の秘匿: 内容は公開物である。
  - チェーン側の変更は一切ない。
- **段階。**
  - P0: この RFC。
  - P1: サーバに i2pd と I2P トラッカーを立て、同じモデルを I2P スウォームで配って計測する。
  - P2: アプリに Network 設定と同梱ルーターを入れ、漏洩ドリルに通す。
  - P3: 必要と計測が示せば、MISAKA 独自のマルチ回路オーバーレイを、別 RFC と外部レビューで設計する。

## Summary

RFC-0001 carries bundles over plain BitTorrent, where every peer sees every other peer's address. This RFC
adds **network modes** to `misaka-torrentd`. A mode changes only the transport. The bundle, its
infohash, rule v1, the Safe Model Profile and L1/L2/L3 are untouched.
- **Direct** is RFC-0001, and stays the default.
- **Private** carries the swarm over I2P with one-hop tunnels.
- **Anonymous** carries it over I2P with I2P's default three-hop tunnels.

In both I2P modes every peer is an I2P destination behind its own inbound tunnels. Seeders are
therefore hidden like downloaders, and no traffic ever leaves the overlay for the clearnet. The engine
is libtorrent's built-in SAM v3 client; the router is i2pd, bundled or the user's own.

A daemon in an I2P mode follows four rules:
- **Clearnet discovery is off.** No DHT, LSD, UPnP or NAT-PMP, PEX with clearnet peers, clearnet
  tracker or web seed.
- **No clearnet.** It cannot reach the clearnet; its only network peer is the router's SAM port on
  loopback. This property is normative, enforced at the packet level (a loopback-only network namespace
  or systemd address filtering on Linux, a sandbox profile on macOS), with Landlock for files and
  seccomp for exec.
- **Discovery.** It finds peers through I2P trackers, which this repository provides as
  `misaka-i2p-tracker`.
- **Separate identity.** Each mode is a separate daemon instance with its own state, port and
  transient I2P destination. The same bundle is not seeded in two modes at once.

Throughput and anonymity are to be measured in Phase P1 before anything is claimed. A MISAKA-specific
onion overlay is deferred to its own RFC, with external review (Phase P3).

## Motivation

### 1. What RFC-0001 exposes

In Direct mode, the swarm of a bundle is a public list of the addresses that hold or want it.
- **Swarm members.** Anyone who joins the swarm, or crawls the public DHT for its infohash, learns the
  list.
- **Trackers.** A tracker, if one is used, learns it too.
- **Seats.** RFC-0001 already names the consequence for seats: an address in a swarm says which
  classes that host holds. The server profile therefore does not seed by default (MT-12).

The same holds for users:
- **Downloaders.** Downloading a model tells the swarm that this address wanted it.
- **Seeders.** Seeding it tells the swarm that this address holds it, for as long as the client runs.

Every downloader seeding is the point of RFC-0001. For some users it is also the reason not to use it.

### 2. Why not Tor

The Tor Project's support page says that using torrent clients over Tor "is unsafe and not
recommended", for two reasons:
- **IP leaks.** Most clients reveal the real IP address to peers anyway.
- **Exit load.** Torrenting puts load and potential legal issues on volunteer exit relays.

Both apply here. libtorrent's peer protocol was not designed to be tunnelled through a SOCKS proxy
without leaks, and a 38 GB model through exits is exactly the load Tor asks people not to create.

### 3. Why I2P first

I2P was designed for traffic between peers inside the network:
- **No exit.** There is no exit by default; both ends are destinations behind tunnels.
- **Built into the engine.** libtorrent 2.0.x already speaks I2P through the SAM v3 bridge, and keeps
  I2P torrents apart from the clearnet unless mixing is explicitly allowed (Sources).
- **An available router.** i2pd, a C++ router under BSD-3-Clause, can be bundled like libtorrent, or
  the user's own router can be used.

So the first private mode needs no new cryptography and no new routing protocol from this repository.
It needs configuration, discipline about leaks, a tracker, and measurement.

### 4. Why not build an onion network now

A Tor-like overlay specialised for swarms is attractive:
- **Parallel circuits.** It could run many circuits in parallel.
- **Hidden seeding.** It could hide seeders behind rendezvous points.
- **No directory server.** It could need no central directory.

Tribler built exactly that: a dedicated Tor-like network for torrents, with three proxy layers and
experimental hidden seeding. It also warns users not to rely on it: "Do not put yourself in danger. Our
anonymous download feature is not yet broadly tested."

An anonymity network is judged by attacks nobody has tried yet. It needs a threat model, a design
review and a population of relays before its label means anything. I2P has all three. Phase P3 starts
only if P1 and P2 show that I2P cannot meet a stated need.

## Goals and non-goals

**Goals.**
1. **Modes.** Direct, Private and Anonymous network modes, chosen per daemon instance and shown per
   bundle.
2. **Same verification.** The same bundles, infohashes and verification in every mode. A mode never
   weakens L1, L2, L3 or the Safe Model Profile.
3. **No clearnet connections** from a daemon in an I2P mode, enforced below the engine and checked by
   drills.
4. **Hidden seeding.** In an I2P mode, peers see neither a downloader's address nor a seeder's.
5. **Separate identities** across modes, and fresh ones across sessions in I2P modes.
6. **Parallelism.** Throughput comes from many peers and many tunnels, not from one long circuit.
7. **Honest labels.** Each mode says what it hides and from whom, and nothing more.

**Non-goals.**
- **Authorship.** Anonymity of publication. A Part B declaration is signed on the chain by the line's
  developer or maintainer; who declared a bundle is public by design.
- **Strong adversaries.** Resistance to a global passive adversary, or to an attacker who controls a
  large share of I2P routers.
- **Confidentiality of content.** Bundles are public; I2P's end-to-end encryption hides who talks to
  whom, not what the public bytes are.
- **Hiding I2P use.** Hiding that a host uses I2P. An I2P router's traffic can be recognised by an
  observer of the host's link.
- **Tor**, VPN integration, or proxying Direct mode through SOCKS.
- **The chain.** Any change to misakas.
- **The overlay.** A specification of the Phase P3 overlay. §9 states only what would justify it.

## 1. Threat model

Who learns what, in each mode, when the user downloads or seeds a bundle:

| observer | Direct | Private (1-hop) | Anonymous (3-hop) |
| --- | --- | --- | --- |
| another peer in the swarm | the user's IP address and that it wants or holds the bundle | a transient destination only | a transient destination only |
| a tracker operator | the IP address, if a clearnet tracker is used | a transient destination only | a transient destination only |
| the public DHT | the IP address (announce and lookup) | nothing: the DHT is not used | nothing |
| the index operator (misakaoptions.com) | that the user fetched the whole index | the same; never which model (§4.3) | the same |
| the user's ISP or local network | the swarm's peers' addresses and the volume | that the host runs I2P, and volume and timing | that the host runs I2P, and volume and timing |
| the first hop of the user's tunnel | — | the user's IP, and the next hop toward the peer | the user's IP and one more hop; not the peer |
| a peer colluding with the user's first hop | — | can link the user's IP to the swarm by timing | needs more of the path; harder, not impossible |
| a global passive adversary | everything | can correlate | can correlate; not a goal |

In every mode the content is public and verified the same way. A malicious peer can waste time, never
corrupt bytes (RFC-0001 §Security).

## 2. Modes

| | Direct | Private | Anonymous |
| --- | --- | --- | --- |
| transport | TCP / µTP over IPv4 and IPv6 | I2P streaming through SAM | I2P streaming through SAM |
| tunnels | — | inbound and outbound length 1 | length 3 (the libtorrent and I2P default) |
| tunnels per direction | — | 4 (configurable, 1–16) | 3 (configurable, 1–16) |
| peer discovery | DHT, PEX, LSD by profile, optional trackers | I2P trackers only | I2P trackers only |
| web seeds (BEP 19) | yes | no | no |
| UPnP / NAT-PMP | by profile | no (I2P handles reachability) | no |
| destination | — | transient, new per SAM session | transient, new per SAM session |
| default | **yes** | no | no |

- **What each mode is for.**
  - **Direct**: the default for ordinary users.
  - **Private**: *low-latency privacy*. It hides the user's clearnet address from peers and trackers,
    with less resistance to traffic correlation than Anonymous (one relay each way sees an end).
  - **Anonymous**: for users who accept much lower speed for that resistance.

  Both I2P modes are optional.
- **Same infohash.** The torrent of a bundle is the same in every mode: rule v1 puts trackers outside
  the info dictionary, so the infohash does not change. A Direct swarm and an I2P swarm of the same
  bundle share the infohash and never share peers.
- **Engine settings.** In an I2P mode the daemon sets:
  - `i2p_hostname` and `i2p_port` to its router's SAM bridge;
  - `allow_i2p_mixed = false`;
  - `enable_dht = enable_lsd = enable_upnp = enable_natpmp = false`;
  - the `i2p_torrent` flag on every torrent it adds;
  - the tunnel lengths and quantities above.
- **Listening.** It listens on no non-loopback interface.
- **Variance.** A non-zero length variance (`i2p_*_length_variance`) is an Open question.

### 2.1 Hosts

| host | Direct seeding | I2P seeding |
| --- | --- | --- |
| an ordinary user's desktop | on by default (RFC-0001 §4.6) | only in the chosen I2P mode |
| a validator / seat host | off by default (MT-12) | **off by default**; enabling it requires an explicit per-mode setting |
| a dedicated seed host (`misaka-model-transport` only) | allowed | allowed |

Validation and model distribution do not share a host's bandwidth, CPU or failure modes by default, and
anonymous routing adds a relay's load on top of seeding. An operator who wants a seat's classes seeded
over I2P runs a dedicated seed host.

## 3. Identity separation

1. **One instance per mode.** A host runs one `misaka-torrentd` instance per mode it uses, each with its
   own state directory, socket and configuration file:
   - `~/.misaka/torrent` for Direct;
   - `~/.misaka/torrent-private` for Private;
   - `~/.misaka/torrent-anonymous` for Anonymous.

   A mode is fixed for the life of an instance; it is never switched in place.
2. **One libtorrent session per instance.** So there is one peer id, one SAM session and one transient
   destination per instance. libtorrent creates the destination with `DESTINATION=TRANSIENT`, so it is
   new each time the session connects. No I2P key is ever stored.
3. **One mode per bundle at a time (default deny).** A bundle is held by at most one instance on a host.
   Fetching or seeding a bundle in one mode while another instance on the host holds it is refused. The
   reason: one host handling one bundle in two networks gives an observer timing and bandwidth patterns
   to correlate. A per-bundle operator override may be added later for experts. It is never the default
   and never offered by the desktop app.
4. **Files are shared, identities are not.** Moving a bundle between modes removes it from one
   instance and adopts it in the other (RFC-0001 §4.5), by hard link. The bytes stay on disk once.
5. **Nothing links the modes on the wire.** No MISAKA identity appears in any mode (MT-11). The IPC
   socket of each instance is local and separate.

## 4. Discovery

### 4.1 I2P trackers

libtorrent finds peers for an I2P torrent only through trackers under `.i2p`: it announces I2P torrents
neither on the DHT nor on LSD, and exchanges no PEX for them, unless mixing is allowed.

This repository therefore adds `misaka-i2p-tracker`, a small HTTP tracker served as an I2P server
tunnel. It speaks the I2P tracker convention that libtorrent implements:
- **Announce.** The announce carries `ip=<base64 destination>.i2p`.
- **Storage.** The tracker stores the SHA-256 of the decoded destination.
- **Response.** It answers `peers` as a string of 32-byte destination hashes.

It holds announces in memory only and expires them. It logs no destination.

- **Anyone may run one.** A tracker can only lie about peers (RFC-0001 §Security).
- **At least three, run by different operators.** An index MUST list at least three I2P trackers, and
  SHOULD list trackers run by more than one operator. File transfer is peer to peer, so a single
  tracker would make peer discovery the central point of failure. The first set is two run by MISAKA
  and one by the community.
- **The user may add trackers.** A client announces to every listed tracker and to any the user adds.
  Trackers live outside `info`, so adding one never changes a bundle.

### 4.2 Where the trackers are listed

The release index gains one top-level field, `i2p_trackers`: a list of `http://<b32>.b32.i2p/announce`
URLs. A client in an I2P mode adds them to the torrent it builds from the bundle. They go outside
`info`, so the infohash is unchanged. A deep link carries no tracker.

### 4.3 Resolving a link without telling the index which model

In an I2P mode the client:
- **Fetches the whole index** and never asks the index for one model. The operator learns that the
  user uses MISAKA, not what they fetch.
- **Fetches it through I2P** when the index is also published as an I2P site; otherwise over HTTPS,
  as a whole, cached.
- **Never resolves anything per bundle over the clearnet**: no tracker, no web seed, no DNS name
  derived from a bundle.

Browsing misakaoptions.com itself is outside the transport. A user who does not want the site to see
which model page they open should browse it over I2P or Tor Browser. The app says so beside the
Network setting.

## 5. No clearnet leaks

Settings are not enough: a bug, a future engine feature or a misread option could open a clearnet
socket. The requirement is therefore a property of the instance, not of one mechanism:

> An instance in an I2P mode MUST NOT be able to establish a clearnet peer, tracker, DHT, web-seed or
> DNS connection. Its only network peer is its router's SAM bridge on loopback.

**No single mechanism guarantees it.** Landlock's network rules (ABI 4, Linux 6.7 and later) filter TCP
bind and connect **by port**, not by address, so they cannot pin connections to `127.0.0.1`. An
implementation layers mechanisms so that each covers what the others do not.

| layer | Linux | macOS |
| --- | --- | --- |
| where packets may go | a network namespace whose only interface is loopback, with the router's SAM port reachable from it; or, under systemd, `IPAddressDeny=any` with `IPAddressAllow=localhost` on the instance's unit | a `misaka-torrentd-private.sb` profile that allows `network-outbound` only to `localhost:<SAM port>` and no `network-bind` but the IPC socket |
| what the engine is told | `allow_i2p_mixed = false`; no DHT, LSD, UPnP or NAT-PMP; no clearnet listen; the `i2p_torrent` flag (§2) | the same |
| what files it may touch | Landlock filesystem rules (the store, its state, its socket), as in Direct mode | the same profile |
| what it may run | seccomp: no `execve`, no `ptrace`, no module or `bpf` (RFC-0001 §6.5); optionally Landlock TCP rules limiting connect to the SAM port as one more fence | the profile denies `process-exec` and `process-fork` |

- **Refusal.** The daemon refuses to start an I2P mode when the packet layer (the first row) cannot be
  applied, unless the operator accepts the risk explicitly.
- **Checked, not trusted.** D-P1 and D-P3 check the property itself.
- **The router.** The router (§7) is a separate process with its own confinement. It needs the
  network; the transport instance does not.

## 6. Throughput

Anonymity costs bandwidth, and this RFC does not pretend otherwise:
- **Amplification.** With tunnels of length `o` (the downloader's outbound) and `i` (the seeder's
  inbound), each payload byte crosses `o + i` routers between the two ends. Each of those routers
  spends that bandwidth relaying. In Anonymous mode that is about six routers per byte; in Private mode,
  about two.
- **Per-tunnel rate.** A single tunnel's throughput is bounded by its slowest router.

Swarms are what make this tolerable:
- **Many peers.** A bundle is fetched as 1–16 MiB pieces from as many peers as hold it.
- **Many tunnels.** Each I2P session keeps 1–16 tunnels in each direction.

A download therefore runs over many tunnels and many seeders at once. The goal is aggregate
throughput, not a fast single circuit.

**Every router relays.** A MISAKA host running an I2P router also routes other users' tunnels. The
bundled router's shared bandwidth is set to at least the instance's own rate (§7). A host with spare
capacity can run a router with a high share and no bundles: a relay that contributes bandwidth, not
storage. That is the "bandwidth node" role, and in I2P it already exists.

**No numbers yet.** Phase P1 measures before this text promises anything (§10).

## 7. The router

- **Bundled router.** The desktop app bundles **i2pd** as a second sidecar, `misaka-i2pd`, pinned in
  `third_party.toml` like libtorrent. It runs as the app's child process with its own data directory
  (`~/.misaka/i2p`) and is configured so that:
  - SAM listens on `127.0.0.1` only;
  - the HTTP and SOCKS proxies and the web console are off;
  - the shared bandwidth is at least the transport's own cap;
  - transit tunnels are on.
- **Confinement.** It is confined to its data directory and the network, never the store.
- **Your own router.** A server, or a user who already runs I2P, can point an instance at an existing
  router (`[i2p] sam = "127.0.0.1:7656"`). The tunnel settings are then the instance's request; the
  router may cap them.
- **Router modes.** The Private and Anonymous instances share one router and use separate SAM
  sessions, so separate transient destinations.
- **The I2P network.** Joining I2P adds MISAKA's traffic to it. The bundled router's transit sharing
  is on by default, so that a MISAKA host gives back at least what it takes. Bulk model transfer is the
  kind of use I2P's own torrent client exists for; the volume is still measured in P1 before the app
  offers the mode to everyone.

## 8. What the user sees

- **Network setting.** The app's Settings gain a Network choice, with these descriptions:
  - **Direct**: fastest; peers can see your IP address. This is BitTorrent.
  - **Private (I2P)**: peers and trackers cannot see your IP address. The relay next to you can, and
    timing analysis is easier than in Anonymous. Somewhat slower.
  - **Anonymous (I2P, 3 hops)**: no single relay sees both you and your peer. Much slower, and not
    proof against powerful observers.
- **First use.** Choosing an I2P mode the first time explains three things: that the ISP can see I2P
  is in use, that the site still sees which pages are browsed (§4.3), and that the router relays other
  people's traffic.
- **Library.** Each bundle shows its mode. Speeds and peer counts are what that instance sees.
- **The site.** misakaoptions.com's Swarm tab reports the Direct swarm as now. An I2P swarm has no
  public count, so the site says only whether the index lists I2P trackers.
- **Never.** No label says "untraceable", "anonymous like Tor" or "private" without the sentence that
  says from whom.

## 9. Phase P3 — a MISAKA overlay (not specified here)

A MISAKA-specific overlay could go further than I2P:
- **Piece-aware circuits.** Choose circuits per piece across many rendezvous points.
- **No directory.** Need no directory.
- **Settlement.** Let relays be paid through the chain.

This RFC does not specify it. P3 starts only when both hold:
- **The need is shown.** P1 and P2 measurements show that I2P cannot meet a stated need (for example,
  a model of a stated size downloaded in a stated time, by a stated share of users).
- **The design is reviewed.** A separate RFC gives a threat model, a design and a security review by
  people outside this project.

Until then, MISAKA's anonymity is I2P's, and the labels in §8 say so.

## 10. Phases

| phase | scope | exit |
| --- | --- | --- |
| P0 | this RFC | review (decisions recorded above) |
| P1 | on a dedicated seed host (not a seat): i2pd, three `misaka-i2p-tracker`s as I2P server tunnels, `misaka-torrentd` in Anonymous mode seeding the published bundle. Two clients (one Private, one Anonymous) fetch it | the measurements in D-P5 recorded and published (throughput, CPU, RAM, latency, seed performance, reconnects, and a 100 GB-class transfer), including the router's transit share; D-P1 passes on the host |
| P2 | `misaka-torrentd` modes (§2–§5); the app's Network setting and bundled router (§7–§8); the index field (§4.2); the leak drills | D-P1…D-P4 pass on Linux and macOS; the labels reviewed against §8 |
| P3 | §9, only if its conditions hold | its own RFC |

## Proposed Spec text (sketch)

### Client conformance (this repository; requirements MP-n)

- **MP-1 (default).** The default mode MUST be Direct. An I2P mode is used only after the user or
  operator chooses it.
- **MP-2 (same bytes).** A mode MUST NOT change a bundle's torrent, infohash, descriptor, verification
  levels or Safe Model Profile. MT-1…MT-12 hold in every mode.
- **MP-3 (no clearnet discovery).** An instance in an I2P mode MUST NOT use the DHT, LSD, UPnP,
  NAT-PMP, PEX with clearnet peers, a clearnet tracker or a web seed. It MUST set `allow_i2p_mixed`
  false and flag every torrent `i2p_torrent`.
- **MP-4 (no clearnet).** An instance in an I2P mode MUST NOT be able to establish a clearnet peer,
  tracker, DHT, web-seed or DNS connection; its only network peer is its router's SAM bridge on loopback.
  The property MUST be enforced below the engine, by a packet-level mechanism such as a loopback-only
  network namespace, systemd address filtering or a sandbox profile (§5), not by engine settings or
  Landlock's port rules alone. The instance MUST refuse to start when it cannot be enforced, unless the
  operator explicitly accepts the risk.
- **MP-5 (one mode per instance).** A daemon instance has exactly one mode for its lifetime, its own
  state directory, socket and configuration, and one libtorrent session.
- **MP-6 (transient destinations).** An instance in an I2P mode MUST use transient destinations and
  MUST NOT persist I2P keys.
- **MP-7 (one mode per bundle).** A host MUST NOT seed one bundle from two instances of different modes
  at once, unless the operator overrides it for that bundle.
- **MP-8 (whole-index resolution).** In an I2P mode, link resolution MUST fetch the whole index and
  MUST NOT make any per-bundle request over the clearnet.
- **MP-9 (labels).** Every mode MUST be described by what it hides and from whom (§8). No label may
  claim more.
- **MP-10 (router hygiene).** A bundled router MUST expose SAM on loopback only, with its proxies and
  console off. Its transit sharing MUST be on, with shared bandwidth at least the instance's own cap.
- **MP-11 (tracker privacy).** `misaka-i2p-tracker` MUST keep announces in memory only, expire them,
  and log no destination or infohash with a destination.
- **MP-12 (seats).** On a validator or seat host, I2P seeding MUST be off by default, like Direct
  seeding (MT-12), and MUST require an explicit per-mode setting.
- **MP-13 (tracker set).** An index that lists I2P trackers MUST list at least three, and SHOULD list
  trackers run by more than one operator. A client MUST also announce to trackers the user adds.

### Drills

- **D-P1 (leak).** Fetch and seed a bundle in each I2P mode while capturing every packet on the host's
  non-loopback interfaces that is attributable to the instance's process. Pass: none. The router's
  traffic is the router's, captured separately.
- **D-P2 (separation).** Run Direct and Anonymous instances side by side and restart each three
  times. Pass when all of these hold:
  - peer ids, listen ports and I2P destinations are pairwise distinct across instances and sessions;
  - no state file is shared;
  - MP-7 refuses a second-mode seed.
- **D-P3 (confinement).** In an I2P mode, try a clearnet TCP connect, a UDP send, a bind and a DNS
  query from inside the instance. Pass: each is refused.
- **D-P4 (verification unchanged).** The RFC-0001 drills for hostile torrents and peers (D-T6, D-T7)
  pass over I2P.
- **D-P5 (measurement).** For the published 1.8 GB bundle and a 100 GB-class bundle, 1, 2 and 4 seeders, and Private and
  Anonymous clients, record:
  - time to the first peer;
  - time to complete;
  - mean and peak throughput;
  - CPU and memory of the instance and the router;
  - the router's transit traffic;
  - recovery after a router or instance restart (new transient destinations, re-announce, resume).

  Publish the numbers with the router versions and settings.

## Alternatives

| alternative | why not (now) |
| --- | --- |
| BitTorrent over Tor | the Tor Project advises against it (IP leaks, exit-relay load and legal exposure); a 38 GB model through exits is the load it asks people not to create |
| Tor onion services for both ends | no exit, but Tor is built for low-latency interactive traffic, its relays are volunteers who did not sign up for bulk transfer, and libtorrent has no onion-service transport; I2P has the same shape with bulk P2P as a first-class use and is already in libtorrent |
| Tribler's network | a working precedent for a torrent-specific overlay with hidden seeding; a separate ecosystem and protocol stack (and, by its own page, not yet broadly tested); a MISAKA client would join a network it does not control or review |
| a MISAKA onion overlay now | an anonymity design is only as good as its review and its relay population; neither exists yet (§9) |
| a VPN or SOCKS proxy for Direct mode | hides the IP from peers by trusting one provider who sees everything; a user may do this already; it is not a transport this project can vouch for |
| mixed mode (`allow_i2p_mixed`) | lets an I2P torrent take clearnet peers, which defeats the point; never enabled |

## Security analysis

| failure | what happens |
| --- | --- |
| a clearnet socket opens from an I2P instance (a bug, an engine feature) | confinement refuses it (MP-4, D-P3); the instance refuses to run unconfined |
| an observer links a host's Direct and I2P activity | separate instances, sessions and destinations (MP-5, MP-6); one mode per bundle (MP-7); a host that seeds in Direct is still visible in Direct |
| a malicious I2P tracker | can lie about peers, not bytes; can learn transient destinations and which infohashes they announce, not addresses; several trackers per index |
| a Sybil set of I2P routers | can degrade or observe tunnels it builds into; Anonymous mode raises the share an attacker needs; not proof against a large share (non-goal) |
| a peer colluding with the user's first hop (Private mode) | can link the user's address to the swarm by timing; this is why Private is labelled as hiding the address from peers, not from relays |
| the index operator | learns that a host fetched the whole index, not which bundle (MP-8); the site's own page views are outside the transport (§4.3) |
| traffic analysis of volume (a 38 GB transfer is conspicuous) | not hidden; a global or link-level observer can see that a large transfer happened over I2P |
| corrupt or hostile data over I2P | the same as Direct: L1, L2, L3, the profile and the bans (D-P4) |
| the bundled router is exploited | it holds no MISAKA key and cannot read the store; it is confined separately from the transport instance |
| I2P is blocked by the network | the I2P modes fail to find peers; the app says so; Direct remains available |

## Compatibility and migration

Nothing changes for Direct users, the chain, Part B or bundles. The index gains an optional
`i2p_trackers` field; older clients ignore it, since the index is read with unknown fields allowed.
`misaka-torrentd` gains a `[network] mode` setting:
- **Absent** means Direct.
- **An I2P value** is refused by builds that do not implement this RFC, because the configuration
  refuses unknown fields.

## Decisions taken in review (2026-10-03)

1. **Private (1-hop) stays**, as *low-latency privacy*. It hides the clearnet address from peers and
   trackers, with weaker resistance to traffic correlation than Anonymous (§2, §8).
2. **I2P trackers: at least three, and more than one operator.** The first set is two run by MISAKA
   and one by the community, and users may add their own (§4.1, MP-13).
3. **Validator and seat hosts do not seed by default**, neither Direct nor I2P. Dedicated seed hosts
   carry I2P seeding (§2.1, MP-12).
4. **Simultaneous seeding of one bundle in two modes is denied by default** (§3, MP-7). An expert
   override, if ever added, is per bundle and outside the desktop app.
5. **On Linux, the no-clearnet property is enforced at the packet level.** A loopback-only network
   namespace or systemd address filtering does this, with Landlock for files and seccomp for exec.
   Landlock's port rules are only an extra fence (§5, MP-4).

## Open questions

1. **Confinement on desktops without systemd units or user namespaces.** Hosts that can create neither
   a network namespace nor a systemd-filtered unit for the instance need another way. Should the
   desktop app refuse I2P modes there?
2. **Tunnel length variance.** Should Anonymous mode use a non-zero variance to blur the hop count?
3. **The index over I2P.** Should misakaoptions.com publish the index (and the site) as an I2P site, so
   that I2P clients never touch the clearnet at all?
4. **The router's bandwidth share and limits** on a laptop on battery, or on a metered link (RFC-0001
   §4.6).
5. **Windows**, where the app is not yet built: the router, the confinement and the sidecars.
6. **Who the community tracker operators are**, and how a dead tracker is removed from an index.

## Decision

Pending review.
