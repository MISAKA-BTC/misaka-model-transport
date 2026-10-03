# misakaoptions.com: "Download with MISAKA Torrent"

The site's side of RFC-0001 §8.1, before Part B exists. The chain does not declare distributions
yet (`ModelDistributionDeclared` is a proposal to misakas), so the site's own release names each
bundle in `torrents/index.json`. That is the release-scoped pointer of §3.5 and §8.4. The page labels
it "declared by the site's release" and never presents it as a chain fact.

| file | what |
| --- | --- |
| `torrent-downloads.patch` | the change to `web/misaka-options` (`app.js`, `style.css`, `index.html`), against the copy deployed on 2026-10-03. That copy is newer than misakas `main`: apply it to the deployed source, or rebase it, before a PR |
| `make_release.py` | builds `downloads/misaka-torrent/latest.json` and `SHA256SUMS` for the Download tab from the files in `downloads/misaka-torrent/<version>/` |
| `release.latest.json` | the release list deployed on 2026-10-03 (0.1.0: macOS arm64 .dmg, Linux .deb and .AppImage, two CLI archives) |
| `make_index.py` | builds `torrents/index.json` from bundle directories with `misaka-torrent inspect` |
| `index.testnet-12.json` | the index deployed on 2026-10-03 |
| `github-theme.css` | the whole site in GitHub's look: Primer dark tokens and Primer-style header, buttons, boxes, tables, labels, tabs, forms, flashes and toasts, over the site's existing markup |
| `primer-hub.css`, `primer/` | the deployed hub stylesheet, its build script (`build.mjs`, `hub-extra.css`), and the Primer and Octicons licenses |

What the page does:
- **Download tab** (`#/download`): MISAKA Model Transport served from this site, not GitHub. It highlights the build for the visitor's OS and lists every file with its size and SHA-256, plus install steps (including the unsigned-build step on macOS). The model page's Download menu leads with **Download with MISAKA**, which opens a `misaka-model://` link; if no app takes the focus, a toast points to the Download tab.
- **Model hub** (`#/models`, now the landing page), laid out like a repository page on GitHub and built on Primer CSS 21.5.1 and Octicons (both MIT) in Primer's dark theme:
  - **List**: a Primer `Box` list with search and the Task, Format, Sort and Downloadable filters.
  - **Model page** (`#/models/<owner>/<name>[/tree/<tab>]`): the tabs Model card (the bundle's `README.md`), Files, Versions, Swarm, Community (a placeholder) and Access (links the existing store). An About sidebar, and a green Download menu in the style of GitHub's "Code" button.
  - **Styles**: `primer-hub.css` is Primer purged to the classes the hub uses, scoped to `.hub`, so no other page changes. Rebuild it with `primer/build.mjs`.
  - **Scope**: no GitHub code, logo or name is used. Lines with no bundle get the slug `line/<symbol>`.
- **Swarm figures**: `torrents/swarm.json`, rewritten every minute by `misaka-torrent-swarm-json.timer` (`swarm-json.sh`). They show only what this site's own seeder sees; the page says so.
- **Models table** (`#/lines`, `#/leaderboard`): a Download column, with `.torrent` and `magnet` links, for each line the index lists.
- **Store page** (`#/store/<id>`, the page a buyer lands on): a "⇩ Download model (.torrent / magnet)" button at the top that scrolls to the same panel, which sits under the buy box.
- **Model page** (`#/line/<id>`): a "Download with MISAKA Torrent" panel. It shows the bundle, its size and files, the v2 infohash, the bundle commitment, and the inventory root, so it can be compared with "Current root" on the same page. It also has the `.torrent`, the magnet, and the verifying command `misaka-torrent pull '<magnet>' --expect <commitment> --kind <kind>`.

## Deployed on misakaoptions.com's server, 2026-10-03

| piece | where |
| --- | --- |
| site | `/var/www/misakaoptions/{app.js,style.css,index.html}`, `/var/www/misakaoptions/torrents/` |
| rollback | `/var/www/misakaoptions.bak-20261003-torrent` (the whole directory as it was) |
| seeder | `misaka-torrentd.service` (`deploy/systemd`), user `misaka_torrent`, config `/etc/misaka/torrent.toml`, state `/var/lib/misaka-torrent`, peer port 55036 TCP/UDP |
| bundle | `/srv/misaka-bundles/qwen25-1.5b-a16-8k/`: hard links to the host's `qwen25-1.5b-a16-8k.palwart{,.palwmanifest}` (no extra disk), plus `LICENSE`, `README.md` and `misaka-bundle.json` |
| binaries | `/usr/local/bin/misaka-torrent{,d}`, linked to libtorrent 2.0.15 in `/opt/misaka-torrent/lt` (rpath) |
| build tree | `/opt/misaka-torrent/{repo,target,src}` |

Adding a bundle:

```bash
cd /srv/misaka-bundles && mkdir <title> && ln <artifact> <title>/ && cp LICENSE README.md <title>/
misaka-torrent create <title> --kind palw-artifact --license <spdx> --palw-root <class_id>:<inventory_root>
MISAKA_TORRENT_PROFILE=server misaka-torrent seed adopt /srv/misaka-bundles/<title> --expect <infohash>
python3 make_index.py --network testnet-12 /srv/misaka-bundles/*/ > /var/www/misakaoptions/torrents/index.json
cp /srv/misaka-bundles/*.torrent /var/www/misakaoptions/torrents/
```

Rolling the site back (before the hub, or before any torrent change):

```bash
rsync -a --delete /var/www/misakaoptions.bak-20261003-pregh/ /var/www/misakaoptions/   # before the site-wide GitHub theme
rsync -a --delete /var/www/misakaoptions.bak-20261003-preprimer/ /var/www/misakaoptions/   # before Primer
rsync -a --delete /var/www/misakaoptions.bak-20261003-prehub/ /var/www/misakaoptions/
rsync -a --delete /var/www/misakaoptions.bak-20261003-torrent/ /var/www/misakaoptions/
```

The seeder's address is visible to every peer of its swarm. RFC-0001 §Security and RFC-0002 MP-12 apply to any host that also runs validator seats: such a host does not seed unless its operator decides so.
