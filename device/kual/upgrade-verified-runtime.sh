#!/bin/sh
# Upgrade the reviewed legacy trial runtime. Never adopts or posts reading time.
set -eu
umask 077
PATH=/sbin:/usr/sbin:/bin:/usr/bin
export PATH
PACKAGE=/mnt/us/kindle-native-sync-package
ROOT=/var/local/kindle-native-sync
OUT=/mnt/us/.kindle-native-sync-export
REVIEW=/mnt/us/.kindle-native-sync-upgrade-review
HERE=/mnt/us/extensions/kindle-native-sync
LEGACY_WORKER=2430ce27e9150ebd06fa99f34d97f1472261d71ed1e3ea2c7512efab35075420
LEGACY_PROTOCOL=233bc9ccdd944ef9bc9f0e96ec37f8af028711f7cf4587b00de618b61810dc1e
LEGACY_NETWORK=1cc550bab418ae47d30b0c987cc9e4281538d17a65bb5a14de3bfaa9404cda73
. "$PACKAGE/install/common.sh"
require_device
[ "$#" = 0 ] || exit 2
[ "$(lipc-get-prop com.lab126.volumd userstoreIsAvailable 2>/dev/null)" = 1 ] || exit 1
[ "$(ksync_root_mount_state)" = read_only ] || exit 1
[ -d "$REVIEW" ] && [ ! -L "$REVIEW" ] && [ ! -L "$OUT" ] || exit 1
mkdir -p "$OUT"
for name in verified-runtime-upgrade.log verified-runtime-upgrade-result.json;do [ ! -L "$OUT/$name" ] || exit 1;done
exec > "$OUT/verified-runtime-upgrade.log" 2>&1
regular() { [ -f "$1" ] && [ ! -L "$1" ]; }
digest() { sha256sum "$1" | awk '{print $1}'; }
json_string() { awk -F '"' -v key="$2" '$2==key {print $4}' "$1"; }
json_value() { awk -F : -v key="\"$2\"" '$1 ~ "^[[:space:]]*"key"[[:space:]]*$" {gsub(/[[:space:],]/,"",$2);print $2}' "$1"; }
hex64() { case "$1" in ''|*[!0-9a-f]*) return 1;; esac;[ "${#1}" = 64 ]; }
fail() { echo "$1" >&2;exit 1; }
(cd "$PACKAGE" && sha256sum -c SHA256SUMS) >/dev/null
for name in SHA256SUMS controlled-review.json raw-SHA256SUMS sidecar-SHA256SUMS request.json;do regular "$REVIEW/$name" || exit 1;done
# The private launcher pins this index hash. Public code contains no user plan,
# account, title, mapping, or reading-source identifiers.
policy_hash=$(digest "$REVIEW/SHA256SUMS")
for name in controlled-review.json raw-SHA256SUMS sidecar-SHA256SUMS request.json;do
    [ "$(awk -v name="$name" '$2==name {n++} END {print n+0}' "$REVIEW/SHA256SUMS")" = 1 ] || exit 1
done
[ "$(wc -l < "$REVIEW/SHA256SUMS" | tr -d '[:space:]')" = 4 ] || exit 1
(cd "$REVIEW" && sha256sum -c SHA256SUMS) >/dev/null
plan=$(json_string "$REVIEW/request.json" plan_id);binding=$(json_string "$REVIEW/request.json" config_binding_sha256)
hex64 "$plan" && hex64 "$binding" || exit 1
[ "$(json_string "$REVIEW/request.json" action)" = adopt-and-enable ] || exit 1
[ "$(json_string "$REVIEW/controlled-review.json" legacy_actual_trial_plan_id)" = "$plan" ] || exit 1
[ "$(json_string "$REVIEW/controlled-review.json" config_binding_sha256)" = "$binding" ] || exit 1
[ "$(json_string "$REVIEW/controlled-review.json" legacy_worker_sha256)" = "$LEGACY_WORKER" ] || exit 1
[ "$(json_string "$REVIEW/controlled-review.json" kind)" = reviewed_native_reading_for_controlled_component_upgrade ] || exit 1
[ "$(json_value "$REVIEW/controlled-review.json" schema_version)" = 1 ] && [ "$(json_value "$REVIEW/controlled-review.json" native_window_counters_continuous)" = true ] && [ "$(json_value "$REVIEW/controlled-review.json" original_trial_must_not_be_reposted)" = true ] && [ "$(json_value "$REVIEW/controlled-review.json" network_writes_performed_by_audit)" = false ] || exit 1
trial_seconds=$(json_value "$REVIEW/controlled-review.json" legacy_actual_trial_seconds)
case "$trial_seconds" in ''|*[!0-9]*) exit 1;; esac
[ "$trial_seconds" -ge 1 ] && [ "$trial_seconds" -le 60 ] || exit 1
for file in "$ROOT/package-manifest.json" "$PACKAGE/manifest.json";do
    regular "$file" || exit 1
    [ "$(json_string "$file" device/worker/src/protocol.rs)" = "$LEGACY_PROTOCOL" ] || fail 'Protocol source changed; legacy trial upgrade refused.'
    [ "$(json_string "$file" device/worker/src/network.rs)" = "$LEGACY_NETWORK" ] || fail 'Transport source changed; legacy trial upgrade refused.'
done
regular "$ROOT/bin/ksync-worker" && [ "$(digest "$ROOT/bin/ksync-worker")" = "$LEGACY_WORKER" ] || fail 'Installed worker is not the reviewed legacy trial binary.'
manifest_worker=$(awk -F '"' '$2=="ksync-worker" {worker=1;next} worker && $2=="sha256" {print $4;exit}' "$ROOT/package-manifest.json")
[ "$manifest_worker" = "$LEGACY_WORKER" ] || exit 1
for marker in .verified-runtime-upgrade .component-repair .preview-upgrade .install-transaction;do
    [ ! -e "$ROOT/$marker" ] && [ ! -L "$ROOT/$marker" ] || fail 'Unresolved upgrade evidence; retain recovery files.'
done
[ ! -e "$ROOT/disabled" ] && [ ! -L "$ROOT/disabled" ] || fail 'The reviewed legacy collector must still be enabled.'
SNAP=$OUT/private-verified-upgrade-$(date -u +%Y%m%dT%H%M%S)-$$
mkdir "$SNAP" "$SNAP/components"
run_readonly() {
    run_output=$1;shift
    run_attempt=1
    while [ "$run_attempt" -le 10 ];do
        if "$@" > "$SNAP/$run_output" 2> "$SNAP/$run_output.error";then return 0;fi
        if [ "$run_attempt" -ge 10 ] || ! grep -Fxq 'worker stopped: worker already running' "$SNAP/$run_output.error";then cat "$SNAP/$run_output.error";return 1;fi
        sleep 3;run_attempt=$((run_attempt+1))
    done
    return 1
}
jobs_running() {
    for job in kindle-native-sync-events kindle-native-sync;do
        state=$(initctl status "$job") || return 1
        case "$state" in "$job start/running, process "*) :;; *) return 1;; esac
        pid=${state##* };case "$pid" in ''|*[!0-9]*) return 1;; esac
        if [ "$job" = kindle-native-sync ];then exe=ksync-collector;else exe=ksync-event-adapter;fi
        [ "$(readlink "/proc/$pid/exe" 2>/dev/null)" = "$ROOT/bin/$exe" ] || return 1
    done
}
status_proof() {
    [ "$(json_value "$1" attempts)" = 1 ] && [ "$(json_value "$1" auto_sync_enabled)" = false ] &&
    [ "$(json_string "$1" binding_sha256)" = "$binding" ] || return 1
    unknown=$(json_value "$1" unknown_batches)
    case "$unknown" in 0|1) :;; *) return 1;; esac
}
verify_sources() {
    [ "$(digest "$REVIEW/SHA256SUMS")" = "$policy_hash" ] || return 1
    (cd "$REVIEW" && sha256sum -c SHA256SUMS) >/dev/null || return 1
    source_count=0
    while IFS= read -r line;do
        hash=${line%%  *};path=${line#"$hash  "};hex64 "$hash" || return 1
        printf '%s\n' "$path" | grep -Eq '^segment-[0-9]+-[0-9]+-[0-9]+[.]log$' || return 1
        regular "$ROOT/events/sealed/$path" && [ "$(digest "$ROOT/events/sealed/$path")" = "$hash" ] || return 1
        source_count=$((source_count+1));[ "$source_count" -le 512 ] || return 1
    done < "$REVIEW/raw-SHA256SUMS"
    [ "$source_count" -gt 0 ] || return 1
    metrics_count=0
    while IFS= read -r line;do
        hash=${line%%  *};path=${line#"$hash  "};hex64 "$hash" || return 1
        case "$path" in /mnt/us/documents/*) :;; *) return 1;; esac
        case "$path" in */../*|*/./*|*//*) return 1;; esac
        regular "$path" && [ "$(digest "$path")" = "$hash" ] || return 1
        parent=${path%/*}
        while [ "$parent" != /mnt/us/documents ];do [ -d "$parent" ] && [ ! -L "$parent" ] || return 1;parent=${parent%/*};done
        metrics_count=$((metrics_count+1));[ "$metrics_count" -le 128 ] || return 1
    done < "$REVIEW/sidecar-SHA256SUMS"
    [ "$metrics_count" -gt 0 ]
}
"$PACKAGE/bin/ksync-collector" --root "$ROOT" --repair-private-state
jobs_running || fail 'Original native collection services are not running.'
run_readonly old-status.json "$ROOT/bin/ksync-worker" --root "$ROOT" status
status_proof "$SNAP/old-status.json" || fail 'Only the one reviewed legacy attempt may exist.'
[ "$(json_value "$SNAP/old-status.json" coverage_gap)" = false ] || fail 'Existing coverage gap must be separately reviewed before any reconciliation.'
verify_sources || fail 'Reviewed native source or sidecar changed before upgrade.'
run_readonly local-selftest.json "$PACKAGE/bin/ksync-worker" --root "$ROOT" self-test
for field in private_filesystem_ok root supported_hardware supported_firmware;do [ "$(json_value "$SNAP/local-selftest.json" "$field")" = true ] || exit 1;done
[ "$(json_string "$SNAP/local-selftest.json" architecture)" = arm ] && [ "$(json_string "$SNAP/local-selftest.json" serial_prefix)" = B0D4 ] && [ "$(json_value "$SNAP/local-selftest.json" network_contacted)" = false ] || exit 1
# This exact command has only GET readbacks; it never confirms/replays a POST.
run_readonly reconcile.json "$PACKAGE/bin/ksync-worker" --root "$ROOT" reconcile --plan-id "$plan"
[ "$(json_string "$SNAP/reconcile.json" state)" = confirmed ] || fail 'Original trial has not converged; no runtime replacement performed.'
run_readonly reconciled-status.json "$ROOT/bin/ksync-worker" --root "$ROOT" status
status_proof "$SNAP/reconciled-status.json" && [ "$(json_value "$SNAP/reconciled-status.json" unknown_batches)" = 0 ] && [ "$(json_value "$SNAP/reconciled-status.json" confirmed_seconds)" = "$trial_seconds" ] || exit 1
verify_sources || exit 1
components='bin/ksync-collector bin/ksync-event-adapter bin/ksync-worker install/common.sh install/control.sh'
for name in $components package-manifest.json;do
    regular "$ROOT/$name" || exit 1
    safe=$(printf '%s' "$name" | tr / _);cp "$ROOT/$name" "$SNAP/components/$safe"
    hash=$(digest "$ROOT/$name");[ "$hash" = "$(digest "$SNAP/components/$safe")" ] || exit 1
    printf '%s  %s\n' "$hash" "$safe" >> "$SNAP/components/SHA256SUMS"
done
sync
: > "$ROOT/disabled"
stop kindle-native-sync-events >/dev/null 2>&1 || :;stop kindle-native-sync >/dev/null 2>&1 || :
for job in kindle-native-sync-events kindle-native-sync;do
    state=$(initctl status "$job");case "$state" in "$job stop/waiting"*) :;; *) exit 1;; esac
done
"$PACKAGE/bin/ksync-collector" --root "$ROOT" --repair-private-state
verify_sources || exit 1
printf '%s\n' "$SNAP" > "$ROOT/.verified-runtime-upgrade"
changed=1
finish() {
    code=$?;trap - EXIT HUP INT TERM;set +e
    if [ "$changed" = 1 ];then
        (cd "$SNAP/components" && sha256sum -c SHA256SUMS) >/dev/null || exit 1
        for name in $components package-manifest.json;do safe=$(printf '%s' "$name" | tr / _);cp "$SNAP/components/$safe" "$ROOT/$name";done
        for name in $components;do chmod 700 "$ROOT/$name";done
        chmod 600 "$ROOT/package-manifest.json"
        echo 'Runtime replacement incomplete; old public components restored where possible, collection remains disabled. Private state and marker retained.'
    fi
    sync;exit "$code"
}
trap finish EXIT;trap 'exit 129' HUP;trap 'exit 130' INT;trap 'exit 143' TERM
for name in $components;do
    [ ! -e "$ROOT/$name.upgrade" ] && [ ! -L "$ROOT/$name.upgrade" ] || exit 1
    item=$(du -sk "$PACKAGE/$name" | awk 'NR==1{print $1}');free=$(space_kib)
    case "$item:$free" in *[!0-9:]*|:*|*:) exit 1;; esac
    [ "$free" -ge "$((24576 + item + 1024))" ] || fail 'Private runtime reserve is insufficient.'
    cp "$PACKAGE/$name" "$ROOT/$name.upgrade";chmod 700 "$ROOT/$name.upgrade"
    [ "$(digest "$PACKAGE/$name")" = "$(digest "$ROOT/$name.upgrade")" ] || exit 1
    mv "$ROOT/$name.upgrade" "$ROOT/$name"
done
[ ! -e "$ROOT/package-manifest.json.upgrade" ] && [ ! -L "$ROOT/package-manifest.json.upgrade" ] || exit 1
cp "$PACKAGE/manifest.json" "$ROOT/package-manifest.json.upgrade";chmod 600 "$ROOT/package-manifest.json.upgrade"
[ "$(digest "$PACKAGE/manifest.json")" = "$(digest "$ROOT/package-manifest.json.upgrade")" ] || exit 1
mv "$ROOT/package-manifest.json.upgrade" "$ROOT/package-manifest.json"
"$ROOT/bin/ksync-collector" --root "$ROOT" --repair-private-state
changed=0
rm "$ROOT/.verified-runtime-upgrade"
note=$ROOT/controlled-upgrade-reading-review.json
[ ! -L "$note" ] && [ ! -e "$note.upgrade" ] && [ ! -L "$note.upgrade" ] || exit 1
if [ -e "$note" ];then regular "$note" && [ "$(digest "$note")" = "$(digest "$REVIEW/controlled-review.json")" ] || exit 1;fi
cp "$REVIEW/controlled-review.json" "$note.upgrade";chmod 600 "$note.upgrade"
[ "$(digest "$note.upgrade")" = "$(digest "$REVIEW/controlled-review.json")" ] || exit 1
mv "$note.upgrade" "$note"
sh "$ROOT/install/control.sh" enable
start_attempt=1
until jobs_running;do
    [ "$start_attempt" -lt 10 ] || fail 'New native collection services did not start.'
    sleep 1;start_attempt=$((start_attempt+1))
done
verify_sources || fail 'Source changed across restart; reviewed gap has not been acknowledged.'
[ "$(digest "$note")" = "$(digest "$REVIEW/controlled-review.json")" ] || exit 1
"$ROOT/bin/ksync-collector" --root "$ROOT" --emit 'maintenance acknowledge-gap'
ack_attempt=1
while [ "$ack_attempt" -le 10 ];do
    run_readonly restarted-status.json "$ROOT/bin/ksync-worker" --root "$ROOT" status
    status_proof "$SNAP/restarted-status.json" && [ "$(json_value "$SNAP/restarted-status.json" unknown_batches)" = 0 ] && [ "$(json_value "$SNAP/restarted-status.json" confirmed_seconds)" = "$trial_seconds" ] || exit 1
    # A checkpoint may preserve witnessed idle. Neither accepted value is
    # manufactured here; active reading invalidates the controlled restart.
    case "$(json_string "$SNAP/restarted-status.json" reader_state)" in unknown|idle) :;; *) fail 'Native reader changed during the controlled restart.';; esac
    if [ "$(json_value "$SNAP/restarted-status.json" coverage_gap)" = false ];then break;fi
    [ "$ack_attempt" -lt 10 ] || fail 'Reviewed gap acknowledgement was not observed.'
    sleep 1;ack_attempt=$((ack_attempt+1))
done
# Restart may retain checkpoint idle or report unknown. No reader event,
# account authorization, plan adoption, or backlog upload is manufactured here.
sh "$HERE/bootstrap.sh" online-self-test
sh "$ROOT/install/control.sh" export
printf '{"schema_version":1,"state":"verified_runtime_upgraded","original_post_replayed":false,"account_time_post_performed":false,"adoption_performed":false,"review_note_preserved":true,"fresh_native_read_required":true}\n' > "$OUT/verified-runtime-upgrade-result.json"
sync
