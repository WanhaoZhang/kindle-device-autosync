#!/bin/sh
# Explicit local diagnostic. No auth/config/body contents and no network calls.
set -eu
umask 077
PATH=/sbin:/usr/sbin:/bin:/usr/bin
export PATH
ROOT=/var/local/kindle-native-sync
OUT=/mnt/us/.kindle-native-sync-export
[ "$(id -u)" = 0 ] && [ ! -L "$ROOT" ] && [ -d "$ROOT" ] || exit 1
[ "$(lipc-get-prop com.lab126.volumd userstoreIsAvailable 2>/dev/null)" = 1 ] || exit 1
[ ! -L "$OUT" ] || exit 1
mkdir -p "$OUT"
SNAP=$OUT/private-audit-$(date -u +%Y%m%dT%H%M%S)-$$
mkdir "$SNAP" "$SNAP/sealed" "$SNAP/parsed"
"$ROOT/bin/ksync-worker" --root "$ROOT" export --out "$SNAP/parsed"
count=0
for file in "$ROOT/events/sealed/"segment-*.log;do
    [ -e "$file" ] || continue
    [ -f "$file" ] && [ ! -L "$file" ] || exit 1
    base=${file##*/}
    case "$base" in *[!0-9a-z.-]*) exit 1;; esac
    count=$((count+1));[ "$count" -le 512 ] || exit 1
    cp "$file" "$SNAP/sealed/$base"
    digest=$(sha256sum "$file" | awk '{print $1}')
    [ "$digest" = "$(sha256sum "$SNAP/sealed/$base" | awk '{print $1}')" ] || exit 1
    printf '%s  sealed/%s\n' "$digest" "$base" >> "$SNAP/SHA256SUMS"
done
for name in collector.checkpoint runtime.json;do
    [ -f "$ROOT/$name" ] && [ ! -L "$ROOT/$name" ] || exit 1
    cp "$ROOT/$name" "$SNAP/$name"
done
printf '{"schema_version":1,"state":"audit_exported","sealed_files":%s,"credentials_exported":false,"network_write":false}\n' "$count"
sync
