"""Unconfigured preview replacement with real hashes; no device or account."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import unittest

from device.install import test_transactions as transactions


class PreviewReplacement(unittest.TestCase):
    old=transactions.InstallTransactions.old
    snapshot=transactions.InstallTransactions.snapshot
    assert_private=transactions.InstallTransactions.assert_private
    def setUp(self):
        transactions.InstallTransactions.setUp(self)
        self.old(collector=False,events=False)
        for name in ('auth.json','config.json','validation-device.json'):(self.root/name).unlink()
        self.private={name:body for name,body in self.private.items() if name not in ('auth.json','config.json')}
        worker=self.root/'bin/ksync-worker'
        worker.write_text('#!/bin/sh\nprintf \'{"attempts":0,"configured":false,"auto_sync_enabled":false}\\n\'\n');worker.chmod(0o700)
        (self.root/'disabled').write_text('')
        self.recovery=self.base/'usb-recovery'
        script=self.install/'preview-upgrade.sh'
        script.write_text(script.read_text().replace('RECOVERY_PARENT=/mnt/us/.kindle-native-sync-recovery','RECOVERY_PARENT='+str(self.recovery)))
        command=self.commands/'lipc-get-prop';command.write_text('#!/bin/sh\necho 1\n');command.chmod(0o700)
        for name in ('cp','initctl'):
            p=self.commands/name;s=p.read_text()
            if name=='cp':s=s.replace("if name=='cp':", "if name=='cp':\n fail_once(failure=='preview_bin' and args[-1]==str(base/'private/bin')+'/')")
            else:s=s.replace("if name=='initctl':", "if name=='initctl':\n if args and args[0]=='status':\n  path=states/args[1]\n  if path.exists() and path.read_text()=='running':print(args[1]+' start/running, process 123');raise SystemExit(0)\n  print(args[1]+' stop/waiting');raise SystemExit(1)")
            p.write_text(s)
        sha=self.commands/'sha256sum'
        sha.write_text('#!'+sys.executable+'''\nimport hashlib,sys
from pathlib import Path
args=sys.argv[1:]
if args[0]=='-c':
 for line in Path(args[1]).read_text().splitlines():
  digest,name=line.split('  ',1)
  if hashlib.sha256(Path(name).read_bytes()).hexdigest()!=digest:raise SystemExit(1)
else:
 p=Path(args[0]);print(hashlib.sha256(p.read_bytes()).hexdigest()+'  '+args[0])
''');sha.chmod(0o700)
        self.sums()
        self.old_snapshot=self.snapshot()

    def sums(self):
        (self.package/'SHA256SUMS').write_text(''.join(hashlib.sha256(p.read_bytes()).hexdigest()+'  '+str(p.relative_to(self.package))+'\n' for p in sorted(self.package.rglob('*')) if p.is_file() and p.name!='SHA256SUMS'))

    def replace(self,failure='',mode='upgrade'):
        return subprocess.run(['sh',str(self.install/'preview-upgrade.sh'),str(self.package),mode],env={**self.env,'TEST_FAIL':failure},capture_output=True,text=True,timeout=30)

    def test_preview_keeps_raw_ledger_and_disabled_with_verified_usb_recovery(self):
        events=self.root/'events/sealed';events.mkdir(parents=True);(events/'one.log').write_text('SYNTHETIC UNSENT READING')
        checkpoint=self.root/'collector.checkpoint';checkpoint.write_text('SYNTHETIC CURSOR')
        r=self.replace();self.assertEqual(r.returncode,0,r.stderr)
        self.assert_private();self.assertTrue((self.root/'disabled').is_file())
        self.assertEqual((events/'one.log').read_text(),'SYNTHETIC UNSENT READING');self.assertEqual(checkpoint.read_text(),'SYNTHETIC CURSOR')
        backups=list(self.recovery.glob('preview-*'));self.assertEqual(len(backups),1)
        self.assertEqual((backups[0]/'components/bin/ksync-worker').read_bytes(),self.old_snapshot['bin/ksync-worker'])
        self.assertFalse((self.root/'.preview-upgrade').exists())
        self.assertTrue((self.root/'bin/ksync-worker').read_bytes().startswith(b'\x7fELF'))
        self.assertEqual((self.states/'kindle-native-sync').read_text(),'stopped')

    def test_copy_or_mount_failure_restores_exact_stopped_preview(self):
        for failure in ('preview_bin','copy_job','mount_ro_lies','reload'):
            with self.subTest(failure=failure):
                case=PreviewReplacement('test_preview_keeps_raw_ledger_and_disabled_with_verified_usb_recovery');case.setUp()
                try:
                    r=case.replace(failure);self.assertNotEqual(r.returncode,0,r.stdout)
                    self.assertEqual(case.snapshot(),case.old_snapshot)
                    case.assert_private()
                finally:case.doCleanups()

    def test_configured_running_or_corrupt_candidate_is_refused_before_replacement(self):
        before=self.snapshot();(self.root/'auth.json').write_text('DO NOT COPY')
        r=self.replace();self.assertNotEqual(r.returncode,0);self.assertFalse(self.recovery.exists())
        (self.root/'auth.json').unlink();self.assertEqual(self.snapshot(),before)
        (self.states/'kindle-native-sync').write_text('running')
        r=self.replace();self.assertNotEqual(r.returncode,0);self.assertFalse(self.recovery.exists())
        (self.states/'kindle-native-sync').write_text('stopped')
        (self.package/'bin/ksync-worker').write_bytes(b'BAD')
        r=self.replace();self.assertNotEqual(r.returncode,0);self.assertEqual(self.snapshot(),before)

    def test_realistic_replacement_works_when_full_private_double_backup_would_not(self):
        worker=self.root/'bin/ksync-worker';worker.write_bytes(worker.read_bytes()+b'#'+b'x'*(5*1024*1024)+b'\n');worker.chmod(0o700)
        new=self.package/'bin/ksync-worker';new.write_bytes(new.read_bytes()+b'X'*(5*1024*1024))
        common=self.install/'common.sh';s=common.read_text().replace('space_kib() { printf "131072\\n"; }','space_kib() { printf "36827\\n"; }');common.write_text(s);self.sums()
        r=self.replace();self.assertEqual(r.returncode,0,r.stderr)
        self.assert_private();self.assertTrue((self.root/'disabled').exists())

    def test_existing_attempt_or_unreadable_old_ledger_blocks_usb_backup(self):
        worker=self.root/'bin/ksync-worker'
        for body in ['#!/bin/sh\necho \'{"attempts":1,"configured":false}\'\n','#!/bin/sh\nexit 23\n']:
            worker.write_text(body);worker.chmod(0o700);before=self.snapshot()
            r=self.replace();self.assertNotEqual(r.returncode,0)
            self.assertEqual(self.snapshot(),before);self.assertFalse(self.recovery.exists())


if __name__=='__main__':unittest.main()
