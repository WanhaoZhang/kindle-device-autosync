#!/bin/sh
# Low-space replacement ONLY for a stopped, disabled, unconfigured preview.
# Recovery components live on USB; private capture/checkpoint/ledger never move.
set -eu
umask 077
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
. "$HERE/common.sh"
. "$HERE/transaction.sh"
require_device
PACKAGE=${1:-$(dirname "$HERE")}
MODE=${2:-upgrade}
MARKER=$ROOT/.preview-upgrade
RECOVERY_PARENT=/mnt/us/.kindle-native-sync-recovery
PREVIEW_COMPONENTS='bin install package-manifest.json ca-bundle.crt'
changed=0
recovering=0
backup_ready=0
recovery=

refuse() { printf '%s\n' "$1" >&2;exit 1; }
is_running() { initctl status "$1" 2>/dev/null | grep -q 'start/running'; }
private_gate() {
    [ -f "$ROOT/disabled" ] && [ ! -L "$ROOT/disabled" ] || refuse 'Disable collection before preview replacement.'
    ! is_running "$COLLECTOR_JOB" && ! is_running "$EVENTS_JOB" || refuse 'Project jobs must be stopped.'
    for f in config.json auth.json validation-device.json;do
        [ ! -e "$ROOT/$f" ] && [ ! -L "$ROOT/$f" ] || refuse 'Configured/accepted instances require normal transactional upgrade.'
    done
    [ "$(lipc-get-prop com.lab126.volumd userstoreIsAvailable 2>/dev/null)" = 1 ] || refuse 'USB userstore unavailable.'
}
safe_components() {
    for name in $PREVIEW_COMPONENTS;do
        [ ! -L "$ROOT/$name" ] || return 1
    done
    for d in bin install;do
        [ -d "$ROOT/$d" ] || return 1
        for file in "$ROOT/$d/"*;do
            [ -e "$file" ] || [ -L "$file" ] || continue
            base=${file##*/}
            case "$base" in *[!A-Za-z0-9._-]*|.*) return 1;; esac
            [ -f "$file" ] && [ ! -L "$file" ] || return 1
        done
    done
}
write_component_sums() {
    # Fixed project component roots, no credentials/ledger/raw input.
    for name in bin install;do
        for file in "$ROOT/$name/"*;do
            [ -f "$file" ] && [ ! -L "$file" ] || continue
            rel=$name/${file##*/}
            digest=$(sha256sum "$file" | awk '{print $1}')
            printf '%s  %s\n' "$digest" "$rel"
        done
    done
    for name in package-manifest.json ca-bundle.crt;do
        if [ -f "$ROOT/$name" ];then
            digest=$(sha256sum "$ROOT/$name" | awk '{print $1}')
            printf '%s  %s\n' "$digest" "$name"
        fi
    done
}
verify_recovery() {
    [ -n "$recovery" ] && [ ! -L "$recovery" ] && [ -d "$recovery/components" ] && [ ! -L "$recovery/components" ] || return 1
    [ -f "$recovery/complete" ] && [ -f "$recovery/components.sha256" ] && [ ! -L "$recovery/components.sha256" ] || return 1
    (cd "$recovery/components" && sha256sum -c ../components.sha256) >/dev/null || return 1
    (cd "$recovery/jobs" && sha256sum -c ../jobs.sha256) >/dev/null || return 1
}
restore_preview() {
    verify_recovery || return 1
    ! is_running "$COLLECTOR_JOB" && ! is_running "$EVENTS_JOB" || return 1
    # No project service can restart while disabled, even across a reboot.
    root_rw || return 1
    restored=1
    for name in $PREVIEW_COMPONENTS;do
        rm -rf "$ROOT/$name" || restored=0
        if [ -e "$recovery/components/$name" ];then cp -R "$recovery/components/$name" "$ROOT/$name" || restored=0;fi
    done
    [ ! -d "$ROOT/bin" ] || chmod 700 "$ROOT/bin" "$ROOT/bin/"* || restored=0
    [ ! -d "$ROOT/install" ] || chmod 700 "$ROOT/install" "$ROOT/install/"* || restored=0
    [ ! -f "$ROOT/ca-bundle.crt" ] || chmod 600 "$ROOT/ca-bundle.crt" || restored=0
    for job in "$COLLECTOR_JOB" "$EVENTS_JOB";do
        rm -f "$JOBS/$job.conf" "$JOBS/.$job.conf.new" || restored=0
        if [ -f "$recovery/jobs/$job.conf" ];then cp "$recovery/jobs/$job.conf" "$JOBS/$job.conf" && chmod 644 "$JOBS/$job.conf" || restored=0;fi
    done
    sync
    root_ro || restored=0
    initctl reload-configuration >/dev/null 2>&1 || restored=0
    (cd "$ROOT" && sha256sum -c "$recovery/components.sha256") >/dev/null || restored=0
    (cd "$JOBS" && sha256sum -c "$recovery/jobs.sha256") >/dev/null || restored=0
    if [ "$restored" = 1 ];then rm -rf "$MARKER";return 0;fi
    return 1
}
finish_preview() {
    result=$?
    trap - EXIT HUP INT TERM
    set +e
    if [ "$changed" = 1 ] && [ "$recovering" = 0 ];then
        if restore_preview;then printf 'Previous stopped preview restored. Private data retained.\n' >&2
        else printf 'Preview recovery incomplete. Keep USB recovery and private marker; do not repeat upgrade.\n' >&2;result=1;fi
    fi
    root_ro || result=1
    exit "$result"
}

private_gate
if [ "$MODE" = recover ];then
    [ -d "$MARKER" ] && [ ! -L "$MARKER" ] && [ -f "$MARKER/recovery-path" ] || refuse 'No recorded preview recovery.'
    read -r recovery < "$MARKER/recovery-path"
    case "$recovery" in "$RECOVERY_PARENT"/preview-*) :;; *) refuse 'Invalid recovery path.';; esac
    case "${recovery##*/}" in *[!A-Za-z0-9._-]*|.*) refuse 'Invalid recovery name.';; esac
    recovering=1
    trap finish_preview EXIT
    trap 'exit 129' HUP;trap 'exit 130' INT;trap 'exit 143' TERM
    restore_preview || refuse 'Recovery incomplete; evidence retained.'
    printf 'Stopped preview restored; collection remains disabled.\n' || :
    exit 0
fi
[ "$MODE" = upgrade ] || refuse 'Unknown preview replacement mode.'
for f in "$MARKER" "$ROOT/.install-stage" "$ROOT/.install-transaction" "$ROOT/upgrade.previous" "$ROOT/.uninstall-backup";do
    [ ! -e "$f" ] && [ ! -L "$f" ] || refuse 'Unresolved transaction/recovery exists.'
done
safe_components || refuse 'Unsafe or incomplete old components.'
status_file=$ROOT/.preview-status.$$
if "$ROOT/bin/ksync-worker" --root "$ROOT" status > "$status_file";then :;else rm -f "$status_file";refuse 'Cannot verify old private ledger.';fi
if grep -Eq '"attempts"[[:space:]]*:[[:space:]]*0([,}])' "$status_file" && grep -Eq '"configured"[[:space:]]*:[[:space:]]*false([,}])' "$status_file";then rm -f "$status_file"
else rm -f "$status_file";refuse 'Preview is not empty/unconfigured; normal upgrade required.';fi
[ ! -L "$PACKAGE" ] && [ -d "$PACKAGE/install" ] || refuse 'Unsafe package.'
(cd "$PACKAGE" && sha256sum -c SHA256SUMS) >/dev/null || refuse 'Candidate checksum failed.'
for name in ksync-collector ksync-worker ksync-event-adapter;do
    file=$PACKAGE/bin/$name
    [ -f "$file" ] && [ ! -L "$file" ] || refuse 'Candidate binary missing.'
    magic=$(dd if="$file" bs=1 count=7 2>/dev/null | od -An -tx1 | tr -d ' \n')
    machine=$(dd if="$file" bs=1 skip=18 count=2 2>/dev/null | od -An -tx1 | tr -d ' \n')
    [ "$magic" = 7f454c46010101 ] && [ "$machine" = 2800 ] || refuse 'Candidate is not ARM32.'
done
free=$(space_kib)
ksync_storage_budget "$PACKAGE" "$free" || refuse 'Private storage budget unavailable.'
[ "$raw_queue_over_limit" = false ] || refuse 'Oversize raw queue retained; replacement refused.'
old_kib=$(ksync_sum_kib "$ROOT/bin" "$ROOT/install" "$ROOT/package-manifest.json" "$ROOT/ca-bundle.crt")
required=$((required_runtime_free_kib + ksync_new_kib - old_kib))
[ "$free" -ge "$required" ] || refuse 'Replacement cannot retain the system and transaction reserves.'
[ ! -L "$RECOVERY_PARENT" ] || refuse 'Unsafe recovery directory.'
mkdir -p "$RECOVERY_PARENT"
stamp=$(date +%Y%m%dT%H%M%S | tr -cd '0-9T')
recovery=$RECOVERY_PARENT/preview-$stamp-$$
mkdir "$recovery" "$recovery/components" "$recovery/jobs"
usb_free=$(df -Pk "$RECOVERY_PARENT" | awk 'END {print $4}')
case "$usb_free" in ''|*[!0-9]*) refuse 'Cannot inspect USB recovery space.';; esac
[ "$usb_free" -ge "$((old_kib+2048))" ] || refuse 'Insufficient USB recovery space.'
for name in $PREVIEW_COMPONENTS;do [ ! -e "$ROOT/$name" ] || cp -R "$ROOT/$name" "$recovery/components/$name";done
write_component_sums > "$recovery/components.sha256"
: > "$recovery/jobs.sha256"
for job in "$COLLECTOR_JOB" "$EVENTS_JOB";do
    [ ! -L "$JOBS/$job.conf" ] || refuse 'Unsafe old job.'
    if [ -f "$JOBS/$job.conf" ];then
        cp "$JOBS/$job.conf" "$recovery/jobs/$job.conf"
        digest=$(sha256sum "$JOBS/$job.conf" | awk '{print $1}')
        printf '%s  %s.conf\n' "$digest" "$job" >> "$recovery/jobs.sha256"
    fi
done
: > "$recovery/complete"
verify_recovery || refuse 'USB recovery verification failed; old components retained.'
: > "$recovery/new-components.sha256"
for file in "$PACKAGE/bin/ksync-collector" "$PACKAGE/bin/ksync-worker" "$PACKAGE/bin/ksync-event-adapter" "$PACKAGE/install/"*.sh;do
    relative=${file#"$PACKAGE/"}
    digest=$(sha256sum "$file" | awk '{print $1}')
    printf '%s  %s\n' "$digest" "$relative" >> "$recovery/new-components.sha256"
done
digest=$(sha256sum "$PACKAGE/manifest.json" | awk '{print $1}')
printf '%s  package-manifest.json\n' "$digest" >> "$recovery/new-components.sha256"
if [ -f "$PACKAGE/ca-bundle.crt" ];then
    digest=$(sha256sum "$PACKAGE/ca-bundle.crt" | awk '{print $1}')
    printf '%s  ca-bundle.crt\n' "$digest" >> "$recovery/new-components.sha256"
fi
sync
mkdir "$MARKER"
printf '%s\n' "$recovery" > "$MARKER/recovery-path"
printf 'recovery_verified\n' > "$MARKER/phase"
sync
trap finish_preview EXIT
trap 'exit 129' HUP;trap 'exit 130' INT;trap 'exit 143' TERM
changed=1
root_rw
for name in $PREVIEW_COMPONENTS;do rm -rf "$ROOT/$name";done
printf 'replacing_components\n' > "$MARKER/phase"
mkdir "$ROOT/bin" "$ROOT/install"
cp "$PACKAGE/bin/ksync-collector" "$PACKAGE/bin/ksync-worker" "$PACKAGE/bin/ksync-event-adapter" "$ROOT/bin/"
chmod 700 "$ROOT/bin" "$ROOT/bin/"*
cp "$PACKAGE/install/"*.sh "$ROOT/install/";chmod 700 "$ROOT/install" "$ROOT/install/"*.sh
cp "$PACKAGE/manifest.json" "$ROOT/package-manifest.json"
if [ -f "$PACKAGE/ca-bundle.crt" ];then cp "$PACKAGE/ca-bundle.crt" "$ROOT/ca-bundle.crt";chmod 600 "$ROOT/ca-bundle.crt";fi
(cd "$ROOT" && sha256sum -c "$recovery/new-components.sha256") >/dev/null
for job in "$COLLECTOR_JOB" "$EVENTS_JOB";do
    cp "$PACKAGE/upstart/$job.conf" "$JOBS/.$job.conf.new";chmod 644 "$JOBS/.$job.conf.new";mv "$JOBS/.$job.conf.new" "$JOBS/$job.conf"
    wanted=$(sha256sum "$PACKAGE/upstart/$job.conf" | awk '{print $1}')
    observed=$(sha256sum "$JOBS/$job.conf" | awk '{print $1}')
    [ "$wanted" = "$observed" ] || refuse 'Installed job readback mismatch.'
done
sync
root_ro
initctl reload-configuration
! is_running "$COLLECTOR_JOB" && ! is_running "$EVENTS_JOB" || refuse 'Disabled jobs unexpectedly running.'
free=$(space_kib);ksync_storage_budget "$PACKAGE" "$free"
[ "$free" -ge "$required_runtime_free_kib" ] || refuse 'Post-replacement runtime reserve unavailable.'
printf 'installed_disabled\n' > "$MARKER/phase";sync
rm -rf "$MARKER"
changed=0
printf 'Preview replaced; collection remains disabled; private records retained.\n' || :
