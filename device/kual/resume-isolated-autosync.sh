#!/bin/sh
# One-time update for the previously verified 2bd766d preview runtime.
set -eu
umask 077
PATH=/sbin:/usr/sbin:/bin:/usr/bin
export PATH
ROOT=/var/local/kindle-native-sync
USB=/mnt/us
PACKAGE=$USB/kindle-native-sync-package
UPDATE=$USB/.kindle-native-sync-queue-update
OUT=$USB/.kindle-native-sync-export
EXT=$USB/extensions/kindle-native-sync
OLD_SHA=f327c8bfc6605093a413aff88462c2056f36c54f74cb3e6e3669268803d22350
WORKER=$ROOT/bin/ksync-worker
NEXT=$ROOT/bin/ksync-worker.queue-next
BACKUP=$ROOT/bin/ksync-worker.pre-queue-update

. "$ROOT/install/common.sh"
require_device
[ "$(lipc-get-prop com.lab126.volumd userstoreIsAvailable 2>/dev/null)" = 1 ]
[ "$(ksync_root_mount_state)" = read_only ]
[ -d "$UPDATE" ] && [ ! -L "$UPDATE" ] && [ ! -L "$OUT" ]
mkdir -p "$OUT"
for f in queue-update.log queue-update-result.json queue-update-tls.json queue-update-auth.json queue-update-sync.json queue-update-sync.error;do
    [ ! -L "$OUT/$f" ]
done
exec > "$OUT/queue-update.log" 2>&1
stage=preflight
finish() {
    rc=$?
    trap - EXIT
    printf '{"state":"%s","stage":"%s","exit_code":%s}\n' "$([ "$rc" -eq 0 ] && echo updated || echo blocked)" "$stage" "$rc" > "$OUT/queue-update-result.json"
    sync
    exit "$rc"
}
trap finish EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
regular() { [ -f "$1" ] && [ ! -L "$1" ]; }
digest() { sha256sum "$1" | awk '{print $1}'; }
regular "$UPDATE/ksync-worker" && regular "$UPDATE/SHA256SUMS" && regular "$WORKER"
candidate_sha=$(digest "$UPDATE/ksync-worker")
[ "$(cat "$UPDATE/SHA256SUMS")" = "$candidate_sha  ksync-worker" ]
installed_sha=$(digest "$WORKER")
[ "$installed_sha" = "$OLD_SHA" ] || [ "$installed_sha" = "$candidate_sha" ]
for marker in .verified-runtime-upgrade .component-repair .preview-upgrade .install-transaction disabled;do
    [ ! -e "$ROOT/$marker" ] && [ ! -L "$ROOT/$marker" ]
done
sh "$PACKAGE/ota/control.sh" require-ready >/dev/null
[ "$(lipc-get-prop com.lab126.wifid cmState 2>/dev/null)" = CONNECTED ]

if [ "$installed_sha" = "$OLD_SHA" ];then
    stage=stage_worker
    [ ! -L "$NEXT" ] && [ ! -L "$BACKUP" ]
    if [ -e "$BACKUP" ];then regular "$BACKUP" && [ "$(digest "$BACKUP")" = "$OLD_SHA" ]
    else ln "$WORKER" "$BACKUP";fi
    if [ -e "$NEXT" ] && regular "$NEXT" &&
        [ "$(digest "$NEXT")" = 2841b2d6bdc446a7088954a3830d7094d601a0a3c23c044238844d1798010800 ];then
        # This exact candidate failed before authorization or replacement.
        rm "$NEXT"
    fi
    if [ -e "$NEXT" ];then regular "$NEXT" && [ "$(digest "$NEXT")" = "$candidate_sha" ]
    else
        bytes=$(wc -c < "$UPDATE/ksync-worker")
        [ "$(space_kib)" -ge "$((24576 + 64 + (bytes + 1023) / 1024 + 64))" ]
        staging=$(mktemp "$ROOT/bin/.queue-worker.XXXXXX")
        if cp "$UPDATE/ksync-worker" "$staging" && chmod 700 "$staging" &&
            [ "$(digest "$staging")" = "$candidate_sha" ];then
            mv "$staging" "$NEXT"
        else
            rm -f "$staging"
            exit 1
        fi
    fi
    stage=tls_selftest
    "$NEXT" --root "$ROOT" self-test --online-self-test > "$OUT/queue-update-tls.json"
    stage=carry_forward_authorization
    "$NEXT" --root "$ROOT" resume-isolated-autosync > "$OUT/queue-update-auth.json"
    sync
    stage=switch_worker
    mv "$NEXT" "$WORKER"
    [ "$(digest "$WORKER")" = "$candidate_sha" ]
    sync
fi

stage=sync_attempt
# Only a proven lock conflict, before the worker has made any request, is retried.
attempt=0
while :;do
    if "$WORKER" --root "$ROOT" sync > "$OUT/queue-update-sync.json" 2> "$OUT/queue-update-sync.error";then break;fi
    attempt=$((attempt+1))
    [ "$attempt" -lt 12 ] && grep -Fxq 'worker stopped: worker already running' "$OUT/queue-update-sync.error" || {
        cat "$OUT/queue-update-sync.error"
        exit 1
    }
    sleep 3
done
stage=export
# The deployed extension already exports the original receipts alongside status.
sh "$EXT/export-verification.sh"
stage=completed
