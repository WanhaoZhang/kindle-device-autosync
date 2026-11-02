#!/bin/sh
# Adopt a previously uploaded and truly verified sample; never resend it.
set -eu
umask 077
PATH=/sbin:/usr/sbin:/bin:/usr/bin
export PATH
PACKAGE=/mnt/us/kindle-native-sync-package
ROOT=/var/local/kindle-native-sync
REVIEW=/mnt/us/.kindle-native-sync-upgrade-review
OUT=/mnt/us/.kindle-native-sync-export
. "$PACKAGE/install/common.sh"
require_device
[ ! -L "$OUT" ] && [ ! -L "$REVIEW" ] || exit 1
mkdir -p "$OUT"
[ ! -L "$OUT/activate-verified.log" ] && [ ! -L "$OUT/activation-result.json" ] || exit 1
exec > "$OUT/activate-verified.log" 2>&1
(cd "$PACKAGE" && sha256sum -c SHA256SUMS) >/dev/null
(cd "$REVIEW" && sha256sum -c SHA256SUMS) >/dev/null
[ "$(sha256sum "$ROOT/bin/ksync-worker" | awk '{print $1}')" = "$(sha256sum "$PACKAGE/bin/ksync-worker" | awk '{print $1}')" ] || exit 1
# Wait using actual collector observations. A sleep never creates reader idle.
count=0
while :;do
    ready=false
    if "$ROOT/bin/ksync-worker" --root "$ROOT" status > "$OUT/activation-status.json" 2> "$OUT/activation-status-error.txt";then
        now=$(date +%s)
        if awk -F '"' -v at="$now" '
          /^    "reader_state": / {reader=$4}
          /^    "power_state": / {power=$4}
          /^    "network_connected": true/ {network=1}
          /^    "usb_export": false/ {usb=1}
          /^    "capture_blocked": false/ {storage=1}
          /^    "coverage_gap": false/ {coverage=1}
          /^    "idle_since_unix": / {s=$0;sub(/^[^:]*:[ ]*/,"",s);gsub(/[, ]/,"",s);idle=s}
          END {if(reader!="idle"||power!="awake"||!network||!usb||!storage||!coverage||idle<=0||at-idle<25)exit 1}
        ' "$OUT/activation-status.json";then ready=true;fi
    elif ! grep -Fxq 'worker stopped: worker already running' "$OUT/activation-status-error.txt";then
        cat "$OUT/activation-status-error.txt";exit 1
    fi
    [ "$ready" != true ] || break
    count=$((count+1));[ "$count" -lt 30 ] || { echo 'Device did not become truly idle/awake/connected.';exit 1; }
    sleep 3
done
# No new trial authorization here: the original execution proof must survive.
# Only a failed top-level Store lock may retry, before the operation ran at all.
count=0
result=1
while :;do
    if "$ROOT/bin/ksync-worker" --root "$ROOT" operate --inbox "$REVIEW" --action adopt-and-enable > "$OUT/activation-result.json.partial" 2> "$OUT/activation-error.txt";then result=0;break;fi
    count=$((count+1))
    if [ "$count" -lt 10 ] && grep -Fxq 'worker stopped: worker already running' "$OUT/activation-error.txt";then sleep 3;else break;fi
done
mv "$OUT/activation-result.json.partial" "$OUT/activation-result.json"
cat "$OUT/activation-error.txt"
# The manual operation is not a collector child. Notify only the fixed hint
# reader; it cannot synthesize network/power/reader or clear any safety flag.
"$ROOT/bin/ksync-collector" --root "$ROOT" --emit 'maintenance check-readonly-hint' || :
sh "$ROOT/install/control.sh" export || :
sync
exit "$result"
