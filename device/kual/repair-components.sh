#!/bin/sh
# Explicit, narrow repair for the configured preview. Never changes account data.
set -eu
umask 077
PATH=/sbin:/usr/sbin:/bin:/usr/bin
export PATH
upgrade_unsubmitted=false
live_collector=false
for option in "$@";do
    case "$option" in
        --unsubmitted-worker-upgrade) [ "$upgrade_unsubmitted" = false ] || exit 2;upgrade_unsubmitted=true;;
        --worker-only-live-collector) [ "$live_collector" = false ] || exit 2;live_collector=true;;
        *) echo 'Unsupported component repair action.' >&2;exit 2;;
    esac
done
[ "$live_collector" = false ] || [ "$upgrade_unsubmitted" = true ] || exit 2
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
[ -f "$ROOT/bin/ksync-worker" ] && [ ! -L "$ROOT/bin/ksync-worker" ] || exit 1
worker_unchanged=true
if [ "$expected" != "$actual" ];then
    [ "$upgrade_unsubmitted" = true ] || { echo 'Worker changed; use a reviewed complete upgrade.';exit 1; }
    worker_unchanged=false
fi
runtime_components='bin/ksync-collector bin/ksync-event-adapter install/common.sh install/control.sh'
if [ "$live_collector" = true ];then
    runtime_components='bin/ksync-worker'
    # Preserve actual native-reader state only when the running event components
    # are already exactly this candidate. Never replace their executable inode.
    for name in ksync-collector ksync-event-adapter;do
        [ -f "$ROOT/bin/$name" ] && [ ! -L "$ROOT/bin/$name" ] || exit 1
        [ "$(sha256sum "$PACKAGE/bin/$name" | awk '{print $1}')" = "$(sha256sum "$ROOT/bin/$name" | awk '{print $1}')" ] || { echo 'Live event components differ from candidate.';exit 1; }
    done
elif [ "$upgrade_unsubmitted" = true ];then runtime_components="$runtime_components bin/ksync-worker";fi
backup_components="$runtime_components package-manifest.json"
verify_live_jobs() {
    for job in kindle-native-sync-events kindle-native-sync;do
        if job_state=$(initctl status "$job" 2>/dev/null);then :;else return 1;fi
        case "$job_state" in "$job start/running, process "*) :;; *) return 1;; esac
        job_pid=${job_state##* }
        case "$job_pid" in ''|*[!0-9]*) return 1;; esac
        [ "$job_pid" -gt 0 ] && [ -d "/proc/$job_pid" ] || return 1
        if [ "$job" = kindle-native-sync ];then job_exe=ksync-collector;else job_exe=ksync-event-adapter;fi
        [ "$(readlink "/proc/$job_pid/exe" 2>/dev/null)" = "$ROOT/bin/$job_exe" ] || return 1
    done
}
if [ "$live_collector" = true ];then verify_live_jobs || { echo 'Cannot verify both unchanged event services are running.';exit 1; };fi
SNAP=$OUT/private-repair-$(date -u +%Y%m%dT%H%M%S)-$$
mkdir "$SNAP" "$SNAP/sealed" "$SNAP/components"
ls -ldn "$ROOT" "$ROOT/config.json" "$ROOT/auth.json" "$ROOT/control.fifo" "$ROOT/adapter-control.fifo" > "$SNAP/permissions-before.txt" 2>&1 || :
"$PACKAGE/bin/ksync-collector" --root "$ROOT" --repair-private-state
[ ! -e "$ROOT/.component-repair" ] && [ ! -L "$ROOT/.component-repair" ] || { echo 'Unresolved component repair; retain recovery.';exit 1; }
[ ! -e "$ROOT/.preview-upgrade" ] && [ ! -L "$ROOT/.preview-upgrade" ] && [ ! -e "$ROOT/.install-transaction" ] && [ ! -L "$ROOT/.install-transaction" ] || exit 1
enabled=1
[ ! -e "$ROOT/disabled" ] || enabled=0
if [ "$live_collector" = false ];then
    : > "$ROOT/disabled"
    stop kindle-native-sync-events >/dev/null 2>&1 || :
    stop kindle-native-sync >/dev/null 2>&1 || :
    for job in kindle-native-sync-events kindle-native-sync;do
        if job_state=$(initctl status "$job" 2>/dev/null);then :;else echo 'Cannot verify stopped project service.';exit 1;fi
        case "$job_state" in "$job stop/waiting"*) :;; *) echo 'Service did not stop.';exit 1;; esac
    done
fi
"$PACKAGE/bin/ksync-collector" --root "$ROOT" --repair-private-state
if [ "$upgrade_unsubmitted" = true ];then
    # The old executable is authoritative about its own account-write ledger.
    # Preserve this proof before the candidate can open the private store.
    "$ROOT/bin/ksync-worker" --root "$ROOT" status > "$SNAP/old-worker-status.json"
    for proof in '"attempts"[[:space:]]*:[[:space:]]*0([,}]|[[:space:]]*$)' '"unknown_batches"[[:space:]]*:[[:space:]]*0([,}]|[[:space:]]*$)' '"auto_sync_enabled"[[:space:]]*:[[:space:]]*false([,}]|[[:space:]]*$)';do
        grep -Eq "$proof" "$SNAP/old-worker-status.json" || { echo 'Worker replacement requires an entirely unsubmitted, inactive ledger.';exit 1; }
    done
    "$PACKAGE/bin/ksync-worker" --root "$ROOT" self-test > "$SNAP/candidate-selftest.json"
    for proof in '"private_filesystem_ok"[[:space:]]*:[[:space:]]*true([,}]|[[:space:]]*$)' '"architecture"[[:space:]]*:[[:space:]]*"arm"([,}]|[[:space:]]*$)' '"serial_prefix"[[:space:]]*:[[:space:]]*"B0D4"([,}]|[[:space:]]*$)' '"supported_firmware"[[:space:]]*:[[:space:]]*true([,}]|[[:space:]]*$)' '"supported_hardware"[[:space:]]*:[[:space:]]*true([,}]|[[:space:]]*$)' '"root"[[:space:]]*:[[:space:]]*true([,}]|[[:space:]]*$)' '"network_contacted"[[:space:]]*:[[:space:]]*false([,}]|[[:space:]]*$)';do
        grep -Eq "$proof" "$SNAP/candidate-selftest.json" || { echo 'Candidate offline device self-test did not meet the PW2 profile.';exit 1; }
    done
fi
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
for name in $backup_components;do
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
        (cd "$SNAP/components" && sha256sum -c SHA256SUMS) >/dev/null || { echo 'Recovery checksum failed; retain prior service state and evidence.';exit 1; }
        for name in $backup_components;do
            safe=$(printf '%s' "$name" | tr / _)
            if [ "$live_collector" = true ];then
                # A collect-only worker may still have the current executable
                # open. Restore by rename, never truncate a live executable.
                [ ! -e "$ROOT/$name.restore" ] && [ ! -L "$ROOT/$name.restore" ] || exit 1
                cp "$SNAP/components/$safe" "$ROOT/$name.restore" || exit 1
                if [ "$name" = package-manifest.json ];then chmod 600 "$ROOT/$name.restore";else chmod 700 "$ROOT/$name.restore";fi
                [ "$(sha256sum "$ROOT/$name.restore" | awk '{print $1}')" = "$(sha256sum "$SNAP/components/$safe" | awk '{print $1}')" ] || exit 1
                mv "$ROOT/$name.restore" "$ROOT/$name" || exit 1
            else cp "$SNAP/components/$safe" "$ROOT/$name";fi
        done
        for name in $runtime_components;do chmod 700 "$ROOT/$name";done
        chmod 600 "$ROOT/package-manifest.json"
        if [ "$live_collector" = true ];then echo 'Worker repair incomplete; live event services and prior enable state retained.'
        else echo 'Component repair incomplete; collection disabled; private data retained.';fi
    fi
    sync
    exit "$code"
}
trap cleanup EXIT
trap 'exit 129' HUP;trap 'exit 130' INT;trap 'exit 143' TERM
if [ "$live_collector" = true ];then verify_live_jobs || exit 1;fi
for name in $runtime_components;do
    [ ! -e "$ROOT/$name.repair" ] && [ ! -L "$ROOT/$name.repair" ] || exit 1
    # Stage one public component at a time, preserving the system plus worker
    # transaction reserve. The old full set is already verified on USB.
    item_kib=$(du -sk "$PACKAGE/$name" | awk 'NR==1 {print $1}')
    free_kib=$(space_kib)
    case "$item_kib:$free_kib" in *[!0-9:]*|:*|*:) exit 1;; esac
    [ "$item_kib" -le 2147483647 ] && [ "$free_kib" -le 2147483647 ] || exit 1
    [ "$free_kib" -ge "$((24576 + item_kib + 1024))" ] || { echo 'Insufficient private space for this component and reserves.';exit 1; }
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
if [ "$live_collector" = false ] && [ "$enabled" = 1 ];then sh "$ROOT/install/control.sh" enable;fi
# A restarted collector deliberately starts with unknown reader state. The
# user must really open/close a book before the separate prepare action.
sh "$ROOT/install/control.sh" export
printf '{"schema_version":1,"state":"components_repaired","worker_unchanged":%s,"unsubmitted_worker_upgrade":%s,"live_collector_preserved":%s,"account_time_write_performed":false}\n' "$worker_unchanged" "$upgrade_unsubmitted" "$live_collector" > "$OUT/component-repair-result.json"
sync
