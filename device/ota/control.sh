#!/bin/sh
# Reversible OTA-binary rename for PW2 5.12.2.1.1. No Wi-Fi or process changes.
set -eu
PATH=/sbin:/usr/sbin:/bin:/usr/bin
export PATH
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
. "$HERE/../install/common.sh"
require_device
OTA_BIN=/usr/bin
OTA_USB=/mnt/us
OTA_BOOT=/proc/sys/kernel/random/boot_id
OTA_DIR=$ROOT/ota
OTA_STATE=$OTA_DIR/state.tsv
OTA_LOCK=$OTA_DIR/transaction
OTA_ACTIVE=0
OTA_MUTATING=0
OTA_COMMITTED=0
OTA_REASON=none
OTA_OLD_STATE=0
OTA_ACTION=${1:-status}
case "$OTA_ACTION" in guard|restore|status|require-ready) :;; *) echo 'Usage: control.sh guard|restore|status|require-ready' >&2;exit 2;; esac

ota_boot_id() {
    if [ -r "$OTA_BOOT" ];then
        ota_boot=$(tr -d '\r\n' < "$OTA_BOOT")
        case "$ota_boot" in *[!0-9a-f-]*|'') return 1;; esac
        [ "${#ota_boot}" = 36 ] || return 1
        printf '%s\n' "$ota_boot"
    else return 1;fi
}
ota_hash() {
    ota_digest=$(sha256sum "$1" 2>/dev/null | awk '{print $1}')
    case "$ota_digest" in *[!0-9a-f]*|'') return 1;; esac
    [ "${#ota_digest}" = 64 ] || return 1
    printf '%s\n' "$ota_digest"
}
ota_regular_executable() { [ -f "$1" ] && [ -x "$1" ] && [ ! -L "$1" ]; }
ota_no_pending_update() {
    [ -d "$OTA_USB" ] && [ ! -L "$OTA_USB" ] || return 1
    [ "$(lipc-get-prop com.lab126.volumd userstoreIsAvailable 2>/dev/null)" = 1 ] || return 1
    for ota_pending in "$OTA_USB/"*.bin "$OTA_USB/"*.bin.* "$OTA_USB/"update*.tmp* "$OTA_USB/"update*.partial*;do
        [ -e "$ota_pending" ] || [ -L "$ota_pending" ] || continue
        case "${ota_pending##*/}" in ._*) continue;; esac
        return 1
    done
    return 0
}
ota_read_state() {
    [ -f "$OTA_STATE" ] && [ ! -L "$OTA_STATE" ] || return 1
    ota_size=$(wc -c < "$OTA_STATE" | tr -d '[:space:]')
    [ "$ota_size" -le 2048 ] || return 1
    [ "$(awk -F '\t' '$1=="schema" {print $2}' "$OTA_STATE")" = 1 ] || return 1
    OTA_SAVED_STATE=$(awk -F '\t' '$1=="state" {print $2}' "$OTA_STATE")
    OTA_SAVED_BOOT=$(awk -F '\t' '$1=="boot_id" {print $2}' "$OTA_STATE")
    OTA_SHA_UPD=$(awk -F '\t' '$1=="otaupd" {print $2}' "$OTA_STATE")
    OTA_SHA_V3=$(awk -F '\t' '$1=="otav3" {print $2}' "$OTA_STATE")
    case "$OTA_SAVED_STATE" in guarded|restored) :;; *) return 1;; esac
    case "$OTA_SAVED_BOOT" in *[!0-9a-f-]*|'') return 1;; esac
    [ "${#OTA_SAVED_BOOT}" = 36 ] || return 1
    for ota_sha in "$OTA_SHA_UPD" "$OTA_SHA_V3";do
        case "$ota_sha" in *[!0-9a-f]*|'') return 1;; esac
        [ "${#ota_sha}" = 64 ] || return 1
    done
}
ota_expected() { case "$1" in otaupd) printf '%s\n' "$OTA_SHA_UPD";; otav3) printf '%s\n' "$OTA_SHA_V3";; *) return 1;; esac; }
ota_pair_matches() {
    ota_suffix=$1
    for ota_name in otaupd otav3;do
        ota_file=$OTA_BIN/$ota_name$ota_suffix
        ota_regular_executable "$ota_file" || return 1
        [ "$(ota_hash "$ota_file")" = "$(ota_expected "$ota_name")" ] || return 1
        if [ "$ota_suffix" = .bck ];then ota_other=$OTA_BIN/$ota_name
        else ota_other=$OTA_BIN/$ota_name.bck;fi
        [ ! -e "$ota_other" ] && [ ! -L "$ota_other" ] || return 1
    done
}
ota_inspect() {
    OTA_OBSERVED=unknown
    OTA_REBOOT_REQUIRED=false
    OTA_READY=false
    OTA_ROOT_STATE=$(ksync_root_mount_state)
    if [ -e "$OTA_LOCK" ] || [ -L "$OTA_LOCK" ];then OTA_OBSERVED=interrupted;return;fi
    if ota_read_state;then
        if [ "$OTA_SAVED_STATE" = guarded ] && ota_pair_matches .bck;then
            OTA_OBSERVED=guarded
            if ota_current_boot=$(ota_boot_id);then
                if [ "$ota_current_boot" = "$OTA_SAVED_BOOT" ];then OTA_REBOOT_REQUIRED=true
                elif [ "$OTA_ROOT_STATE" = read_only ] && ota_no_pending_update;then OTA_READY=true;fi
            fi
        elif [ "$OTA_SAVED_STATE" = restored ] && ota_pair_matches '';then
            OTA_OBSERVED=restored
            if ota_current_boot=$(ota_boot_id) && [ "$ota_current_boot" = "$OTA_SAVED_BOOT" ];then OTA_REBOOT_REQUIRED=true;fi
        else OTA_OBSERVED=conflict;fi
    elif [ ! -e "$OTA_STATE" ] && [ ! -L "$OTA_STATE" ];then
        if ota_regular_executable "$OTA_BIN/otaupd" && ota_regular_executable "$OTA_BIN/otav3" &&
            [ ! -e "$OTA_BIN/otaupd.bck" ] && [ ! -L "$OTA_BIN/otaupd.bck" ] &&
            [ ! -e "$OTA_BIN/otav3.bck" ] && [ ! -L "$OTA_BIN/otav3.bck" ];then OTA_OBSERVED=originals_present
        else OTA_OBSERVED=foreign_or_incomplete;fi
    else OTA_OBSERVED=conflict;fi
}
ota_report() {
    ota_inspect
    ota_pending_update=false;ota_no_pending_update || ota_pending_update=true
    printf '{"schema_version":1,"state":"%s","reason":"%s","root_mount_state":"%s","reboot_required":%s,"ready_after_reboot":%s,"pending_update":%s,"wifi_changed":false,"processes_killed":false}\n' "$OTA_OBSERVED" "$OTA_REASON" "$OTA_ROOT_STATE" "$OTA_REBOOT_REQUIRED" "$OTA_READY" "$ota_pending_update"
}
ota_fail() {
    OTA_REASON=$1;printf 'OTA operation refused: %s\n' "$OTA_REASON" >&2
    if [ "$OTA_ACTIVE" = 0 ];then
        printf '{"schema_version":1,"state":"refused","reason":"%s","ready_after_reboot":false,"reboot_required":false,"wifi_changed":false,"processes_killed":false}\n' "$OTA_REASON"
    fi
    exit 1
}
ota_rollback() {
    root_rw || return 1
    for ota_name in otaupd otav3;do
        if [ "$OTA_ACTION" = guard ];then ota_from=$OTA_BIN/$ota_name.bck;ota_to=$OTA_BIN/$ota_name
        else ota_from=$OTA_BIN/$ota_name;ota_to=$OTA_BIN/$ota_name.bck;fi
        if [ -e "$ota_from" ] || [ -L "$ota_from" ];then
            ota_regular_executable "$ota_from" || return 1
            [ ! -e "$ota_to" ] && [ ! -L "$ota_to" ] || return 1
            [ "$(ota_hash "$ota_from")" = "$(ota_expected "$ota_name")" ] || return 1
            mv "$ota_from" "$ota_to" || return 1
        fi
        ota_regular_executable "$ota_to" || return 1
        [ "$(ota_hash "$ota_to")" = "$(ota_expected "$ota_name")" ] || return 1
    done
    if [ "$OTA_OLD_STATE" = 1 ];then cp "$OTA_LOCK/previous-state.tsv" "$OTA_STATE" || return 1
    else rm -f "$OTA_STATE" || return 1;fi
    rm -f "$OTA_DIR/state.next" || return 1
    sync
}
ota_finish() {
    OTA_EXIT=$?
    trap - EXIT HUP INT TERM
    set +e
    OTA_RECOVERED=1
    if [ "$OTA_ACTIVE" = 1 ] && [ "$OTA_COMMITTED" != 1 ];then
        if [ "$OTA_MUTATING" = 1 ];then ota_rollback || OTA_RECOVERED=0;fi
        [ "$OTA_REASON" != none ] || OTA_REASON=operation_failed
    fi
    root_ro || { OTA_RECOVERED=0;OTA_REASON=root_read_only_not_verified; }
    if [ "$OTA_ACTIVE" = 1 ] && [ "$OTA_RECOVERED" = 1 ];then
        rm -rf "$OTA_LOCK" || OTA_RECOVERED=0
        sync
    fi
    if [ "$OTA_RECOVERED" != 1 ];then OTA_EXIT=1;fi
    ota_report
    exit "$OTA_EXIT"
}

[ ! -L "$ROOT" ] && [ -d "$ROOT" ] || ota_fail private_root_missing
[ ! -L "$OTA_BIN" ] && [ -d "$OTA_BIN" ] || ota_fail unsafe_binary_directory
[ ! -L "$OTA_DIR" ] || ota_fail unsafe_state_directory
if [ "$OTA_ACTION" = status ] || [ "$OTA_ACTION" = require-ready ];then
    ota_report
    if [ "$OTA_ACTION" = require-ready ] && [ "$OTA_READY" != true ];then exit 1;fi
    exit 0
fi
[ "$(lipc-get-prop com.lab126.volumd userstoreIsAvailable 2>/dev/null)" = 1 ] || ota_fail userstore_unavailable
# The old OTA process still exists until reboot. Never mutate updater paths
# while Wi-Fi is connected or trying to connect; the user keeps airplane mode.
ota_wifi=$(lipc-get-prop com.lab126.wifid cmState 2>/dev/null || :)
case "$ota_wifi" in NA|DISABLED|UNAVAILABLE) :;; *) ota_fail enable_flight_mode_first;; esac
ota_no_pending_update || ota_fail pending_firmware_update
OTA_NOW_BOOT=$(ota_boot_id) || ota_fail boot_id_unavailable
ota_inspect
case "$OTA_ACTION:$OTA_OBSERVED" in
    guard:guarded|restore:restored) root_ro || ota_fail root_read_only_not_verified;ota_report;exit 0;;
    guard:originals_present|guard:restored)
        OTA_SHA_UPD=$(ota_hash "$OTA_BIN/otaupd") || ota_fail hash_failed
        OTA_SHA_V3=$(ota_hash "$OTA_BIN/otav3") || ota_fail hash_failed;;
    restore:guarded) :;;
    *) ota_fail unexpected_or_interrupted_binary_state;;
esac
ota_free=$(space_kib)
case "$ota_free" in ''|*[!0-9]*) ota_fail space_unknown;; esac
[ "$ota_free" -ge 20992 ] || ota_fail system_space_reserve
umask 077
mkdir -p "$OTA_DIR"
for ota_path in "$OTA_STATE" "$OTA_DIR/state.next" "$OTA_LOCK";do [ ! -L "$ota_path" ] || ota_fail unsafe_state_path;done
[ ! -e "$OTA_DIR/state.next" ] && [ ! -e "$OTA_LOCK" ] || ota_fail unresolved_transaction
mkdir "$OTA_LOCK" || ota_fail transaction_busy
OTA_ACTIVE=1
trap ota_finish EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
if [ -e "$OTA_STATE" ];then cp "$OTA_STATE" "$OTA_LOCK/previous-state.tsv";OTA_OLD_STATE=1;fi
printf 'schema\t1\naction\t%s\nboot_id\t%s\notaupd\t%s\notav3\t%s\n' "$OTA_ACTION" "$OTA_NOW_BOOT" "$OTA_SHA_UPD" "$OTA_SHA_V3" > "$OTA_LOCK/intent.tsv"
sync
root_rw || ota_fail root_read_write_not_verified
OTA_MUTATING=1
for ota_name in otaupd otav3;do
    if [ "$OTA_ACTION" = guard ];then ota_from=$OTA_BIN/$ota_name;ota_to=$ota_from.bck
    else ota_from=$OTA_BIN/$ota_name.bck;ota_to=$OTA_BIN/$ota_name;fi
    ota_regular_executable "$ota_from" || ota_fail source_changed
    [ ! -e "$ota_to" ] && [ ! -L "$ota_to" ] || ota_fail destination_conflict
    [ "$(ota_hash "$ota_from")" = "$(ota_expected "$ota_name")" ] || ota_fail source_changed
    mv "$ota_from" "$ota_to"
done
if [ "$OTA_ACTION" = guard ];then OTA_NEW_STATE=guarded;ota_pair_matches .bck || ota_fail renamed_hash_mismatch
else OTA_NEW_STATE=restored;ota_pair_matches '' || ota_fail restored_hash_mismatch;fi
printf 'schema\t1\nstate\t%s\nboot_id\t%s\notaupd\t%s\notav3\t%s\n' "$OTA_NEW_STATE" "$OTA_NOW_BOOT" "$OTA_SHA_UPD" "$OTA_SHA_V3" > "$OTA_DIR/state.next"
mv "$OTA_DIR/state.next" "$OTA_STATE"
sync
root_ro || ota_fail root_read_only_not_verified
OTA_COMMITTED=1
OTA_REASON=none
# Success is reported by the EXIT handler only after the real RO check and
# durable lock cleanup. Reboot is a separate, explicit KUAL/user action.
exit 0
