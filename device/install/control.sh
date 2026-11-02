#!/bin/sh
set -eu
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
. "$HERE/common.sh"
require_device
case "${1:-status}" in
 enable)
    [ ! -e "$ROOT/.preview-upgrade" ] && [ ! -e "$ROOT/.install-transaction" ] || { echo 'Resolve the interrupted transaction before enabling collection.' >&2;exit 1; }
    rm -f "$ROOT/disabled"
    for job in kindle-native-sync kindle-native-sync-events;do
        if ! initctl status "$job" 2>/dev/null | grep -q 'start/running';then start "$job" >/dev/null 2>&1 || :;fi
        initctl status "$job" 2>/dev/null | grep -q 'start/running' || { echo 'Collection job did not start.' >&2;exit 1; }
    done;;
 disable)
    umask 077;mkdir -p "$ROOT";: > "$ROOT/disabled"
    stop kindle-native-sync-events >/dev/null 2>&1 || :
    stop kindle-native-sync >/dev/null 2>&1 || :
    for job in kindle-native-sync-events kindle-native-sync;do
        if initctl status "$job" 2>/dev/null | grep -q 'start/running';then echo 'Collection job did not stop.' >&2;exit 1;fi
    done;;
 status) "$ROOT/bin/ksync-worker" --root "$ROOT" status;;
 import-config)
    "$ROOT/bin/ksync-worker" --root "$ROOT" import-config --inbox /mnt/us/.kindle-native-sync-inbox;;
 export)
    # Export only sanitized status. Never copy auth, config, raw logs or ledger into FAT.
    [ "$(lipc-get-prop com.lab126.volumd userstoreIsAvailable 2>/dev/null)" = 1 ] || exit 1
    out=/mnt/us/.kindle-native-sync-export
     [ ! -L "$out" ] || exit 1
    mkdir -p "$out"
     for name in probe.json service-probe.json status.json;do
         [ ! -L "$out/$name" ] && [ ! -L "$out/$name.partial" ] || exit 1
     done
    "$HERE/probe.sh" > "$out/probe.json.partial"
     sh "$HERE/service-probe.sh" > "$out/service-probe.json.partial"
    mv "$out/probe.json.partial" "$out/probe.json"
     mv "$out/service-probe.json.partial" "$out/service-probe.json"
     # Keep low-space / busy diagnostics available even if Store::open is blocked.
     if "$ROOT/bin/ksync-worker" --root "$ROOT" status > "$out/status.json.partial";then :
     else
         code=$?
         printf '{"schema_version":1,"state":"worker_status_unavailable","worker_exit_code":%s,"auto_sync_enabled":false}\n' "$code" > "$out/status.json.partial"
     fi
     mv "$out/status.json.partial" "$out/status.json"
     sync;;
 uninstall) exec "$HERE/uninstall.sh";;
 *) echo 'Usage: control.sh enable|disable|status|import-config|export|uninstall' >&2;exit 2;;
esac
