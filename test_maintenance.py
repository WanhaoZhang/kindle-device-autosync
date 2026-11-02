"""One-shot maintenance against generated files; no Kindle/account/network access."""
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

HERE=Path(__file__).resolve().parent


class MaintenanceTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.base=Path(self.tmp.name);self.root=self.base/'private';self.root.mkdir()
        (self.root/'events/sealed').mkdir(parents=True)
        (self.root/'events/sealed/segment-1-2-3.log').write_text('synthetic counted evidence\n')
        (self.root/'events/active.log').write_text('ACTIVE_MUST_NOT_EXPORT')
        (self.root/'ledger.sqlite').write_text('LEDGER_MUST_NOT_CHANGE_OR_EXPORT')
        (self.root/'.install-transaction').mkdir()
        (self.root/'.install-transaction/unknown').write_text('KEEP_UNKNOWN')
        self.out=self.base/'export';self.package=self.base/'package';self.package.mkdir()
        (self.package/'fixture').write_text('original candidate')
        digest=hashlib.sha256((self.package/'fixture').read_bytes()).hexdigest()
        (self.package/'SHA256SUMS').write_text(digest+'  fixture\n')
        self.mounts=self.base/'mounts';self.mounts.write_text('/dev/root / ext3 rw,noatime 0 0\nrootfs / rootfs rw 0 0\n')
        self.usid=self.base/'usid';self.usid.write_text('B0D4SYNTHETIC-SERIAL-TAIL')
        self.version=self.base/'version';self.version.write_text('Kindle 5.12.2.1.1 (test)\n')
        self.commands=self.base/'commands';self.commands.mkdir();self.calls=self.base/'native-calls'
        self.tool('id','#!/bin/sh\necho 0\n');self.tool('uname','#!/bin/sh\necho armv7l\n')
        self.tool('lipc-get-prop','#!/bin/sh\necho 1\n');self.tool('sync','#!/bin/sh\nexit 0\n')
        self.tool('initctl','#!/bin/sh\nprintf "%s start/running, process 123\\n" "$2"\n')
        self.tool('df','#!/bin/sh\nprintf "Filesystem 1024-blocks Used Available Capacity Mounted\\n/dev/fake 999999 1 999998 1%% /fake\\n"\n')
        self.tool('mntroot','#!'+sys.executable+'\nimport os\nfrom pathlib import Path\np=Path(os.environ["TEST_BASE"])\nwith (p/"native-calls").open("a") as f:f.write("ro\\n")\nif os.environ.get("FAIL_NATIVE"):raise SystemExit(7)\n(p/"mounts").write_text("/dev/root / ext3 ro,noatime 0 0\\nrootfs / rootfs rw 0 0\\n")\n')
        self.tool('sha256sum','#!'+sys.executable+'''\nfrom pathlib import Path
import sys,hashlib
if sys.argv[1]=='-c':
 for line in Path(sys.argv[2]).read_text().splitlines():
  digest,name=line.split('  ',1)
  if hashlib.sha256(Path(name).read_bytes()).hexdigest()!=digest:raise SystemExit(1)
else:
 p=Path(sys.argv[1]);print(hashlib.sha256(p.read_bytes()).hexdigest()+'  '+str(p))
''')
        body=(HERE/'device/kual/maintenance.sh').read_text()
        body=body.replace('PATH=/sbin:/usr/sbin:/bin:/usr/bin','PATH='+str(self.commands)+':/usr/bin:/bin')
        body=body.replace('PACKAGE=/mnt/us/kindle-native-sync-package','PACKAGE='+str(self.package)).replace('ROOT=/var/local/kindle-native-sync','ROOT='+str(self.root))
        body=body.replace('OUT=/mnt/us/.kindle-native-sync-export','OUT='+str(self.out)).replace('MOUNTS=/proc/mounts','MOUNTS='+str(self.mounts)).replace('/proc/usid',str(self.usid))
        body=body.replace('/etc/prettyversion.txt /etc/prettyversion /etc/version.txt',str(self.version))
        body=body.replace('/sbin/mntroot|/usr/sbin/mntroot|/bin/mntroot|/usr/bin/mntroot',str(self.commands/'mntroot'))
        expected=hashlib.sha256((self.package/'SHA256SUMS').read_bytes()).hexdigest()
        body=body.replace('EXPECTED_SUMS=d44239659e7ca9e513b49154d7b18c86636a97df58da9b0f48f315e76c6a287a','EXPECTED_SUMS='+expected)
        self.script=self.base/'maintenance.sh';self.script.write_text(body)
        self.env={**os.environ,'TEST_BASE':str(self.base)}
        self.before=self.snapshot()

    def tool(self,name,body):
        p=self.commands/name;p.write_text(body);p.chmod(0o700)

    def snapshot(self):return {str(p.relative_to(self.root)):p.read_bytes() for p in self.root.rglob('*') if p.is_file()}
    def run_script(self,**extra):return subprocess.run(['sh',str(self.script)],env={**self.env,**extra},capture_output=True,text=True,timeout=8)
    def result(self):return json.loads((self.out/'maintenance-result.json').read_text())

    def test_ro_verified_from_real_mounts_and_only_sealed_exported(self):
        p=self.run_script();self.assertEqual(p.returncode,0,p.stderr)
        result=self.result();self.assertTrue(result['root_read_only']);self.assertEqual(result['state'],'diagnostics_complete_root_read_only')
        folder=self.out/result['diagnostic_directory']
        self.assertEqual([p.name for p in (folder/'sealed').iterdir()],['segment-1-2-3.log'])
        self.assertEqual(result['sealed_files'],1);self.assertLessEqual(result['sealed_bytes'],4194304)
        text='\n'.join(p.read_text() for p in folder.rglob('*') if p.is_file())
        self.assertNotIn('ACTIVE_MUST_NOT_EXPORT',text);self.assertNotIn('LEDGER_MUST_NOT',text);self.assertNotIn('SYNTHETIC-SERIAL-TAIL',text)
        self.assertEqual(self.snapshot(),self.before)
        self.assertEqual(self.calls.read_text().splitlines(),['ro','ro'])

    def test_configured_or_corrupt_candidate_refuses_without_remount(self):
        (self.root/'auth.json').write_text('MUST_NOT_READ')
        self.assertNotEqual(self.run_script().returncode,0);self.assertFalse(self.calls.exists());self.assertFalse(self.out.exists())
        (self.root/'auth.json').unlink();(self.package/'fixture').write_text('changed')
        self.assertNotEqual(self.run_script().returncode,0);self.assertFalse(self.calls.exists());self.assertFalse(self.out.exists())

    def test_output_creation_failure_still_attempts_native_ro(self):
        self.out.write_text('not a directory')
        p=self.run_script();self.assertNotEqual(p.returncode,0)
        self.assertEqual(self.calls.read_text().splitlines(),['ro'])
        self.assertIn('ext3 ro',self.mounts.read_text());self.assertEqual(self.snapshot(),self.before)

    def test_failed_native_ro_cannot_be_reported_as_success(self):
        p=self.run_script(FAIL_NATIVE='1');self.assertNotEqual(p.returncode,0)
        result=self.result();self.assertFalse(result['root_read_only']);self.assertEqual(result['final_mntroot_exit_code'],7)
        self.assertEqual(result['state'],'needs_review');self.assertEqual(self.snapshot(),self.before)

    def test_already_ro_nonzero_native_status_is_preserved_without_false_failure(self):
        self.tool('mntroot','#!/bin/sh\nprintf "/dev/root / ext3 ro,noatime 0 0\\nrootfs / rootfs rw 0 0\\n" > "$TEST_BASE/mounts"\nexit 1\n')
        p=self.run_script();self.assertEqual(p.returncode,0,p.stderr)
        result=self.result();self.assertTrue(result['root_read_only']);self.assertEqual(result['final_mntroot_exit_code'],1)
        self.assertEqual(result['state'],'diagnostics_complete_root_read_only');self.assertEqual(self.snapshot(),self.before)

    def test_oversized_sealed_source_is_retained_and_not_exported(self):
        f=self.root/'events/sealed/segment-2-2-3.log'
        with f.open('wb') as stream:stream.truncate(4194305)
        before=self.snapshot();p=self.run_script();self.assertNotEqual(p.returncode,0)
        result=self.result();self.assertEqual(result['sealed_export_state'],'limit_exceeded');self.assertEqual(result['sealed_files'],0)
        self.assertEqual(self.snapshot(),before);self.assertTrue(result['root_read_only'])

    def test_signal_still_attempts_final_ro_without_service_changes(self):
        self.tool('head','#!/bin/sh\ntouch "$TEST_BASE/head-ready"\nsleep 0.3\nexec /usr/bin/head "$@"\n')
        p=subprocess.Popen(['sh',str(self.script)],env=self.env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        deadline=time.monotonic()+4
        while not (self.base/'head-ready').exists() and p.poll() is None and time.monotonic()<deadline:time.sleep(.01)
        self.assertTrue((self.base/'head-ready').exists());p.send_signal(signal.SIGTERM);p.communicate(timeout=5)
        self.assertNotEqual(p.returncode,0);self.assertEqual(self.calls.read_text().splitlines(),['ro','ro'])
        self.assertEqual(self.snapshot(),self.before);self.assertTrue(self.result()['root_read_only'])


if __name__=='__main__':unittest.main()
