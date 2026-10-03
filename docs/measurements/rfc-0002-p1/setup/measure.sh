#!/bin/bash
# RFC-0002 P1 (D-P5): fetch the measurement bundle over I2P in one mode, from a confined instance on
# router "client", and record time to first peer, time to complete, throughput, CPU and RSS.
set -u
MODE=$1                                   # private | anonymous
IH=d4c56b97f96ef008f12de4b9ec9788822cb80d8ce73c33e9eca4bd8ae02cfdf3
COMMIT=391ef0a4aa18b9fe121a6b96b1c70b77bd79f3f6073774508eca6301283c389b1fd240c12f03b63d4554e7ced668bc033746503e6792b975c1de86ea5935378d
T=http://wcetu2kcbbawy5oofbiz5cft4nsolt4fndvwrpuwpwz2tyn2ftoq.b32.i2p/announce
B=/opt/misaka-torrent/p1/bin; R=/opt/misaka-torrent/p1/results; TS=$(date -u +%Y%m%dT%H%M%SZ)
H=/var/lib/mt-p1-client-$MODE; S=/run/mt-p1-client-$MODE/s.sock; UNIT=mt-p1-client-$MODE
OUT=$R/$MODE-$TS; mkdir -p $OUT
systemctl stop $UNIT 2>/dev/null; chmod -R u+w $H 2>/dev/null; rm -rf $H; mkdir -p $H /run/mt-p1-client-$MODE
cat > $OUT/torrent.toml <<CFG
profile = "server"
[seeding]
enabled = false
[network]
mode = "$MODE"
[i2p]
sam = "127.0.0.1:7756"
trackers = ["$T"]
[ipc]
allowed_uids = [0]
CFG
router() { curl -s http://127.0.0.1:$1/ | sed "s/<[^>]*>/ /g; s/&nbsp;/ /g" | grep -E "Uptime|Received|Sent|Transit:|Tunnel creation|Routers" | tr -s " " | tr "\n" ";"; }
echo "seed router: $(router 7070)" > $OUT/routers-start.txt
echo "client router: $(router 7170)" >> $OUT/routers-start.txt
systemd-run --quiet --unit=$UNIT -p IPAddressDeny=any -p IPAddressAllow=localhost -p MemoryMax=2G \
  $B/misaka-torrentd run --profile server --config $OUT/torrent.toml --home $H --socket $S
for i in $(seq 1 100); do [ -S $S ] && break; sleep 0.2; done
journalctl -u $UNIT --no-pager -o cat | head -3 > $OUT/daemon-start.txt
PID=$(systemctl show -p MainPID --value $UNIT)
T0=$(date +%s.%N)
$B/misaka-torrent --socket $S pull "magnet:?xt=urn:btmh:1220$IH&dn=p1-measure-512m" --expect $COMMIT --kind source-weights --no-seed --no-wait > /dev/null
echo "t,state,done_bytes,down_rate,peers,seeds,rss_kb,cpu_s" > $OUT/progress.csv
FIRST=""; DONE=""
while :; do
  NOW=$(date +%s.%N); EL=$(echo "$NOW - $T0" | bc)
  J=$($B/misaka-torrent --socket $S status --json 2>/dev/null)
  L=$(echo "$J" | python3 -c "import json,sys; b=[x for x in json.load(sys.stdin) if x[\"infohash\"]==\"$IH\"]; b=b[0] if b else {}; print(b.get(\"state\",\"?\"), b.get(\"done_bytes\",0), b.get(\"download_rate\",0), b.get(\"peers\",0), b.get(\"seeds\",0))")
  set -- $L; ST=$1; DB=$2; DR=$3; PE=$4; SE=$5
  RSS=$(ps -o rss= -p $PID 2>/dev/null | tr -d " "); CPU=$(ps -o cputimes= -p $PID 2>/dev/null | tr -d " ")
  echo "$EL,$ST,$DB,$DR,$PE,$SE,$RSS,$CPU" >> $OUT/progress.csv
  [ -z "$FIRST" ] && [ "$PE" != "0" ] && FIRST=$EL
  case $ST in idle|seeding|sealed) DONE=$EL; break;; refused|mismatch|failed) DONE="FAILED:$ST"; break;; esac
  [ $(echo "$EL > 10800" | bc) = 1 ] && { DONE="TIMEOUT"; break; }
  sleep 5
done
echo "seed router: $(router 7070)" > $OUT/routers-end.txt
echo "client router: $(router 7170)" >> $OUT/routers-end.txt
ss -tnp | grep "pid=$PID," | awk "{print \$5}" | sort | uniq -c > $OUT/connections.txt
SIZE=536883325
python3 - "$OUT" "$FIRST" "$DONE" "$SIZE" "$MODE" <<PY
import sys,csv
out,first,done,size,mode=sys.argv[1],sys.argv[2],sys.argv[3],int(sys.argv[4]),sys.argv[5]
rows=list(csv.DictReader(open(out+"/progress.csv")))
peak=max((int(r["down_rate"] or 0) for r in rows), default=0)
rss=max((int(r["rss_kb"] or 0) for r in rows if r["rss_kb"]), default=0)
cpu=rows[-1]["cpu_s"] if rows else "?"
s=[f"mode {mode}", f"bundle {size} bytes (512 MiB, random)", f"time to first peer: {first or never} s", f"time to complete: {done} s"]
try:
    d=float(done); s.append(f"mean throughput: {size/d/1024:.1f} KiB/s")
except ValueError: pass
s+= [f"peak download rate: {peak/1024:.1f} KiB/s", f"instance peak RSS: {rss/1024:.1f} MiB", f"instance CPU: {cpu} s"]
open(out+"/summary.txt","w").write("\n".join(s)+"\n"); print("\n".join(s))
PY
systemctl stop $UNIT
