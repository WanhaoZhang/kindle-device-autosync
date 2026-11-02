"""Synthetic OTA transactions only. Never executes against a Kindle or network."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent


class OtaGuardTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.root = self.base / 'private'
        self.root.mkdir(mode=0o700)
        self.binaries = self.base / 'system-bin'
        self.binaries.mkdir()
        self.usb = self.base / 'usb'
        self.usb.mkdir()
        self.boot = self.base / 'boot-id'
        self.boot.write_text('11111111-1111-1111-1111-111111111111\n')
        self.mounts = self.base / 'mounts'
        self.set_mount('ro')
        self.commands = self.base / 'commands'
        self.commands.mkdir()
        self.log = self.base / 'commands.log'
        self.fail_mv = self.base / 'fail-mv'
        self.fail_ro = self.base / 'fail-ro'
        self.fail_rw = self.base / 'fail-rw'
        self.fail_cp = self.base / 'fail-cp'
        self.available = self.base / 'space'
        self.available.write_text('50000')
        self.userstore = self.base / 'userstore'
        self.userstore.write_text('1')
        self.wifi = self.base / 'wifi'
        self.wifi.write_text('NA')
        self.original = {}
        for name in ('otaupd', 'otav3'):
            data = ('original-test-binary-' + name).encode()
            self.original[name] = data
            file = self.binaries / name
            file.write_bytes(data)
            file.chmod(0o755)
        self.package = self.base / 'package'
        (self.package / 'ota').mkdir(parents=True)
        (self.package / 'install').mkdir()
        common = (HERE / 'device/install/common.sh').read_text().replace('/bin/mount -n', 'false -n').replace('sleep 1', ':')
        common = common.replace('ROOT=/var/local/kindle-native-sync', 'ROOT=' + str(self.root))
        common = common.replace('/proc/mounts', str(self.mounts))
        common += '\nrequire_device() { return 0; }\n'
        (self.package / 'install/common.sh').write_text(common)
        source = (HERE / 'device/ota/control.sh').read_text()
        source = source.replace('PATH=/sbin:/usr/sbin:/bin:/usr/bin', 'PATH=' + str(self.commands) + ':/bin:/usr/bin')
        source = source.replace('OTA_BIN=/usr/bin', 'OTA_BIN=' + str(self.binaries))
        source = source.replace('OTA_USB=/mnt/us', 'OTA_USB=' + str(self.usb))
        source = source.replace('OTA_BOOT=/proc/sys/kernel/random/boot_id', 'OTA_BOOT=' + str(self.boot))
        self.script = self.package / 'ota/control.sh'
        self.script.write_text(source)
        sha=self.commands/'sha256sum'
        sha.write_text('#!'+sys.executable+'\nimport hashlib,sys\nfrom pathlib import Path\nfor name in sys.argv[1:]: print(hashlib.sha256(Path(name).read_bytes()).hexdigest()+"  "+name)\n')
        sha.chmod(0o700)
        self.command('lipc-get-prop', 'case "$2" in cmState) cat '+str(self.wifi)+';; *) cat '+str(self.userstore)+';; esac\n')
        self.command('df', "printf 'Filesystem 1024-blocks Used Available Capacity Mounted\\n/dev/test 63461 1000 %s 2%% /var/local\\n' \"$(cat " + str(self.available) + ")\"\n")
        self.command('sync', 'exit 0\n')
        native = '''printf 'mntroot %s\\n' "$1" >> LOG
case "$1" in
 ro) if [ -f FAIL_RO ];then rm -f FAIL_RO;exit 0;fi;;
 rw) if [ -f FAIL_RW ];then rm -f FAIL_RW;exit 0;fi;;
esac
printf 'rootfs / rootfs rw 0 0\\n/dev/root / ext3 %s,noatime 0 0\\n' "$1" > MOUNTS
'''.replace('LOG', str(self.log)).replace('FAIL_RO', str(self.fail_ro)).replace('FAIL_RW', str(self.fail_rw)).replace('MOUNTS', str(self.mounts))
        self.command('mntroot', native)
        real_mv = shutil.which('mv')
        self.command('mv', '''if [ -f FAIL ];then
    wanted=$(cat FAIL)
    case "$1" in *"$wanted") rm -f FAIL;exit 19;; esac
fi
exec REAL_MV "$@"
'''.replace('FAIL', str(self.fail_mv)).replace('REAL_MV', real_mv))
        self.command('cp', '''if [ -f FAIL ];then
    case "$2" in */previous-state.tsv) rm -f FAIL;exit 18;; esac
fi
exec REAL_CP "$@"
'''.replace('FAIL', str(self.fail_cp)).replace('REAL_CP', shutil.which('cp')))
        for name in ('reboot', 'pkill', 'killall', 'lipc-set-prop'):
            self.command(name, "printf 'FORBIDDEN " + name + "\\n' >> " + str(self.log) + '\nexit 99\n')

    def command(self, name, body):
        file = self.commands / name
        file.write_text('#!/bin/sh\n' + body)
        file.chmod(0o700)

    def set_mount(self, state):
        self.mounts.write_text('rootfs / rootfs rw 0 0\n/dev/root / ext3 ' + state + ',noatime 0 0\n')

    def run_action(self, action, code=0):
        result = subprocess.run(['sh', str(self.script), action], text=True, capture_output=True, timeout=10)
        self.assertEqual(result.returncode, code, result.stderr + '\n' + result.stdout)
        value = json.loads(result.stdout)
        self.assertFalse(value['wifi_changed'])
        self.assertFalse(value['processes_killed'])
        if self.log.exists():
            self.assertNotIn('FORBIDDEN', self.log.read_text())
        return value

    def assert_originals(self):
        for name, data in self.original.items():
            self.assertEqual((self.binaries / name).read_bytes(), data)
            self.assertFalse((self.binaries / (name + '.bck')).exists())

    def assert_guarded_files(self):
        for name, data in self.original.items():
            self.assertFalse((self.binaries / name).exists())
            self.assertEqual((self.binaries / (name + '.bck')).read_bytes(), data)

    def test_guard_requires_reboot_then_restore_and_reguard_preserve_hashes(self):
        guarded = self.run_action('guard')
        self.assertEqual(guarded['state'], 'guarded')
        self.assertTrue(guarded['reboot_required'])
        self.assertFalse(guarded['ready_after_reboot'])
        self.assert_guarded_files()
        self.assertFalse((self.root / 'ota/transaction').exists())
        self.assertEqual(self.run_action('require-ready', 1)['state'], 'guarded')
        self.boot.write_text('22222222-2222-2222-2222-222222222222\n')
        ready = self.run_action('require-ready')
        self.assertTrue(ready['ready_after_reboot'])
        self.assertFalse(ready['reboot_required'])
        before = (self.root / 'ota/state.tsv').read_bytes()
        self.run_action('guard')
        self.assertEqual((self.root / 'ota/state.tsv').read_bytes(), before)
        restored = self.run_action('restore')
        self.assertEqual(restored['state'], 'restored')
        self.assertTrue(restored['reboot_required'])
        self.assertFalse(restored['ready_after_reboot'])
        self.assert_originals()
        self.run_action('guard')
        self.assert_guarded_files()
        self.assertIn('/dev/root / ext3 ro,', self.mounts.read_text())

    def test_pending_firmware_or_partial_blocks_without_deleting_anything(self):
        real = self.usb / 'Update_kindl_test.bin'
        real.write_bytes(b'pending-user-firmware')
        self.assertEqual(self.run_action('guard', 1)['reason'], 'pending_firmware_update')
        self.assertEqual(real.read_bytes(), b'pending-user-firmware')
        self.assert_originals()
        self.assertFalse(self.log.exists())
        real.unlink()
        partial = self.usb / 'update.bin.tmp.partial'
        partial.mkdir()
        self.assertEqual(self.run_action('guard', 1)['reason'], 'pending_firmware_update')
        self.assertTrue(partial.is_dir())
        partial.rmdir()
        (self.usb / '._Update_kindl_test.bin').write_bytes(b'AppleDouble metadata')
        self.run_action('guard')

    def test_backup_conflicts_and_symlink_sources_are_never_overwritten(self):
        backup = self.binaries / 'otaupd.bck'
        backup.write_bytes(b'foreign-backup')
        self.run_action('guard', 1)
        self.assertEqual(backup.read_bytes(), b'foreign-backup')
        backup.unlink()
        source = self.binaries / 'otav3'
        source.unlink()
        target = self.base / 'target'
        target.write_bytes(b'must-not-move');target.chmod(0o755)
        source.symlink_to(target)
        self.run_action('guard', 1)
        self.assertTrue(source.is_symlink())
        self.assertEqual(target.read_bytes(), b'must-not-move')
        self.assertFalse(self.log.exists())

    def test_second_guard_rename_failure_rolls_back_first_and_restores_ro(self):
        self.fail_mv.write_text('otav3')
        value = self.run_action('guard', 19)
        self.assertEqual(value['state'], 'originals_present')
        self.assert_originals()
        self.assertFalse((self.root / 'ota/state.tsv').exists())
        self.assertFalse((self.root / 'ota/transaction').exists())
        self.assertIn('/dev/root / ext3 ro,', self.mounts.read_text())

    def test_stock_helper_zero_exit_does_not_mask_failed_rw_transition(self):
        self.fail_rw.touch()
        value = self.run_action('guard', 1)
        self.assertEqual(value['reason'], 'root_read_write_not_verified')
        self.assert_originals()
        self.assertFalse((self.root / 'ota/transaction').exists())

    def test_failed_ro_after_rename_rolls_back_without_claiming_guarded(self):
        self.fail_ro.touch()
        value = self.run_action('guard', 1)
        self.assertEqual(value['reason'], 'root_read_only_not_verified')
        self.assertEqual(value['state'], 'originals_present')
        self.assert_originals()
        self.assertFalse((self.root / 'ota/state.tsv').exists())
        self.assertIn('/dev/root / ext3 ro,', self.mounts.read_text())

    def test_second_restore_rename_failure_restores_both_backups_and_old_record(self):
        self.run_action('guard')
        before = (self.root / 'ota/state.tsv').read_bytes()
        self.fail_mv.write_text('otav3.bck')
        value = self.run_action('restore', 19)
        self.assertEqual(value['state'], 'guarded')
        self.assert_guarded_files()
        self.assertEqual((self.root / 'ota/state.tsv').read_bytes(), before)
        self.assertFalse((self.root / 'ota/transaction').exists())

    def test_tampered_backup_and_interrupted_marker_block_ready_and_restore(self):
        self.run_action('guard')
        file = self.binaries / 'otav3.bck'
        file.write_bytes(b'tampered')
        value = self.run_action('restore', 1)
        self.assertEqual(value['reason'], 'unexpected_or_interrupted_binary_state')
        self.assertEqual(file.read_bytes(), b'tampered')
        file.write_bytes(self.original['otav3'])
        lock = self.root / 'ota/transaction'
        lock.mkdir()
        (lock / 'intent.tsv').write_text('incomplete simulated power loss')
        self.assertEqual(self.run_action('status')['state'], 'interrupted')
        self.run_action('require-ready', 1)
        self.run_action('guard', 1)
        self.run_action('restore', 1)
        self.assertTrue(lock.exists())
        self.assert_guarded_files()

    def test_low_system_space_and_unavailable_userstore_refuse_without_changes(self):
        self.available.write_text('20991')
        self.assertEqual(self.run_action('guard', 1)['reason'], 'system_space_reserve')
        self.assert_originals()
        self.available.write_text('50000')
        self.userstore.write_text('0')
        self.assertEqual(self.run_action('guard', 1)['reason'], 'userstore_unavailable')
        self.assert_originals()
        self.assertFalse(self.log.exists())

    def test_ready_rechecks_pending_files_and_actual_read_only_mount(self):
        self.run_action('guard')
        self.boot.write_text('22222222-2222-2222-2222-222222222222\n')
        self.run_action('require-ready')
        self.set_mount('rw')
        self.assertFalse(self.run_action('require-ready', 1)['ready_after_reboot'])
        self.set_mount('ro')
        (self.usb / 'update.bin.tmp.partial').write_bytes(b'pending')
        value = self.run_action('require-ready', 1)
        self.assertTrue(value['pending_update'])
        self.assertFalse(value['ready_after_reboot'])

    def test_wifi_must_be_off_for_mutation_but_ready_can_be_checked_online(self):
        for state in ('CONNECTED','PENDING','READY','DISCONNECTED','unexpected'):
            self.wifi.write_text(state)
            self.assertEqual(self.run_action('guard',1)['reason'],'enable_flight_mode_first')
            self.assert_originals()
        self.assertFalse(self.log.exists())
        self.wifi.write_text('NA');self.run_action('guard')
        self.boot.write_text('22222222-2222-2222-2222-222222222222\n')
        self.wifi.write_text('CONNECTED')
        self.assertTrue(self.run_action('require-ready')['ready_after_reboot'])
        self.assertEqual(self.run_action('restore',1)['reason'],'enable_flight_mode_first')

    def test_failed_private_record_backup_does_not_delete_existing_state(self):
        self.run_action('guard')
        state = self.root/'ota/state.tsv'
        before = state.read_bytes()
        self.fail_cp.touch()
        value = self.run_action('restore',18)
        self.assertEqual(value['state'],'guarded')
        self.assertEqual(state.read_bytes(),before)
        self.assert_guarded_files()
        self.assertFalse((self.root/'ota/transaction').exists())

    def test_term_after_first_rename_rolls_back_and_restores_read_only(self):
        real_mv=shutil.which('mv')
        signal_once=self.base/'signal-once';signal_once.touch()
        self.command('mv', '''if [ -f ONCE ];then
    case "$1" in */otaupd)
      REAL_MV "$@" || exit $?
      rm -f ONCE
      kill -TERM "$PPID"
      exit 0;;
    esac
fi
exec REAL_MV "$@"
'''.replace('ONCE',str(signal_once)).replace('REAL_MV',real_mv))
        value=self.run_action('guard',143)
        self.assertEqual(value['state'],'originals_present')
        self.assert_originals()
        self.assertFalse((self.root/'ota/transaction').exists())
        self.assertIn('/dev/root / ext3 ro,',self.mounts.read_text())


if __name__ == '__main__':
    unittest.main()
