#!/bin/sh
# Manual, bounded preview only. Never confirms or uploads a reading plan.
set -eu
PATH=/sbin:/usr/sbin:/bin:/usr/bin
export PATH
ROOT=/var/local/kindle-native-sync
OUT=/mnt/us/.kindle-native-sync-export
HERE=/mnt/us/extensions/kindle-native-sync
[ "$(id -u)" = 0 ] && [ ! -L "$OUT" ] || exit 1
mkdir -p "$OUT"
for name in online-preview.log action-authorize-prepare.json action-authorize-prepare.json.partial preview-error.txt preview-status.json preview-status.json.partial preview-status-error.txt tls-preview-error.txt tls-selftest.json tls-selftest.before-preview.json;do
    [ ! -L "$OUT/$name" ] || exit 1
done
exec > "$OUT/online-preview.log" 2>&1
store_busy() { grep -Fxq 'worker stopped: worker already running' "$1"; }
status_once() {
    if "$ROOT/bin/ksync-worker" --root "$ROOT" status > "$OUT/preview-status.json.partial" 2> "$OUT/preview-status-error.txt";then
        mv "$OUT/preview-status.json.partial" "$OUT/preview-status.json"
        return 0
    else return $?;fi
}
status_attempt=1
wait_store() {
    while [ "$status_attempt" -le 10 ];do
        if status_once;then return 0;fi
        if ! store_busy "$OUT/preview-status-error.txt" || [ "$status_attempt" -ge 10 ];then
            cat "$OUT/preview-status-error.txt";return 1
        fi
        printf 'Store busy before preview; bounded read-only retry %s.\n' "$status_attempt"
        sleep 3
        status_attempt=$((status_attempt+1))
    done
    return 1
}
# A collector child can acquire the store between the status probe and the
# local TLS filesystem test. Retry only a proven lock race, never accept a
# false filesystem result, failed CA check, or unrelated setup failure.
tls_attempt=1
while [ "$tls_attempt" -le 10 ];do
    wait_store || exit 1
    # Bind retry evidence to this invocation, never an older successful probe.
    if [ -f "$OUT/tls-selftest.json" ];then mv "$OUT/tls-selftest.json" "$OUT/tls-selftest.before-preview.json";fi
    if sh "$HERE/bootstrap.sh" online-self-test 2> "$OUT/tls-preview-error.txt";then break;fi
    tls_busy=false
    if store_busy "$OUT/tls-preview-error.txt";then tls_busy=true
    elif grep -Eq '"private_filesystem_ok"[[:space:]]*:[[:space:]]*false([,}]|[[:space:]]*$)' "$OUT/tls-selftest.json" &&
         grep -Eq '"https_certificate_validated"[[:space:]]*:[[:space:]]*true([,}]|[[:space:]]*$)' "$OUT/tls-selftest.json" &&
         grep -Eq '"state"[[:space:]]*:[[:space:]]*"completed"([,}]|[[:space:]]*$)' "$OUT/tls-selftest.json";then
        if status_once;then :;elif store_busy "$OUT/preview-status-error.txt";then tls_busy=true;fi
    fi
    if [ "$tls_busy" != true ] || [ "$tls_attempt" -ge 10 ];then
        cat "$OUT/tls-preview-error.txt";exit 1
    fi
    printf 'TLS local test raced a busy store; retry %s without accepting the failed result.\n' "$tls_attempt"
    sleep 3
    tls_attempt=$((tls_attempt+1))
done
result=1
attempt=1
while [ "$attempt" -le 10 ];do
    if "$ROOT/bin/ksync-worker" --root "$ROOT" operate --inbox /mnt/us/.kindle-native-sync-actions --action authorize-prepare > "$OUT/action-authorize-prepare.json.partial" 2> "$OUT/preview-error.txt";then
        result=0
        break
    fi
    # This action has no reading-time POST. Retry only the exact store lock or
    # short idle interval, never an active/unknown reader or auth failure.
    if [ "$attempt" -lt 10 ] && { store_busy "$OUT/preview-error.txt" || grep -Fxq 'worker stopped: idle window not established' "$OUT/preview-error.txt"; };then
        sleep 5
        attempt=$((attempt+1))
    else
        break
    fi
done
if [ ! -s "$OUT/action-authorize-prepare.json.partial" ];then
    printf '{"schema_version":1,"state":"preview_blocked","account_time_write_performed":false,"exit_code":%s}\n' "$result" > "$OUT/action-authorize-prepare.json.partial"
fi
mv "$OUT/action-authorize-prepare.json.partial" "$OUT/action-authorize-prepare.json"
cat "$OUT/preview-error.txt"
sh "$HERE/export-audit.sh" || echo 'Audit export incomplete; retain available evidence.'
sh "$ROOT/install/control.sh" export || echo 'Status export incomplete; retain the prepared result and available diagnostics.'
sync
exit "$result"
