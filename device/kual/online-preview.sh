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
for name in online-preview.log action-authorize-prepare.json action-authorize-prepare.json.partial preview-error.txt;do
    [ ! -L "$OUT/$name" ] || exit 1
done
exec > "$OUT/online-preview.log" 2>&1
sh "$HERE/bootstrap.sh" online-self-test
result=1
attempt=1
while [ "$attempt" -le 10 ];do
    if "$ROOT/bin/ksync-worker" --root "$ROOT" operate --inbox /mnt/us/.kindle-native-sync-actions --action authorize-prepare > "$OUT/action-authorize-prepare.json.partial" 2> "$OUT/preview-error.txt";then
        result=0
        break
    fi
    # This action has no reading-time POST. Retry only the precise short idle
    # interval, never an active/unknown reader or an authentication failure.
    if [ "$attempt" -lt 10 ] && grep -Fq 'idle window not established' "$OUT/preview-error.txt";then
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
sh "$ROOT/install/control.sh" export
sync
exit "$result"
