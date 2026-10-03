# RFC-0002 P1 — first I2P measurements (2026-10-03)

**Status: a first run, not the P1 exit.** It ran on one host, with one seeder, one run per mode, and a
512 MiB bundle. RFC-0002 §10 asks for 1, 2 and 4 seeders, a 100 GB-class bundle, and a dedicated
seed host. Read the numbers as an order of magnitude, not a benchmark.

## Setup

| piece | what |
| --- | --- |
| host | one server (8 vCPU, 23 GB RAM, Ubuntu 24.04) that also runs other MISAKA services, so this run is an operator override of RFC-0002 MP-12, for measurement only; a 24 h fuzzing job ran at the same time. The host is not named here, by RFC-0002's own reasoning about addresses |
| routers | two i2pd 2.61.0 instances (signed tag, commit `635b013`) on the same host: `seed` and `client`, `bandwidth = 4096` KB/s, `share = 100`, transit on, SAM on 127.0.0.1 only (`setup/i2pd-seed.conf`) |
| tracker | `misaka-i2p-tracker` behind an I2P server tunnel on the `seed` router (`setup/tunnels.conf`) |
| seeder | `misaka-torrentd` in Anonymous mode (3 hops each way, 3 tunnels) on the `seed` router, as a systemd unit with `IPAddressDeny=any` / `IPAddressAllow=localhost` (`setup/misaka-torrentd-p1seed.service`) |
| clients | `misaka-torrentd` in Private mode (1 hop, 4 tunnels), then Anonymous mode (3 hops, 3 tunnels), on the `client` router, each in a transient unit with the same IP filter (`setup/measure.sh`) |
| bundle | `p1-measure-512m`: 512 MiB of random bytes (`source-weights`, piece 1 MiB), seeded in no other mode (MP-7) |
| engine | libtorrent 2.0.15 **with `third_party/patches/libtorrent-2.0.15-i2p-v2-peers.patch`** (see Findings) |

## Results

| | Private (1 hop) | Anonymous (3 hops) |
| --- | --- | --- |
| time to first peer (tracker reply) | 21 s | 79 s |
| time to the first byte of content | 169 s | 198 s |
| time to complete 512 MiB | **2,292 s (38 min)** | **10,771 s (3 h 0 min)** |
| mean throughput | **229 KiB/s** | **49 KiB/s** |
| peak download rate (5 s samples) | 1,826 KiB/s | 724 KiB/s |
| instance CPU | 31 s | 87 s |
| instance peak RSS | 518 MiB (includes the mapped file) | 158 MiB |
| sockets of the instance at the end | 3, all to `127.0.0.1:7756` (SAM) | 2, all to `127.0.0.1:7756` |
| verification | L1 per block, L2 commitment matched, sealed | the same |

For comparison, the same host's Direct seeder delivered 1.8 GB to a Direct client on the same host
in 2 min 31 s (the RFC-0001 end-to-end test).

**Router traffic.** Over the client router's 3 h 40 min:
- **Volume.** It received 4.23 GiB, sent 3.28 GiB, and carried 2.88 GiB of transit for other I2P users.
- **What this shows.** Each payload byte costs relaying elsewhere in the network, and every MISAKA
  router relays in turn (RFC-0002 §6). The counters include traffic unrelated to MISAKA, so they show
  the scale, not a per-byte factor.

Raw data: `private-20261003T085708Z/` and `anonymous-20261003T093523Z/`. Each run has:
- `progress.csv`: a sample every 5 s;
- `summary.txt`;
- the router counters at the start and the end;
- the instance's sockets;
- its configuration.

## Findings

1. **libtorrent 2.0.15 cannot fetch a v2-only torrent over I2P.**
   - **Cause.** Peers learned from an I2P tracker are added without the v2 flag, so the client sends
     its handshake with the torrent's v1 info-hash. A v2-only torrent has no v1 info-hash, so every
     I2P peer drops the connection, and the download never starts. Every MISAKA bundle is v2-only
     (rule v1).
   - **Patch.** `third_party/patches/libtorrent-2.0.15-i2p-v2-peers.patch` marks such peers v2 when
     the torrent has no v1 hash, as libtorrent already does for peers with an IP address.
   - **Upstream.** No fix was found in the recent history of upstream's `RC_2_0` branch. The patch
     should be proposed upstream.
2. **The MP-4 self-check needs UDP, not TCP.**
   - **Why.** systemd's `IPAddressDeny` drops packets in a cgroup filter. A TCP connect to a clearnet
     address times out exactly as it does on an open network, so it cannot tell the two apart.
   - **What works.** A UDP send fails with `EPERM` when filtered, and succeeds when not.
   - **The check now.** `misaka-torrentd` sends an empty datagram to the documentation addresses
     192.0.2.1 and 2001:db8::1, and refuses an I2P mode unless both sends fail. Verified: refused
     outside the unit, accepted inside it.
3. **An I2P-mode unit needs `AF_NETLINK`.**
   - **What went wrong.** The first seeder unit dropped `AF_NETLINK` from `RestrictAddressFamilies`.
     libtorrent then could not enumerate interfaces: the session never started its torrents and
     posted no alerts.
   - **Why it is safe to allow.** Netlink is local to the kernel and does not reach the clearnet.
   - **Follow-up.** This belongs in the deployment notes for an I2P instance.
4. **No clearnet sockets.** In both runs the instance's only sockets were to the SAM bridge on
   loopback, and the daemon refused to start an I2P mode outside the IP filter. A packet capture of
   the instance (D-P1 proper) is still to do.
5. **Anonymous is about 4.7× slower than Private** with one seeder (49 against 229 KiB/s mean), and
   its rate collapsed to around 10 KiB/s for long stretches. Private reached 1.8 MiB/s at peak. With
   one seeder, the transfer rides a handful of tunnels. More seeders are the lever RFC-0002 §6 relies
   on, and they were not tested.

## Caveats

- **One run per mode, one seeder.** Both routers ran on one host with one IP address, while a fuzzing
  job and other services were running.
- **The client router was young** (2 minutes of uptime) when the Private run started. It had been
  restarted earlier; a router integrates over tens of minutes.
- **Not done yet:**
  - the 100 GB-class transfer: the host has 35 GB free;
  - 2 and 4 seeders;
  - recovery after a restart;
  - D-P1 by packet capture.

## What this suggests for RFC-0002 (not yet decided)

- **Private (1 hop)** is usable for models of a few GB today: about 13 MiB/min with one seeder.
- **Anonymous (3 hops)** with one seeder is suitable for small bundles only: about 3 MiB/min. It
  needs many seeders, measured, before the app offers it for a 38 GB model.
- **The libtorrent patch** is required for either I2P mode.
