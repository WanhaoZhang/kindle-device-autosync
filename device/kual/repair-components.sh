#!/bin/sh
# Explicit, narrow repair for the configured preview. Never changes account data.
set -eu
umask 077
PATH=/sbin:/usr/sbin:/bin:/usr/bin
export PATH
PACKAGE=/mnt/us/kindle-native-sync-package
ROOT=/var/local/kindle-native-sync
OUT=/mnt/us/.kindle-native-sync-export
. "$PACKAGE/install/common.sh"
require_device
[ "$(lipc-get-prop com.lab126.volumd userstoreIsAvailable 2>/dev/null)" = 1 ] || exit 1
[ ! -L "$OUT" ] || exit 1
mkdir -p "$OUT"
[ ! -L "$OUT/component-repair.log" ] && [ ! -L "$OUT/component-repair-result.json" ] || exit 1
exec > "$OUT/component-repair.log" 2>&1
date -u
(cd "$PACKAGE" && sha256sum -c SHA256SUMS) >/dev/null
expected=$(sha256sum "$PACKAGE/bin/ksync-worker" | awk '{print $1}')
actual=$(sha256sum "$ROOT/bin/ksync-worker" | awk '{print $1}')
[ "$expected" = "$actual" ] || { echo 'Worker changed; use a reviewed complete upgrade.';exit 1; }
SNAP=$OUT/private-repair-$(date -u +%Y%m%dT%H%M%S)-$$
mkdir "$SNAP" "$SNAP/sealed" "$SNAP/components"
ls -ldn "$ROOT" "$ROOT/config.json" "$ROOT/auth.json" "$ROOT/control.fifo" "$ROOT/adapter-control.fifo" > "$SNAP/permissions-before.txt" 2>&1 || :
"$PACKAGE/bin/ksync-collector" --root "$ROOT" --repair-private-state
[ ! -e "$ROOT/.component-repair" ] && [ ! -L "$ROOT/.component-repair" ] || { echo 'Unresolved component repair; retain recovery.';exit 1; }
[ ! -e "$ROOT/.preview-upgrade" ] && [ ! -L "$ROOT/.preview-upgrade" ] && [ ! -e "$ROOT/.install-transaction" ] && [ ! -L "$ROOT/.install-transaction" ] || exit 1
enabled=1
[ ! -e "$ROOT/disabled" ] || enabled=0
: > "$ROOT/disabled"
stop kindle-native-sync-events >/dev/null 2>&1 || :
stop kindle-native-sync >/dev/null 2>&1 || :
for job in kindle-native-sync-events kindle-native-sync;do
    if initctl status "$job" 2>/dev/null | grep -q start/running;then echo 'Service did not stop.';exit 1;fi
done
"$PACKAGE/bin/ksync-collector" --root "$ROOT" --repair-private-state
for name in collector.checkpoint runtime.json;do
    [ ! -f "$ROOT/$name" ] || cp "$ROOT/$name" "$SNAP/$name"
done
for file in "$ROOT/events/sealed/"segment-*.log;do
    [ -f "$file" ] && [ ! -L "$file" ] || continue
    cp "$file" "$SNAP/sealed/${file##*/}"
    [ "$(sha256sum "$file" | awk '{print $1}')" = "$(sha256sum "$SNAP/sealed/${file##*/}" | awk '{print $1}')" ] || exit 1
done
ls -ldn /var/log/messages* /var/local/log/messages* > "$SNAP/source-file-metadata.txt" 2>&1 || :
for file in /etc/upstart/*.conf /etc/init.d/*;do
    [ -f "$file" ] && [ ! -L "$file" ] || continue
    if grep -n -E '(var/local|chmod|chown|framework)' "$file";then printf 'SOURCE:%s\n' "$file";fi
done > "$SNAP/boot-permission-source.txt" 2>/dev/null
for name in bin/ksync-collector bin/ksync-event-adapter install/common.sh install/control.sh package-manifest.json;do
    [ -f "$ROOT/$name" ] && [ ! -L "$ROOT/$name" ] || exit 1
    safe=$(printf '%s' "$name" | tr / _)
    cp "$ROOT/$name" "$SNAP/components/$safe"
    digest=$(sha256sum "$ROOT/$name" | awk '{print $1}')
    [ "$digest" = "$(sha256sum "$SNAP/components/$safe" | awk '{print $1}')" ] || exit 1
    printf '%s  %s\n' "$digest" "$safe" >> "$SNAP/components/SHA256SUMS"
done
printf '%s\n' "$SNAP" > "$ROOT/.component-repair"
changed=1
cleanup() {
    code=$?
    trap - EXIT HUP INT TERM
    set +e
    if [ "$changed" = 1 ];then
        (cd "$SNAP/components" && sha256sum -c SHA256SUMS) >/dev/null || { echo 'Recovery checksum failed; collection disabled; preserve evidence.';exit 1; }
        for name in bin/ksync-collector bin/ksync-event-adapter install/common.sh install/control.sh package-manifest.json;do
            safe=$(printf '%s' "$name" | tr / _)
            cp "$SNAP/components/$safe" "$ROOT/$name"
        done
        chmod 700 "$ROOT/bin/ksync-collector" "$ROOT/bin/ksync-event-adapter" "$ROOT/install/common.sh" "$ROOT/install/control.sh"
        echo 'Component repair incomplete; collection disabled; private data retained.'
    fi
    sync
    exit "$code"
}
trap cleanup EXIT
trap 'exit 129' HUP;trap 'exit 130' INT;trap 'exit 143' TERM
for name in bin/ksync-collector bin/ksync-event-adapter install/common.sh install/control.sh;do
    [ ! -e "$ROOT/$name.repair" ] && [ ! -L "$ROOT/$name.repair" ] || exit 1
    cp "$PACKAGE/$name" "$ROOT/$name.repair"
    chmod 700 "$ROOT/$name.repair"
    [ "$(sha256sum "$PACKAGE/$name" | awk '{print $1}')" = "$(sha256sum "$ROOT/$name.repair" | awk '{print $1}')" ] || exit 1
    mv "$ROOT/$name.repair" "$ROOT/$name"
done
[ ! -e "$ROOT/package-manifest.json.repair" ] && [ ! -L "$ROOT/package-manifest.json.repair" ] || exit 1
cp "$PACKAGE/manifest.json" "$ROOT/package-manifest.json.repair"
chmod 600 "$ROOT/package-manifest.json.repair"
mv "$ROOT/package-manifest.json.repair" "$ROOT/package-manifest.json"
"$ROOT/bin/ksync-collector" --root "$ROOT" --repair-private-state
changed=0
rm "$ROOT/.component-repair"
if [ "$enabled" = 1 ];then sh "$ROOT/install/control.sh" enable;fi
sleep 25
sh /mnt/us/extensions/kindle-native-sync/bootstrap.sh authorize-prepare
sh "$ROOT/install/control.sh" export
printf '{"schema_version":1,"state":"components_repaired","worker_unchanged":true,"account_time_write_performed":false}\n' > "$OUT/component-repair-result.json"
sync
