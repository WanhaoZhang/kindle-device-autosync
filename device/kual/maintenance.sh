#!/bin/sh
# Explicit, one-shot diagnostic for the unconfigured 5423150 preview only.
# No account, ledger, binary, job, queue or validation changes. No network.
set -eu
PATH=/sbin:/usr/sbin:/bin:/usr/bin
export PATH
PACKAGE=/mnt/us/kindle-native-sync-package
ROOT=/var/local/kindle-native-sync
OUT=/mnt/us/.kindle-native-sync-export
MOUNTS=/proc/mounts
EXPECTED_SUMS=d44239659e7ca9e513b49154d7b18c86636a97df58da9b0f48f315e76c6a287a
refuse() { printf '%s\n' "$1" >&2;exit 1; }
[ "$(id -u)" = 0 ] || refuse 'Root launcher required.'
[ "$(uname -m)" = armv7l ] || refuse 'Only ARMv7 PW2 is allowed.'
[ "$(dd if=/proc/usid bs=1 count=4 2>/dev/null)" = B0D4 ] || refuse 'PW2 model mismatch.'
firmware_ok=0
for f in /etc/prettyversion.txt /etc/prettyversion /etc/version.txt;do
    if [ -f "$f" ] && grep -Eq '^Kindle[[:space:]]+5[.]12[.]2[.]1[.]1([[:space:]]|$)' "$f";then firmware_ok=1;fi
done
[ "$firmware_ok" = 1 ] || refuse 'Firmware mismatch.'
[ ! -L "$ROOT" ] && [ -d "$ROOT" ] || refuse 'Unsafe or missing private root.'
for f in config.json auth.json;do
    [ ! -e "$ROOT/$f" ] && [ ! -L "$ROOT/$f" ] || refuse 'Maintenance is limited to the unconfigured preview.'
done
[ "$(lipc-get-prop com.lab126.volumd userstoreIsAvailable 2>/dev/null)" = 1 ] || refuse 'USB userstore unavailable.'
[ ! -L "$PACKAGE" ] && [ ! -L "$PACKAGE/SHA256SUMS" ] && [ -f "$PACKAGE/SHA256SUMS" ] || refuse 'Unsafe candidate.'
[ "$(sha256sum "$PACKAGE/SHA256SUMS" | awk '{print $1}')" = "$EXPECTED_SUMS" ] || refuse 'This action requires the original 5423150 candidate.'
(cd "$PACKAGE" && sha256sum -c SHA256SUMS) >/dev/null || refuse 'Candidate checksum failed.'
native=$(command -v mntroot 2>/dev/null || :)
case "$native" in /sbin/mntroot|/usr/sbin/mntroot|/bin/mntroot|/usr/bin/mntroot) :;; *) refuse 'Cannot identify native mntroot.';; esac
[ -f "$native" ] && [ -x "$native" ] || refuse 'Native mntroot missing.'
DIR=;name=;output_ready=false
ok=false;sealed_state=not_started;sealed_bytes=0;sealed_files=0;first_ro_code=125;final_ro_code=125
step() { [ "$output_ready" != true ] || printf '%s\t%s\n' "$1" "$2" >> "$DIR/steps.tsv"; }
mount_snapshot() { awk '$2=="/" {print $1 "\t" $3 "\t" $4}' "$MOUNTS" > "$DIR/root-mounts-$1.tsv"; }
root_state() {
    awk '$2=="/" && $3!="rootfs" {n++;if ($4 ~ /(^|,)ro(,|$)/) ro++;else bad++} END {if(n>0 && bad==0 && ro==n) print "read_only";else if(n>0) print "read_write_or_ambiguous";else print "unknown"}' "$MOUNTS"
}
native_ro() {
    phase=$1
    if [ "$output_ready" != true ];then "$native" ro >/dev/null 2>&1;return "$?";fi
    if "$native" ro > "$DIR/.native-output" 2>&1;then rc=0;else rc=$?;fi
    # If USB output redirection failed (e.g. ENOSPC), the command did not run.
    # The safety action must not depend on a writable diagnostic directory.
    if [ "$rc" != 0 ];then
        if "$native" ro >/dev/null 2>&1;then fallback=0;else fallback=$?;fi
        step "mntroot_ro_${phase}_without_output" "$fallback"
    fi
    dd if="$DIR/.native-output" of="$DIR/mntroot-$phase.txt" bs=1024 count=64 2>/dev/null || :
    rm -f "$DIR/.native-output"
    step "mntroot_ro_$phase" "$rc"
    return "$rc"
}
finish() {
    exit_code=$?
    trap - EXIT HUP INT TERM
    set +e
    native_ro final;final_ro_code=$?
    if [ "$output_ready" != true ];then
        printf 'Maintenance output unavailable; native ro exit=%s, observed root=%s.\n' "$final_ro_code" "$(root_state)" >&2
        exit 1
    fi
    mount_snapshot final;step root_mount_snapshot_final "$?"
    final_state=$(root_state)
    root_ro=false;[ "$final_state" != read_only ] || root_ro=true
    state=needs_review
    # Native helpers may return nonzero when already RO. Preserve that result,
    # but use actual mount state as the authoritative postcondition.
    if [ "$exit_code" = 0 ] && [ "$ok" = true ] && [ "$root_ro" = true ];then state=diagnostics_complete_root_read_only;fi
    printf '{"schema_version":1,"state":"%s","diagnostic_directory":"%s","diagnostics_complete":%s,"root_mount_state":"%s","root_read_only":%s,"first_mntroot_exit_code":%s,"final_mntroot_exit_code":%s,"script_exit_code":%s,"sealed_export_state":"%s","sealed_files":%s,"sealed_bytes":%s,"private_reading_diagnostic":true,"services_changed":false,"account_data_changed":false,"auto_upload_enabled":false}\n' "$state" "$name" "$ok" "$final_state" "$root_ro" "$first_ro_code" "$final_ro_code" "$exit_code" "$sealed_state" "$sealed_files" "$sealed_bytes" > "$DIR/result.json.partial"
    mv "$DIR/result.json.partial" "$DIR/result.json"
    if [ ! -L "$OUT/maintenance-result.json" ] && [ ! -L "$OUT/maintenance-result.json.partial" ];then
        cp "$DIR/result.json" "$OUT/maintenance-result.json.partial" && mv "$OUT/maintenance-result.json.partial" "$OUT/maintenance-result.json"
    fi
    sync
    [ "$state" = diagnostics_complete_root_read_only ] || exit_code=1
    exit "$exit_code"
}
trap finish EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
[ ! -L "$OUT" ] || refuse 'Unsafe output directory.'
mkdir -p "$OUT"
stamp=$(date +%Y%m%dT%H%M%S | tr -cd '0-9T')
name=maintenance-$stamp-$$
DIR=$OUT/$name
mkdir "$DIR" "$DIR/sealed"
output_ready=true
mount_snapshot before
printf '%s\n' "$native" > "$DIR/mntroot-path.txt"
sha256sum "$native" > "$DIR/mntroot-sha256.txt"
size=$(wc -c < "$native" | tr -d '[:space:]')
case "$size" in ''|*[!0-9]*) size=65537;; esac
if [ "$size" -le 65536 ] && [ "$(dd if="$native" bs=1 count=2 2>/dev/null)" = '#!' ];then cp "$native" "$DIR/mntroot.native.txt";fi
for tool in initctl start stop status;do
    printf '%s\t%s\n' "$tool" "$(command -v "$tool" 2>/dev/null || :)" >> "$DIR/command-paths.tsv"
done
for job in kindle-native-sync kindle-native-sync-events;do
    if initctl status "$job" > "$DIR/$job.status.txt" 2>&1;then step "status_$job" 0;else step "status_$job" "$?";fi
done
for relative in bin/ksync-collector bin/ksync-worker bin/ksync-event-adapter package-manifest.json install/common.sh install/transaction.sh;do
    p=$ROOT/$relative
    if [ -f "$p" ] && [ ! -L "$p" ];then sha256sum "$p" >> "$DIR/components.sha256";else printf '%s\tmissing_or_unsafe\n' "$relative" >> "$DIR/components-missing.tsv";fi
done
for marker in .install-transaction .install-stage upgrade.previous .uninstall-backup disabled validation-device.json;do
    exists=false;[ ! -e "$ROOT/$marker" ] && [ ! -L "$ROOT/$marker" ] || exists=true
    printf '%s\t%s\n' "$marker" "$exists" >> "$DIR/markers.tsv"
done
if native_ro first;then first_ro_code=0;else first_ro_code=$?;fi
mount_snapshot after_first
# The unconfigured worker cannot compact. Only already sealed immutable files
# are exported; active.log and every credential/ledger/config are excluded.
[ ! -L "$ROOT/events" ] && [ ! -L "$ROOT/events/sealed" ] || refuse 'Unsafe event directory.'
sealed_state=complete
planned_bytes=0
: > "$DIR/sealed-sources.tsv"
step sealed_plan_start 0
for p in "$ROOT/events/sealed/"*.log;do
    [ -e "$p" ] || [ -L "$p" ] || continue
    base=${p##*/}
    case "$base" in *[!A-Za-z0-9._-]*|.*) sealed_state=unsafe_source;exit 1;; esac
    [ -f "$p" ] && [ ! -L "$p" ] || { sealed_state=unsafe_source;exit 1; }
    bytes=$(wc -c < "$p" | tr -d '[:space:]')
    case "$bytes" in ''|*[!0-9]*) sealed_state=unsafe_source;exit 1;; esac
    [ "$bytes" -le 4194304 ] || { sealed_state=limit_exceeded;step sealed_limit 1;exit 1; }
    planned_bytes=$((planned_bytes+bytes))
    [ "$planned_bytes" -le 4194304 ] || { sealed_state=limit_exceeded;step sealed_limit 1;exit 1; }
    printf '%s\t%s\n' "$bytes" "$base" >> "$DIR/sealed-sources.tsv"
done
step sealed_plan_complete 0
usb_free=$(df -Pk "$DIR" 2> "$DIR/usb-space-error.txt" | awk 'END {print $4}')
case "$usb_free" in ''|*[!0-9]*) sealed_state=space_unknown;step usb_space 1;exit 1;; esac
required_kib=$(((planned_bytes+1023)/1024+1024))
[ "$usb_free" -ge "$required_kib" ] || { sealed_state=usb_space_low;step usb_space 1;exit 1; }
step usb_space 0
while IFS="$(printf '\t')" read -r bytes base;do
    p=$ROOT/events/sealed/$base
    [ -f "$p" ] && [ ! -L "$p" ] || { sealed_state=source_changed;exit 1; }
    current_bytes=$(wc -c < "$p" | tr -d '[:space:]')
    [ "$current_bytes" = "$bytes" ] || { sealed_state=source_changed;step sealed_size_changed 1;exit 1; }
    before=$(sha256sum "$p" | awk '{print $1}')
    # Fixed length keeps the USB export bounded even if an invariant is broken
    # and a supposedly sealed file grows during this maintenance operation.
    if head -c "$bytes" "$p" > "$DIR/sealed/$base.partial" 2> "$DIR/sealed-copy-error.txt";then step sealed_copy 0
    else code=$?;step sealed_copy "$code";sealed_state=copy_failed;rm -f "$DIR/sealed/$base.partial";exit 1;fi
    copied=$(sha256sum "$DIR/sealed/$base.partial" | awk '{print $1}')
    after=$(sha256sum "$p" | awk '{print $1}')
    [ "$before" = "$copied" ] && [ "$before" = "$after" ] || { sealed_state=source_changed;rm -f "$DIR/sealed/$base.partial";exit 1; }
    mv "$DIR/sealed/$base.partial" "$DIR/sealed/$base"
    printf '%s\t%s\n' "$before" "$base" >> "$DIR/sealed-files.sha256"
    sealed_bytes=$((sealed_bytes+bytes));sealed_files=$((sealed_files+1))
done < "$DIR/sealed-sources.tsv"
ok=true
