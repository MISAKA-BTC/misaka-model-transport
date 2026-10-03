#!/bin/bash
# Publishes this host seeder's view of each swarm for misakaoptions.com (Swarm tab). Server paths stripped.
set -euo pipefail
out=/var/www/misakaoptions/torrents/swarm.json
MISAKA_TORRENT_PROFILE=server /usr/local/bin/misaka-torrent status --json | python3 -c "
import json,sys,time
keep=(\"infohash\",\"state\",\"label\",\"total_bytes\",\"done_bytes\",\"uploaded\",\"upload_rate\",\"peers\",\"seeds\")
r=[{k:b.get(k) for k in keep} for b in json.load(sys.stdin)]
json.dump({\"schema\":\"misaka/torrent-swarm/v1\",\"seeder\":\"misakaoptions.com\",\"updated\":int(time.time()),\"bundles\":r},sys.stdout)
" > "$out.tmp"
chmod 644 "$out.tmp"; mv "$out.tmp" "$out"
