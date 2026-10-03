#!/usr/bin/env python3
"""Builds misakaoptions.com's torrents/index.json from bundle directories (RFC-0001 §3.5, §8.4).

Until Part B (ModelDistributionDeclared) is armed, the site's own release names each bundle: this
index is that release-scoped pointer. It is the release's word, not a chain fact, and the page says
so. Every byte is still checked by the client: BEP 52 per block, the descriptor commitment at L2,
and the node's digest check at load.

usage: make_index.py --network testnet-12 [--line-id <128 hex>] <bundle-dir> [...] > index.json

Each <bundle-dir> holds misaka-bundle.json (as `misaka-torrent create` leaves it) and has
<bundle-dir>.torrent beside it. A palw-artifact bundle's line defaults to its class's founding line,
whose line id is the class id.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import time


def inspect(exe, d):
    out = subprocess.run([exe, "inspect", d], check=True, capture_output=True, text=True).stdout
    field = lambda k: re.search(r"^%s\s+(.+)$" % k, out, re.M).group(1).strip()
    return {"infohash": field("infohash"), "commitment": field("commitment"), "piece": field("piece")}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--network", required=True)
    ap.add_argument("--line-id", action="append", default=[], help="one per bundle, in order")
    ap.add_argument("--exe", default=os.environ.get("MISAKA_TORRENT", "misaka-torrent"))
    ap.add_argument("--namespace", default="misaka", help="the hub's owner namespace for these bundles")
    ap.add_argument("--task", action="append", default=[], help="pipeline task, one per bundle, e.g. text-generation")
    ap.add_argument("--params", action="append", default=[], help="parameter count, one per bundle, e.g. 1.5B")
    ap.add_argument("--version", action="append", type=int, default=[], help="the line version each bundle carries (default 1)")
    ap.add_argument("dirs", nargs="+")
    a = ap.parse_args()
    bundles = []
    for i, d in enumerate(a.dirs):
        d = os.path.abspath(d)
        desc_bytes = open(os.path.join(d, "misaka-bundle.json"), "rb").read()
        desc = json.loads(desc_bytes)
        info = inspect(a.exe, d)
        container = next((f for f in desc["files"] if f["role"] == "palw-container"), None)
        class_id = container["palw"]["roots"][0]["class_id"] if container else None
        line_id = a.line_id[i] if i < len(a.line_id) else class_id
        if not line_id:
            sys.exit(f"{d}: give --line-id for a bundle with no PALW container")
        title = desc["title"]
        readme = None
        if any(f["path"] == "README.md" for f in desc["files"]):
            readme = open(os.path.join(d, "README.md"), encoding="utf-8", errors="replace").read()[:65536]
        base_model, variant = None, None
        man = next((f for f in desc["files"] if f["path"].endswith(".palwmanifest")), None)
        if man:
            m = json.load(open(os.path.join(d, man["path"])))
            model_id = (m.get("classes") or [{}])[0].get("model_id", "")
            parts = model_id.split("/")
            if len(parts) >= 2:
                base_model = "/".join(parts[:2])
                rest = "/".join(parts[2:])
                if rest:
                    graph, _, ctx = rest.partition("@")
                    variant = graph + (" · n_ctx " + ctx if ctx else "")
        created = time.strftime("%Y-%m-%d", time.gmtime(os.path.getmtime(d + ".torrent")))
        total = sum(f["size"] for f in desc["files"]) + len(desc_bytes)
        bundles.append({
            "line_id": line_id,
            "repo": a.namespace + "/" + title,
            "version": a.version[i] if i < len(a.version) else 1,
            "task": a.task[i] if i < len(a.task) else None,
            "params": a.params[i] if i < len(a.params) else None,
            "base_model": base_model,
            "variant": variant,
            "created": created,
            "readme": readme,
            # The canonical descriptor: lets a client adopt files obtained elsewhere (same names and
            # sizes) by writing it beside them; the daemon then re-hashes everything.
            "descriptor": desc_bytes.decode("utf-8"),
            "class_id": class_id,
            "kind": desc["kind"],
            "title": title,
            "btv2_infohash": info["infohash"],
            "bundle_commitment": info["commitment"],
            "total_bytes": total,
            "piece_length": info["piece"],
            "license": desc["license"]["spdx"],
            "torrent": f"torrents/{title}.torrent",
            "files": [{"path": f["path"], "size": f["size"], "role": f["role"], "sha256": f["sha256"]} for f in desc["files"]],
            "palw": {
                "magic": container["palw"]["magic"],
                "roots": container["palw"]["roots"],
                "artifact_digest": container["palw"].get("artifact_digest"),
            } if container else None,
        })
    json.dump({"schema": "misaka/torrent-index/v1", "network": a.network, "source": "release", "bundles": bundles},
              sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
