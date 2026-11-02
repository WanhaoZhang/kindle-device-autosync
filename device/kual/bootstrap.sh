#!/bin/sh
# First-time actions from KUAL. Does not jailbreak, read auth, or create acceptance.
set -eu
PATH=/sbin:/usr/sbin:/bin:/usr/bin
export PATH
PACKAGE=/mnt/us/kindle-native-sync-package
ROOT=/var/local/kindle-native-sync
OUT=/mnt/us/.kindle-native-sync-export
[ "$(id -u)" = 0 ] || { echo 'A verified root launcher is required.' >&2;exit 1; }
[ "$(lipc-get-prop com.lab126.volumd userstoreIsAvailable 2>/dev/null)" = 1 ] || exit 1
[ -d "$PACKAGE/install" ] && [ ! -L "$PACKAGE" ] || { echo 'Copy the verified candidate to /mnt/us/kindle-native-sync-package first.' >&2;exit 1; }
prepare_output() {
    [ ! -L "$OUT" ] || exit 1
    mkdir -p "$OUT"
    [ ! -L "$OUT/$1.partial" ] && [ ! -L "$OUT/$1" ] || exit 1
}
run_candidate() {
    name=$1;shift
    prepare_output "$name"
    [ -f "$PACKAGE/bin/ksync-worker" ] && [ ! -L "$PACKAGE/bin/ksync-worker" ] || exit 1
    (cd "$PACKAGE" && sha256sum -c SHA256SUMS) >/dev/null || { echo 'Candidate checksum failed.' >&2;exit 1; }
    # Core limits affect this test child only; no system-wide crash policy changes.
    if (ulimit -c 0 || exit 125; exec "$PACKAGE/bin/ksync-worker" --root "$ROOT" self-test "$@") > "$OUT/$name.partial" 2>/dev/null;then
        :
    else
        code=$?
        printf '{"schema_version":1,"state":"candidate_process_failed","worker_exit_code":%s,"acceptance_created":false}\n' "$code" > "$OUT/$name.partial"
    fi
    mv "$OUT/$name.partial" "$OUT/$name"
    sync
}
run_installed() {
    name=$1;shift
    prepare_output "$name"
    [ -f "$ROOT/bin/ksync-worker" ] && [ ! -L "$ROOT/bin/ksync-worker" ] || exit 1
    if "$ROOT/bin/ksync-worker" --root "$ROOT" "$@" > "$OUT/$name.partial";then :
    else
        code=$?
        # Worker JSON already describes a blocked/uncertain operation. Preserve
        # it; never turn an error into success or repeat an account operation.
        if [ ! -s "$OUT/$name.partial" ];then printf '{"schema_version":1,"state":"operation_failed","exit_code":%s,"replay_allowed":false}\n' "$code" > "$OUT/$name.partial";fi
        mv "$OUT/$name.partial" "$OUT/$name";sync;return "$code"
    fi
    mv "$OUT/$name.partial" "$OUT/$name";sync
}
setup_result() {
    prepare_output setup-result.json
    printf '{"schema_version":1,"state":"%s","stage":"%s","exit_code":%s,"account_time_write_performed":false,"automatic_uploads_authorized_by_setup":false,"wifi_enabled_by_setup":false}\n' "$1" "$setup_stage" "$2" > "$OUT/setup-result.json.partial"
    mv "$OUT/setup-result.json.partial" "$OUT/setup-result.json";sync
}
finish_setup() {
    result=$?
    trap - EXIT HUP INT TERM
    set +e
    setup_result setup_blocked "$result"
    exit "$result"
}
check_installed_candidate() {
    (cd "$PACKAGE" && sha256sum -c SHA256SUMS) >/dev/null || return 1
    for name in ksync-collector ksync-worker ksync-event-adapter;do
        [ -f "$ROOT/bin/$name" ] && [ ! -L "$ROOT/bin/$name" ] || return 1
        expected=$(sha256sum "$PACKAGE/bin/$name" | awk '{print $1}')
        actual=$(sha256sum "$ROOT/bin/$name" | awk '{print $1}')
        [ "$expected" = "$actual" ] || return 1
    done
}
run_ota() {
    action=$1;name=$2
    prepare_output "$name"
    if sh "$PACKAGE/ota/control.sh" "$action" > "$OUT/$name.partial";then
        mv "$OUT/$name.partial" "$OUT/$name";sync
    else
        result=$?
        mv "$OUT/$name.partial" "$OUT/$name";sync;return "$result"
    fi
}
case "${1:-probe}" in
 probe)
    prepare_output probe.json
    sh "$PACKAGE/install/probe.sh" > "$OUT/probe.json.partial"
    mv "$OUT/probe.json.partial" "$OUT/probe.json"
    sync;;
 self-test)
    run_candidate selftest.json;;
 tls-self-test)
    prepare_output tls-selftest.json
    if [ "$(lipc-get-prop com.lab126.wifid cmState 2>/dev/null)" != CONNECTED ];then
        printf '{"schema_version":1,"state":"wifi_not_connected","network_contacted":false,"acceptance_created":false}\n' > "$OUT/tls-selftest.json.partial"
        mv "$OUT/tls-selftest.json.partial" "$OUT/tls-selftest.json"
        sync
        exit 1
    fi
    run_candidate tls-selftest.json --online-self-test;;
 install)
    prepare_output install-result.json
    # Keep the outcome visible after USB reconnection; no account data is read.
    if sh "$PACKAGE/install/install.sh" "$PACKAGE";then
        printf '{"schema_version":1,"state":"installed_collection_only","exit_code":0,"auto_upload_enabled":false}\n' > "$OUT/install-result.json.partial"
    else
        code=$?
        printf '{"schema_version":1,"state":"installation_failed","exit_code":%s,"auto_upload_enabled":false}\n' "$code" > "$OUT/install-result.json.partial"
    fi
    mv "$OUT/install-result.json.partial" "$OUT/install-result.json"
    sync;;
 preview-upgrade)
    prepare_output preview-upgrade-result.json
    if sh "$PACKAGE/install/preview-upgrade.sh" "$PACKAGE";then
        printf '{"schema_version":1,"state":"preview_replaced_disabled","auto_upload_enabled":false}\n' > "$OUT/preview-upgrade-result.json.partial"
    else
        code=$?
        printf '{"schema_version":1,"state":"preview_replacement_failed","exit_code":%s,"auto_upload_enabled":false}\n' "$code" > "$OUT/preview-upgrade-result.json.partial"
        mv "$OUT/preview-upgrade-result.json.partial" "$OUT/preview-upgrade-result.json";sync;exit "$code"
    fi
    mv "$OUT/preview-upgrade-result.json.partial" "$OUT/preview-upgrade-result.json";sync;;
 preview-recover)
    exec sh "$PACKAGE/install/preview-upgrade.sh" "$PACKAGE" recover;;
 setup-preview|resume-setup)
    # Explicit deployment action: no account request. Configuration must have
    # been staged by the Mac helper, and Wi-Fi remains controlled by the user.
    setup_stage=verify_installed
    trap finish_setup EXIT
    trap 'exit 129' HUP;trap 'exit 130' INT;trap 'exit 143' TERM
    if [ "$1" = setup-preview ];then
        setup_stage=replace_preview
        sh "$0" preview-upgrade
    fi
    check_installed_candidate
    setup_stage=local_selftest
    run_installed selftest.json self-test
    setup_stage=import_configuration
    if [ -d /mnt/us/.kindle-native-sync-inbox ];then
        run_installed import-result.json import-config --inbox /mnt/us/.kindle-native-sync-inbox
    else
        [ -f "$ROOT/config.json" ] && [ -f "$ROOT/auth.json" ] || { echo 'USB configuration missing.' >&2;exit 1; }
    fi
    setup_stage=protect_firmware
    [ -f "$PACKAGE/ota/control.sh" ] || { echo 'Verified OTA guard component missing; remain in airplane mode.' >&2;exit 1; }
    run_ota guard ota-guard.json
    setup_stage=enable_collection
    sh "$ROOT/install/control.sh" enable
    setup_stage=restart_requested
    setup_result setup_complete_restart_requested 0
    sync
    trap - EXIT HUP INT TERM
    if reboot;then :;else result=$?;setup_result reboot_failed "$result";exit "$result";fi;;
 online-self-test)
    [ "$(lipc-get-prop com.lab126.wifid cmState 2>/dev/null)" = CONNECTED ] || { echo 'Connect Wi-Fi explicitly first.' >&2;exit 1; }
    [ -f "$PACKAGE/ota/control.sh" ] || exit 1
    run_ota require-ready ota-status.json
    run_installed tls-selftest.json self-test --online-self-test;;
 verify-setup)
    run_ota require-ready ota-status.json
    sh "$ROOT/install/control.sh" export;;
 authorize-trial|prepare|confirm|enable-experimental|disable-experimental)
    action=$1
    run_installed "action-$action.json" operate --inbox /mnt/us/.kindle-native-sync-actions --action "$action";;
 authorize-prepare)
    sh "$0" online-self-test
    run_installed action-authorize-prepare.json operate --inbox /mnt/us/.kindle-native-sync-actions --action authorize-prepare;;
 confirm-enable)
    run_installed action-confirm-enable.json operate --inbox /mnt/us/.kindle-native-sync-actions --action confirm-enable;;
 *) echo 'Usage: bootstrap.sh probe|self-test|tls-self-test|install|preview-upgrade|preview-recover|setup-preview|resume-setup|online-self-test|authorize-trial|prepare|confirm|enable-experimental|disable-experimental|authorize-prepare|confirm-enable' >&2;exit 2;;
esac
