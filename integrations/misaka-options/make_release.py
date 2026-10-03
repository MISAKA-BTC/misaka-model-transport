#!/usr/bin/env python3
"""Writes downloads/misaka-torrent/latest.json and <version>/SHA256SUMS for the site's Download tab.

usage: make_release.py --version 0.1.0 --dir /var/www/misakaoptions/downloads/misaka-torrent

<dir>/<version>/ holds the release files. Each one is described from its name:
  *_macos-arm64.dmg, *_amd64.deb, *_amd64.AppImage, misaka-torrent-cli-*-<os>-<arch>.tar.gz
"""
import argparse, hashlib, json, os, re, time

KINDS = [
    (r"_macos-arm64\.dmg$", dict(os="macos", arch="arm64", kind="app", label="macOS · Apple silicon", requires="macOS 12 or later, Apple silicon", primary=True)),
    (r"_amd64\.deb$", dict(os="linux", arch="x86_64", kind="app", label="Linux · .deb (x86_64)", requires="Ubuntu 24.04 / Debian 13 or later, x86_64", primary=True)),
    (r"_amd64\.AppImage$", dict(os="linux", arch="x86_64", kind="app", label="Linux · AppImage (x86_64)", requires="glibc 2.39 or later, x86_64")),
    (r"-macos-arm64\.tar\.gz$", dict(os="macos", arch="arm64", kind="cli", label="CLI · macOS Apple silicon", requires="macOS 12 or later")),
    (r"-linux-x86_64\.tar\.gz$", dict(os="linux", arch="x86_64", kind="cli", label="CLI · Linux x86_64", requires="glibc 2.39 or later")),
]

def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", required=True)
    ap.add_argument("--dir", required=True)
    ap.add_argument("--base", default="downloads/misaka-torrent")
    a = ap.parse_args()
    vdir = os.path.join(a.dir, a.version)
    assets, sums = [], []
    for name in sorted(os.listdir(vdir)):
        meta = next((m for pat, m in KINDS if re.search(pat, name)), None)
        if not meta:
            continue
        path = os.path.join(vdir, name)
        digest = sha256(path)
        sums.append(f"{digest}  {name}\n")
        assets.append(dict(meta, name=name, size=os.path.getsize(path), sha256=digest, url=f"{a.base}/{a.version}/{name}"))
    order = {"app": 0, "cli": 1}
    assets.sort(key=lambda x: (order[x["kind"]], x["os"], x["name"]))
    open(os.path.join(vdir, "SHA256SUMS"), "w").writelines(sums)
    rel = {
        "schema": "misaka/app-release/v1",
        "product": "MISAKA Model Transport",
        "version": a.version,
        "date": time.strftime("%Y-%m-%d"),
        "sums": f"{a.base}/{a.version}/SHA256SUMS",
        "assets": assets,
    }
    tmp = os.path.join(a.dir, "latest.json.tmp")
    with open(tmp, "w") as f:
        json.dump(rel, f, indent=2)
        f.write("\n")
    os.replace(tmp, os.path.join(a.dir, "latest.json"))
    print(f"{len(assets)} assets")

if __name__ == "__main__":
    main()
